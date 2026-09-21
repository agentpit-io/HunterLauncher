//! 子进程环境（I6 的 P0）。
//!
//! ## 为什么要有这个模块
//!
//! I4 修的是「**启动器**找不到 docker」：macOS 上从访达 / 程序坞双击起来的程序
//! 拿到的 PATH 只有 `/usr/bin:/bin:/usr/sbin:/sbin`，于是 `Command::new("docker")`
//! 必然 ENOENT。[`super::which`] 按已知位置把 docker 的**绝对路径**找了出来，那一条修好了。
//!
//! 0.1.5 在用户 Mac 上真机验收时撞出的是**下一层**的同一个毛病：
//! **docker 自己也找不到它的插件与凭据助手**。现场原话（`plan/mac-auto-0.1.5.log`）：
//!
//! ```text
//! error getting credentials - err: exec: "docker-credential-osxkeychain":
//!   executable file not found in $PATH, out: ``
//! ```
//!
//! 用户的 `~/.docker/config.json` 里写着 `"credsStore": "osxkeychain"`
//! （OrbStack / Docker Desktop 装的时候自己写进去的），而
//! `docker-credential-osxkeychain` 在 `/usr/local/bin`（软链到 OrbStack 的 xbin）。
//! 我们用绝对路径把 docker 起起来了，但**子进程继承的还是那份受限 PATH**，
//! docker 去 `$PATH` 上找助手，一样找不到。
//!
//! 实测：同一条 `docker compose pull`，把 `/usr/local/bin` 补进 PATH 之后立刻成功。
//!
//! 0.1.4 那次 GUI 拉取能成功，是因为那一次是从终端 `open` 起来的、继承了终端的 PATH ——
//! **不代表双击能成功**。
//!
//! ## 这个模块做什么
//!
//! 给所有子进程统一构造环境，**一处实现、所有调用点复用**（[`crate::proc::base_command`]
//! 是唯一的建 `Command` 的地方，这里挂在它上面）：
//!
//! | 项 | 取值 |
//! |---|---|
//! | `PATH` | 原 PATH + docker 所在目录 + 它软链指向的目录 + 平台已知位置（[`super::which::builtin_dirs`] 同一份清单）。**去重、保序、只加真实存在的目录** |
//! | `DOCKER_CONFIG` | 只有在开了「隔离 docker 配置」时才设（见 [`crate::dockercfg`]）；否则原样继承 |
//!
//! 补进来的目录**排在原 PATH 后面**：原来能找到的东西优先级不变，我们只负责把
//! 「本来一个都找不到」变成「找得到」，不抢用户 PATH 里已有的同名程序。
//!
//! Linux 与 Windows 一样走这一条：Windows 上 Docker Desktop 的
//! `docker-credential-desktop.exe` 与 `docker-compose.exe` 在
//! `C:\Program Files\Docker\Docker\resources\bin`，从计划任务 / 服务里起来的进程
//! 同样可能没有它。

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use super::which;

/// 一次环境构造的结果。日志与诊断报文直接用它。
#[derive(Debug, Clone, Default)]
pub struct SubEnv {
    /// 最终给子进程的 PATH
    pub path: String,
    /// 在原 PATH 基础上**补进去**的目录（按补进去的顺序）
    pub added: Vec<String>,
    /// 给子进程的 `DOCKER_CONFIG`；`None` = 原样继承调用方的
    pub docker_config: Option<String>,
}

impl SubEnv {
    /// 给人看的一行。没补任何目录时也要如实说「一个都没补」。
    pub fn one_line(&self) -> String {
        let head = if self.added.is_empty() {
            "子进程 PATH：原样继承（没有需要补的目录）".to_string()
        } else {
            format!(
                "子进程 PATH 补了 {} 个目录：{}",
                self.added.len(),
                self.added.join("、")
            )
        };
        match &self.docker_config {
            Some(d) => format!("{head}；DOCKER_CONFIG={}", crate::redact::mask_home(d)),
            None => head,
        }
    }
}

/// PATH 的分隔符。
fn sep() -> char {
    if cfg!(windows) {
        ';'
    } else {
        ':'
    }
}

/// 比较两个目录是不是同一个。
///
/// 先按字符串规范化（去掉末尾分隔符；Windows 上大小写不敏感），
/// 再拿 `canonicalize` 的结果兜一道 —— `/usr/local/bin` 与
/// `/Applications/OrbStack.app/…/xbin` 在用户那台 mac 上是软链关系，
/// 两个都补进去没有坏处，但 `~/.docker/bin` 与 `$HOME/.docker/bin` 这种
/// 写法差异必须认出来，否则 PATH 里会出现一串重复项。
fn same_dir(a: &str, b: &str) -> bool {
    if norm(a) == norm(b) {
        return true;
    }
    match (std::fs::canonicalize(a).ok(), std::fs::canonicalize(b).ok()) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn norm(s: &str) -> String {
    let t = s.trim();
    let t = t.trim_end_matches(['/', '\\']);
    let t = if t.is_empty() { s.trim() } else { t };
    if cfg!(windows) {
        t.to_lowercase()
    } else {
        t.to_string()
    }
}

/// 一个目录值不值得补：得**存在**、得**是目录**。
///
/// 不存在的目录补进 PATH 只会让每一次 `exec` 多一次无用的 `stat`，
/// 而且会把日志里那一行「补了哪些目录」变成一串谎话。
fn usable(dir: &str) -> bool {
    let p = which::expand_tilde(dir);
    p.is_dir()
}

/// 候选目录清单：docker 所在目录 → 它软链指向的目录 → 配置追加的目录 → 平台已知位置。
///
/// **与 [`super::which`] 用的是同一份清单**（`builtin_dirs` + `Policy::extra_dirs`），
/// 不在这里另抄一份 —— 抄一份就一定会有哪天只改了一边。
pub fn candidate_dirs() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let probe = which::docker_probe();
    // ① docker 自己在哪，它的插件与助手十有八九就在旁边
    if let Some(p) = probe.resolved.as_deref() {
        if let Some(d) = Path::new(p).parent() {
            out.push(d.to_string_lossy().into_owned());
        }
    }
    // ② 软链指向哪（用户 mac 上 /usr/local/bin/docker → OrbStack 的 xbin，
    //    `docker-credential-osxkeychain` 两边都有一份）
    if let Some(t) = probe.link_target.as_deref() {
        if let Some(d) = Path::new(t).parent() {
            out.push(d.to_string_lossy().into_owned());
        }
    }
    // ③④ 与 which 同一份清单
    let policy = which::Policy::current();
    out.extend(policy.extra_dirs.clone());
    if policy.use_builtin {
        out.extend(which::builtin_dirs().into_iter().map(str::to_string));
    }
    out
}

/// 纯函数版本：给定原 PATH 与候选目录，算出最终 PATH 与补进去的那几个。
///
/// 拆成纯函数是为了能在**任何平台的 CI 上**测它（`exists` 由调用方给），
/// 不依赖跑测试的这台机器上到底装没装 docker。
pub fn build_path(base: &str, candidates: &[String], exists: &dyn Fn(&str) -> bool) -> SubEnv {
    let mut dirs: Vec<String> = base
        .split(sep())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let mut added: Vec<String> = Vec::new();
    for c in candidates {
        let expanded = which::expand_tilde(c).to_string_lossy().into_owned();
        if expanded.trim().is_empty() {
            continue;
        }
        if dirs.iter().any(|d| same_dir(d, &expanded)) {
            continue;
        }
        if added.iter().any(|d| same_dir(d, &expanded)) {
            continue;
        }
        if !exists(&expanded) {
            continue;
        }
        dirs.push(expanded.clone());
        added.push(expanded);
    }
    SubEnv {
        path: dirs.join(&sep().to_string()),
        added,
        docker_config: None,
    }
}

/// 算一次当前这台机器上的子进程环境。
fn compute() -> SubEnv {
    let base = std::env::var("PATH").unwrap_or_default();
    let mut e = build_path(&base, &candidate_dirs(), &usable);
    e.docker_config =
        crate::dockercfg::isolated_dir_if_enabled().map(|p| p.to_string_lossy().into_owned());
    e
}

fn cache() -> &'static RwLock<Option<SubEnv>> {
    static C: OnceLock<RwLock<Option<SubEnv>>> = OnceLock::new();
    C.get_or_init(|| RwLock::new(None))
}

/// 当前的子进程环境（进程内缓存一次）。
pub fn current() -> SubEnv {
    if let Ok(g) = cache().read() {
        if let Some(e) = g.as_ref() {
            return e.clone();
        }
    }
    let e = compute();
    crate::linfo!("{}", e.one_line());
    if let Ok(mut g) = cache().write() {
        *g = Some(e.clone());
    }
    e
}

/// 清掉缓存。用户点「重新检测」、或者刚开了隔离 docker 配置时调。
pub fn invalidate() {
    if let Ok(mut g) = cache().write() {
        *g = None;
    }
}

/// 把环境套到一个 `Command` 上。**所有子进程都从这里过**。
pub fn apply(cmd: &mut std::process::Command) {
    // 测试里要能关掉它：单测造的是「受限 PATH」的现场，补全会把现场毁掉
    if matches!(
        std::env::var("HUNTER_SUBPROCESS_PATH").as_deref(),
        Ok("0") | Ok("false") | Ok("off")
    ) {
        return;
    }
    let e = current();
    if !e.path.is_empty() {
        cmd.env("PATH", &e.path);
    }
    if let Some(d) = &e.docker_config {
        cmd.env("DOCKER_CONFIG", d);
    }
}

/// 在**补全后的搜索路径**上找一个程序（例如 `docker-credential-osxkeychain`）。
///
/// 用的就是给子进程的那份 PATH —— 「我们能不能找到它」和「docker 能不能找到它」
/// 必须是同一个答案，分成两套逻辑迟早会对不上。
pub fn find_on_subprocess_path(program: &str) -> Option<PathBuf> {
    let e = current();
    for dir in e.path.split(sep()) {
        let dir = dir.trim();
        if dir.is_empty() {
            continue;
        }
        for name in name_variants(program) {
            let cand = Path::new(dir).join(&name);
            if is_exec(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

fn name_variants(program: &str) -> Vec<String> {
    if cfg!(windows) && !program.contains('.') {
        vec![
            format!("{program}.exe"),
            format!("{program}.cmd"),
            format!("{program}.bat"),
            program.to_string(),
        ]
    } else {
        vec![program.to_string()]
    }
}

fn is_exec(p: &Path) -> bool {
    let Ok(md) = std::fs::metadata(p) else {
        return false;
    };
    if !md.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return md.permissions().mode() & 0o111 != 0;
    }
    #[cfg(not(unix))]
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("hunter-subenv-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn fake_exe(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    fn joined(v: &[&str]) -> String {
        v.join(&sep().to_string())
    }

    fn all_exist(_: &str) -> bool {
        true
    }

    /// **这一条就是用户 Mac 上 0.1.5 那个 P0 的复现**：受限 PATH（GUI 程序拿到的那一份）
    /// 里没有 `/usr/local/bin`，而 `docker-credential-osxkeychain` 就在那儿。
    #[test]
    fn 受限_path_下要把_docker_所在目录补进来() {
        let base = joined(&["/usr/bin", "/bin", "/usr/sbin", "/sbin"]);
        let cands = vec!["/usr/local/bin".to_string()];
        let e = build_path(&base, &cands, &all_exist);
        assert_eq!(e.added, vec!["/usr/local/bin".to_string()]);
        assert!(
            e.path.ends_with("/usr/local/bin"),
            "补进来的要排在原 PATH 后面：{}",
            e.path
        );
        assert!(e.path.starts_with("/usr/bin"), "原 PATH 的顺序不能动");
        assert!(e.one_line().contains("/usr/local/bin"), "{}", e.one_line());
    }

    #[test]
    fn 已经在_path_里的目录不重复补() {
        let base = joined(&["/usr/bin", "/usr/local/bin"]);
        let cands = vec!["/usr/local/bin".to_string(), "/usr/bin/".to_string()];
        let e = build_path(&base, &cands, &all_exist);
        assert!(e.added.is_empty(), "{:?}", e.added);
        assert_eq!(e.path, base);
        assert!(
            e.one_line().contains("没有需要补的目录"),
            "{}",
            e.one_line()
        );
    }

    #[test]
    fn 候选里重复的只补一次() {
        let base = joined(&["/usr/bin"]);
        let cands = vec![
            "/usr/local/bin".to_string(),
            "/usr/local/bin/".to_string(),
            "/usr/local/bin".to_string(),
        ];
        let e = build_path(&base, &cands, &all_exist);
        assert_eq!(e.added.len(), 1, "{:?}", e.added);
    }

    #[test]
    fn 不存在的目录不补() {
        let base = joined(&["/usr/bin"]);
        let cands = vec!["/no/such/dir".to_string(), "/usr/local/bin".to_string()];
        let e = build_path(&base, &cands, &|d: &str| d == "/usr/local/bin");
        assert_eq!(e.added, vec!["/usr/local/bin".to_string()]);
    }

    #[test]
    fn 候选保序() {
        let base = joined(&["/usr/bin"]);
        let cands = vec!["/a".to_string(), "/b".to_string(), "/c".to_string()];
        let e = build_path(&base, &cands, &all_exist);
        assert_eq!(
            e.added,
            vec!["/a".to_string(), "/b".to_string(), "/c".to_string()]
        );
    }

    #[test]
    fn 家目录写法要展开再比() {
        let home = crate::paths::home();
        let docker_bin = home.join(".docker/bin");
        let base = joined(&[docker_bin.to_string_lossy().as_ref()]);
        let cands = vec!["~/.docker/bin".to_string()];
        let e = build_path(&base, &cands, &all_exist);
        assert!(
            e.added.is_empty(),
            "`~/.docker/bin` 和展开后的同一个目录不能补两遍：{:?}",
            e.added
        );
    }

    /// 内置清单必须覆盖 macOS 上凭据助手真实所在的那几个目录 ——
    /// 这一条**在 Linux 上也要过**，只检查清单本身（macOS runner 上另有真跑的那一条）。
    #[test]
    fn 候选清单覆盖凭据助手的已知位置() {
        if cfg!(target_os = "macos") {
            let dirs = which::builtin_dirs();
            for d in [
                "/usr/local/bin",
                "/opt/homebrew/bin",
                "/Applications/OrbStack.app/Contents/MacOS/xbin",
                "/Applications/Docker.app/Contents/Resources/bin",
                "~/.docker/bin",
            ] {
                assert!(dirs.contains(&d), "macOS 清单少了 {d}");
            }
        } else if cfg!(target_os = "windows") {
            let dirs = which::builtin_dirs();
            assert!(dirs.contains(&r"C:\Program Files\Docker\Docker\resources\bin"));
        } else {
            let dirs = which::builtin_dirs();
            assert!(dirs.contains(&"/usr/local/bin"));
        }
    }

    /// **CI 的 macOS runner 上真跑这一条**（Linux / Windows 上也一样跑得动，
    /// 因为造的是临时目录，不依赖这台机器上有没有 docker）：
    /// 受限 PATH + 助手在已知位置 → 必须找得到。
    #[test]
    #[cfg(unix)]
    fn 受限_path_下要能找到凭据助手() {
        let d = tmpdir("credhelper");
        let helper = fake_exe(&d, "docker-credential-osxkeychain");
        let base = joined(&["/usr/bin", "/bin", "/usr/sbin", "/sbin"]);
        let cands = vec![d.to_string_lossy().into_owned()];
        let e = build_path(&base, &cands, &usable);
        assert_eq!(e.added.len(), 1, "{:?}", e.added);

        // 用最终那份 PATH 去找，结果必须是那个假助手
        let found = e
            .path
            .split(sep())
            .map(|dir| Path::new(dir).join("docker-credential-osxkeychain"))
            .find(|p| is_exec(p));
        assert_eq!(found.as_deref(), Some(helper.as_path()));

        // 反过来：受限 PATH 本身找不到它 —— 这正是 0.1.5 的现场
        let none = base
            .split(sep())
            .map(|dir| Path::new(dir).join("docker-credential-osxkeychain"))
            .find(|p| is_exec(p));
        assert!(none.is_none(), "受限 PATH 不该找得到它：{none:?}");

        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 没有可执行位的助手不算找到() {
        let d = tmpdir("noexec");
        let p = d.join("docker-credential-x");
        std::fs::write(&p, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(!is_exec(&p));
        }
        #[cfg(not(unix))]
        assert!(is_exec(&p));
        let _ = std::fs::remove_dir_all(&d);
    }
}
