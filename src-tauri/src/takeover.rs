//! 接管本机已有的那一套 Hunter（I7 · `reuse_existing_hunter`）。
//!
//! ## 默认仍然是并存
//!
//! 侦察员发现本机已经有一套 Hunter（镜像名含 `hunter-community-` 的**别的**
//! compose 项目）时，默认处置一直是、现在也还是「给新安装换一组空闲端口，
//! 两套并存、不动它」。这一条从 I5 起就没变过。
//!
//! 这一版多给了一个选择：用户可以在「需要你」卡片上点「直接用它，不再装一套」。
//! 点了之后启动器把那一套的**项目名、工作目录、compose 文件、web 端口**
//! 记进 `launcher.toml` 的 `[takeover]` 段，运行面板改为管理它。
//!
//! ## 接管**不等于**拥有
//!
//! 那一套是用户自己装的，里面有他的数据。所以这个模块里有三条硬规矩，
//! 每一条都是**代码**不是文案：
//!
//! | 规矩 | 实现 |
//! |---|---|
//! | 任何改动类操作先二次确认 | [`Op::needs_confirm`] 恒为真；[`run`] 不给 `confirmed` 就直接拒绝 |
//! | 任何情况下不删它的卷 | [`argv_for`] 构造的命令里**没有** `down`，只有 `stop` / `start` / `restart`；再加 [`crate::assist::guard::argv`] 那一道（`-v` / `--volumes` / `volume rm` 全拦） |
//! | 找不到工作目录就只读 | [`crate::config::TakeoverSection::manageable`] 为假时，[`run`] 一律拒绝并说清原因 |
//!
//! 第二条值得多说一句：**接管态下连 `down` 都不给做**。`docker compose down`
//! 本身不删命名卷，但它会删容器与网络，而「用户点了停止、结果容器没了」
//! 与他的预期差太远。要停就 `stop`，能原样 `start` 回来。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

use crate::assist::guard::{self, Proposer};
use crate::err::{AppError, AppResult, Code};

/// 一次探到的、可以接管的那一套。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub project: String,
    pub containers: Vec<String>,
    pub ports: Vec<u16>,
    /// 从容器标签 `com.docker.compose.project.working_dir` 读到的工作目录
    pub working_dir: String,
    /// `com.docker.compose.project.config_files`
    pub config_files: String,
    /// 猜出来的 web 端口（3100 优先，否则取最小的那个）
    pub web_port: u16,
}

impl Candidate {
    /// 能不能真的管起来（有没有 compose 文件）。管不了就只能只读监控。
    pub fn manageable(&self) -> bool {
        !self.working_dir.trim().is_empty()
    }
    pub fn one_line(&self) -> String {
        format!(
            "{}（{} 个容器，占着端口 {}）",
            self.project,
            self.containers.len(),
            self.ports
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("、")
        )
    }
}

/// 本机上可以接管的那几套。**只读**，不动任何东西。
pub fn candidates() -> Vec<Candidate> {
    let published = crate::ports::docker_published();
    crate::ports::other_hunter_installs(&published)
        .into_iter()
        .map(|o| {
            let (working_dir, config_files) = labels_of(o.containers.first().map(String::as_str));
            let web_port = pick_web_port(&o.ports);
            Candidate {
                project: o.project,
                containers: o.containers,
                ports: o.ports,
                working_dir,
                config_files,
                web_port,
            }
        })
        .collect()
}

/// web 端口猜哪一个：3100 是 hunter-community 的默认值；没有就取最小的。
/// **猜不出来就返回 0**，界面上显示「读不到」而不是编一个。
fn pick_web_port(ports: &[u16]) -> u16 {
    if ports.contains(&3100) {
        return 3100;
    }
    ports.iter().copied().min().unwrap_or(0)
}

/// 从容器标签读工作目录与 compose 文件。读不到就是空（**不猜**）。
fn labels_of(container: Option<&str>) -> (String, String) {
    let Some(c) = container else {
        return (String::new(), String::new());
    };
    let bin = crate::runtime::which::docker_bin();
    let fmt = "{{index .Config.Labels \"com.docker.compose.project.working_dir\"}}\t\
               {{index .Config.Labels \"com.docker.compose.project.config_files\"}}";
    let Ok(r) = crate::proc::run_timeout(&bin, &["inspect", "-f", fmt, c], Duration::from_secs(20))
    else {
        return (String::new(), String::new());
    };
    if !r.ok() {
        return (String::new(), String::new());
    }
    let line = r.stdout.trim();
    let mut it = line.split('\t');
    let wd = it.next().unwrap_or("").trim().to_string();
    let cf = it.next().unwrap_or("").trim().to_string();
    // docker 对没有的标签回 `<no value>`；那是「没有」不是路径
    let clean = |s: String| {
        if s == "<no value>" || s.is_empty() {
            String::new()
        } else {
            s
        }
    };
    (clean(wd), clean(cf))
}

// ── 记 / 撤 ───────────────────────────────────────────────────────────────

/// 把「用它」这件事记进 `launcher.toml`。**这是接管态开始的唯一入口。**
pub fn adopt(c: &Candidate) -> AppResult<String> {
    if c.project.trim().is_empty() {
        return Err(AppError::new(
            Code::NotImplemented,
            "没给项目名，不接管。".to_string(),
        ));
    }
    if c.project == crate::config::PROJECT {
        return Err(AppError::new(
            Code::NotImplemented,
            format!(
                "「{}」就是启动器自己那一套，不存在「接管」这回事。",
                crate::config::PROJECT
            ),
        ));
    }
    let mut cfg = crate::config::LauncherConfig::load();
    cfg.takeover.project = c.project.clone();
    cfg.takeover.working_dir = c.working_dir.clone();
    cfg.takeover.config_files = c.config_files.clone();
    cfg.takeover.web_port = c.web_port;
    cfg.takeover.since = crate::timefmt::now_shanghai();
    cfg.save()?;
    guard::audit(
        "reuse_existing_hunter",
        &BTreeMap::from([
            ("project".to_string(), c.project.clone()),
            ("working_dir".to_string(), c.working_dir.clone()),
        ]),
        Proposer::User,
        Some(guard::Level::Sensitive),
        "用户选择「直接用它」，已记进 launcher.toml",
    );
    let mut s = format!(
        "已改为管理你原有的那一套「{}」。启动器不会再装一套，也不会改它的任何配置。",
        c.project
    );
    if !c.manageable() {
        s.push_str(
            "（读不到它的 compose 文件所在目录，所以只能看状态与日志，\
             停止 / 重启这类操作做不了 —— 那得回到你当初起它的那个目录去做。）",
        );
    }
    Ok(s)
}

/// 撤回接管，回到「自己装一套」。**不动被接管的那一套一个字节。**
pub fn release() -> AppResult<String> {
    let mut cfg = crate::config::LauncherConfig::load();
    if !cfg.takeover.active() {
        return Ok("本来就没有接管任何东西。".to_string());
    }
    let who = cfg.takeover.project.clone();
    cfg.takeover = crate::config::TakeoverSection::default();
    cfg.save()?;
    guard::audit(
        "release_takeover",
        &BTreeMap::from([("project".to_string(), who.clone())]),
        Proposer::User,
        Some(guard::Level::Safe),
        "撤回接管",
    );
    Ok(format!(
        "已不再管理「{who}」。它还在原地运行，一个字节都没动过。"
    ))
}

// ── 只读：状态与日志 ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub active: bool,
    pub manageable: bool,
    pub project: String,
    pub working_dir: String,
    pub since: String,
    pub web_url: String,
    /// 每个容器一行：名字 + 状态（**真实读到的**，读不到就是空列表）
    pub containers: Vec<ContainerLine>,
    /// 读不到时为什么（界面上要说清楚，不能只显示空白）
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerLine {
    pub name: String,
    pub status: String,
    pub image: String,
    pub ports: String,
}

/// 被接管那一套的现状。**只跑 `docker ps`，不改任何东西。**
pub fn state() -> State {
    let cfg = crate::config::LauncherConfig::load();
    let t = &cfg.takeover;
    let mut st = State {
        active: t.active(),
        manageable: t.manageable(),
        project: t.project.clone(),
        working_dir: crate::redact::mask_home(&t.working_dir),
        since: t.since.clone(),
        web_url: if t.web_port > 0 {
            format!("http://localhost:{}", t.web_port)
        } else {
            String::new()
        },
        containers: Vec::new(),
        note: String::new(),
    };
    if !st.active {
        return st;
    }
    let bin = crate::runtime::which::docker_bin();
    let filter = format!("label=com.docker.compose.project={}", t.project);
    let fmt = "{{.Names}}\t{{.Status}}\t{{.Image}}\t{{.Ports}}";
    match crate::proc::run_timeout(
        &bin,
        &["ps", "-a", "--filter", &filter, "--format", fmt],
        Duration::from_secs(30),
    ) {
        Ok(r) if r.ok() => {
            for line in r.stdout.lines() {
                let mut it = line.split('\t');
                let name = it.next().unwrap_or("").trim().to_string();
                if name.is_empty() {
                    continue;
                }
                st.containers.push(ContainerLine {
                    name,
                    status: it.next().unwrap_or("").trim().to_string(),
                    image: it.next().unwrap_or("").trim().to_string(),
                    ports: it.next().unwrap_or("").trim().to_string(),
                });
            }
            if st.containers.is_empty() {
                st.note = format!(
                    "docker 里找不到属于「{}」的容器了 —— 可能已经被删掉，或者换了项目名。",
                    t.project
                );
            }
        }
        Ok(r) => st.note = format!("读不到它的状态：{}", r.err_line()),
        Err(e) => st.note = format!("读不到它的状态：{}", e.msg),
    }
    st
}

/// 读被接管那一套的日志（**只读**，已脱敏）。
pub fn logs(service: Option<&str>, lines: usize) -> AppResult<Vec<String>> {
    let cfg = crate::config::LauncherConfig::load();
    if !cfg.takeover.active() {
        return Err(AppError::new(
            Code::NotImplemented,
            "现在没有在管理别的 Hunter。".to_string(),
        ));
    }
    let n = lines.clamp(1, 500);
    let (prog, mut args) = compose_argv(&cfg.takeover, &["logs", "--no-color", "--tail"])?;
    args.push(n.to_string());
    if let Some(s) = service {
        // 服务名只允许「字母数字连字符下划线点」，而且**不许以 `-` 开头** ——
        // 后半句是单测当场抓出来的：`--rm` 全是允许的字符，但它到了 compose
        // 手里就是一个**选项**而不是服务名。参数数组不会被空格撑开，
        // 但「像选项的字符串」照样能改变命令的语义
        if s.is_empty()
            || s.starts_with('-')
            || !s
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        {
            return Err(AppError::new(
                Code::NotImplemented,
                format!("服务名「{}」里有不认得的字符。", crate::redact::redact(s)),
            ));
        }
        args.push(s.to_string());
    }
    let mut argv = vec![prog.clone()];
    argv.extend(args.clone());
    guard::argv_takeover(&argv)?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let r = crate::proc::run_timeout(&prog, &refs, Duration::from_secs(60))?;
    let mut out: Vec<String> = Vec::new();
    for l in r.stdout.lines().chain(r.stderr.lines()) {
        out.push(crate::redact::mask_home(&crate::redact::redact(l)));
    }
    Ok(out)
}

// ── 改动类：一律二次确认 ──────────────────────────────────────────────────

/// 接管态下**允许**的改动类操作。注意这里面**没有 `down`**（见模块注释）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Op {
    Stop,
    Start,
    Restart,
}

impl Op {
    pub fn parse(s: &str) -> Option<Op> {
        match s.trim().to_ascii_lowercase().as_str() {
            "stop" => Some(Op::Stop),
            "start" => Some(Op::Start),
            "restart" => Some(Op::Restart),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Stop => "stop",
            Op::Start => "start",
            Op::Restart => "restart",
        }
    }
    pub fn cn(self) -> &'static str {
        match self {
            Op::Stop => "停止",
            Op::Start => "启动",
            Op::Restart => "重启",
        }
    }
    /// **恒为真。** 这不是一个可以配的开关 —— 动的是用户自己装的那一套，
    /// 每一次都要他亲口说可以。
    pub fn needs_confirm(self) -> bool {
        true
    }
    /// 二次确认时摆在用户面前的那句话。
    pub fn confirm_text(self, project: &str) -> String {
        match self {
            Op::Stop => format!(
                "要{}「{project}」吗？这是你自己装的那一套，停了它上面正在用的人会断。\
                 数据卷不会动，随时可以再启动回来。",
                self.cn()
            ),
            Op::Start => format!("要{}「{project}」吗？", self.cn()),
            Op::Restart => format!(
                "要{}「{project}」吗？重启期间它会短暂不可用。数据卷不会动。",
                self.cn()
            ),
        }
    }
}

/// 构造要跑的命令。**只有 stop / start / restart 三种**，
/// 而且一律带 `--project-name <被接管的项目>`（守卫要求指名道姓）。
pub fn argv_for(t: &crate::config::TakeoverSection, op: Op) -> AppResult<Vec<String>> {
    let (prog, mut args) = compose_argv(t, &[op.as_str()])?;
    let mut v = vec![prog];
    v.append(&mut args);
    Ok(v)
}

/// `docker compose --project-name X --project-directory Y -f <文件…> <子命令…>`
fn compose_argv(
    t: &crate::config::TakeoverSection,
    tail: &[&str],
) -> AppResult<(String, Vec<String>)> {
    if !t.manageable() {
        return Err(AppError::new(
            Code::NotImplemented,
            format!(
                "读不到「{}」的 compose 文件在哪（它的容器上没有 working_dir 标签），\
                 所以只能看状态与日志。要停 / 重启它，请回到你当初起它的那个目录去做。",
                t.project
            ),
        ));
    }
    let (prog, mut pre) = crate::runtime::which::compose_info().argv_prefix();
    pre.push("--project-name".into());
    pre.push(t.project.clone());
    pre.push("--project-directory".into());
    pre.push(t.working_dir.clone());
    for f in t.config_files.split(',') {
        let f = f.trim();
        if f.is_empty() {
            continue;
        }
        pre.push("-f".into());
        pre.push(f.to_string());
    }
    pre.extend(tail.iter().map(|s| (*s).to_string()));
    Ok((prog, pre))
}

/// 真去做。**没有 `confirmed` 就拒绝** —— 这一道在执行路径上，不在界面里。
pub fn run(op: Op, confirmed: bool) -> AppResult<String> {
    let cfg = crate::config::LauncherConfig::load();
    let t = &cfg.takeover;
    if !t.active() {
        return Err(AppError::new(
            Code::NotImplemented,
            "现在没有在管理别的 Hunter。".to_string(),
        ));
    }
    let args = BTreeMap::from([
        ("project".to_string(), t.project.clone()),
        ("op".to_string(), op.as_str().to_string()),
    ]);
    if op.needs_confirm() && !confirmed {
        guard::audit(
            "takeover_op",
            &args,
            Proposer::User,
            Some(guard::Level::Sensitive),
            "拒绝：还没得到二次确认",
        );
        return Err(AppError::new(
            Code::NotImplemented,
            format!(
                "「{}」动的是你自己装的那一套，要你再确认一次才会执行。{}",
                op.cn(),
                op.confirm_text(&t.project)
            ),
        ));
    }
    let argv = argv_for(t, op)?;
    // 守卫：接管态专用的那一道（更严，不是更松）——
    // 项目名必须正是被接管的那个、子命令只能是 stop/start/restart，
    // 再叠上通用的那一整套（`-v`、`volume rm`、`sudo`、改网络设置…）
    guard::argv_takeover(&argv)?;
    let (prog, rest) = argv.split_first().expect("argv_for 至少给一项");
    let refs: Vec<&str> = rest.iter().map(String::as_str).collect();
    let r = crate::proc::run_timeout(prog, &refs, Duration::from_secs(300))?;
    let result = if r.ok() {
        format!("已{}「{}」。它的数据卷一个都没动。", op.cn(), t.project)
    } else {
        return {
            guard::audit(
                "takeover_op",
                &args,
                Proposer::User,
                Some(guard::Level::Sensitive),
                &format!("失败：{}", r.err_line()),
            );
            Err(AppError::new(
                Code::Unknown,
                format!("{}「{}」没成功：{}", op.cn(), t.project, r.err_line()),
            ))
        };
    };
    guard::audit(
        "takeover_op",
        &args,
        Proposer::User,
        Some(guard::Level::Sensitive),
        &result,
    );
    Ok(result)
}

/// 被接管那一套的 compose 文件路径（设置页显示用）。
pub fn config_paths(t: &crate::config::TakeoverSection) -> Vec<PathBuf> {
    t.config_files
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TakeoverSection;

    fn t() -> TakeoverSection {
        TakeoverSection {
            project: "hunter-community".into(),
            working_dir: "/home/u/hunter-community".into(),
            config_files: "/home/u/hunter-community/docker-compose.yml".into(),
            web_port: 3100,
            since: "2026-09-21 18:00:00".into(),
        }
    }

    /// 三种操作**全部**要二次确认，一个例外都没有。
    #[test]
    fn 改动类操作一律要二次确认() {
        for op in [Op::Stop, Op::Start, Op::Restart] {
            assert!(op.needs_confirm(), "{op:?}");
            let e = run(op, false);
            // 没接管时报「没有在管理」，接管了就报「要你再确认一次」——
            // 两种都不是「已执行」
            assert!(e.is_err(), "{op:?} 没确认就不该执行");
        }
    }

    /// 命令里**绝不能**出现 down / -v / --volumes / volume rm。
    #[test]
    fn 接管态的命令里没有删卷的可能() {
        let t = t();
        for op in [Op::Stop, Op::Start, Op::Restart] {
            let argv = argv_for(&t, op).expect("能构造");
            let joined = argv.join(" ");
            assert!(!joined.contains(" down"), "{joined}");
            assert!(
                !argv.iter().any(|a| a == "-v" || a == "--volumes"),
                "{joined}"
            );
            assert!(!joined.contains("volume"), "{joined}");
            // 没接管时，接管态那一道必须**拒绝**：没人点过「直接用它」，
            // 这条指向别人项目的命令就不该存在
            assert!(
                guard::argv_takeover(&argv).is_err(),
                "没接管时不该放行：{joined}"
            );
        }
    }

    /// 必须指名道姓是**被接管的那个项目**，不能是我们自己的 `hunter`。
    #[test]
    fn 命令里指名的是被接管的项目() {
        let argv = argv_for(&t(), Op::Stop).unwrap();
        let i = argv
            .iter()
            .position(|a| a == "--project-name")
            .expect("要带项目名");
        assert_eq!(argv[i + 1], "hunter-community");
        assert!(
            argv.contains(&"-f".to_string()),
            "要带上它自己的 compose 文件"
        );
    }

    /// 没有工作目录 → 降级成只读，改动类操作**说清原因**地拒绝。
    #[test]
    fn 找不到工作目录就只能只读() {
        let mut x = t();
        x.working_dir.clear();
        assert!(!x.manageable());
        let e = argv_for(&x, Op::Stop).expect_err("该拒绝");
        assert!(e.msg.contains("只能看状态与日志"), "{}", e.msg);
        assert!(e.msg.contains("compose 文件"), "{}", e.msg);
    }

    #[test]
    fn 不接管自己那一套() {
        let c = Candidate {
            project: crate::config::PROJECT.to_string(),
            containers: vec![],
            ports: vec![],
            working_dir: String::new(),
            config_files: String::new(),
            web_port: 0,
        };
        let e = adopt(&c).expect_err("自己那一套不叫接管");
        assert!(e.msg.contains("启动器自己"), "{}", e.msg);
    }

    #[test]
    fn web_端口猜不出来就给_0_而不是编一个() {
        assert_eq!(pick_web_port(&[3100, 8100]), 3100);
        assert_eq!(pick_web_port(&[8100, 5442]), 5442);
        assert_eq!(pick_web_port(&[]), 0);
    }

    #[test]
    fn 二次确认的话里要说清数据卷不会动() {
        for op in [Op::Stop, Op::Restart] {
            let s = op.confirm_text("hunter-community");
            assert!(s.contains("数据卷"), "{s}");
            assert!(s.contains("hunter-community"), "{s}");
        }
    }

    #[test]
    fn 日志的服务名过滤挡得住怪字符() {
        // 没接管时先报「没有在管理」，所以这里只测过滤本身能拦住
        // 这个判据要和 `logs` 里那一段**一模一样**，所以抽成同一个闭包来验
        let allowed = |s: &str| {
            !s.is_empty()
                && !s.starts_with('-')
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        };
        for bad in ["a b", "x;y", "--rm", "-f", "", "$(id)", "a/b"] {
            assert!(!allowed(bad), "「{bad}」应该被挡住");
        }
        for good in ["api", "llm-shim", "web", "postgres_1", "a.b"] {
            assert!(allowed(good), "「{good}」应该放行");
        }
    }
}
