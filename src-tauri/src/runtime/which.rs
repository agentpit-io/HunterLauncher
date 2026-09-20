//! 可执行文件定位（I4 的 P0）。
//!
//! ## 为什么不能只靠 PATH
//!
//! 用户在自己的 Mac 上第一次真机跑 0.1.3 时，Docker 检测页报了
//! `E_DOCKER_MISSING · 命令行里找不到 docker`，而他机器上 OrbStack 装着、
//! 终端里 `docker version` 能返回 Server 29.4.0。实测下来的根因是：
//!
//! * `docker` 在 `/usr/local/bin/docker`，是指向
//!   `/Applications/OrbStack.app/Contents/MacOS/xbin/docker` 的软链，能用；
//! * 但 macOS 上**从访达 / 程序坞启动的 GUI 程序**拿到的 PATH 只有
//!   `/usr/bin:/bin:/usr/sbin:/sbin`（`launchctl getenv PATH` 是空的 → 走系统默认），
//!   **不含 `/usr/local/bin`**；
//! * 于是 `Command::new("docker")` 必然 ENOENT。
//!
//! 这跟用户装没装 Docker 毫无关系，是 macOS GUI 程序的固有行为 ——
//! 终端里能跑是因为 shell 读过 `/etc/paths` 与 `~/.zshrc`，GUI 程序读不到那些。
//!
//! ## 这个模块做什么
//!
//! 给一个程序名，按固定顺序找出**绝对路径**：
//!
//! | 顺序 | 来源 | 说明 |
//! |---|---|---|
//! | 1 | `[runtime] docker_path` / 环境变量 | 用户手动指定，指了就用，找不到就如实报错**不再往下找** |
//! | 2 | PATH | 自己走一遍 PATH，不调 `which`（`which` 本身也可能不在 PATH 上） |
//! | 3 | 内置的已知目录清单 | 见 [`builtin_dirs`]，**按平台**给，可以从配置追加与关闭 |
//!
//! 找到第一个**存在且可执行**的就用。全过程记在 [`Probe`] 里 ——
//! 界面上要显示「用的是哪个路径」，排查时要看「探过哪些位置、分别是什么结果」。
//!
//! ## 这是确定性规则，不交给 AI 判断
//!
//! I4 的 AI 诊断助手分两层，这个模块整个属于**第一层**：零延迟、零 token、
//! 结果可复现。AI 那一层永远拿这里的探测结果当输入，不负责「猜 docker 在哪」。
//!
//! ## 缓存
//!
//! 一次解析的结果进程内缓存。用户点「重新检测」时调 [`invalidate`] 清掉 ——
//! 他很可能刚刚装完 Docker 或刚把 OrbStack 点起来。

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

/// 一次候选路径的探测结果。**如实记**：不存在就是不存在，不可执行就是不可执行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// 找到了，就用它
    Found,
    /// 这个位置没有这个文件
    Missing,
    /// 文件在，但没有可执行位（mac 上从 dmg 拷出来又被 quarantine 改过权限的情况见过）
    NotExecutable,
}

/// 探过的一个位置。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// 这一步的来源：`配置` / `PATH` / `已知位置`
    pub source: String,
    /// 完整的候选路径
    pub candidate: String,
    pub outcome: Outcome,
}

/// 一次完整解析的全过程。界面与诊断报文都直接用它。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub program: String,
    /// 最终用的绝对路径；一个都没找到时为 `None`
    pub resolved: Option<String>,
    /// 解析出的那一条是从哪来的（`配置` / `PATH` / `已知位置`）
    pub source: Option<String>,
    /// 软链指向哪里。`/usr/local/bin/docker` → OrbStack 的 xbin 就是靠这一条看出来的
    pub link_target: Option<String>,
    /// 按顺序探过的每一个位置
    pub steps: Vec<Step>,
}

impl Probe {
    pub fn found(&self) -> bool {
        self.resolved.is_some()
    }

    /// 给人看的一行：`/usr/local/bin/docker（已知位置 · 软链指向 …/OrbStack.app/…/xbin/docker）`
    pub fn one_line(&self) -> String {
        match &self.resolved {
            None => format!(
                "没找到 {}（探了 {} 个位置）",
                self.program,
                self.steps.len()
            ),
            Some(p) => {
                let src = self.source.as_deref().unwrap_or("?");
                match &self.link_target {
                    Some(t) => format!("{p}（{src} · 软链指向 {t}）"),
                    None => format!("{p}（{src}）"),
                }
            }
        }
    }

    /// 探测明细，一行一条。进诊断报文与日志。
    pub fn detail_lines(&self) -> Vec<String> {
        self.steps
            .iter()
            .map(|s| {
                let mark = match s.outcome {
                    Outcome::Found => "✓",
                    Outcome::Missing => "·",
                    Outcome::NotExecutable => "!",
                };
                let why = match s.outcome {
                    Outcome::Found => "可执行",
                    Outcome::Missing => "没有这个文件",
                    Outcome::NotExecutable => "文件在但没有可执行位",
                };
                format!("{mark} [{}] {} — {}", s.source, s.candidate, why)
            })
            .collect()
    }
}

/// 搜索策略。从 `launcher.toml` 的 `[runtime]` 段来（见 [`crate::config::RuntimeSection`]），
/// 也可以被环境变量盖掉（测试与排障用）。
///
/// **路径清单只在这一个结构里定义一次** —— 十几个调用点谁也不许自己写死一条路径。
#[derive(Debug, Clone)]
pub struct Policy {
    /// 用户手动指定的 docker 绝对路径。非空时**只认它**
    pub docker_path: String,
    /// 用户手动指定的独立 docker-compose 绝对路径
    pub compose_path: String,
    /// 追加在内置清单**前面**的目录
    pub extra_dirs: Vec<String>,
    /// 要不要用内置清单
    pub use_builtin: bool,
    /// 要不要走 PATH
    pub use_env_path: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            docker_path: String::new(),
            compose_path: String::new(),
            extra_dirs: Vec::new(),
            use_builtin: true,
            use_env_path: true,
        }
    }
}

impl Policy {
    /// 从配置读一份，再让环境变量有机会盖掉。
    ///
    /// 环境变量存在的理由只有一个：**测试要能造出「docker 完全没装」的现场**
    /// （I4 测试场景 1）。真机上没人会去设它们。
    pub fn current() -> Self {
        let mut p = crate::config::LauncherConfig::load().runtime.to_policy();
        if let Ok(v) = std::env::var("HUNTER_DOCKER_PATH") {
            if !v.is_empty() {
                p.docker_path = v;
            }
        }
        if let Ok(v) = std::env::var("HUNTER_COMPOSE_PATH") {
            if !v.is_empty() {
                p.compose_path = v;
            }
        }
        if let Ok(v) = std::env::var("HUNTER_SEARCH_PATHS") {
            if !v.is_empty() {
                p.extra_dirs = split_dirs(&v);
            }
        }
        if env_off("HUNTER_USE_BUILTIN_PATHS") {
            p.use_builtin = false;
        }
        if env_off("HUNTER_USE_ENV_PATH") {
            p.use_env_path = false;
        }
        p
    }
}

fn env_off(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("0") | Ok("false") | Ok("off")
    )
}

fn split_dirs(s: &str) -> Vec<String> {
    let sep = if cfg!(windows) { ';' } else { ':' };
    s.split(sep)
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .collect()
}

/// 把开头的 `~` 展开成家目录。配置里写 `~/.orbstack/bin` 比写死绝对路径可读。
pub fn expand_tilde(p: &str) -> PathBuf {
    match p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        Some(rest) => crate::paths::home().join(rest),
        None if p == "~" => crate::paths::home(),
        None => PathBuf::from(p),
    }
}

/// 内置的已知安装位置，**按平台**。顺序就是探测顺序。
///
/// macOS 的前几条正是这次 P0 的解法：GUI 程序的 PATH 里没有它们，但
/// OrbStack / Docker Desktop / Homebrew / Colima 装出来的 docker 就在这几个目录里。
pub fn builtin_dirs() -> Vec<&'static str> {
    if cfg!(target_os = "macos") {
        vec![
            // Homebrew（Intel）与各家安装器都往这里放软链 —— 用户这台机器就是这一条命中的
            "/usr/local/bin",
            // Homebrew（Apple Silicon）
            "/opt/homebrew/bin",
            // OrbStack 给当前用户装的那一份
            "~/.orbstack/bin",
            // Docker Desktop 的 per-user CLI 目录
            "~/.docker/bin",
            // Docker Desktop 的 app 内自带
            "/Applications/Docker.app/Contents/Resources/bin",
            // OrbStack 的 app 内自带（`/usr/local/bin/docker` 就是指到这里的）
            "/Applications/OrbStack.app/Contents/MacOS/xbin",
            // 有人把 app 装在家目录下的 Applications 里
            "~/Applications/OrbStack.app/Contents/MacOS/xbin",
            "~/Applications/Docker.app/Contents/Resources/bin",
            // Colima 走 brew 装，上面两条 brew 目录已覆盖；它自己的 bin 也带一下
            "~/.colima/bin",
            "~/.rd/bin", // Rancher Desktop
            "~/.local/bin",
            // GUI 程序默认 PATH 里的那四个，兜底也探一遍
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ]
    } else if cfg!(target_os = "windows") {
        vec![
            r"C:\Program Files\Docker\Docker\resources\bin",
            r"C:\Program Files\Docker\Docker\resources",
            r"~\AppData\Local\Programs\Docker\Docker\resources\bin",
            r"C:\ProgramData\DockerDesktop\version-bin",
            r"~\.docker\bin",
            r"C:\Program Files\RedHat\Podman",
            r"C:\Windows\System32",
        ]
    } else {
        // Linux
        vec![
            "/usr/bin",
            "/usr/local/bin",
            "/bin",
            "/snap/bin",
            "~/.docker/bin",
            "~/.local/bin",
            "/opt/docker/bin",
            "/usr/sbin",
            "/sbin",
        ]
    }
}

/// Windows 上一个「程序名」要试的几种后缀。非 Windows 只有它自己。
fn name_variants(program: &str) -> Vec<String> {
    if cfg!(windows) {
        if program.contains('.') {
            vec![program.to_string()]
        } else {
            vec![
                format!("{program}.exe"),
                format!("{program}.cmd"),
                format!("{program}.bat"),
                program.to_string(),
            ]
        }
    } else {
        vec![program.to_string()]
    }
}

/// 文件在不在、能不能执行。
fn check(p: &Path) -> Option<Outcome> {
    let md = std::fs::metadata(p).ok()?;
    if !md.is_file() {
        // 目录 / 其它类型当作「没有这个文件」
        return Some(Outcome::Missing);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if md.permissions().mode() & 0o111 == 0 {
            return Some(Outcome::NotExecutable);
        }
    }
    Some(Outcome::Found)
}

fn link_target(p: &Path) -> Option<String> {
    let t = std::fs::read_link(p).ok()?;
    // 相对软链（`../foo`）拼回去再显示，否则用户看不懂
    let abs = if t.is_absolute() {
        t
    } else {
        p.parent()?.join(t)
    };
    Some(abs.to_string_lossy().into_owned())
}

/// 按 [`Policy`] 找一个程序。**这是本模块唯一的解析入口**。
pub fn resolve_with(program: &str, explicit: &str, policy: &Policy) -> Probe {
    let mut probe = Probe {
        program: program.to_string(),
        resolved: None,
        source: None,
        link_target: None,
        steps: Vec::new(),
    };

    // ① 用户手动指定：指了就只认它。找不到也**不往下找** ——
    //    悄悄回退到别的 docker，会让「我明明指了这一个」变成一桩无头案。
    if !explicit.trim().is_empty() {
        let p = expand_tilde(explicit.trim());
        let outcome = check(&p).unwrap_or(Outcome::Missing);
        probe.steps.push(Step {
            source: "配置".into(),
            candidate: p.to_string_lossy().into_owned(),
            outcome,
        });
        if outcome == Outcome::Found {
            probe.link_target = link_target(&p);
            probe.resolved = Some(p.to_string_lossy().into_owned());
            probe.source = Some("配置".into());
        }
        return probe;
    }

    // ② PATH。自己走一遍，不调 `which` —— 那个程序自己也可能不在 PATH 上，
    //    而且它在三个平台上的行为与输出都不一样。
    if policy.use_env_path {
        if let Ok(path) = std::env::var("PATH") {
            for dir in split_dirs(&path) {
                if try_dir(&mut probe, "PATH", &dir, program) {
                    return probe;
                }
            }
        }
    }

    // ③ 已知位置。配置里追加的排在内置清单前面 —— 用户指明的目录优先。
    let mut dirs: Vec<String> = policy.extra_dirs.clone();
    if policy.use_builtin {
        dirs.extend(builtin_dirs().into_iter().map(str::to_string));
    }
    for dir in dirs {
        if try_dir(&mut probe, "已知位置", &dir, program) {
            return probe;
        }
    }

    probe
}

/// 探一个目录。找到了就把 probe 填好并返回 true。
/// 同一个目录**只探一次** —— PATH 与内置清单大量重合（`/usr/bin` 两边都有），
/// 重复探会把明细列表撑成一堆废话。
fn try_dir(probe: &mut Probe, source: &str, dir: &str, program: &str) -> bool {
    let base = expand_tilde(dir);
    for name in name_variants(program) {
        let cand = base.join(&name);
        let cand_s = cand.to_string_lossy().into_owned();
        if probe.steps.iter().any(|s| s.candidate == cand_s) {
            continue;
        }
        let outcome = check(&cand).unwrap_or(Outcome::Missing);
        probe.steps.push(Step {
            source: source.to_string(),
            candidate: cand_s.clone(),
            outcome,
        });
        if outcome == Outcome::Found {
            probe.link_target = link_target(&cand);
            probe.resolved = Some(cand_s);
            probe.source = Some(source.to_string());
            return true;
        }
    }
    false
}

/// 按当前配置解析一个程序（不走缓存）。
pub fn resolve(program: &str) -> Probe {
    let policy = Policy::current();
    let explicit = match program {
        "docker" => policy.docker_path.clone(),
        "docker-compose" => policy.compose_path.clone(),
        _ => String::new(),
    };
    resolve_with(program, &explicit, &policy)
}

// ── 缓存 ──────────────────────────────────────────────────────────────────

fn cache() -> &'static RwLock<Option<Probe>> {
    static C: OnceLock<RwLock<Option<Probe>>> = OnceLock::new();
    C.get_or_init(|| RwLock::new(None))
}

/// docker 的探测结果。整个进程只解析一次，之后从缓存取。
pub fn docker_probe() -> Probe {
    if let Ok(g) = cache().read() {
        if let Some(p) = g.as_ref() {
            return p.clone();
        }
    }
    let p = resolve("docker");
    crate::linfo!("docker 定位：{}", p.one_line());
    if !p.found() {
        for l in p.detail_lines() {
            crate::linfo!("  {l}");
        }
    }
    if let Ok(mut g) = cache().write() {
        *g = Some(p.clone());
    }
    p
}

/// **所有调用 docker 的地方都走这一个函数**，不许有的地方用 PATH、有的地方用绝对路径。
///
/// 解析不到时返回裸程序名 `"docker"`：这样最坏情况和 I4 之前完全一样
/// （由操作系统自己去 PATH 上找，找不到就是 ENOENT → `installed: false`），
/// 而不是多出一种「启动器自己不肯试」的新失败模式。
pub fn docker_bin() -> String {
    docker_probe()
        .resolved
        .unwrap_or_else(|| "docker".to_string())
}

/// 清掉缓存。用户点「重新检测」时调 —— 他很可能刚装完 Docker 或刚把 OrbStack 点起来。
pub fn invalidate() {
    if let Ok(mut g) = cache().write() {
        *g = None;
    }
    if let Ok(mut g) = compose_cache().write() {
        *g = None;
    }
}

// ── compose ───────────────────────────────────────────────────────────────

/// compose 是怎么来的。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "path")]
pub enum ComposeMode {
    /// `docker compose`（v2 的 CLI 插件，现在的标准形态）
    Plugin,
    /// 独立的 `docker-compose` 可执行文件（v1 的形态，也有人手动装 v2 的独立版）
    Standalone(String),
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeInfo {
    pub mode: ComposeMode,
    pub version: Option<String>,
    /// 独立 docker-compose 的探测过程；用的是插件时为 `None`
    pub probe: Option<Probe>,
}

impl ComposeInfo {
    /// 跑一条 compose 命令时的 (程序, 前缀参数)。
    ///
    /// * 插件：`docker` + `["compose"]`
    /// * 独立：`/usr/local/bin/docker-compose` + `[]`
    pub fn argv_prefix(&self) -> (String, Vec<String>) {
        match &self.mode {
            ComposeMode::Plugin => (docker_bin(), vec!["compose".to_string()]),
            ComposeMode::Standalone(p) => (p.clone(), Vec::new()),
        }
    }

    pub fn label(&self) -> String {
        match &self.mode {
            ComposeMode::Plugin => format!("docker compose 插件{}", ver_suffix(&self.version)),
            ComposeMode::Standalone(p) => {
                format!("独立 docker-compose{} · {p}", ver_suffix(&self.version))
            }
        }
    }
}

fn ver_suffix(v: &Option<String>) -> String {
    match v {
        Some(v) => format!(" {v}"),
        None => String::new(),
    }
}

fn compose_cache() -> &'static RwLock<Option<ComposeInfo>> {
    static C: OnceLock<RwLock<Option<ComposeInfo>>> = OnceLock::new();
    C.get_or_init(|| RwLock::new(None))
}

/// compose 用哪一种。先试 `docker compose version`，不行再找独立的 `docker-compose`。
///
/// 顺序不能反：现在绝大多数机器上是 CLI 插件，先试插件能省一次目录遍历；
/// 而独立 `docker-compose` 在 v1 与部分 Podman 环境里才是唯一的那一个。
pub fn compose_info() -> ComposeInfo {
    if let Ok(g) = compose_cache().read() {
        if let Some(c) = g.as_ref() {
            return c.clone();
        }
    }
    let info = detect_compose();
    crate::linfo!("compose 定位：{}", info.label());
    if let Ok(mut g) = compose_cache().write() {
        *g = Some(info.clone());
    }
    info
}

fn detect_compose() -> ComposeInfo {
    use std::time::Duration;
    // ① docker compose（插件）
    if let Ok(r) = crate::proc::run_timeout(
        &docker_bin(),
        &["compose", "version", "--format", "json"],
        Duration::from_secs(20),
    ) {
        if r.ok() {
            let v = serde_json::from_str::<serde_json::Value>(r.stdout.trim())
                .ok()
                .and_then(|v| v.get("version")?.as_str().map(str::to_string));
            return ComposeInfo {
                mode: ComposeMode::Plugin,
                version: v,
                probe: None,
            };
        }
    }
    // ② 独立 docker-compose
    let probe = resolve("docker-compose");
    if let Some(p) = probe.resolved.clone() {
        let v = crate::proc::run_timeout(&p, &["version", "--short"], Duration::from_secs(20))
            .ok()
            .filter(|r| r.ok())
            .map(|r| r.stdout.trim().to_string())
            .filter(|s| !s.is_empty());
        return ComposeInfo {
            mode: ComposeMode::Standalone(p),
            version: v,
            probe: Some(probe),
        };
    }
    // 两条都没有：仍然按插件形态返回（跑起来会失败并给出 docker 的原话），
    // 版本为 None，上层据此判「compose 不可用」。
    ComposeInfo {
        mode: ComposeMode::Plugin,
        version: None,
        probe: Some(probe),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("hunter-which-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn fake_exe(dir: &Path, name: &str, mode: u32) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(b"#!/bin/sh\necho hi\n").unwrap();
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let _ = mode;
        p
    }

    fn policy(dirs: &[&Path]) -> Policy {
        Policy {
            docker_path: String::new(),
            compose_path: String::new(),
            extra_dirs: dirs
                .iter()
                .map(|d| d.to_string_lossy().into_owned())
                .collect(),
            // 单测不能受跑测试的这台机器上真有没有 docker 的影响
            use_builtin: false,
            use_env_path: false,
        }
    }

    /// 这一条就是用户 Mac 上那个 P0 的复现：PATH 里没有，已知位置里有。
    #[test]
    fn path_里没有但已知位置里有时也能找到() {
        let d = tmpdir("known");
        fake_exe(&d, "docker", 0o755);
        let p = resolve_with("docker", "", &policy(&[&d]));
        assert!(p.found(), "{:?}", p.steps);
        assert_eq!(
            p.resolved.as_deref(),
            Some(d.join("docker").to_string_lossy().as_ref())
        );
        assert_eq!(p.source.as_deref(), Some("已知位置"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 按顺序取第一个可执行的() {
        let a = tmpdir("order-a");
        let b = tmpdir("order-b");
        fake_exe(&a, "docker", 0o755);
        fake_exe(&b, "docker", 0o755);
        let p = resolve_with("docker", "", &policy(&[&a, &b]));
        assert_eq!(
            p.resolved.as_deref(),
            Some(a.join("docker").to_string_lossy().as_ref()),
            "前面的目录优先"
        );
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    /// 文件在但没有可执行位 —— 要**跳过它继续找**，并且如实记下这一条是什么原因跳的。
    #[test]
    #[cfg(unix)]
    fn 没有可执行位的要跳过而不是当成找到了() {
        let a = tmpdir("noexec-a");
        let b = tmpdir("noexec-b");
        fake_exe(&a, "docker", 0o644);
        fake_exe(&b, "docker", 0o755);
        let p = resolve_with("docker", "", &policy(&[&a, &b]));
        assert_eq!(
            p.resolved.as_deref(),
            Some(b.join("docker").to_string_lossy().as_ref())
        );
        let first = &p.steps[0];
        assert_eq!(first.outcome, Outcome::NotExecutable);
        assert!(
            p.detail_lines()[0].contains("没有可执行位"),
            "{:?}",
            p.detail_lines()
        );
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    /// 探测路径全指向空目录 = I4 测试场景 1「Docker 完全没装」。
    #[test]
    fn 全是空目录时如实报没找到并留下探测明细() {
        let d = tmpdir("empty");
        let p = resolve_with("docker", "", &policy(&[&d]));
        assert!(!p.found());
        // Windows 上一个目录要试 4 种后缀（.exe/.cmd/.bat/裸名），别的平台只有 1 种
        assert_eq!(p.steps.len(), name_variants("docker").len());
        assert!(p.steps.iter().all(|s| s.outcome == Outcome::Missing));
        assert!(p.one_line().contains("没找到 docker"), "{}", p.one_line());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 手动指定了就只认它：指的那个不存在时**不许**悄悄回退到别处的 docker。
    #[test]
    fn 手动指定的路径不存在时不回退() {
        let d = tmpdir("explicit");
        fake_exe(&d, "docker", 0o755);
        let p = resolve_with("docker", "/nowhere/nothing/docker", &policy(&[&d]));
        assert!(!p.found(), "指定的那个不在，就该如实说不在");
        assert_eq!(p.steps.len(), 1, "不该再往下探：{:?}", p.steps);
        assert_eq!(p.steps[0].source, "配置");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 手动指定的路径存在时直接用() {
        let d = tmpdir("explicit-ok");
        let exe = fake_exe(&d, "mydocker", 0o755);
        let p = resolve_with("docker", exe.to_string_lossy().as_ref(), &Policy::default());
        assert_eq!(p.resolved.as_deref(), Some(exe.to_string_lossy().as_ref()));
        assert_eq!(p.source.as_deref(), Some("配置"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 软链要显示指向哪里 —— 用户 Mac 上 `/usr/local/bin/docker` 指向 OrbStack 的 xbin，
    /// 界面上不写出来的话，他根本看不出启动器用的到底是哪一个 docker。
    #[test]
    #[cfg(unix)]
    fn 软链要报出指向哪里() {
        let real = tmpdir("link-real");
        let link = tmpdir("link-dir");
        let target = fake_exe(&real, "docker", 0o755);
        std::os::unix::fs::symlink(&target, link.join("docker")).unwrap();
        let p = resolve_with("docker", "", &policy(&[&link]));
        assert!(p.found());
        assert_eq!(
            p.link_target.as_deref(),
            Some(target.to_string_lossy().as_ref())
        );
        assert!(p.one_line().contains("软链指向"), "{}", p.one_line());
        let _ = std::fs::remove_dir_all(&real);
        let _ = std::fs::remove_dir_all(&link);
    }

    /// 同一个目录在 PATH 与内置清单里都出现时只探一次，明细里不该有重复行。
    #[test]
    fn 重复目录只探一次() {
        let d = tmpdir("dup");
        let mut pol = policy(&[&d, &d]);
        pol.extra_dirs.push(d.to_string_lossy().into_owned());
        let p = resolve_with("docker", "", &pol);
        // 三个条目指的是同一个目录，所以只该探出「一个目录份」的候选
        // （Windows 上一个目录是 4 条后缀变体，别的平台是 1 条）
        assert_eq!(
            p.steps.len(),
            name_variants("docker").len(),
            "同一个目录被探了不止一遍：{:?}",
            p.steps
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 家目录展开() {
        let home = crate::paths::home();
        assert_eq!(expand_tilde("~/.orbstack/bin"), home.join(".orbstack/bin"));
        assert_eq!(
            expand_tilde("/usr/local/bin"),
            PathBuf::from("/usr/local/bin")
        );
    }

    /// 内置清单必须覆盖 I4 点名的那几个 macOS 位置。
    /// 这一条是**在 Linux 上也要过**的编译期清单检查 —— macOS 分支跑不了真机测试，
    /// 至少保证清单本身不会被谁顺手删掉一条。
    #[test]
    fn macos_内置清单覆盖点名的位置() {
        // 直接检查源码里那一段常量，避免依赖当前编译目标
        let mac = [
            "/usr/local/bin",
            "/opt/homebrew/bin",
            "~/.orbstack/bin",
            "~/.docker/bin",
            "/Applications/Docker.app/Contents/Resources/bin",
            "/Applications/OrbStack.app/Contents/MacOS/xbin",
            "/usr/bin",
            "/bin",
        ];
        if cfg!(target_os = "macos") {
            let dirs = builtin_dirs();
            for m in mac {
                assert!(dirs.contains(&m), "内置清单少了 {m}");
            }
        } else {
            // 非 macOS 上至少确认 Linux 的常见位置在
            let dirs = builtin_dirs();
            if cfg!(target_os = "linux") {
                for m in ["/usr/bin", "/usr/local/bin", "/bin", "/snap/bin"] {
                    assert!(dirs.contains(&m), "Linux 清单少了 {m}");
                }
            }
        }
    }

    #[test]
    fn 目录分隔符按平台() {
        if cfg!(windows) {
            assert_eq!(split_dirs(r"C:\a;C:\b"), vec![r"C:\a", r"C:\b"]);
        } else {
            assert_eq!(split_dirs("/a:/b::/c"), vec!["/a", "/b", "/c"]);
        }
    }

    #[test]
    fn compose_两种形态的参数前缀() {
        let plugin = ComposeInfo {
            mode: ComposeMode::Plugin,
            version: Some("v5.5.1".into()),
            probe: None,
        };
        let (_, pre) = plugin.argv_prefix();
        assert_eq!(pre, vec!["compose".to_string()]);
        assert!(plugin.label().contains("插件"));

        let alone = ComposeInfo {
            mode: ComposeMode::Standalone("/usr/local/bin/docker-compose".into()),
            version: Some("2.29.0".into()),
            probe: None,
        };
        let (prog, pre) = alone.argv_prefix();
        assert_eq!(prog, "/usr/local/bin/docker-compose");
        assert!(pre.is_empty(), "独立形态不该再带 compose 子命令");
        assert!(alone.label().contains("独立"));
    }
}
