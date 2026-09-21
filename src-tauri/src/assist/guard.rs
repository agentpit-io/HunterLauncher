//! 守卫（I5 · 设计文档 §3.3、§四）。**不靠提示词，靠代码。**
//!
//! 一次授权页上写给用户的那五条「AI 绝不会做的事」，在这里各有一条对应的硬校验。
//! 提示词里也写了同样的话，但提示词是**建议**，这个文件是**执行路径上唯一的门**：
//! 模型想干、界面有 bug、或者以后有人往动作表里加了危险的东西，都要先过这里。
//!
//! | 承诺 | 这里的实现 |
//! |---|---|
//! | 不删你的任何文件 | [`writable_path`]：`canonicalize` 之后必须落在 `~/.hunter/` 内；[`deletable_path`] 再限定到「启动器自己生成的那几个文件」的白名单 |
//! | 不删不停你别的容器 | [`own_container`]：执行前查 `com.docker.compose.project` 标签，不是 `hunter` 一律拒绝；[`argv`] 强制 docker 操作带 `--project-name hunter` |
//! | 不改你的网络设置 | [`argv`]：`networksetup` / `scutil` / `pfctl` / `iptables` / `/etc/hosts` 等一律拒绝 |
//! | 不用管理员密码 | [`argv`]：`sudo` / `pkexec` / `runas` / `osascript … administrator privileges` 一律拒绝 |
//! | 不执行它自己编的命令 | 动作表机制（[`super::actions`]）+ [`argv`] 里的 `sh -c` 类拦截 |
//!
//! ## 为什么路径要 `canonicalize`
//!
//! `~/.hunter/../../Documents/x` 这种写法，字符串前缀比对会放行。`canonicalize`
//! 会解开 `..` 与软链，拿到真实位置再比 —— 这是唯一可靠的判断方式。
//! 文件不存在时 `canonicalize` 会失败，所以对「要创建的文件」改为 canonicalize
//! 它的父目录，再拼上文件名。
//!
//! ## 审计
//!
//! 每一次校验（通过与拒绝）都写 `~/.hunter/logs/assist-audit.log`，
//! 一行一条 JSON：时间、动作、参数、由谁提出、结果。**这个文件不进诊断包也不上报**
//! （它里面有路径），只给用户自己看。

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::PROJECT;
use crate::err::{AppError, AppResult, Code};

/// 动作的风险级别（设计文档 §五）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// 只看不改
    ReadOnly,
    /// 只改 `~/.hunter`，或只影响本项目 `hunter` 的容器
    Safe,
    /// 会影响这台机器上别的东西。**`auto` 档下也要先问用户**
    Sensitive,
}

impl Level {
    pub fn cn(self) -> &'static str {
        match self {
            Level::ReadOnly => "只读",
            Level::Safe => "安全（只动 Hunter 自己的东西）",
            Level::Sensitive => "需要你同意",
        }
    }
}

/// 授权档位（设计文档 §3.2）。存 `launcher.toml [assist] mode`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// 自动驾驶：动作表内的 Safe 动作直接执行；Sensitive 仍然要问
    Auto,
    /// 逐步确认：和 0.1.4 一样，改动类动作逐条弹确认
    Confirm,
    /// 关闭：只跑确定性规则，不调模型
    Off,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Mode::Auto,
            "off" => Mode::Off,
            // 认不得的值一律落到最保守的那一档（不是最方便的那一档）
            _ => Mode::Confirm,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Confirm => "confirm",
            Mode::Off => "off",
        }
    }
    pub fn cn(self) -> &'static str {
        match self {
            Mode::Auto => "自动驾驶",
            Mode::Confirm => "逐步确认",
            Mode::Off => "关闭",
        }
    }
    /// 这个级别的动作，在这一档下要不要先问用户。
    pub fn needs_confirm(self, level: Level) -> bool {
        match (self, level) {
            (_, Level::ReadOnly) => false,
            // Sensitive 在**任何**档位下都要问 —— 包括自动驾驶
            (_, Level::Sensitive) => true,
            (Mode::Auto, Level::Safe) => false,
            (Mode::Confirm, Level::Safe) => true,
            (Mode::Off, Level::Safe) => true,
        }
    }
}

// ── 路径守卫 ──────────────────────────────────────────────────────────────

/// 把一个路径解析成真实位置，并确认它落在 `~/.hunter/` 内。
///
/// 返回解析后的绝对路径。**只有它能进后续的写/删**（不要再用调用方给的原串）。
pub fn writable_path(p: &Path) -> AppResult<PathBuf> {
    let root = canon_root()?;
    let real = canon_for_write(p)?;
    if !real.starts_with(&root) {
        return Err(reject(format!(
            "路径 {} 解析之后落在 {} 外面，拒绝（AI 只能在 Hunter 自己的文件夹里动东西）。",
            crate::redact::mask_home(&real.to_string_lossy()),
            crate::redact::mask_home(&root.to_string_lossy())
        )));
    }
    Ok(real)
}

/// 启动器**自己生成**的文件。只有这张表里的才允许删。
///
/// 为什么是白名单而不是「`~/.hunter` 下随便删」：用户完全可能把自己的东西放进去
/// （备份、导出的诊断包、手抄的 .env 副本），而数据卷更是一删就没。
/// 会员名单短一点、清楚一点，比规则复杂一点好。
pub fn deletable_files() -> Vec<PathBuf> {
    vec![
        crate::paths::override_file(),
        crate::paths::compose_file(),
        crate::paths::version_file(),
        crate::paths::app_dir().join("docker-compose.launcher.yml.bak"),
    ]
}

/// 允许**整棵删掉**的目录。目前只有一个：内置运行时（I7）。
///
/// 它和 [`deletable_files`] 是同一条原则的两种形态 —— 只有**启动器自己从零
/// 生成**的东西才允许删。`~/.hunter/runtime` 下的每一个字节都是
/// [`crate::runtime::builtin::install`] 写进去的：下载回来的四个二进制、
/// 解压出来的 lima、colima 建的虚拟机磁盘。用户不会往这里放东西
/// （界面上也从不引导他往这里放），所以整棵删是安全的。
///
/// `~/.hunter` 下别的目录一个都不在这张表里：`app/` 有 `.env`（他的 key）、
/// `backups/` 是他的备份、`diagnostics/` 是他导出的包。
pub fn deletable_trees() -> Vec<PathBuf> {
    vec![crate::paths::runtime_dir()]
}

/// 允许整棵删吗。先过 [`writable_path`]，再比对目录白名单。
pub fn deletable_tree(p: &Path) -> AppResult<PathBuf> {
    let real = writable_path(p)?;
    let ok = deletable_trees()
        .iter()
        .any(|w| matches!(canon_for_write(w), Ok(c) if c == real));
    if !ok {
        return Err(reject(format!(
            "{} 不在「可以整棵删掉」的清单里（那里面可能有你自己的东西），拒绝。",
            crate::redact::mask_home(&real.to_string_lossy())
        )));
    }
    Ok(real)
}

/// 允许删吗。先过 [`writable_path`]，再比对白名单。
pub fn deletable_path(p: &Path) -> AppResult<PathBuf> {
    let real = writable_path(p)?;
    let ok = deletable_files().iter().any(|w| {
        // 白名单里的文件可能还不存在，canonicalize 不了；按父目录 + 文件名比
        match canon_for_write(w) {
            Ok(c) => c == real,
            Err(_) => false,
        }
    });
    if !ok {
        return Err(reject(format!(
            "{} 不在「启动器自己生成的文件」清单里，拒绝删除。",
            crate::redact::mask_home(&real.to_string_lossy())
        )));
    }
    Ok(real)
}

fn canon_root() -> AppResult<PathBuf> {
    let root = crate::paths::root();
    std::fs::create_dir_all(&root).ok();
    root.canonicalize().map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("解析不了工作目录 {}：{e}", root.display()),
        )
    })
}

/// 文件可能还不存在（要创建），所以 canonicalize 它的父目录再拼文件名。
fn canon_for_write(p: &Path) -> AppResult<PathBuf> {
    if let Ok(c) = p.canonicalize() {
        return Ok(c);
    }
    let parent = p.parent().ok_or_else(|| {
        reject(format!(
            "路径 {} 没有父目录，判断不了它在哪，拒绝。",
            crate::redact::mask_home(&p.to_string_lossy())
        ))
    })?;
    let name = p.file_name().ok_or_else(|| {
        reject(format!(
            "路径 {} 没有文件名，拒绝。",
            crate::redact::mask_home(&p.to_string_lossy())
        ))
    })?;
    let cp = parent.canonicalize().map_err(|e| {
        reject(format!(
            "解析不了 {}：{e}，拒绝。",
            crate::redact::mask_home(&parent.to_string_lossy())
        ))
    })?;
    Ok(cp.join(name))
}

// ── 容器守卫 ──────────────────────────────────────────────────────────────

/// 这个容器是**本项目**的吗。查 `com.docker.compose.project` 标签，
/// 不是 `hunter` 一律拒绝；查不到也拒绝（**不确定就不动**）。
pub fn own_container(id_or_name: &str) -> AppResult<()> {
    if id_or_name.trim().is_empty() {
        return Err(reject("没给容器名，拒绝。".to_string()));
    }
    let bin = crate::runtime::which::docker_bin();
    let r = crate::proc::run_timeout(
        &bin,
        &[
            "inspect",
            "-f",
            "{{index .Config.Labels \"com.docker.compose.project\"}}",
            id_or_name,
        ],
        std::time::Duration::from_secs(20),
    )?;
    let project = r.stdout.trim();
    if !r.ok() {
        return Err(reject(format!(
            "查不到容器「{}」的 compose 项目标签（{}），拒绝对它动手。",
            safe(id_or_name),
            r.err_line()
        )));
    }
    if project != PROJECT {
        return Err(reject(format!(
            "容器「{}」属于 compose 项目「{}」，不是本次安装的「{PROJECT}」—— \
             AI 不会动你电脑上别的容器，拒绝。",
            safe(id_or_name),
            safe(project)
        )));
    }
    Ok(())
}

/// 本项目的 6 个服务名。`restart_own_service` 只认这些。
pub const OWN_SERVICES: &[&str] = &["api", "llm-shim", "opencode", "postgres", "redis", "web"];

// ── 命令守卫 ──────────────────────────────────────────────────────────────

/// 一律拒绝的程序名（取 basename 比对，`/usr/bin/sudo` 也拦得住）。
const FORBIDDEN_PROGRAMS: &[&str] = &[
    // 提权
    "sudo",
    "su",
    "doas",
    "pkexec",
    "runas",
    "gsudo",
    // 网络与安全设置
    "networksetup",
    "scutil",
    "pfctl",
    "iptables",
    "ip6tables",
    "nft",
    "ufw",
    "firewall-cmd",
    "netsh",
    "route",
    "resolvectl",
    "systemd-resolve",
    "defaults",
    "csrutil",
    "spctl",
    "security",
    "dscl",
    // 任意 shell / 任意代码
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "cmd",
    "cmd.exe",
    "powershell",
    "pwsh",
    "osascript",
    "python",
    "python3",
    "perl",
    "ruby",
    "node",
    "env",
    "xargs",
    "eval",
    // 删除
    "rm",
    "rmdir",
    "del",
    "erase",
    "shred",
    "unlink",
    "mv",
];

/// docker 子命令里一律拒绝的组合。
const FORBIDDEN_DOCKER: &[&[&str]] = &[
    &["volume", "rm"],
    &["volume", "prune"],
    &["system", "prune"],
    &["image", "prune"],
    &["network", "prune"],
    &["builder", "prune"],
    &["container", "prune"],
    &["swarm"],
];

/// 会碰到网络/安全设置的文件。谁都不许写。
const FORBIDDEN_PATH_HINTS: &[&str] = &[
    "/etc/hosts",
    "/etc/resolv.conf",
    "/private/etc/hosts",
    "\\drivers\\etc\\hosts",
    "/Library/LaunchDaemons",
    "/Library/LaunchAgents",
];

/// 对将要 `spawn` 的参数数组做最后一道硬校验。
///
/// **执行路径上的每一个子进程都要过这里**，不管是谁提出来的。
pub fn argv(argv: &[String]) -> AppResult<()> {
    argv_scoped(argv, false)
}

/// 接管态（I7）专用的那一道。**比默认那一道更严，不是更松。**
///
/// 默认规则是「改动容器状态的 docker 命令必须 `--project-name hunter`」——
/// 接管用户自己那一套时项目名当然不是 `hunter`，所以要开一个口子。
/// 这个口子有三把锁，全是代码：
///
/// 1. `launcher.toml` 的 `[takeover] project` 必须**非空**，且命令里指名的
///    就是它 —— 用户没在「需要你」卡片上点过「直接用它」，这条路根本不存在；
/// 2. 子命令只能是 `stop` / `start` / `restart` —— `down`、`rm`、`kill` 一律不行；
/// 3. 剩下的全部检查照旧走 [`argv_scoped`]（`-v`、`volume rm`、`sudo`、
///    改网络设置……一条都不少）。
pub fn argv_takeover(argv: &[String]) -> AppResult<()> {
    let cfg = crate::config::LauncherConfig::load();
    argv_takeover_for(argv, &cfg.takeover.project)
}

/// 同上，但被接管的项目名由调用方给。
///
/// 拆出来是为了**能测它**：判定只跟「project 是什么」有关，不该为了测一条规则
/// 去改用户真实的 `launcher.toml`（测试跑在开发者自己的 `~/.hunter` 上）。
pub fn argv_takeover_for(argv: &[String], takeover_project: &str) -> AppResult<()> {
    let project = takeover_project.trim().to_ascii_lowercase();
    if project.is_empty() {
        return Err(reject(
            "现在没有在管理别的 Hunter，这条命令没有理由指向别的 compose 项目，拒绝。".to_string(),
        ));
    }
    let rest: Vec<String> = argv
        .iter()
        .skip(1)
        .map(|s| s.to_ascii_lowercase())
        .collect();
    // 指名的必须正是被接管的那一个
    let named = rest
        .windows(2)
        .find(|w| w[0] == "-p" || w[0] == "--project-name")
        .map(|w| w[1].clone());
    match named.as_deref() {
        Some(n) if n == project => {}
        Some(n) => {
            return Err(reject(format!(
                "这条命令指向 compose 项目「{}」，而你让启动器管理的是「{}」，拒绝。",
                safe(n),
                safe(&project)
            )))
        }
        None => {
            return Err(reject(
                "接管态下的命令必须写明 `--project-name`，拒绝。".to_string(),
            ))
        }
    }
    // 只有这三个子命令。`down` 会删容器与网络，`rm` / `kill` 更不用说
    const ALLOWED: &[&str] = &["stop", "start", "restart", "logs", "ps"];
    if !rest.iter().any(|a| ALLOWED.contains(&a.as_str())) {
        return Err(reject(format!(
            "接管别人那一套时只允许 {} 这几个子命令，拒绝。",
            ALLOWED.join(" / ")
        )));
    }
    for bad in ["down", "rm", "kill", "create", "up", "recreate"] {
        if rest.iter().any(|a| a == bad) {
            return Err(reject(format!(
                "`{bad}` 会改动你自己装的那一套的结构，接管态下不允许，拒绝。"
            )));
        }
    }
    // **任何**带卷的写法都拒。通用那一道只在 `down` / `rm` 上拦 `-v`
    //（那两个子命令这里本来就不允许），所以这一条是接管态自己加的一层：
    // 动的是用户自己的数据，不留任何可能碰到卷的写法
    if rest.iter().any(|a| a == "-v" || a == "--volumes") {
        return Err(reject(
            "接管态下的命令里不允许出现 `-v` / `--volumes`，拒绝。".to_string(),
        ));
    }
    argv_scoped(argv, true)
}

/// 同上，但调用方已经用 [`own_container`] 逐个核过目标容器的项目标签。
///
/// 只有这一种情况可以免掉 `--project-name hunter` 的要求：`docker rm <id>`
/// 这类按 ID 操作的命令带不了项目名，它的范围保证来自那次 `docker inspect`。
/// `container_verified` 传 true 的地方全项目只有两处（[`crate::compose::remove_own_stale_containers`]
/// 与上面的 [`argv_takeover`]），别的地方一律走 [`argv`]。
pub fn argv_scoped(argv: &[String], container_verified: bool) -> AppResult<()> {
    let Some(prog) = argv.first() else {
        return Err(reject("空命令，拒绝。".to_string()));
    };
    let base = Path::new(prog)
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| prog.to_ascii_lowercase());
    let base = base.trim_end_matches(".exe").to_string();

    if FORBIDDEN_PROGRAMS.contains(&base.as_str()) {
        return Err(reject(format!(
            "「{}」不在 AI 能用的程序里（提权、改网络设置、跑任意脚本、删文件都不允许），拒绝。",
            safe(&base)
        )));
    }

    let rest: Vec<String> = argv[1..].iter().map(|s| s.to_ascii_lowercase()).collect();

    // 任何参数里都不许出现这些路径
    for a in &rest {
        for h in FORBIDDEN_PATH_HINTS {
            if a.contains(&h.to_ascii_lowercase()) {
                return Err(reject(format!(
                    "命令参数里出现了 {h}（网络/系统配置），拒绝。"
                )));
            }
        }
    }

    let is_docker = base == "docker" || base == "docker-compose" || base.starts_with("podman");
    if is_docker {
        // 危险子命令
        for f in FORBIDDEN_DOCKER {
            if window_matches(&rest, f) {
                return Err(reject(format!(
                    "`{} {}` 会影响这台机器上别的数据，不在动作表里，拒绝。",
                    base,
                    f.join(" ")
                )));
            }
        }
        // `down -v` / `--volumes` 会删数据卷
        if rest.iter().any(|a| a == "down") && rest.iter().any(|a| a == "-v" || a == "--volumes") {
            return Err(reject(
                "`docker compose down -v` 会删掉数据卷（你的对话记录就在里面），拒绝。".to_string(),
            ));
        }
        // `rm -v`
        if rest.first().map(|s| s == "rm").unwrap_or(false)
            && rest.iter().any(|a| a == "-v" || a == "--volumes")
        {
            return Err(reject("`docker rm -v` 会连着删匿名卷，拒绝。".to_string()));
        }
        // 会改动状态的 compose / 容器操作必须指名道姓是本项目
        if needs_project_scope(&rest) && !has_project_flag(&rest) && !container_verified {
            return Err(reject(format!(
                "这条 docker 命令没有指定 `--project-name {PROJECT}`，\
                 有可能动到你电脑上别的容器，拒绝。"
            )));
        }
    }
    Ok(())
}

/// 这条 docker 命令是不是「会改动容器状态」的那一类。
fn needs_project_scope(rest: &[String]) -> bool {
    const MUTATING: &[&str] = &[
        "up", "down", "stop", "start", "restart", "rm", "kill", "pause", "unpause", "create",
        "recreate",
    ];
    rest.iter().any(|a| MUTATING.contains(&a.as_str()))
}

fn has_project_flag(rest: &[String]) -> bool {
    let want = PROJECT.to_ascii_lowercase();
    rest.windows(2)
        .any(|w| (w[0] == "-p" || w[0] == "--project-name") && w[1] == want)
        || rest
            .iter()
            .any(|a| a == &format!("--project-name={want}") || a == &format!("-p={want}"))
}

fn window_matches(rest: &[String], pat: &[&str]) -> bool {
    if pat.is_empty() {
        return false;
    }
    rest.windows(pat.len())
        .any(|w| w.iter().zip(pat).all(|(a, b)| a == b))
}

// ── 审计日志 ──────────────────────────────────────────────────────────────

/// `~/.hunter/logs/assist-audit.log`
pub fn audit_path() -> PathBuf {
    crate::paths::logs_dir().join("assist-audit.log")
}

/// 谁提出了这个动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Proposer {
    /// 确定性规则层
    Rule,
    /// 模型
    Model,
    /// 总指挥自己的固定流程
    Orchestrator,
    /// 用户点的
    User,
}

impl Proposer {
    pub fn as_str(self) -> &'static str {
        match self {
            Proposer::Rule => "rule",
            Proposer::Model => "model",
            Proposer::Orchestrator => "orchestrator",
            Proposer::User => "user",
        }
    }
}

/// 写一条审计。**通过和拒绝都写** —— 只记拒绝的话，「它到底做了什么」就查不出来了。
pub fn audit(
    action: &str,
    args: &std::collections::BTreeMap<String, String>,
    by: Proposer,
    level: Option<Level>,
    result: &str,
) {
    let line = serde_json::json!({
        "at": crate::timefmt::now_shanghai(),
        "action": crate::redact::redact(action),
        "args": args.iter().map(|(k, v)| (k.clone(), crate::redact::mask_home(&crate::redact::redact(v)))).collect::<std::collections::BTreeMap<_, _>>(),
        "by": by.as_str(),
        "level": level.map(|l| format!("{l:?}")),
        "result": crate::redact::mask_home(&crate::redact::redact(result)),
    });
    let p = audit_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let r = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = r {
        crate::lwarn!("写审计日志 {} 失败：{e}", p.display());
        return;
    }
    let _ = crate::paths::chmod_600(&p);
}

/// 读末尾若干条审计（设置页与测试用）。
pub fn audit_tail(n: usize) -> Vec<String> {
    let Ok(s) = std::fs::read_to_string(audit_path()) else {
        return Vec::new();
    };
    let all: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    all.iter()
        .skip(all.len().saturating_sub(n))
        .map(|s| (*s).to_string())
        .collect()
}

fn reject(msg: String) -> AppError {
    crate::lwarn!("守卫拒绝：{msg}");
    AppError::new(Code::NotImplemented, msg)
}

/// 把不可信的字符串截短、脱敏之后再放进错误信息里。
fn safe(s: &str) -> String {
    let s = crate::redact::redact(s);
    s.chars().take(60).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn 三档授权里_sensitive_永远要问() {
        for m in [Mode::Auto, Mode::Confirm, Mode::Off] {
            assert!(m.needs_confirm(Level::Sensitive), "{m:?}");
            assert!(!m.needs_confirm(Level::ReadOnly), "{m:?}");
        }
        // Safe 只有自动驾驶档不问
        assert!(!Mode::Auto.needs_confirm(Level::Safe));
        assert!(Mode::Confirm.needs_confirm(Level::Safe));
        assert!(Mode::Off.needs_confirm(Level::Safe));
    }

    #[test]
    fn 认不得的档位落到最保守的一档() {
        assert_eq!(Mode::parse("auto"), Mode::Auto);
        assert_eq!(Mode::parse("AUTO"), Mode::Auto);
        assert_eq!(Mode::parse("off"), Mode::Off);
        assert_eq!(Mode::parse("confirm"), Mode::Confirm);
        assert_eq!(Mode::parse(""), Mode::Confirm);
        assert_eq!(Mode::parse("yolo"), Mode::Confirm, "认不得就按逐步确认来");
    }

    #[test]
    fn 提权一律拒绝() {
        for p in ["sudo", "/usr/bin/sudo", "pkexec", "runas", "doas"] {
            assert!(argv(&a(&[p, "docker", "restart"])).is_err(), "{p} 该被拒");
        }
    }

    #[test]
    fn 改网络与安全设置一律拒绝() {
        for p in [
            "networksetup",
            "/usr/sbin/networksetup",
            "scutil",
            "pfctl",
            "iptables",
            "netsh",
            "security",
        ] {
            assert!(
                argv(&a(&[p, "-setwebproxy", "Wi-Fi", "127.0.0.1", "8080"])).is_err(),
                "{p} 该被拒"
            );
        }
    }

    #[test]
    fn 任意_shell_与脚本解释器一律拒绝() {
        for p in [
            "sh",
            "bash",
            "zsh",
            "python3",
            "osascript",
            "powershell",
            "node",
        ] {
            assert!(argv(&a(&[p, "-c", "echo hi"])).is_err(), "{p} 该被拒");
        }
    }

    #[test]
    fn 删文件的程序一律拒绝() {
        for p in ["rm", "/bin/rm", "shred", "del", "mv"] {
            assert!(
                argv(&a(&[p, "-rf", "/Users/me/Documents"])).is_err(),
                "{p} 该被拒"
            );
        }
    }

    #[test]
    fn 碰_hosts_与_launchdaemons_的参数一律拒绝() {
        // 就算程序名是白的（docker），参数里出现这些也拒绝
        assert!(argv(&a(&["docker", "cp", "x:/a", "/etc/hosts"])).is_err());
        assert!(argv(&a(&[
            "docker",
            "cp",
            "x:/a",
            "/Library/LaunchDaemons/y.plist"
        ]))
        .is_err());
    }

    #[test]
    fn docker_的危险子命令一律拒绝() {
        assert!(argv(&a(&["docker", "volume", "rm", "hunter_pgdata"])).is_err());
        assert!(argv(&a(&["docker", "volume", "prune", "-f"])).is_err());
        assert!(argv(&a(&["docker", "system", "prune", "-a"])).is_err());
        assert!(argv(&a(&["docker", "image", "prune", "-a"])).is_err());
    }

    #[test]
    fn down_带_v_一律拒绝() {
        assert!(argv(&a(&[
            "docker",
            "compose",
            "--project-name",
            "hunter",
            "down",
            "-v"
        ]))
        .is_err());
        assert!(argv(&a(&[
            "docker",
            "compose",
            "--project-name",
            "hunter",
            "down",
            "--volumes"
        ]))
        .is_err());
        // 不带 -v 的 down 是允许的
        assert!(argv(&a(&[
            "docker",
            "compose",
            "--project-name",
            "hunter",
            "down",
            "--remove-orphans"
        ]))
        .is_ok());
    }

    #[test]
    fn 改动容器状态的命令必须指名本项目() {
        // 没有 --project-name hunter → 拒绝（否则可能停到用户别的那一套）
        assert!(argv(&a(&["docker", "compose", "stop"])).is_err());
        assert!(argv(&a(&[
            "docker",
            "compose",
            "--project-name",
            "hunter-community",
            "stop"
        ]))
        .is_err());
        // 指名了就放行
        assert!(argv(&a(&[
            "docker",
            "compose",
            "--project-name",
            "hunter",
            "up",
            "-d"
        ]))
        .is_ok());
        assert!(argv(&a(&["docker", "compose", "-p", "hunter", "restart", "api"])).is_ok());
        // 只读命令不需要
        assert!(argv(&a(&["docker", "ps", "--format", "{{.Names}}"])).is_ok());
        assert!(argv(&a(&["docker", "version"])).is_ok());
    }

    #[test]
    fn 停用户另一套_hunter_会被拒() {
        // 场景 7 的第四条：诱导模型去停 hunter-community 旧栈
        assert!(argv(&a(&[
            "docker",
            "compose",
            "--project-name",
            "hunter-community",
            "down"
        ]))
        .is_err());
        assert!(argv(&a(&["docker", "stop", "hunter-community-web-1"])).is_err());
    }

    #[test]
    fn 路径守卫解得开_双点_与软链() {
        let root = crate::paths::root();
        std::fs::create_dir_all(&root).unwrap();
        // 正常的：~/.hunter 下的文件
        assert!(writable_path(&root.join("launcher.toml")).is_ok());
        assert!(writable_path(&root.join("app").join(".env")).is_ok());
        // 用 .. 跳出去的：拒绝
        let escape = root.join("..").join("Documents").join("x.txt");
        assert!(
            writable_path(&escape).is_err(),
            "{} 应该被拒",
            escape.display()
        );
    }

    /// I6：凭据助手找不到时，**不许**去改用户的 `~/.docker/config.json`。
    /// 这一条不是靠「代码里我们没写那一行」保证的，是守卫真的拦得住。
    #[test]
    fn 用户的_docker_配置写不进去() {
        let user_cfg = crate::paths::home().join(".docker").join("config.json");
        assert!(
            writable_path(&user_cfg).is_err(),
            "{} 是用户的文件，必须拒绝",
            user_cfg.display()
        );
        // 我们自己那一份在 ~/.hunter 里，允许
        let ours = crate::dockercfg::isolated_dir().join("config.json");
        std::fs::create_dir_all(crate::dockercfg::isolated_dir()).unwrap();
        assert!(writable_path(&ours).is_ok(), "{} 应当允许", ours.display());
        // 而且它也不在「可删清单」里 —— 我们只写不删
        assert!(deletable_path(&ours).is_err());
    }

    #[test]
    fn 删除只限启动器自己生成的文件() {
        let root = crate::paths::root();
        std::fs::create_dir_all(root.join("app")).unwrap();
        // 白名单里的
        assert!(deletable_path(&crate::paths::override_file()).is_ok());
        // ~/.hunter 里但不是我们生成的（用户自己放的备份）
        assert!(deletable_path(&root.join("我的笔记.txt")).is_err());
        // launcher.toml 是设置，也不许删
        assert!(deletable_path(&crate::paths::launcher_toml()).is_err());
        // ~/.hunter 外的
        assert!(deletable_path(&crate::paths::home().join("Documents/a.txt")).is_err());
    }

    #[test]
    fn 审计能写能读() {
        let mut args = std::collections::BTreeMap::new();
        args.insert("port".to_string(), "8100".to_string());
        audit(
            "remap_ports",
            &args,
            Proposer::Rule,
            Some(Level::Safe),
            "ok",
        );
        let t = audit_tail(1);
        assert_eq!(t.len(), 1);
        assert!(t[0].contains("remap_ports"), "{}", t[0]);
        assert!(t[0].contains("\"by\":\"rule\""), "{}", t[0]);
    }

    #[test]
    fn 审计里不会出现_key() {
        let mut args = std::collections::BTreeMap::new();
        args.insert(
            "x".to_string(),
            "hunt_tools_abcdefghijklmnopqrstuvwxyz012345".to_string(),
        );
        audit(
            "x",
            &args,
            Proposer::Model,
            None,
            "hunt_tools_abcdefghijklmnopqrstuvwxyz012345",
        );
        let t = audit_tail(1);
        assert!(!t[0].contains("abcdefghij"), "{}", t[0]);
    }

    // ── I7 ────────────────────────────────────────────────────────────────

    /// 整棵删的白名单里**只有内置运行时**那一个目录。
    #[test]
    fn 只有内置运行时目录可以整棵删() {
        let root = crate::paths::root();
        std::fs::create_dir_all(root.join("runtime")).unwrap();
        assert!(deletable_tree(&crate::paths::runtime_dir()).is_ok());
        // ~/.hunter 里别的目录一个都不行 —— 它们放着用户的东西
        for d in [
            crate::paths::app_dir(),
            crate::paths::backups_dir(),
            crate::paths::diagnostics_dir(),
            crate::paths::logs_dir(),
            root.clone(),
        ] {
            std::fs::create_dir_all(&d).unwrap();
            assert!(
                deletable_tree(&d).is_err(),
                "{} 不该在「可以整棵删」的清单里",
                d.display()
            );
        }
        // ~/.hunter 外的更不行
        assert!(deletable_tree(&crate::paths::home().join("Documents")).is_err());
    }

    /// 没接管任何东西时，指向别人 compose 项目的命令**一律拒绝**。
    #[test]
    fn 没接管时不许动别人的项目() {
        let argv = a(&[
            "docker",
            "compose",
            "--project-name",
            "hunter-community",
            "stop",
        ]);
        let e = argv_takeover_for(&argv, "").expect_err("没接管就该拒绝");
        assert!(e.msg.contains("没有在管理"), "{}", e.msg);
        // 只有空白也一样
        assert!(argv_takeover_for(&argv, "   ").is_err());
    }

    /// 接管之后：只放行 stop / start / restart / logs / ps，
    /// **而且只对那一个项目**；down / rm / kill 一概拒绝。
    #[test]
    fn 接管态只放行三个子命令且只对那一个项目() {
        const P: &str = "hunter-community";
        let ok =
            |sub: &str| argv_takeover_for(&a(&["docker", "compose", "--project-name", P, sub]), P);
        for sub in ["stop", "start", "restart", "logs", "ps"] {
            assert!(ok(sub).is_ok(), "{sub} 该放行：{:?}", ok(sub).err());
        }
        for sub in ["down", "rm", "kill", "up", "create"] {
            assert!(ok(sub).is_err(), "{sub} 该拒绝");
        }
        // 指向**别的**项目：拒绝
        assert!(argv_takeover_for(
            &a(&[
                "docker",
                "compose",
                "--project-name",
                "hunter-other",
                "stop"
            ]),
            P
        )
        .is_err());
        // 连我们自己那一套都不行 —— 那条路走 `argv`，不走这里
        assert!(argv_takeover_for(
            &a(&["docker", "compose", "--project-name", PROJECT, "stop"]),
            P
        )
        .is_err());
        // 不写项目名：拒绝
        assert!(argv_takeover_for(&a(&["docker", "compose", "stop"]), P).is_err());
        // 带 -v：拒绝（通用那一道拦的）
        assert!(argv_takeover_for(
            &a(&["docker", "compose", "--project-name", P, "stop", "-v"]),
            P
        )
        .is_err());
    }

    /// 接管态那道口子**不放松**通用规则：sudo、改网络、删文件照样拒。
    #[test]
    fn 接管态不放松通用规则() {
        const P: &str = "hunter-community";
        for prog in ["sudo", "rm", "bash", "networksetup"] {
            assert!(
                argv_takeover_for(
                    &a(&[prog, "docker", "compose", "--project-name", P, "stop"]),
                    P
                )
                .is_err(),
                "{prog} 该被通用那一道拦下"
            );
        }
        // hosts 这类路径照样拦
        assert!(argv_takeover_for(
            &a(&[
                "docker",
                "compose",
                "--project-name",
                P,
                "stop",
                "/etc/hosts"
            ]),
            P
        )
        .is_err());
    }
}
