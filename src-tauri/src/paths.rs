//! `~/.hunter/` 下的目录结构（技术方案 §7）。
//!
//! 测试与「同一台机器上跑两套」靠环境变量 `HUNTER_HOME` 覆盖根目录 ——
//! 单元测试要往临时目录里写文件，不能真去碰用户的 `~/.hunter`。

use std::path::PathBuf;

use crate::err::{AppError, AppResult, Code};

/// 工作目录根。默认 `~/.hunter`；`HUNTER_HOME` 存在时用它。
pub fn root() -> PathBuf {
    if let Ok(p) = std::env::var("HUNTER_HOME") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    default_root()
}

#[cfg(not(test))]
fn default_root() -> PathBuf {
    home().join(".hunter")
}

/// **跑单测时，没设 `HUNTER_HOME` 也绝不落到真的 `~/.hunter` 上**（I9）。
///
/// 为什么要这一条：测试机上的 `~/.hunter` 是一套**真的、装好的** Hunter ——
/// 里面有 `.env`（真 key）、compose 文件、还有跑着的容器对应的配置。
/// 一条忘了拿锁的测试往那儿写一次，毁的是真实安装。
///
/// I7、I9 各抓过一轮「忘了拿 `HUNTER_HOME` 那把锁」的测试，症状都长得
/// 和根因毫无关系（在别的文件里红、只在某个平台上红）。逐条去补是治标；
/// 把默认值挪出真实目录才是治本 —— 忘了拿锁最多是两条测试互相打架，
/// 不会再变成「把开发者/测试机的安装搞坏」。
#[cfg(test)]
fn default_root() -> PathBuf {
    std::env::temp_dir().join(format!("hunter-unittest-{}", std::process::id()))
}

/// 用户主目录。Windows 用 `USERPROFILE`，其余用 `HOME`。
pub fn home() -> PathBuf {
    #[cfg(windows)]
    let key = "USERPROFILE";
    #[cfg(not(windows))]
    let key = "HOME";
    std::env::var(key)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn app_dir() -> PathBuf {
    root().join("app")
}
pub fn logs_dir() -> PathBuf {
    root().join("logs")
}
pub fn backups_dir() -> PathBuf {
    root().join("backups")
}
/// 遥测本地队列所在目录（里程碑 M3 第 6 项、方案 §12）。
pub fn telemetry_dir() -> PathBuf {
    root().join("telemetry")
}
/// 遥测本地队列文件。**只写本机，默认没有任何上报地址**。
pub fn telemetry_queue() -> PathBuf {
    telemetry_dir().join("queue.jsonl")
}
/// 诊断包的默认落地目录（反馈页导出 zip 用）。
pub fn diagnostics_dir() -> PathBuf {
    root().join("diagnostics")
}
/// 自更新下载下来的安装包放这里（`.deb` 这类装不了的格式只能下到本地让用户自己装，
/// 见 [`crate::selfupdate`]）。
pub fn updates_dir() -> PathBuf {
    root().join("updates")
}
/// 内置容器运行时装在这里（I7）。**整棵树都是启动器自己生成的**，
/// 所以「卸载内置运行时」允许整个删掉。
///
/// ## 它可以被搬到别的盘（S-02 · 技术方案 §4.2.3）
///
/// `launcher.toml` 的 `[runtime] data_dir` 非空时用它，空则沿用 `~/.hunter/runtime`。
/// 「不要装到系统盘」这件事只在这一个函数上落地：`LIMA_HOME` / `COLIMA_HOME` /
/// 下载缓存 / 解压出来的整包**全都从这里取**，改这一个值它们一起跟着走。
///
/// ## ⚠️ 这里会去读 `launcher.toml`，别在配置加载的路上调它
///
/// [`crate::config::LauncherConfig::load`] 只读 `paths::launcher_toml()`（走 [`root`]），
/// **不碰这个函数** —— 所以眼下没有环。以后往 `load()` 里加东西时要留意这一点，
/// 一旦那条路上掉进 `runtime_dir()` 就是无限递归。
///
/// ## 守卫认不认这个新目录
///
/// 搬走之后，运行时那棵树就落在 `~/.hunter` 外面了。守卫的「自家地盘」
/// （[`crate::assist::guard::writable_path`]）与 colima 的隔离守卫
/// （[`crate::assist::guard::colima_call`]）都是**当场读这个函数**的，
/// 所以它们认的就是搬过去之后的目录 —— 两处各有单测钉着（S-03）。
/// ## ⚠️ 认不得的值一律落回默认 —— 这不是洁癖，是安全
///
/// 见 [`data_dir_usable`]。一句话：`data_dir = "~"` 这种写法会让守卫把整个家目录
/// 当成「自家地盘」，`writable_path` 那道门当场失守。
pub fn runtime_dir() -> PathBuf {
    let custom = crate::config::LauncherConfig::load().runtime.data_dir;
    let s = custom.trim();
    if s.is_empty() {
        return root().join("runtime");
    }
    let p = crate::config::expand_home(s);
    if !data_dir_usable(&p) {
        crate::lwarn!(
            "[runtime] data_dir 写的「{}」不能用（要一个绝对路径，既不能是盘符 / 根目录，\
             也不能是 {} 的上一层），这次仍然用 {}",
            crate::redact::mask_home(s),
            crate::redact::mask_home(&root().to_string_lossy()),
            crate::redact::mask_home(&root().join("runtime").to_string_lossy()),
        );
        return root().join("runtime");
    }
    p
}

/// `[runtime] data_dir` 认不认这个值。
///
/// 四条否决，每条都对应一种真实的坏结果：
///
/// 1. **相对路径** —— 落在哪儿取决于进程当时的工作目录，等于没写；
/// 2. **盘符 / 根目录**（`/`、`C:\`）—— 那是把整台机器交给启动器写；
/// 3. **用户家目录本身** —— 运行时那棵树是直接往它的根上写
///    `bin` / `colima` / `lima` / `dist` / `cache` / `installed.json` 的，
///    写进 `~` 等于把用户的家目录摊开一地；
/// 4. **工作目录的祖先** —— 这一条最要命：
///    [`crate::assist::guard::writable_path`] 把运行时那棵树也算进「自家地盘」，
///    指到 `~/.hunter` 的上一层就等于把那个白名单放宽到整片目录。
fn data_dir_usable(p: &std::path::Path) -> bool {
    use std::path::Component;
    // 先按字面消掉 `.` 与 `..` —— 不这么做的话 `~/..` 这种写法能绕过下面每一条比对
    // （`components()` 里那个 `..` 谁都不等于，前缀比对必然落空）
    let p = normalize(p);
    if !p.is_absolute() {
        return false;
    }
    // 至少要有一个「正常」的路径段 —— `/` 与 `C:\` 都只有根、没有段
    if !p.components().any(|c| matches!(c, Component::Normal(_))) {
        return false;
    }
    if p == normalize(&home()) {
        return false;
    }
    !normalize(&root()).starts_with(&p)
}

/// 按字面把 `.` 与 `..` 消掉。**不碰文件系统** —— 目标目录多半还不存在，
/// `canonicalize` 在这种地方直接失败，而这里要的只是「比较之前先摆平写法」。
fn normalize(p: &std::path::Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
/// 内置运行时的可执行文件（docker / colima）。
pub fn runtime_bin() -> PathBuf {
    runtime_dir().join("bin")
}
/// 解压出来的整包（lima 要求 `bin/limactl` 与 `share/lima` 保持相对位置）。
pub fn runtime_dist() -> PathBuf {
    runtime_dir().join("dist")
}
/// 下载中转（校验通过才往外搬）。
pub fn runtime_cache() -> PathBuf {
    runtime_dir().join("cache")
}
/// `LIMA_HOME`：虚拟机的磁盘镜像就在这里面。
pub fn lima_home() -> PathBuf {
    runtime_dir().join("lima")
}
/// `COLIMA_HOME`：colima 的 profile 与 docker.sock。
pub fn colima_home() -> PathBuf {
    runtime_dir().join("colima")
}
/// 内置运行时自己的 `DOCKER_CONFIG`（compose 作为 CLI 插件放在它的 `cli-plugins` 下）。
/// **和用户的 `~/.docker` 没有任何关系。**
pub fn runtime_docker_config() -> PathBuf {
    runtime_dir().join("docker-config")
}
/// 装了什么版本（卸载与「设置页显示」都读它）。
pub fn runtime_manifest() -> PathBuf {
    runtime_dir().join("installed.json")
}

pub fn launcher_toml() -> PathBuf {
    root().join("launcher.toml")
}
pub fn env_file() -> PathBuf {
    app_dir().join(".env")
}
pub fn compose_file() -> PathBuf {
    app_dir().join("docker-compose.yml")
}
pub fn override_file() -> PathBuf {
    app_dir().join("docker-compose.launcher.yml")
}
pub fn version_file() -> PathBuf {
    app_dir().join("VERSION")
}
pub fn launcher_log() -> PathBuf {
    logs_dir().join("launcher.log")
}

/// 建好全部目录。失败一律报 `E_CONFIG_WRITE` 并带上真实路径，
/// 「写不进去」这种事只有说清是哪个目录才有得查。
pub fn ensure_dirs() -> AppResult<()> {
    for d in [
        root(),
        app_dir(),
        logs_dir(),
        backups_dir(),
        telemetry_dir(),
        diagnostics_dir(),
        updates_dir(),
    ] {
        std::fs::create_dir_all(&d).map_err(|e| {
            AppError::new(
                Code::ConfigWrite,
                format!("建目录 {} 失败：{e}", d.display()),
            )
        })?;
    }
    // 工作目录整体收成 700。`.env` 本身已经是 600（红线 2），
    // 这一层是纵深防御：同一台机器上别的用户连目录都列不出来。
    //
    // **子目录也要收**（I2 自审发现）：原来只收了 root，子目录跟着 umask 走 ——
    // 测试机上的 umask 是 002，于是 `~/.hunter/app` 这几个实际是 775。
    // 根目录 700 已经挡住了遍历，所以这只是纵深防御的一层；但「工作目录里的东西
    // 只有属主能碰」要么是真的，要么就别写在文档里。
    chmod_700(&root())?;
    for d in [
        app_dir(),
        logs_dir(),
        backups_dir(),
        telemetry_dir(),
        diagnostics_dir(),
        updates_dir(),
    ] {
        chmod_700(&d)?;
    }
    Ok(())
}

/// 把目录权限收成 700。
pub fn chmod_700(path: &std::path::Path) -> AppResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
            AppError::new(
                Code::ConfigWrite,
                format!("设置 {} 权限失败：{e}", path.display()),
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// 把文件权限收成 600（只有属主能读写）。红线 2 要求 `.env` 必须是这个权限。
/// Windows 上没有 POSIX 权限位，靠的是用户目录本身的 ACL，这里直接跳过并如实返回 Ok。
pub fn chmod_600(path: &std::path::Path) -> AppResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
            AppError::new(
                Code::ConfigWrite,
                format!("设置 {} 权限失败：{e}", path.display()),
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// `HUNTER_HOME` 是**进程级**环境变量，两条测试同时改它必然打架。
/// 凡是动这个变量的测试都要先拿这把锁。
///
/// ## 为什么这把锁必须只有一把
///
/// 原来 `paths` / `config` / `telemetry` / `dockercfg` / `log` **各有一把自己的锁**。
/// 每个模块内部是串行的，模块之间照样并行 —— 而它们抢的是同一个进程级变量。
/// 症状是间歇性的、而且长得完全不像根因：`config` 那边写覆盖文件时
/// `std::fs::write` 成功、紧接着 `chmod` 报 `No such file or directory`，
/// 因为中间 `dockercfg` 那边把它自己的临时目录（也就是当时的 `HUNTER_HOME`）删掉了。
///
/// 它在 I7 之前一直没炸过，只是因为时序没撞上；I7 给 `write_override` 加了一次
/// `LauncherConfig::load()`，时序变了一点点，macOS runner 上当场翻车。
/// **这种测试不是"偶尔失败"，是一直坏着、只是还没被看见。**
///
/// 所以这把锁挪到 `paths` 里，`pub(crate)`，全进程**只有这一把**。
#[cfg(test)]
pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// 测试专用：把 `HUNTER_HOME` 指到一个**空的临时目录**，并在 guard 析构时还原。
///
/// I9 加的。在这之前每个模块各自抄一遍「存旧值 → set_var → 断言 → 还原」，
/// 抄漏一次（忘了还原、或者忘了拿 [`env_lock`]）就会让**别的文件里的测试**
/// 莫名其妙地红 —— 而且红的位置和肇事者毫无关系，最难查的那一类。
/// 现在只有这一份实现，锁与还原都在 `Drop` 里。
#[cfg(test)]
pub(crate) struct TestHome {
    _guard: std::sync::MutexGuard<'static, ()>,
    old: Option<String>,
    dir: PathBuf,
}

#[cfg(test)]
impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        match &self.old {
            Some(v) => std::env::set_var("HUNTER_HOME", v),
            None => std::env::remove_var("HUNTER_HOME"),
        }
    }
}

/// 拿一个干净的临时 `HUNTER_HOME`。`tag` 只用来区分目录名，随便起。
#[cfg(test)]
pub(crate) fn test_home(tag: &str) -> TestHome {
    let guard = env_lock();
    let old = std::env::var("HUNTER_HOME").ok();
    let dir = std::env::temp_dir().join(format!("hunter-t-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("HUNTER_HOME", &dir);
    // **整棵目录树都建出来**：不少测试会顺手 `LauncherConfig::save()`，
    // 而那条路会去 chmod `app/` 之类的子目录 —— 只建根目录的话它们会报
    // 「No such file or directory」。CI 的 macOS runner 上就是这么红的一条。
    ensure_dirs().expect("建临时工作目录");
    TestHome {
        _guard: guard,
        old,
        dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn 工作目录建出来是_700() {
        let _g = env_lock();
        use std::os::unix::fs::PermissionsExt;
        let old = std::env::var("HUNTER_HOME").ok();
        let dir = std::env::temp_dir().join(format!("hunter-perm-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("HUNTER_HOME", &dir);
        ensure_dirs().expect("建目录应当成功");
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "工作目录必须是 700，实际是 {mode:o}");
        // 子目录也要 700：umask 是 002 的机器上它们原本会是 775（I2 自审）
        for sub in [
            "app",
            "logs",
            "backups",
            "telemetry",
            "diagnostics",
            "updates",
        ] {
            let m = std::fs::metadata(dir.join(sub))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(m, 0o700, "{sub} 必须是 700，实际是 {m:o}");
        }
        let _ = std::fs::remove_dir_all(&dir);
        match old {
            Some(v) => std::env::set_var("HUNTER_HOME", v),
            None => std::env::remove_var("HUNTER_HOME"),
        }
    }

    /// **S-02**：`[runtime] data_dir` 是运行时唯一可搬的那一块。
    ///
    /// 它跟着走的不只是 `runtime_dir()` 本身 —— `bin` / `dist` / `cache` /
    /// `lima` / `colima` / `runtime_manifest` 全是它的子路径，
    /// 所以这些**一个都不用单独改**。这条测试钉的就是这个「一个值管到底」。
    #[test]
    fn data_dir_能把运行时整棵树搬到别的盘() {
        let _h = crate::paths::test_home("paths-data-dir");

        // 默认：空 = 沿用 ~/.hunter/runtime
        assert_eq!(runtime_dir(), root().join("runtime"));

        // 指一个绝对路径
        let mut c = crate::config::LauncherConfig::load();
        c.runtime.data_dir = "/tmp/别的一块盘/Hunter".to_string();
        c.save().expect("写配置");
        assert_eq!(runtime_dir(), PathBuf::from("/tmp/别的一块盘/Hunter"));
        // 整棵树的子路径一起跟过去
        assert_eq!(runtime_bin(), PathBuf::from("/tmp/别的一块盘/Hunter/bin"));
        assert_eq!(lima_home(), PathBuf::from("/tmp/别的一块盘/Hunter/lima"));
        assert_eq!(
            colima_home(),
            PathBuf::from("/tmp/别的一块盘/Hunter/colima")
        );
        assert_eq!(
            runtime_manifest(),
            PathBuf::from("/tmp/别的一块盘/Hunter/installed.json")
        );

        // 写 `~/` 开头也认（配置文件里写波浪号是很自然的事）
        let mut c = crate::config::LauncherConfig::load();
        c.runtime.data_dir = "~/别的一块盘".to_string();
        c.save().expect("写配置");
        assert_eq!(runtime_dir(), home().join("别的一块盘"));

        // 改回空 = 回到默认（老配置不会因为多了一行就搬家）
        let mut c = crate::config::LauncherConfig::load();
        c.runtime.data_dir = "   ".to_string();
        c.save().expect("写配置");
        assert_eq!(runtime_dir(), root().join("runtime"));
    }

    /// **认不得的 `data_dir` 一律落回默认，而且理由不是洁癖是安全。**
    ///
    /// 尤其是 `~` 这一种：[`crate::assist::guard::writable_path`] 会把运行时那棵树
    /// 也算进「自家地盘」，指到 `~/.hunter` 的上一层就等于把白名单放宽到整个家目录。
    #[test]
    fn data_dir_认不得的值一律落回默认() {
        let _h = crate::paths::test_home("paths-data-dir-bad");
        let default = root().join("runtime");

        // ① 写进配置里、走完整条读取路径的几种坏值
        for bad in [
            "~",             // 用户家目录本身（运行时那棵树会往它的根上写东西）
            "/",             // 文件系统根
            "relative/别处", // 相对路径：落在哪儿取决于当时的工作目录
            ".",             // 同上
            "x",             // 同上
        ] {
            let mut c = crate::config::LauncherConfig::load();
            c.runtime.data_dir = bad.to_string();
            c.save().expect("写配置");
            assert_eq!(runtime_dir(), default, "「{bad}」不该被采用");
        }

        // ② 工作目录的祖先（这一条与环境无关：拿当前工作目录的上一层来试）
        let parent = root().parent().expect("工作目录得有上一层").to_path_buf();
        let mut c = crate::config::LauncherConfig::load();
        c.runtime.data_dir = parent.to_string_lossy().into_owned();
        c.save().expect("写配置");
        assert_eq!(runtime_dir(), default, "工作目录的上一层不该被采用");

        // ③ 正常的绝对路径不许被误伤
        assert!(data_dir_usable(&PathBuf::from("/mnt/别的盘/Hunter")));
        // ④ `~` 这种写法**先按字面消掉 `..` 再比**，不能靠 components 的前缀比对糊过去
        assert_eq!(
            normalize(std::path::Path::new("/home/u/..")),
            PathBuf::from("/home")
        );
        assert_eq!(
            normalize(std::path::Path::new("/home/u/./x/../y")),
            PathBuf::from("/home/u/y")
        );
    }

    #[test]
    fn hunter_home_能覆盖根目录() {
        let _g = env_lock();
        // 注：单测里改进程级环境变量，同文件内的测试要串行跑（cargo 默认并行），
        // 所以这里用一个别处不会用到的值，并在断言后立刻恢复。
        let old = std::env::var("HUNTER_HOME").ok();
        std::env::set_var("HUNTER_HOME", "/tmp/hunter-paths-test");
        assert_eq!(root(), PathBuf::from("/tmp/hunter-paths-test"));
        assert_eq!(env_file(), PathBuf::from("/tmp/hunter-paths-test/app/.env"));
        match old {
            Some(v) => std::env::set_var("HUNTER_HOME", v),
            None => std::env::remove_var("HUNTER_HOME"),
        }
    }
}
