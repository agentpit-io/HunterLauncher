//! 内置容器运行时（I7 · `install_runtime` 的 macOS 默认路线）。
//!
//! ## 要解决的问题
//!
//! 用户 2026-09-21 17:00 的原话：**「所有流程只要 AI 能协助执行，不要人去操作」**。
//! 可是「这台 Mac 上没有 Docker」这件事，I6 之前的做法是发一张「需要你」卡片，
//! 让用户自己去 orbstack.dev 下载、拖进 /Applications、点开、过欢迎页 ——
//! 那是**六次点击**，而且中间还会弹管理员密码。
//!
//! ## 这一版的做法
//!
//! 装一套**完全用户态**的运行时，全程零点击、不要管理员密码、可一键卸载：
//!
//! ```text
//! ~/.hunter/runtime/
//!   bin/            docker（官方静态 CLI）、colima
//!   dist/lima/      lima 整包（bin/limactl + share/lima，相对位置不能拆）
//!   docker-config/  我们自己的 DOCKER_CONFIG，cli-plugins/docker-compose 在这儿
//!   lima/           LIMA_HOME —— 虚拟机磁盘在这里面
//!   colima/         COLIMA_HOME —— profile 与 docker.sock
//!   cache/          下载中转，校验通过才往外搬
//!   installed.json  装了哪几个版本
//! ```
//!
//! **一个字节都不会落在 `~/.hunter` 外面**：不写 `/usr/local`、不改 shell 配置、
//! 不碰 `~/.docker`。这不是靠「我们没写那一行」保证的 —— 所有写文件的地方都过
//! [`crate::assist::guard::writable_path`]，越界会被拒（`guard.rs` 里有对应的单测）。
//!
//! ## 为什么是 Colima 而不是 OrbStack
//!
//! OrbStack 的 dmg 装起来更快，但它**首次启动会弹系统级提示**（欢迎页、安装
//! 辅助程序要管理员密码）—— 那是 macOS 与 OrbStack 自己的交互，启动器代不了点，
//! 也不该去代点。所以 OrbStack 是设置里的**备选**路线（[`super::orbstack`]），
//! 默认路线是这里的 Colima：`colima start` 之后除了等，什么都不用做。
//!
//! ## 与已有运行时共存
//!
//! * 本机已有 OrbStack / Docker Desktop 且**正在跑** → 一个字节都不下，直接用它
//! * 装了但没跑 → 先 `start_runtime`，不另装
//! * 都没有 → 才走这里
//!
//! 判定在 [`should_install`]，不在界面里。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::manifest::{self, Item, Unpack};
use crate::err::{AppError, AppResult, Code};

/// colima 的 profile 名。**用我们自己的名字**，不碰用户可能已经有的 `default`。
pub const PROFILE: &str = "hunter";

/// 下一个文件的超时。100 MB 的包在慢网络上也得给够时间。
const DL_TIMEOUT: Duration = Duration::from_secs(1800);
/// 测速时探一下两个源，只要响应头。
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
/// `colima start` 第一次要下 Ubuntu 镜像 + 起虚拟机，给 15 分钟。
const START_TIMEOUT: Duration = Duration::from_secs(900);

// ── 装了什么 ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledItem {
    pub component: String,
    pub version: String,
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
    /// 实际从哪个源下的（`github` / `tencent-hk`）
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    #[serde(default)]
    pub items: Vec<InstalledItem>,
    #[serde(default)]
    pub installed_at: String,
    #[serde(default)]
    pub launcher_version: String,
}

impl Installed {
    pub fn one_line(&self) -> String {
        if self.items.is_empty() {
            return "没有装内置运行时".to_string();
        }
        let v: Vec<String> = self
            .items
            .iter()
            .map(|i| format!("{} {}", i.component, i.version))
            .collect();
        format!("内置运行时：{}（{}）", v.join(" · "), self.installed_at)
    }
}

/// 读 `installed.json`。没装过就是 `None`。
pub fn installed() -> Option<Installed> {
    let s = std::fs::read_to_string(crate::paths::runtime_manifest()).ok()?;
    serde_json::from_str(&s).ok()
}

/// 内置运行时的 docker 可执行文件；没装就是 `None`。
pub fn docker_bin() -> Option<PathBuf> {
    let p = crate::paths::runtime_bin().join(exe("docker"));
    p.is_file().then_some(p)
}

fn colima_bin() -> Option<PathBuf> {
    let p = crate::paths::runtime_bin().join(exe("colima"));
    p.is_file().then_some(p)
}

fn limactl_dir() -> PathBuf {
    crate::paths::runtime_dist().join("lima").join("bin")
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// 这个 profile 的 docker socket。colima 把它放在 `$COLIMA_HOME/<profile>/docker.sock`。
pub fn socket_path() -> PathBuf {
    crate::paths::colima_home()
        .join(PROFILE)
        .join("docker.sock")
}

/// 给子进程的 `DOCKER_HOST`。**socket 文件真的存在时才返回** ——
/// 指向一个不存在的 socket 只会把「没装」变成「连不上」，更难查。
pub fn docker_host() -> Option<String> {
    let s = socket_path();
    s.exists()
        .then(|| format!("unix://{}", s.to_string_lossy()))
}

/// 虚拟机系统镜像放在哪儿（I8）。`dist/diskimage/<原文件名>`。
pub fn disk_image_dir() -> PathBuf {
    crate::paths::runtime_dist().join("diskimage")
}

/// 这台机器要的那份虚拟机镜像，**下好了就返回它的路径**。
///
/// 返回 `Some` 的条件是文件真的在那儿；不核 sha512（那是下载时做的事，
/// 而且 colima 自己还要再核一遍）。要「连内容一起核」请用
/// [`verified_disk_image`]（I9 起 `start_argv` 走的是那一个）。
pub fn disk_image_path() -> Option<PathBuf> {
    let item = manifest::disk_image_for_host()?;
    let p = disk_image_dir().join(item.file);
    p.is_file().then_some(p)
}

/// 同上，但**先按清单核一遍 sha256 与 sha512**（I9 的 P0-1）。
///
/// 核不过就返回 `Err(那一句说得清的话)`。为什么要在这儿核而不是只信下载时那一次：
/// 用户 Mac 上这个文件已经躺了一天，中间经历过一次装到一半被放弃、
/// 一次磁盘写满、若干次 `colima start`。**「上次下载时是好的」不等于「现在是好的」**，
/// 而 colima 拿到坏文件只会报一句它自己的 sha512 对不上 ——
/// 那句话里没有「残骸」两个字，查起来要绕一大圈。
pub fn verified_disk_image() -> Result<Option<PathBuf>, String> {
    let Some(item) = manifest::disk_image_for_host() else {
        return Ok(None);
    };
    let p = disk_image_dir().join(item.file);
    if !p.is_file() {
        return Ok(None);
    }
    match verify_file(&p, item) {
        Ok(_) => Ok(Some(p)),
        Err(why) => Err(format!("{}{}", item.file, why)),
    }
}

/// 清单里这一条落地之后应该在哪儿。
fn dest_path(item: &Item) -> PathBuf {
    match item.unpack {
        Unpack::Binary if item.component == "docker-compose" => {
            crate::paths::runtime_docker_config()
                .join("cli-plugins")
                .join(exe(item.dest))
        }
        Unpack::Binary | Unpack::TarGzPick(_) => crate::paths::runtime_bin().join(exe(item.dest)),
        Unpack::TarGz => crate::paths::runtime_dist().join(item.dest),
        Unpack::Keep => crate::paths::runtime_dist().join(item.dest).join(item.file),
    }
}

/// 这一条落地了吗。
fn placed(item: &Item) -> bool {
    let p = dest_path(item);
    match item.unpack {
        Unpack::TarGz => p.is_dir(),
        _ => p.is_file(),
    }
}

/// 清单里**还缺**的那几条（I8）。
///
/// 为什么不是一个 bool：0.1.7 装过一次的机器上前四个文件都在，缺的只有
/// I8 新加的那份虚拟机镜像。缺哪几条就只下哪几条，不要为了一个文件重下 95 MB。
pub fn missing_items() -> Vec<&'static Item> {
    manifest::for_host()
        .into_iter()
        .filter(|i| !placed(i))
        .collect()
}

/// **工具装齐了吗**（docker、colima、limactl、compose 插件四件）。
///
/// 注意它和 [`missing_items`] 问的是两个问题：这里问「能不能用」，那里问
/// 「还差什么要下」。I8 往清单里加了虚拟机镜像之后，0.1.7 装过的机器上
/// 「工具齐了但镜像还没下」是一个**真实存在的中间态** —— 两个问题合成一个的话，
/// 那种机器上 `DOCKER_CONFIG` 会突然不指向内置那份，compose 插件当场就找不到了。
pub fn is_installed() -> bool {
    docker_bin().is_some()
        && colima_bin().is_some()
        && limactl_dir().join(exe("limactl")).is_file()
        && crate::paths::runtime_docker_config()
            .join("cli-plugins")
            .join(exe("docker-compose"))
            .is_file()
}

/// 还有东西要下吗（含 I8 新加的虚拟机镜像）。
pub fn needs_download() -> bool {
    !missing_items().is_empty()
}

/// **已经落地的文件里，有哪几条内容对不上清单**（I9）。
///
/// [`missing_items`] 只问「文件在不在」，这里问「文件对不对」。两个问题分开，
/// 是因为算哈希要读几百 MB —— 每次开启动器都算一遍没有道理。
///
/// 什么时候该问这一条：**走内置主线之前**（任务书 P0-1 明写「校验 disk-image
/// 清单与 sha512」）。0.1.7 那次装到一半被用户中断、磁盘写满、或者
/// `colima start` 自己把镜像改坏了，留下的都是「文件在、内容不对」的残骸；
/// 只看「在不在」的话，我们会拿着一个坏文件去 `--disk-image`，
/// colima 按它内置的 sha512 一核对不上，报出来的错和「残骸」两个字毫无关系。
///
/// 返回 `(组件, 说不上哪里不对的那一句)`。
pub fn broken_items() -> Vec<(&'static Item, String)> {
    let mut out = Vec::new();
    for item in manifest::for_host() {
        if !placed(item) {
            continue;
        }
        // 解压成一棵树的（lima）没法按整包哈希核 —— 那一条只能看目录在不在
        if matches!(item.unpack, Unpack::TarGz | Unpack::TarGzPick(_)) {
            continue;
        }
        let p = dest_path(item);
        if let Err(why) = verify_file(&p, item) {
            out.push((item, format!("{}{}", item.file, why)));
        }
    }
    out
}

/// 把内容对不上的那几个文件删掉，好让 [`install`] 重新下一份。
///
/// **只删 `~/.hunter/runtime` 里的东西**，而且每一个都过
/// [`crate::assist::guard::writable_path`]。返回删掉了哪几条（给人看的话）。
pub fn drop_broken_items() -> Vec<String> {
    let mut said = Vec::new();
    for (item, why) in broken_items() {
        let p = dest_path(item);
        match crate::assist::guard::writable_path(&p) {
            Ok(real) => {
                if std::fs::remove_file(&real).is_ok() {
                    crate::lwarn!("内置运行时的 {why} —— 已删掉，等下重新下一份");
                    said.push(format!("{} 内容对不上清单，已删掉重下", item.label()));
                }
            }
            Err(e) => crate::lwarn!("要删 {} 却被守卫拦下：{}", p.display(), e.msg),
        }
    }
    said
}

/// 虚拟机在跑吗（socket 在 = colima 起着）。
pub fn is_running() -> bool {
    socket_path().exists()
}

/// 设置页与诊断报文用的一份状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// 这个平台支持内置运行时吗
    pub supported: bool,
    /// 不支持的原因（支持时为空）
    pub unsupported_reason: String,
    pub installed: bool,
    pub running: bool,
    pub profile: String,
    pub dir: String,
    pub socket: String,
    pub items: Vec<InstalledItem>,
    pub installed_at: String,
    /// 清单里这台机器要下的总字节数（还没装时界面要显示「大约要下 xx MB」）
    pub download_bytes: u64,
}

pub fn status() -> Status {
    let sup = supported();
    let inst = installed().unwrap_or_default();
    Status {
        supported: sup.is_ok(),
        unsupported_reason: sup.err().unwrap_or_default(),
        installed: is_installed(),
        running: is_running(),
        profile: PROFILE.to_string(),
        dir: crate::redact::mask_home(&crate::paths::runtime_dir().to_string_lossy()),
        socket: crate::redact::mask_home(&socket_path().to_string_lossy()),
        items: inst.items,
        installed_at: inst.installed_at,
        // **还要下多少**，不是「一共多大」—— 装过一半的机器上这两个数不一样
        download_bytes: manifest::total_bytes(&missing_items()),
    }
}

// ── 这台机器能不能装 ──────────────────────────────────────────────────────

/// 支持就返回 `Ok(())`，不支持就返回**一句说得清的原因**。
///
/// 不支持时绝不「先装了再说」—— 装一半失败比一开始就说清楚糟得多。
pub fn supported() -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err(
            "内置运行时目前只做了 macOS 这一条路线（Linux 装 Docker 要管理员权限，\
             Windows 要 WSL，两边都做不到「零操作」）。"
                .to_string(),
        );
    }
    if manifest::for_host().is_empty() {
        return Err(format!(
            "下载清单里没有 {} / {} 这台机器要的文件。",
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    }
    if let Some((major, raw)) = macos_major() {
        if major < 13 {
            return Err(format!(
                "内置运行时用的是 macOS 自带的虚拟化框架（vz），它要 macOS 13 或更新；\
                 这台机器是 {raw}。换成 qemu 要再下一套东西，本版没有做。"
            ));
        }
    }
    Ok(())
}

/// macOS 大版本号。取不到就返回 `None`（**不猜**，调用方按「取不到」处理）。
fn macos_major() -> Option<(u32, String)> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let r = crate::proc::run_timeout(
        "/usr/bin/sw_vers",
        &["-productVersion"],
        Duration::from_secs(10),
    )
    .ok()?;
    if !r.ok() {
        return None;
    }
    let raw = r.stdout.trim().to_string();
    let major = raw.split('.').next()?.parse::<u32>().ok()?;
    Some((major, raw))
}

/// 要不要装。**先问「本机已经有没有能用的」**，有就一个字节都不下。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// 已经有正在跑的运行时，什么都不用做
    AlreadyRunning(String),
    /// 装着但没起来，先把它点起来（`start_runtime`），不另装
    StartExisting(String),
    /// 内置运行时已经装好了，起它就行
    StartBuiltin,
    /// 装内置运行时
    Install,
    /// 这个平台装不了，原因见里面那句话
    Unsupported(String),
}

pub fn decide() -> Decision {
    // **顺序按 I9 的 P0-1 改过**：先问内置运行时，再问用户自己的 Docker。
    //
    // 0.1.8 之前这里第一句是 `docker::detect()`，而 `detect` 找到的那个 docker
    // 很可能正是**我们自己下的那一份**（`which` 把 `runtime/bin` 排在最前面）。
    // 用户 Mac 上的后果：残骸被当成「用户装的 Docker、只是 daemon 没起」，
    // 内置主线根本没走到。
    let eff = super::effective::current();
    if eff.builtin.running() {
        return Decision::AlreadyRunning(format!("内置运行时（Colima profile {PROFILE}）"));
    }
    if eff.kind == super::effective::Kind::User && eff.running {
        let d = super::docker::detect();
        if d.installed && d.daemon_running {
            return Decision::AlreadyRunning(d.runtime_label.unwrap_or_else(|| eff.label.clone()));
        }
        return Decision::AlreadyRunning(eff.label);
    }
    if is_installed() && !needs_download() {
        return Decision::StartBuiltin;
    }
    // 工具装着、只差几个文件（0.1.7 装过的机器上差的是虚拟机镜像）：
    // 照样走「装」，只是 install() 只会下缺的那几个
    if is_installed() {
        return match supported() {
            Ok(()) => Decision::Install,
            // 这台机器装不了新东西，但工具是齐的 —— 起它，colima 会自己想办法
            Err(_) => Decision::StartBuiltin,
        };
    }
    // 装了但没跑的（OrbStack / Docker Desktop / 用户自己的 Colima）优先点它，
    // 不另装一套。**`builtin` 那一条要排除掉** —— 它不是「用户装的」，
    // 走到这里说明内置运行时连半截都没有（上面两个 `is_installed()` 已经返回过了）
    for a in crate::assist::probe::runtime_apps() {
        if a.id == "builtin" {
            continue;
        }
        if a.installed == Some(true) && a.running == Some(false) {
            return Decision::StartExisting(a.id);
        }
    }
    match supported() {
        Ok(()) => Decision::Install,
        Err(why) => Decision::Unsupported(why),
    }
}

// ── 下载源择优 ────────────────────────────────────────────────────────────

/// 一个可选的下载源。
#[derive(Debug, Clone)]
pub struct Source {
    pub id: &'static str,
    pub label: &'static str,
    pub url: String,
}

/// 两个源，**并列测速择优**（沿用镜像源那一套）。测不通的直接排除。
///
/// 返回的顺序就是要试的顺序；两个都测不通时仍然把它们按原顺序返回 ——
/// 「测速失败」和「下载失败」不是一回事（HEAD 被挡、代理只放 GET 的情况都有），
/// 真下一把才知道。
pub fn sources_for(item: &Item) -> Vec<Source> {
    let mut v = vec![
        Source {
            id: "github",
            label: "GitHub / docker.com",
            url: item.url.to_string(),
        },
        Source {
            id: "tencent-hk",
            label: "腾讯云 · 香港",
            url: item.cn_url(),
        },
    ];
    let want = item.size;
    let mut timed: Vec<(Option<u128>, Source)> = v
        .drain(..)
        .map(|s| {
            let t = Instant::now();
            // **只问头**：拿状态码与 content-length，不取正文
            let ms = match crate::http::head(&s.url, PROBE_TIMEOUT) {
                Ok(r) if r.ok() => {
                    // 顺手把「这个地址上的文件多大」和清单对一遍。对不上就当它不可用 ——
                    // 真下下来也会被 sha256 拦住，不如在测速这一步就排除掉，
                    // 省一次几十 MB 的无用下载
                    let len = r
                        .header("content-length")
                        .and_then(|x| x.parse::<u64>().ok());
                    match len {
                        Some(n) if n != want => {
                            crate::lwarn!(
                                "{} 上的 {} 是 {n} 字节，清单里写的是 {want} —— 这个源本次不用",
                                s.label,
                                item.file
                            );
                            None
                        }
                        _ => Some(t.elapsed().as_millis()),
                    }
                }
                Ok(r) => {
                    crate::lwarn!("测速 {}（{}）返回 HTTP {}", s.label, item.file, r.status);
                    None
                }
                Err(e) => {
                    crate::lwarn!("测速 {}（{}）失败：{}", s.label, item.file, e.msg);
                    None
                }
            };
            (ms, s)
        })
        .collect();
    // 通的排前面、快的排前面；不通的按原顺序排后面
    timed.sort_by_key(|(ms, _)| ms.unwrap_or(u128::MAX));
    timed.into_iter().map(|(_, s)| s).collect()
}

// ── 安装 ──────────────────────────────────────────────────────────────────

/// 安装过程中的一条进度。**字节数全部是真的**（红线 1）。
pub struct Progress<'a> {
    /// 一句人话（事件流的标题）
    pub say: &'a mut dyn FnMut(&str),
    /// (已下字节, 总字节)
    pub bytes: &'a mut dyn FnMut(u64, u64),
    /// 要不要中止
    pub cancel: &'a dyn Fn() -> bool,
}

/// 下载 + 校验 + 落地。**校验不过就删掉重来一个源，两个源都不过就失败**。
pub fn install(p: &mut Progress) -> AppResult<Installed> {
    supported().map_err(|e| AppError::new(Code::NotImplemented, e))?;
    // **只下还缺的那几个**（I8）：0.1.7 装过一次的机器上前四个都在，
    // 缺的只有新加进清单的那份虚拟机镜像
    let items = missing_items();
    let total = manifest::total_bytes(&items);
    if items.is_empty() {
        (p.say)("需要的文件都已经在本机了，一个字节都不用下");
    } else {
        (p.say)(&format!(
            "要下 {} 个文件，合计 {}",
            items.len(),
            crate::assist::probe::human_bytes(total)
        ));
    }
    ensure_dirs()?;

    let mut done_bytes: u64 = 0;
    let mut installed_items = Vec::new();
    for item in &items {
        if (p.cancel)() {
            return Err(AppError::new(Code::Unknown, "安装被取消了。".to_string()));
        }
        let cached = crate::paths::runtime_cache().join(item.file);
        // 上一次下到一半又重来时，已经校验通过的那一份不必再下一遍
        let mut source_id = "cache".to_string();
        if !(cached.is_file() && verify_file(&cached, item).is_ok()) {
            let srcs = sources_for(item);
            let mut last: Option<AppError> = None;
            let mut ok = false;
            for s in &srcs {
                (p.say)(&format!(
                    "正在下载 {}（{}，从{}）",
                    item.label(),
                    crate::assist::probe::human_bytes(item.size),
                    s.label
                ));
                let base = done_bytes;
                let r = crate::http::download_to_file(
                    &s.url,
                    &cached,
                    DL_TIMEOUT,
                    // 给 4 MB 的富余：上游偶尔会重新打包，多出来的那一点由
                    // sha256 去拦，这里只挡「拿错地址下到一个 GB 级的东西」
                    item.size + 4 * 1024 * 1024,
                    p.cancel,
                    &mut |got, _| (p.bytes)(base + got, total),
                );
                match r {
                    Ok(n) => match verify_file(&cached, item) {
                        Ok(which) => {
                            (p.say)(&format!(
                                "{} 校验通过（{} 对上了，{} 字节）",
                                item.label(),
                                which,
                                n
                            ));
                            source_id = s.id.to_string();
                            ok = true;
                            break;
                        }
                        Err(msg) => {
                            let _ = std::fs::remove_file(&cached);
                            let msg = format!(
                                "{} 从「{}」下回来的内容{}。已删掉，不会使用。",
                                item.file, s.label, msg
                            );
                            crate::lwarn!("{msg}");
                            last = Some(AppError::new(Code::Unknown, msg));
                        }
                    },
                    Err(e) => {
                        crate::lwarn!("下 {} 失败（{}）：{}", item.file, s.label, e.msg);
                        last = Some(e);
                    }
                }
            }
            if !ok {
                return Err(last.unwrap_or_else(|| {
                    AppError::new(Code::Unknown, format!("{} 两个下载源都没拿到。", item.file))
                }));
            }
        } else {
            (p.say)(&format!("{} 之前已经下好并校验过了，跳过", item.label()));
        }
        done_bytes += item.size;
        (p.bytes)(done_bytes, total);
        place(item, &cached)?;
        installed_items.push(InstalledItem {
            component: item.component.to_string(),
            version: item.version.to_string(),
            file: item.file.to_string(),
            sha256: item.sha256.to_string(),
            bytes: item.size,
            source: source_id,
        });
    }

    write_docker_config()?;
    // 这一次只下了缺的那几条，**历史记录不能丢**（否则设置页会显示「只装了一个文件」）
    let mut merged = installed().unwrap_or_default().items;
    for it in installed_items {
        merged.retain(|o| o.component != it.component || o.file != it.file);
        merged.push(it);
    }
    let rec = Installed {
        items: merged,
        installed_at: crate::timefmt::now_shanghai(),
        launcher_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let path = crate::paths::runtime_manifest();
    crate::assist::guard::writable_path(&path)?;
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&rec).unwrap_or_default(),
    )
    .map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("写 {} 失败：{e}", path.display()),
        )
    })?;
    let _ = crate::paths::chmod_600(&path);
    // 下载中转留着没有意义（校验过的东西已经搬到位了），100 MB 不该白占
    let _ = std::fs::remove_dir_all(crate::paths::runtime_cache());
    (p.say)(&format!(
        "{} 个文件都装好了，都在 ~/.hunter/runtime 里，没动系统任何地方",
        manifest::for_host().len()
    ));
    super::which::invalidate();
    super::env::invalidate();
    Ok(rec)
}

fn ensure_dirs() -> AppResult<()> {
    for d in [
        crate::paths::runtime_dir(),
        crate::paths::runtime_bin(),
        crate::paths::runtime_dist(),
        crate::paths::runtime_cache(),
        crate::paths::lima_home(),
        crate::paths::colima_home(),
        crate::paths::runtime_docker_config().join("cli-plugins"),
    ] {
        std::fs::create_dir_all(&d).map_err(|e| {
            AppError::new(
                Code::ConfigWrite,
                format!("建目录 {} 失败：{e}", d.display()),
            )
        })?;
        let _ = crate::paths::chmod_700(&d);
    }
    Ok(())
}

/// 把校验过的文件搬到该去的位置。**每一次写都过守卫**。
fn place(item: &Item, cached: &Path) -> AppResult<()> {
    match item.unpack {
        Unpack::Binary => {
            let dest = if item.component == "docker-compose" {
                crate::paths::runtime_docker_config()
                    .join("cli-plugins")
                    .join(exe(item.dest))
            } else {
                crate::paths::runtime_bin().join(exe(item.dest))
            };
            crate::assist::guard::writable_path(&dest)?;
            std::fs::copy(cached, &dest).map_err(|e| {
                AppError::new(
                    Code::ConfigWrite,
                    format!("放 {} 失败：{e}", dest.display()),
                )
            })?;
            chmod_exec(&dest)
        }
        Unpack::TarGz => {
            let dir = crate::paths::runtime_dist().join(item.dest);
            crate::assist::guard::writable_path(&dir)?;
            // 重装时先清干净，免得新旧版本的文件混在一起
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).map_err(|e| {
                AppError::new(
                    Code::ConfigWrite,
                    format!("建目录 {} 失败：{e}", dir.display()),
                )
            })?;
            untar(cached, &dir, None)
        }
        Unpack::Keep => {
            // 虚拟机镜像：原样搬过去，不解压、不加可执行位。
            // 它唯一的用处是当 `colima start --disk-image` 的参数
            let dir = disk_image_dir();
            crate::assist::guard::writable_path(&dir)?;
            std::fs::create_dir_all(&dir).map_err(|e| {
                AppError::new(
                    Code::ConfigWrite,
                    format!("建目录 {} 失败：{e}", dir.display()),
                )
            })?;
            let dest = dir.join(item.file);
            crate::assist::guard::writable_path(&dest)?;
            std::fs::copy(cached, &dest).map_err(|e| {
                AppError::new(
                    Code::ConfigWrite,
                    format!("放 {} 失败：{e}", dest.display()),
                )
            })?;
            Ok(())
        }
        Unpack::TarGzPick(inner) => {
            let tmp = crate::paths::runtime_cache().join(format!("{}.unpack", item.dest));
            let _ = std::fs::remove_dir_all(&tmp);
            std::fs::create_dir_all(&tmp).map_err(|e| {
                AppError::new(
                    Code::ConfigWrite,
                    format!("建目录 {} 失败：{e}", tmp.display()),
                )
            })?;
            untar(cached, &tmp, Some(inner))?;
            let src = tmp.join(inner);
            let dest = crate::paths::runtime_bin().join(exe(item.dest));
            crate::assist::guard::writable_path(&dest)?;
            std::fs::copy(&src, &dest).map_err(|e| {
                AppError::new(
                    Code::ConfigWrite,
                    format!("从 {} 取 {inner} 失败：{e}", item.file),
                )
            })?;
            let _ = std::fs::remove_dir_all(&tmp);
            chmod_exec(&dest)
        }
    }
}

/// 解一个 tar.gz。**用系统自带的 `tar`**：三个平台都有（macOS 与 Windows 10+
/// 自带的都是 bsdtar，Linux 是 GNU tar），比往依赖树里再塞两个解压 crate 划算。
///
/// 安全性来自上一步：这个文件的 sha256 已经和清单对上了，内容是确定的；
/// 解压目标是我们自己建的空目录，`-C` 指死。
fn untar(archive: &Path, into: &Path, only: Option<&str>) -> AppResult<()> {
    let tar = super::which::resolve("tar")
        .resolved
        .unwrap_or_else(|| "/usr/bin/tar".to_string());
    let mut args: Vec<String> = vec![
        "-xzf".into(),
        archive.to_string_lossy().into_owned(),
        "-C".into(),
        into.to_string_lossy().into_owned(),
    ];
    if let Some(o) = only {
        args.push(o.to_string());
    }
    let mut argv = vec![tar.clone()];
    argv.extend(args.clone());
    crate::assist::guard::argv(&argv)?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let r = crate::proc::run_timeout(&tar, &refs, Duration::from_secs(300))?;
    if !r.ok() {
        return Err(AppError::new(
            Code::Unknown,
            format!("解压 {} 失败：{}", archive.display(), r.err_line()),
        ));
    }
    Ok(())
}

fn chmod_exec(p: &Path) -> AppResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
            AppError::new(
                Code::ConfigWrite,
                format!("给 {} 加可执行位失败：{e}", p.display()),
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = p;
    Ok(())
}

/// 我们自己那一份 `DOCKER_CONFIG`。**没有 credsStore、没有 auths** ——
/// Hunter 的六个镜像都是公开的，本来就不需要登录（I6 的结论）。
fn write_docker_config() -> AppResult<()> {
    let p = crate::paths::runtime_docker_config().join("config.json");
    crate::assist::guard::writable_path(&p)?;
    let body = serde_json::json!({
        "cliPluginsExtraDirs": [],
        "auths": {},
    });
    std::fs::write(&p, serde_json::to_string_pretty(&body).unwrap_or_default())
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("写 {} 失败：{e}", p.display())))?;
    let _ = crate::paths::chmod_600(&p);
    Ok(())
}

// ── 起虚拟机 ──────────────────────────────────────────────────────────────

/// 给虚拟机分多少资源。**按本机配置取保守值**，不把用户的机器榨干。
///
/// | 项 | 取法 |
/// |---|---|
/// | CPU | 本机核数的一半，夹在 2–4 之间 |
/// | 内存 | 本机内存的 1/4，夹在 4–8 GiB 之间（六个容器实测峰值约 2.5 GiB） |
/// | 磁盘 | 剩余空间的 1/3，夹在 20–60 GiB 之间（镜像解压后约 3 GiB） |
///
/// 任何一项读不到就用下限 —— **不猜一个大的**。
pub fn vm_params() -> (u32, u32, u32) {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(2);
    let cpu = (cores / 2).clamp(2, 4);

    let mem_gib = host_mem_gib().unwrap_or(8);
    let memory = (mem_gib / 4).clamp(4, 8);

    let disk = match crate::assist::probe::disk_free(&crate::paths::root()) {
        Some((free, _)) => ((free / (1024 * 1024 * 1024)) as u32 / 3).clamp(20, 60),
        None => 20,
    };
    (cpu, memory, disk)
}

fn host_mem_gib() -> Option<u32> {
    if cfg!(target_os = "macos") {
        let r = crate::proc::run_timeout(
            "/usr/sbin/sysctl",
            &["-n", "hw.memsize"],
            Duration::from_secs(10),
        )
        .ok()?;
        let b: u64 = r.stdout.trim().parse().ok()?;
        return Some((b / (1024 * 1024 * 1024)) as u32);
    }
    // Linux：/proc/meminfo 第一行是 MemTotal
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb: u64 = s
        .lines()
        .find(|l| l.starts_with("MemTotal:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some((kb / (1024 * 1024)) as u32)
}

/// `colima start` 要跑的那条命令（**展示与执行同一个来源**）。
///
/// I8 加了两样东西，都是 0.1.7 真机首测逼出来的：
///
/// | 参数 | 为什么 |
/// |---|---|
/// | `--disk-image <本地文件>` | 不让 colima 自己去 GitHub 下那 341 MB（用户 Mac 上就是这一步超时的）。给了本地文件之后它按内置 sha512 核一遍就用，一个字节都不下 |
/// | `--env HTTP_PROXY=…` | 虚拟机里的 docker 拉镜像也要能走用户的代理。**只监听本机的代理不传**（虚拟机里的 127.0.0.1 是它自己），那种情况这里就没有 `--env` |
pub fn start_argv() -> AppResult<Vec<String>> {
    let c = colima_bin().ok_or_else(|| {
        AppError::new(
            Code::NotImplemented,
            "内置运行时还没装（找不到 colima）。".to_string(),
        )
    })?;
    let (cpu, mem, disk) = vm_params();
    let mut v = vec![
        c.to_string_lossy().into_owned(),
        "start".into(),
        "--profile".into(),
        PROFILE.into(),
        "--vm-type".into(),
        "vz".into(),
        "--cpu".into(),
        cpu.to_string(),
        "--memory".into(),
        mem.to_string(),
        "--disk".into(),
        disk.to_string(),
    ];
    // **核过内容的才交给 colima**（I9）。核不过就当没有这份镜像：
    // 宁可让 colima 自己去下（慢、但会成），也不要拿一个坏文件去换一句
    // 看不懂的 sha512 错误。核不过的那个文件由 `drop_broken_items` 负责删掉重下
    match verified_disk_image() {
        Ok(Some(img)) => {
            v.push("--disk-image".into());
            v.push(img.to_string_lossy().into_owned());
        }
        Ok(None) => {}
        Err(why) => crate::lwarn!("本地那份虚拟机镜像{why} —— 这一次不用它"),
    }
    for (k, val) in crate::netproxy::current().for_vm() {
        // colima 的 --env 收的是 KEY=VALUE；大小写两份都给（VM 里跑的程序两种都有）
        v.push("--env".into());
        v.push(format!("{k}={val}"));
    }
    Ok(v)
}

/// 起虚拟机之前要不要先说一句代理的事（事件流用）。**说实话**：
/// 传进去了就说传了，没传进去要说清为什么没传。
pub fn vm_proxy_note() -> Option<String> {
    let p = crate::netproxy::current();
    if !p.any() {
        return None;
    }
    if p.for_vm().is_empty() {
        Some(format!(
            "{}。不过它只监听你这台电脑本机（127.0.0.1），虚拟机里访问不到，\
             所以没有把它传进虚拟机 —— 虚拟机拉镜像会走腾讯云香港的源。",
            p.one_line()
        ))
    } else {
        Some(format!("{}，并把它传给了虚拟机里的 docker。", p.one_line()))
    }
}

/// colima / limactl 要的环境变量。**全部指向 `~/.hunter/runtime`**，
/// 用户可能已经有的 `~/.lima` / `~/.colima` 一个字节都不碰。
pub fn env_pairs() -> Vec<(String, String)> {
    let mut v = vec![
        (
            "LIMA_HOME".to_string(),
            crate::paths::lima_home().to_string_lossy().into_owned(),
        ),
        (
            "COLIMA_HOME".to_string(),
            crate::paths::colima_home().to_string_lossy().into_owned(),
        ),
        (
            "DOCKER_CONFIG".to_string(),
            crate::paths::runtime_docker_config()
                .to_string_lossy()
                .into_owned(),
        ),
    ];
    if let Some(h) = docker_host() {
        v.push(("DOCKER_HOST".to_string(), h));
    }
    // colima / limactl 自己也会下东西（lima 的 guest agent、模板），直连不通时
    // 它们得能走用户的代理 —— 这几个变量是它们唯一认得的入口（I8）
    v.extend(crate::netproxy::current().env_pairs());
    v
}

/// 要补进子进程 PATH 的目录（`limactl` 必须让 colima 找得到）。
pub fn path_dirs() -> Vec<String> {
    let mut v = Vec::new();
    let b = crate::paths::runtime_bin();
    if b.is_dir() {
        v.push(b.to_string_lossy().into_owned());
    }
    let l = limactl_dir();
    if l.is_dir() {
        v.push(l.to_string_lossy().into_owned());
    }
    v
}

/// 起虚拟机。返回给用户看的那一句（**真实耗时**）。
pub fn start(p: &mut Progress) -> AppResult<String> {
    if is_running() {
        return Ok("内置运行时的虚拟机已经在跑了。".to_string());
    }
    let argv = start_argv()?;
    crate::assist::guard::argv(&argv)?;
    if let Some(note) = vm_proxy_note() {
        (p.say)(&note);
    }
    // **说的必须是刚才真的做过的事**（红线 1）：核过了才说「校验过了」
    match verified_disk_image() {
        Ok(Some(_)) => {
            (p.say)("正在启动虚拟机（系统镜像刚刚按清单核过 sha256 与 sha512，这一步不再下东西）")
        }
        Ok(None) => {
            // 清单里没有这台机器的镜像时如实说明：colima 会自己去下
            (p.say)("正在启动虚拟机（本机没有预先下好的系统镜像，colima 会自己去下一份）")
        }
        Err(why) => (p.say)(&format!(
            "本地那份系统镜像{why} —— 这一次不用它，让 colima 自己去下一份",
            why = why
        )),
    }
    let t = Instant::now();
    let (prog, args) = argv.split_first().expect("start_argv 至少有一项");
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let pairs = env_pairs();
    let env: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let r = crate::proc::run_timeout_env(prog, &refs, START_TIMEOUT, &env)?;
    if !r.ok() {
        return Err(AppError::new(
            Code::DaemonDown,
            format!(
                "虚拟机没起来（colima start 退出码 {:?}）：{}",
                r.status,
                r.err_line()
            ),
        ));
    }
    super::env::invalidate();
    super::which::invalidate();
    if !is_running() {
        return Err(AppError::new(
            Code::DaemonDown,
            format!(
                "colima 说起好了，但 {} 这个 socket 没出现 —— 不当成成功。",
                crate::redact::mask_home(&socket_path().to_string_lossy())
            ),
        ));
    }
    let secs = t.elapsed().as_secs();
    (p.say)(&format!("虚拟机起来了（用时 {secs} 秒）"));
    Ok(format!(
        "内置运行时已就绪（profile {PROFILE}，用时 {secs} 秒，socket 在 {}）",
        crate::redact::mask_home(&socket_path().to_string_lossy())
    ))
}

// ── 修残骸（I9 的 P0-1） ─────────────────────────────────────────────────

/// 这个 profile 的虚拟机目录（`$COLIMA_HOME/<profile>`）。
fn profile_dir() -> PathBuf {
    crate::paths::colima_home().join(PROFILE)
}

/// lima 那一侧对应的实例目录（colima 建的实例叫 `colima-<profile>`）。
fn lima_instance_dir() -> PathBuf {
    crate::paths::lima_home().join(format!("colima-{PROFILE}"))
}

/// 内置运行时有残骸吗（**装了、但那台虚拟机处在一个说不清的中间态**）。
///
/// 判据：profile 目录在、socket 却不在。正常停机的 colima 也是这个样子，
/// 所以这一条**不能**单独拿来做「要重建」的依据 —— 它只是「值得看一眼」。
/// 真正决定重建的是「`colima start` 起不来」那一刻（见 [`repair`] 的调用点）。
pub fn has_residue() -> bool {
    profile_dir().is_dir() && !is_running()
}

/// **重建那台虚拟机**：删掉 profile 与它的 lima 实例，再把内容对不上的文件重下。
///
/// 三条边界，一条都不松：
///
/// 1. 删的每一个目录都先过 [`crate::assist::guard::writable_path`]，
///    解析之后必须落在 `~/.hunter` 里 —— 用户的 `~/.colima` / `~/.lima`
///    一个字节都不碰（任务书 P0-2 的红线）；
/// 2. 下载回来的二进制与虚拟机镜像**不删**（那是 436 MB，删了就得重下）；
///    只有 [`broken_items`] 认定内容对不上的才删；
/// 3. `colima delete` 这一步失败**不算失败** —— 目录照样删干净，
///    因为残骸的定义本来就是「colima 自己也说不清它是什么状态」。
///
/// 返回给用户看的那一句（**真实做了什么**，做不到的如实说）。
pub fn repair(p: &mut Progress) -> AppResult<String> {
    let mut said: Vec<String> = Vec::new();

    // ① 让 colima 自己先试着删。带全隔离参数，profile 写死 hunter
    if let Some(c) = colima_bin() {
        let args = ["delete", "--profile", PROFILE, "--force"];
        let pairs = env_pairs();
        let env: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        (p.say)("正在清掉上次没装成功的那台虚拟机（只动 Hunter 自己的，profile hunter）");
        match crate::proc::run_timeout_env(
            &c.to_string_lossy(),
            &args,
            Duration::from_secs(300),
            &env,
        ) {
            Ok(r) if r.ok() => said.push("colima 已经删掉了 hunter 这台虚拟机".to_string()),
            Ok(r) => said.push(format!(
                "colima delete 退出码 {:?}（{}）—— 目录照样清干净",
                r.status,
                r.err_line()
            )),
            Err(e) => said.push(format!(
                "colima delete 没跑起来（{}）—— 目录照样清干净",
                e.msg
            )),
        }
    }

    // ② 目录层面再清一遍。**每一个都过守卫**
    for dir in [profile_dir(), lima_instance_dir()] {
        if !dir.exists() {
            continue;
        }
        let real = crate::assist::guard::writable_path(&dir)?;
        match std::fs::remove_dir_all(&real) {
            Ok(()) => said.push(format!(
                "已清掉 {}",
                crate::redact::mask_home(&real.to_string_lossy())
            )),
            Err(e) => {
                return Err(AppError::new(
                    Code::ConfigWrite,
                    format!(
                        "清 {} 失败：{e}",
                        crate::redact::mask_home(&real.to_string_lossy())
                    ),
                ))
            }
        }
    }

    // ③ 内容对不上的文件删掉重下（下载那一步由调用方的 install 接着做）
    said.extend(drop_broken_items());

    super::which::invalidate();
    super::env::invalidate();
    let line = said.join("；");
    (p.say)(&line);
    Ok(line)
}

// ── 卸载 ──────────────────────────────────────────────────────────────────

/// 一键卸载：`colima delete` + 删掉 `~/.hunter/runtime` 整棵树。
///
/// 允许整个删，是因为这棵树**完全是启动器自己生成的**、而且在 `~/.hunter` 里
/// （守卫的 [`crate::assist::guard::writable_path`] 放行）。用户自己的东西
/// 一个字节都不在里面。
pub fn uninstall() -> AppResult<String> {
    let mut said = Vec::new();
    if let Some(c) = colima_bin() {
        let args = ["delete", "--profile", PROFILE, "--force"];
        let mut argv = vec![c.to_string_lossy().into_owned()];
        argv.extend(args.iter().map(|s| s.to_string()));
        crate::assist::guard::argv(&argv)?;
        let pairs = env_pairs();
        let env: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        match crate::proc::run_timeout_env(
            &c.to_string_lossy(),
            &args,
            Duration::from_secs(300),
            &env,
        ) {
            Ok(r) if r.ok() => said.push(format!("已删掉虚拟机（profile {PROFILE}）")),
            Ok(r) => said.push(format!(
                "colima delete 退出码 {:?}（{}）—— 目录照样删干净",
                r.status,
                r.err_line()
            )),
            Err(e) => said.push(format!(
                "colima delete 没跑起来（{}）—— 目录照样删干净",
                e.msg
            )),
        }
    }
    let dir = crate::paths::runtime_dir();
    // 守卫：必须在「可以整棵删掉」的白名单里（全项目只有这一个目录），
    // 而且要解析到 `~/.hunter` 里面。差一条就一个字节都不删
    let real = crate::assist::guard::deletable_tree(&dir)?;
    if real.exists() {
        std::fs::remove_dir_all(&real).map_err(|e| {
            AppError::new(
                Code::ConfigWrite,
                format!(
                    "删 {} 失败：{e}",
                    crate::redact::mask_home(&real.to_string_lossy())
                ),
            )
        })?;
        said.push(format!(
            "已删掉 {}",
            crate::redact::mask_home(&real.to_string_lossy())
        ));
    } else {
        said.push("本来就没有内置运行时的目录".to_string());
    }
    // 设置里指着内置 docker 的那一条也要撤掉，否则下次启动会找一个不存在的文件
    let mut cfg = crate::config::LauncherConfig::load();
    let bin = crate::paths::runtime_bin().to_string_lossy().into_owned();
    if cfg.runtime.docker_path.starts_with(&bin) {
        cfg.runtime.docker_path.clear();
        cfg.save()?;
        said.push("设置里的 docker 路径也撤回了".to_string());
    }
    super::which::invalidate();
    super::env::invalidate();
    Ok(said.join("；"))
}

// ── 工具 ──────────────────────────────────────────────────────────────────

/// 按清单校验一个下回来的文件。
///
/// **上游发了 sha512 的，两个都要对上**（虚拟机镜像就是这一类：colima 自己
/// 还会按 sha512 再核一遍，我们这边先核一遍是为了「对不上就别往下走」）。
/// 返回值是「对上了哪几种」，给人看的那一行里要写清楚。
fn verify_file(path: &Path, item: &Item) -> Result<String, String> {
    let got = sha256_file(path).ok_or_else(|| "的 sha256 算不出来".to_string())?;
    if got != item.sha256 {
        return Err(format!(
            " sha256 对不上：期望 {}，实际 {}",
            item.sha256, got
        ));
    }
    if item.sha512.is_empty() {
        return Ok("sha256".to_string());
    }
    let got = sha512_file(path).ok_or_else(|| "的 sha512 算不出来".to_string())?;
    if got != item.sha512 {
        return Err(format!(
            " sha512 对不上：期望 {}，实际 {}",
            item.sha512, got
        ));
    }
    Ok("sha256 与 sha512".to_string())
}

/// 一个文件的 sha512（小写十六进制）。读不动就返回 `None`。
pub fn sha512_file(p: &Path) -> Option<String> {
    use sha2::{Digest, Sha512};
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut h = Sha512::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(hex(&h.finalize()))
}

/// 一个文件的 sha256（小写十六进制）。读不动就返回 `None`。
pub fn sha256_file(p: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut h = Sha256::new();
    // 100 MB 的包不整个读进内存 —— 一块一块喂给摘要
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(hex(&h.finalize()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_名是我们自己的不是_default() {
        assert_eq!(PROFILE, "hunter");
        let e = env_pairs();
        let home: Vec<&String> = e.iter().map(|(_, v)| v).collect();
        for h in home {
            assert!(
                h.contains(".hunter") || h.starts_with("unix://"),
                "内置运行时的环境变量必须都指向 ~/.hunter：{h}"
            );
        }
    }

    /// 三个 HOME 变量一个都不能少 —— 少一个就会写到用户的 `~/.lima` 去。
    #[test]
    fn 三个隔离变量齐全() {
        let e = env_pairs();
        for k in ["LIMA_HOME", "COLIMA_HOME", "DOCKER_CONFIG"] {
            assert!(e.iter().any(|(a, _)| a == k), "少了 {k}");
        }
    }

    #[test]
    fn socket_不存在时不给_docker_host() {
        // 测试进程的 HUNTER_HOME 是临时目录，里面不会有 socket
        if !socket_path().exists() {
            assert_eq!(docker_host(), None, "指向一个不存在的 socket 只会更难查");
        }
    }

    /// 资源取的是保守值，而且**一定落在设计文档写的区间里**。
    #[test]
    fn 虚拟机资源是保守值() {
        let (cpu, mem, disk) = vm_params();
        assert!((2..=4).contains(&cpu), "cpu={cpu}");
        assert!((4..=8).contains(&mem), "mem={mem}");
        assert!((20..=60).contains(&disk), "disk={disk}");
    }

    #[test]
    fn sha256_算得对() {
        let _g = crate::paths::test_home("builtin-sha");
        let d = crate::paths::root().join("sha-test.bin");
        std::fs::create_dir_all(crate::paths::root()).unwrap();
        std::fs::write(&d, b"abc").unwrap();
        assert_eq!(
            sha256_file(&d).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_file(&d);
    }

    #[test]
    fn 算不出_sha256_的文件返回_none_而不是空串() {
        assert_eq!(sha256_file(Path::new("/绝对不存在/x.bin")), None);
    }

    /// 非 macOS 上 [`supported`] 必须**说清原因**而不是含糊拒绝。
    #[test]
    fn 平台不支持时给得出原因() {
        let r = supported();
        if cfg!(target_os = "macos") {
            // mac 上要么支持，要么是版本太老 —— 两种都要有话说
            if let Err(e) = r {
                assert!(e.contains("macOS"), "{e}");
            }
        } else {
            let e = r.expect_err("非 macOS 上不该支持");
            assert!(e.contains("macOS"), "{e}");
            assert!(!e.is_empty());
        }
    }

    /// 两个源都要给出来，而且国内那个必须落在香港桶。
    #[test]
    fn 每个文件都有两个源() {
        let item = &manifest::MACOS[0];
        let urls: Vec<String> = vec![item.url.to_string(), item.cn_url()];
        assert!(urls[0].starts_with("https://github.com/"));
        assert!(urls[1].contains("hunter-dl-hk-"));
        assert_ne!(urls[0], urls[1]);
    }

    /// **真的从两个源各下一个文件，按清单核对 sha256。**
    ///
    /// 默认 `#[ignore]`：它要联网、要下十几 MB，不该在每次 `cargo test` 时跑。
    /// 验收时显式跑：
    ///
    /// ```text
    /// cargo test --lib runtime::builtin::tests::两个源都真能下到与清单一致的文件 -- --ignored --nocapture
    /// ```
    ///
    /// 这一条是 macOS 那条链路里**唯一能在 Linux 上验的部分**（下载 + 校验），
    /// 剩下的（colima 起虚拟机）必须真机。挑的是清单里最小的那个文件。
    #[test]
    #[ignore = "要联网、要下十几 MB；验收时用 --ignored 显式跑"]
    fn 两个源都真能下到与清单一致的文件() {
        let item = manifest::MACOS
            .iter()
            .min_by_key(|i| i.size)
            .expect("清单非空");
        println!("挑的是 {}（{} 字节）", item.file, item.size);
        let dir = crate::paths::runtime_cache();
        std::fs::create_dir_all(&dir).unwrap();
        for src in [
            Source {
                id: "github",
                label: "GitHub / docker.com",
                url: item.url.to_string(),
            },
            Source {
                id: "tencent-hk",
                label: "腾讯云 · 香港",
                url: item.cn_url(),
            },
        ] {
            let dest = dir.join(format!("{}.{}", item.file, src.id));
            let t = Instant::now();
            let mut last = 0u64;
            let n = crate::http::download_to_file(
                &src.url,
                &dest,
                Duration::from_secs(600),
                item.size + 4 * 1024 * 1024,
                &|| false,
                &mut |got, _| {
                    if got > last + 4 * 1024 * 1024 {
                        last = got;
                        println!("  {} 已下 {got} 字节", src.label);
                    }
                },
            )
            .unwrap_or_else(|e| panic!("从 {} 下 {} 失败：{}", src.label, item.file, e.msg));
            let ms = t.elapsed().as_millis();
            assert_eq!(n, item.size, "{} 给的字节数与清单不符", src.label);
            let got = sha256_file(&dest).expect("算得出 sha256");
            assert_eq!(
                got, item.sha256,
                "{} 上的内容与清单里的 sha256 对不上",
                src.label
            );
            println!("  ✓ {} · {n} 字节 · {ms} ms · sha256 与清单一致", src.label);
            let _ = std::fs::remove_file(&dest);
        }
    }

    /// 测速这一步**只问头**，而且会把 content-length 和清单对一遍。
    /// 同样要联网，所以也 `#[ignore]`。
    #[test]
    #[ignore = "要联网；验收时用 --ignored 显式跑"]
    fn 测速两个源都通且文件大小对得上() {
        for item in manifest::MACOS.iter() {
            let srcs = sources_for(item);
            assert_eq!(srcs.len(), 2);
            for s in &srcs {
                let r = crate::http::head(&s.url, Duration::from_secs(15))
                    .unwrap_or_else(|e| panic!("HEAD {} 失败：{}", s.label, e.msg));
                assert!(
                    r.ok(),
                    "{} 上的 {} 回了 HTTP {}",
                    s.label,
                    item.file,
                    r.status
                );
                let len: u64 = r
                    .header("content-length")
                    .and_then(|x| x.parse().ok())
                    .unwrap_or_else(|| panic!("{} 没给 content-length", s.label));
                assert_eq!(
                    len, item.size,
                    "{} 上的 {} 大小与清单不符",
                    s.label, item.file
                );
                println!("  ✓ {} · {} · {len} 字节", s.label, item.file);
            }
        }
    }
}
