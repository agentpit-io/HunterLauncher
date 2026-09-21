//! `hunter-launcher --headless` —— 给 SSH 用户的纯命令行版（方案 §6 Linux 一节）。
//!
//! **和界面版跑的是同一套 [`crate::flow`]**，不是另写一遍。两条路径的区别只有
//! 「进度怎么显示」：这里打到终端，那里推事件给前端。
//!
//! ```text
//! hunter-launcher --headless                  首次安装 / 继续安装（交互式问 key）
//! hunter-launcher --headless --key-file PATH  从文件读 key，适合脚本与 CI
//! hunter-launcher --headless --registry ghcr  固定镜像源（ghcr | tencent | 自定义前缀）
//! hunter-launcher --headless --tag 1.2.0      指定 Hunter 版本
//! hunter-launcher --status                    看一眼现在什么情况
//! hunter-launcher --stop / --start / --restart / --down
//! hunter-launcher --logs [服务名]
//! hunter-launcher --diagnose                  打印脱敏诊断 + 确定性规则的结论
//! hunter-launcher --diagnose --ai             再问一轮 AI（会花额度，所以要显式开关）
//! ```

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use crate::compose;
use crate::config;
use crate::err::{AppError, AppResult, Code};
use crate::flow::{self, human_bytes, AppState, InstallOptions};
use crate::gateway;
use crate::{paths, registry};

pub struct Args {
    pub headless: bool,
    pub key_file: Option<String>,
    pub registry: Option<String>,
    pub tag: Option<String>,
    pub action: Option<String>,
    pub yes: bool,
    /// 只把镜像拉下来，不起容器。
    /// 用处：网速慢的机器可以先在空闲时间预下载，装的时候就快了；
    /// 也方便在不动现有容器的前提下换源重拉一遍。
    pub pull_only: bool,
    /// `--upgrade <tag>`：把 Hunter 升到这个版本（方案 §10）
    pub upgrade_to: Option<String>,
    /// `--import-images <tar>`：导入离线包（方案 §9）
    pub import_images: Option<String>,
    /// `--export-images <tar>`：把本机的六个镜像打成离线包
    pub export_images: Option<String>,
    pub help: bool,
    pub version: bool,
    /// `--diagnose --ai`：诊断完再**问一轮 AI**（I4 §三的第二层）。
    ///
    /// 为什么要一个显式开关：`--diagnose` 是给脚本用的路径，问 AI 要花用户的额度，
    /// 不能在他没要求的时候悄悄花掉。默认只跑第一层（确定性规则，零 token）。
    pub ai: bool,
    /// `--assist-replay <文件>`：把一份存下来的网关响应喂给动作白名单那道闸。
    /// **不发任何网络请求**，纯离线验证安全边界（见 [`cmd_assist_replay`]）。
    pub assist_replay: Option<String>,
    /// `--code <E_XXX>`：告诉诊断助手「我刚才撞上的是这个错误码」。
    ///
    /// 界面版是从状态机里拿这个码的（错误页知道自己是怎么来的），命令行下
    /// `--diagnose` 是一条独立的路径，不给的话规则层只能看当下的现场 ——
    /// 而「拉取失败」这种事跑完就没痕迹了。
    pub code: Option<String>,
    /// I5：`--auto` 走 AI 自动驾驶安装（总指挥 + 侦察 + 诊断 + 守卫 + 执行 + 验证）
    pub auto: bool,
    /// I5：`--assist-mode auto|confirm|off`，不给就用 `launcher.toml` 里存的那一档
    pub assist_mode: Option<String>,
}

/// 手写参数解析。为这几个开关拖一个 clap 进来不划算，而且 Tauri 的可执行文件
/// 平时是双击启动的，参数解析越简单越不容易出意外。
pub fn parse_args<I: IntoIterator<Item = String>>(argv: I) -> Args {
    let mut a = Args {
        headless: false,
        key_file: None,
        registry: None,
        tag: None,
        action: None,
        yes: false,
        pull_only: false,
        upgrade_to: None,
        import_images: None,
        export_images: None,
        help: false,
        version: false,
        ai: false,
        assist_replay: None,
        code: None,
        auto: false,
        assist_mode: None,
    };
    let v: Vec<String> = argv.into_iter().collect();
    let mut i = 0;
    while i < v.len() {
        match v[i].as_str() {
            "--headless" => a.headless = true,
            "--key-file" => {
                a.key_file = v.get(i + 1).cloned();
                i += 1;
            }
            "--registry" => {
                a.registry = v.get(i + 1).cloned();
                i += 1;
            }
            "--tag" => {
                a.tag = v.get(i + 1).cloned();
                i += 1;
            }
            "-y" | "--yes" => a.yes = true,
            "--auto" => {
                a.auto = true;
                a.headless = true;
            }
            "--assist-mode" => {
                a.assist_mode = v.get(i + 1).cloned();
                i += 1;
            }
            "--pull-only" => {
                a.pull_only = true;
                a.headless = true;
            }
            "--upgrade" => {
                a.upgrade_to = v.get(i + 1).cloned();
                a.action = Some("upgrade".into());
                a.headless = true;
                i += 1;
            }
            "--import-images" => {
                a.import_images = v.get(i + 1).cloned();
                a.action = Some("import-images".into());
                a.headless = true;
                i += 1;
            }
            "--export-images" => {
                a.export_images = v.get(i + 1).cloned();
                a.action = Some("export-images".into());
                a.headless = true;
                i += 1;
            }
            "--check-update" => {
                a.action = Some("check-update".into());
                a.headless = true;
            }
            "--self-update" => {
                a.action = Some("self-update".into());
                a.headless = true;
            }
            "--assist-replay" => {
                a.assist_replay = v.get(i + 1).cloned();
                a.action = Some("assist-replay".into());
                a.headless = true;
                i += 1;
            }
            "--code" => {
                a.code = v.get(i + 1).cloned();
                a.action = Some("diagnose".into());
                a.headless = true;
                i += 1;
            }
            "--ai" => {
                a.ai = true;
                a.action = Some("diagnose".into());
                a.headless = true;
            }
            "-h" | "--help" => a.help = true,
            "-V" | "--version" => a.version = true,
            "--status" | "--stop" | "--start" | "--restart" | "--down" | "--diagnose"
            | "--backups" => {
                a.action = Some(v[i].trim_start_matches("--").to_string());
                a.headless = true;
            }
            "--logs" => {
                a.action = Some("logs".into());
                a.headless = true;
                if let Some(next) = v.get(i + 1) {
                    if !next.starts_with('-') {
                        a.key_file = None;
                        a.tag = Some(next.clone()); // 借 tag 位存服务名，见 run() 里的注释
                        i += 1;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    a
}

pub const HELP: &str = "\
Hunter 启动器 · 命令行模式

  hunter-launcher --headless [选项]     在没有桌面环境的机器上完成安装
  hunter-launcher --auto [选项]         AI 自动驾驶安装：出问题自己查、自己修，过程实时打印
  hunter-launcher --status              看当前状态
  hunter-launcher --start | --stop | --restart | --down
  hunter-launcher --logs [服务名]       看容器日志（已脱敏）
  hunter-launcher --diagnose            打印脱敏诊断信息 + 确定性规则的结论（零 token）
  hunter-launcher --diagnose --ai       再问一轮 AI 诊断助手（会花你的 hunter 额度）
  hunter-launcher --diagnose --code E_PULL_FAILED  按指定错误码跑一遍规则层
  hunter-launcher --assist-replay <文件>  把一份存下来的网关响应喂给动作白名单（离线，不联网）
  hunter-launcher --check-update        查 Hunter 与启动器有没有新版本
  hunter-launcher --self-update         更新启动器自己（AppImage 就地换；.deb 只下载）
  hunter-launcher --upgrade <版本>      升级 Hunter（先自动备份，失败自动回滚）
  hunter-launcher --backups             列出 ~/.hunter/backups/ 里的备份
  hunter-launcher --import-images <tar> 从离线包导入镜像，之后安装不再联网拉
  hunter-launcher --export-images <tar> 把本机的六个镜像打成离线包

选项
  --key-file <路径>   从文件读 hunter key（文件建议权限 600）。不给就交互式输入
  --registry <源>     固定镜像源：ghcr | tencent | 自定义前缀。不给就自动测速选
  --tag <版本>        Hunter 版本，默认 1.2.0
  --pull-only         只拉镜像不起容器（网速慢时可以先预下载）
  --auto              AI 自动驾驶安装（等同界面上「授权 AI 自动安装」那一档）
  --assist-mode <档>  auto | confirm | off，不给就用设置里存的那一档
  -y, --yes           不要任何确认，一路走完
  -h, --help          这段说明
  -V, --version       版本号

工作目录 ~/.hunter（可用环境变量 HUNTER_HOME 改）。
key 只会写进 ~/.hunter/app/.env（权限 600），不进日志、不进诊断输出。
";

pub fn run(args: &Args) -> i32 {
    if args.help {
        print!("{HELP}");
        return 0;
    }
    if args.version {
        println!("hunter-launcher {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    if let Err(e) = paths::ensure_dirs() {
        eprintln!("{e}");
        return 1;
    }
    crate::log::init(paths::launcher_log());
    crate::linfo!(
        "headless 模式启动，参数 action={:?} registry={:?}",
        args.action,
        args.registry
    );

    let st = AppState::new();
    let r = match args.action.as_deref() {
        Some("status") => cmd_status(&st),
        Some("stop") => cmd_simple("stop"),
        Some("start") => cmd_simple("start"),
        Some("restart") => cmd_simple("restart"),
        Some("down") => cmd_simple("down"),
        Some("logs") => cmd_logs(args.tag.as_deref()),
        Some("assist-replay") => match args.assist_replay.as_deref() {
            Some(p) => cmd_assist_replay(p),
            None => Err(AppError::new(
                Code::Unknown,
                "--assist-replay 要跟一个 JSON 文件路径".to_string(),
            )),
        },
        Some("diagnose") if args.ai => cmd_assist(&st, args),
        Some("diagnose") => cmd_diagnose(&st, args.code.as_deref()),
        Some("check-update") => cmd_check_update(&st),
        Some("self-update") => cmd_self_update(),
        Some("upgrade") => cmd_upgrade(&st, args),
        Some("backups") => cmd_backups(),
        Some("import-images") => cmd_import_images(&st, args),
        Some("export-images") => cmd_export_images(&st, args),
        _ if args.auto => cmd_auto(&st, args),
        _ => cmd_install(&st, args),
    };
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("\n✗ {e}");
            eprintln!("  日志：{}", paths::launcher_log().display());
            crate::lerror!("headless 失败：{}", e.msg);
            1
        }
    }
}

// ── I5 · AI 自动驾驶安装 ──────────────────────────────────────────────────

/// `--auto`：把界面上那条「授权之后零点击」的路径原样跑一遍。
///
/// 这条路和界面版走的是**同一个** [`crate::assist::auto::Orchestrator`]、
/// 同一张动作表、同一套守卫 —— 区别只有事件往哪儿发（这里发到 stdout 与
/// `~/.hunter/logs/assist-events.jsonl`，界面版发到窗口）。
/// 测试机上的七个场景验收就靠它。
fn cmd_auto(st: &AppState, args: &Args) -> AppResult<()> {
    use crate::assist::auto::Orchestrator;
    use crate::assist::events::{Bus, Stdout};
    use crate::assist::guard::Mode;

    title("Hunter 启动器 · AI 自动驾驶安装");
    println!("工作目录 {}", paths::root().display());

    // 1) key：模型那一层要用它；没有 key 也能装，只是退回确定性规则
    let key = read_key(args)?;
    let check = gateway::check_key(&key, Duration::from_secs(25));
    if !check.valid {
        println!("  ✗ {}", check.message.clone().unwrap_or_default());
        let code = match check.reason {
            gateway::KeyReason::Exhausted => Code::QuotaExhausted,
            gateway::KeyReason::RateLimited => Code::RateLimited,
            gateway::KeyReason::Network => Code::ProxyBlock,
            _ => Code::KeyInvalid,
        };
        return Err(AppError::new(code, "key 没通过校验。".to_string()));
    }
    st.set_hunter_key(&key);
    println!("  ✓ key 有效");

    // 2) 授权档位。命令行给了就用命令行的，并**像界面一样把授权记下来**
    let mut cfg = st.config();
    let mode = match args.assist_mode.as_deref() {
        Some(m) => {
            let m = Mode::parse(m);
            cfg.assist.mode = m.as_str().to_string();
            cfg.assist.enabled = m != Mode::Off;
            cfg.assist.consented_at = crate::timefmt::now_shanghai();
            cfg.save()?;
            st.set_config(cfg.clone());
            m
        }
        None => cfg.assist.mode(),
    };
    println!("  授权档位：{}（{}）", mode.as_str(), mode.cn());
    // I7 · 待办池 P1-25 的结论：**stdin 是终端就真的问，不是终端才按「不」处理。**
    //
    // I5 那一版的做法是「命令行下一律按不」，并在这里打一句提醒。那条结论对了一半：
    // `--auto` 的典型用法（CI、远程脚本、cron）确实没人能答，阻塞等下去就是把进程挂死。
    // 但人**坐在终端前面**手敲这条命令的情况同样常见 —— 那时候让他答一句
    // 「y / n」比让他重跑一遍换成 `--assist-mode auto` 讲道理。
    //
    // 判据用 `stdin` 是不是终端：管道、重定向、nohup、CI 全都不是，行为与 I5 一致；
    // 人在终端里敲的就是，于是「逐步确认」在命令行下第一次真的能用了。
    let stdin_is_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    if mode == Mode::Confirm && !stdin_is_tty {
        // 不说清楚的话，用户会看着它在第一个改动动作上「莫名其妙地失败」——
        // 其实是它在等一个永远不会来的回答
        println!(
            "  ⚠ 「逐步确认」档在这里等不到回答（stdin 不是终端，多半是 CI 或管道）：\n\
               凡是要你点头的动作都会按「不」处理。要全自动请用 --assist-mode auto。"
        );
    } else if stdin_is_tty {
        println!("  要你拍板的地方会在这里问你（输入 y 或 n 回车）。");
    }
    println!(
        "  预算：单问题 {} 回合 · 整次 {} 回合 · {} token · 单次模型 45 秒",
        crate::assist::auto::MAX_ROUNDS_PER_ISSUE,
        crate::assist::auto::MAX_ROUNDS_TOTAL,
        crate::assist::auto::MAX_TOKENS
    );
    println!();

    let opts = InstallOptions {
        registry: args.registry.clone(),
        tag: args.tag.clone().unwrap_or_else(|| cfg.hunter.tag.clone()),
    };
    let bus = std::sync::Arc::new(Bus::new(Box::new(Stdout::new()), true));
    let mut orch = Orchestrator::new(bus, mode, Some(key), st.cancel.clone());
    if stdin_is_tty {
        // 人就在终端前面 —— 把问题打出来、读一行回答（I7 · P1-25）
        orch.set_answer_reader(Box::new(|question, choices| {
            use std::io::Write;
            println!();
            println!("？ {question}");
            for c in choices {
                println!("    {} {}", if c.primary { "›" } else { " " }, c.label);
            }
            print!("  你的选择（y = 第一项 / n = 第二项）：");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line).is_err() {
                // 读不到（stdin 被关了）就按「不」——**不猜他想要什么**
                println!("  （读不到回答，按「不」处理）");
                return Some("no".into());
            }
            let ans = match line.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" | "是" | "好" => "yes",
                _ => "no",
            };
            println!("  已记下：{}", if ans == "yes" { "是" } else { "不" });
            Some(ans.to_string())
        }));
    } else {
        // 没有人能点「需要你」卡片上的按钮 —— 如实告诉总指挥，
        // 别让它在那儿等一个永远不会来的回答
        orch.set_interactive(false);
    }
    let out = orch.run(st, &opts);

    println!();
    println!(
        "事件流：{}",
        paths::logs_dir().join("assist-events.jsonl").display()
    );
    println!("审计日志：{}", crate::assist::guard::audit_path().display());
    println!(
        "统计：自动解决 {} 个问题 · {} 个修复回合 · {} token · 用时 {} 秒",
        out.solved,
        out.rounds,
        out.tokens,
        out.elapsed_ms / 1000
    );
    if let Some(p) = &out.takeover {
        println!(
            "\n✓ 没有再装一套 —— 按你的选择，启动器现在管理你已有的「{p}」。{}",
            match &out.url {
                Some(u) => format!("打开 {u}"),
                None => "（读不到它的 web 端口）".to_string(),
            }
        );
        return Ok(());
    }
    if out.ok {
        println!("\n✓ 装好了。打开 {}", out.url.unwrap_or_default());
        Ok(())
    } else {
        Err(AppError::new(
            out.code
                .as_deref()
                .and_then(code_from_str)
                .unwrap_or(Code::Unknown),
            out.message.unwrap_or_else(|| "没能自动装好".into()),
        ))
    }
}

fn code_from_str(s: &str) -> Option<Code> {
    [
        Code::DockerMissing,
        Code::DaemonDown,
        Code::WslMissing,
        Code::KeyInvalid,
        Code::QuotaExhausted,
        Code::PullFailed,
        Code::PortInUse,
        Code::PortConflict,
        Code::StartTimeout,
        Code::ProxyBlock,
        Code::UpdateFailed,
        Code::ComposeFetch,
        Code::ConfigWrite,
        Code::ProjectConflict,
        Code::RateLimited,
        Code::NotImplemented,
    ]
    .into_iter()
    .find(|c| c.as_str() == s)
}

// ── 安装 ──────────────────────────────────────────────────────────────────

fn cmd_install(st: &AppState, args: &Args) -> AppResult<()> {
    let t0 = Instant::now();
    let steps = if args.pull_only { 4 } else { 6 };
    title(if args.pull_only {
        "Hunter 启动器 · 只拉镜像"
    } else {
        "Hunter 启动器 · 命令行安装"
    });
    println!("工作目录 {}", paths::root().display());

    // 1) key（I4 把它挪到了最前面：界面版的 AI 诊断助手要用这把 key 调网关，
    //    两条路径的顺序必须一致，否则 --headless 与界面版走出来的步骤号对不上）
    step(1, steps, "hunter key");
    let key = read_key(args)?;
    let check = gateway::check_key(&key, Duration::from_secs(25));
    if !check.valid {
        println!("  ✗ {}", check.message.clone().unwrap_or_default());
        let code = match check.reason {
            gateway::KeyReason::Exhausted => Code::QuotaExhausted,
            gateway::KeyReason::RateLimited => Code::RateLimited,
            gateway::KeyReason::Network => Code::ProxyBlock,
            _ => Code::KeyInvalid,
        };
        return Err(AppError::new(code, "key 没通过校验。".to_string()));
    }
    st.set_hunter_key(&key);
    // key 是好的但额度用完了：照装，但把话说在前面（I1 实测，见迭代报告用例 6）
    if check.reason == gateway::KeyReason::Exhausted {
        println!("  ⚠ {}", check.message.clone().unwrap_or_default());
    }
    match &check.quota {
        Some(q) => println!(
            "  ✓ key 有效 · 今日已用 {} / {} · 剩 {} · 每分钟 {} 次 · 并发 {}",
            gateway::thousands(q.used_today),
            gateway::thousands(q.limit_daily),
            gateway::thousands(q.remaining),
            q.rpm.map(|x| x.to_string()).unwrap_or_else(|| "—".into()),
            q.concurrency
                .map(|x| x.to_string())
                .unwrap_or_else(|| "—".into())
        ),
        None => println!("  ✓ key 有效（额度信息没读到）"),
    }
    if !check.models.is_empty() {
        println!(
            "  可用模型：{}",
            check
                .models
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>()
                .join(" · ")
        );
    }

    // 2) Docker（I4 起排在 key 后面，与界面版的向导顺序一致）
    step(2, steps, "检查 Docker");
    let d = crate::runtime::docker::detect();
    if !d.ready() {
        println!(
            "  ✗ {}",
            d.problem.clone().unwrap_or_else(|| "Docker 不可用".into())
        );
        if let Some(g) = &d.install_guide {
            println!("\n  {}", g.title);
            for s in &g.steps {
                println!("   · {}", s.text);
                if let Some(u) = &s.url {
                    println!("     {u}");
                }
            }
            println!("\n  {}", g.license_note);
        }
        let code = d.error_code().unwrap_or(Code::DockerMissing);
        return Err(AppError::new(
            code,
            "Docker 还没准备好，按上面的步骤装完再来一次。".to_string(),
        ));
    }
    println!(
        "  ✓ {} · compose {} · {}",
        d.runtime_label.clone().unwrap_or_default(),
        d.compose_version.clone().unwrap_or_default(),
        d.arch.clone().unwrap_or_default()
    );

    // 3) 准备（选源 / compose / 端口 / .env）
    step(3, steps, "准备配置");
    let mut cfg = st.config();
    if let Some(t) = &args.tag {
        cfg.hunter.tag = t.clone();
    }
    st.set_config(cfg.clone());
    let opts = InstallOptions {
        registry: args.registry.clone(),
        tag: cfg.hunter.tag.clone(),
    };
    let prep = flow::prepare(st, &opts, |line| println!("  {line}"))?;

    // 4) 拉镜像
    step(4, steps, "拉取镜像");
    let tty = std::io::stdout().is_terminal();
    let pull_t0 = Instant::now();
    let mut last_line_len = 0usize;
    let mut last_print = Instant::now() - Duration::from_secs(5);
    flow::pull(st, &prep, &opts, |p| {
        // 终端里原地刷新；重定向到文件时改成每 10 秒一行，日志才读得下去
        let should = if tty {
            last_print.elapsed() >= Duration::from_millis(500)
        } else {
            last_print.elapsed() >= Duration::from_secs(10)
        };
        if !should {
            return;
        }
        last_print = Instant::now();
        let line = format!(
            "  {:>3}% {} / {} · {}{} · 第 {} 次尝试",
            p.percent,
            human_bytes(p.downloaded_bytes),
            human_bytes(p.total_bytes),
            p.speed_bps
                .map(|s| format!("{}/s", human_bytes(s)))
                .unwrap_or_else(|| "—".into()),
            p.eta_seconds
                .map(|s| format!(" · 还需约 {}", human_secs(s)))
                .unwrap_or_default(),
            p.attempt
        );
        if tty {
            print!("\r{:<width$}", line, width = last_line_len.max(line.len()));
            last_line_len = line.len();
            let _ = std::io::stdout().flush();
        } else {
            println!("{line}");
        }
    })?;
    if tty {
        println!();
    }
    let pull_secs = pull_t0.elapsed().as_secs();
    let snap = st
        .pull
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| compose::PullProgress::empty());
    println!("  ✓ 拉取完成，用时 {}", human_secs(pull_secs));
    for i in &snap.images {
        println!(
            "    {:<26} {:>9}  {}",
            i.short_ref,
            human_bytes(i.total_bytes),
            i.seconds.map(human_secs).unwrap_or_else(|| "—".into())
        );
        crate::linfo!(
            "镜像 {} · {} · {:?} 秒",
            i.short_ref,
            human_bytes(i.total_bytes),
            i.seconds
        );
    }

    if args.pull_only {
        println!(
            "\n  只拉镜像模式，不起容器。总用时 {}\n",
            human_secs(t0.elapsed().as_secs())
        );
        crate::linfo!("--pull-only 完成，拉取 {} 秒", pull_secs);
        return Ok(());
    }

    // 5) 起容器
    step(5, steps, "启动服务");
    let start_t0 = Instant::now();
    let mut last_summary = String::new();
    let services = flow::start(st, |v| {
        let s = v
            .iter()
            .map(|x| format!("{}={}", x.service, compose::health_cn(x.health)))
            .collect::<Vec<_>>()
            .join(" ");
        if s != last_summary {
            println!("  {s}");
            last_summary = s;
        }
    })?;
    println!(
        "  ✓ {} 个服务全部就绪，用时 {}",
        services.len(),
        human_secs(start_t0.elapsed().as_secs())
    );

    // 6) 完成
    step(6, steps, "完成");
    let status = flow::runtime_status(st);
    let url = status
        .web_url
        .clone()
        .unwrap_or_else(|| format!("http://localhost:{}", prep.ports.web));
    println!("\n  Hunter 已经跑起来了：{url}");
    if !prep.changes.is_empty() {
        println!("  注意：这些端口被占用，已自动改过 —");
        for c in &prep.changes {
            println!("    {} {} → {}", c.service, c.wanted, c.actual);
        }
    }
    match status.api_key_configured {
        Some(true) => println!("  api 自报 key 已配置（/api/health 的 hunter_api_key 字段）"),
        Some(false) => {
            println!("  ⚠ api 自报没读到 key（/api/health 说 missing），工具调用可能会 403")
        }
        None => println!("  api 的 /api/health 没读到，key 是否落进容器未确认"),
    }
    println!("  配置 {}（权限 600）", paths::env_file().display());
    println!("  日志 {}", paths::launcher_log().display());
    println!("\n  总用时 {}\n", human_secs(t0.elapsed().as_secs()));
    crate::linfo!(
        "headless 安装完成，总用时 {} 秒（拉取 {} 秒）",
        t0.elapsed().as_secs(),
        pull_secs
    );
    Ok(())
}

/// 读 key：优先 `--key-file`，否则交互式输入（终端里不回显）。
/// key 文件的 POSIX 权限位。Windows 上没有这一套，返回 None（靠的是用户目录的 ACL）。
fn file_mode(path: &str) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .ok()
            .map(|m| m.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

fn read_key(args: &Args) -> AppResult<String> {
    if let Some(p) = &args.key_file {
        let s = std::fs::read_to_string(p)
            .map_err(|e| AppError::new(Code::KeyInvalid, format!("读不到 key 文件 {p}：{e}")))?;
        let k = s.trim().to_string();
        if k.is_empty() {
            return Err(AppError::new(
                Code::KeyInvalid,
                format!("key 文件 {p} 是空的"),
            ));
        }
        crate::redact::register_secret(&k);
        println!("  从 {p} 读到一把 key（{}）", crate::redact::mask_key(&k));
        // 帮助里写着「文件建议权限 600」，但只写不查等于没写（I3 自审）。
        // 同机的其他用户读得到这个文件，这把 key 就等于是公开的。
        // 只提醒不中断：脚本化场景里用户可能有自己的理由（例如挂在只读的 secret 卷上）。
        if let Some(mode) = file_mode(p) {
            if mode & 0o077 != 0 {
                println!(
                    "  ⚠ {p} 的权限是 {:o}，同机的其他用户也读得到这把 key。建议 chmod 600 {p}",
                    mode
                );
            }
        }
        return Ok(k);
    }
    if !std::io::stdin().is_terminal() {
        return Err(AppError::new(
            Code::KeyInvalid,
            "标准输入不是终端，没法交互式问 key。用 --key-file <路径> 指一个文件。".to_string(),
        ));
    }
    println!(
        "  还没有 key 的话，到 {} 免费申请一把（约 30 秒）。",
        gateway::APPLY_URL
    );
    print!("  请输入 hunter key（输入时不显示）：");
    let _ = std::io::stdout().flush();
    let k = rpassword::read_password()
        .map_err(|e| AppError::new(Code::KeyInvalid, format!("读输入失败：{e}")))?
        .trim()
        .to_string();
    println!();
    if k.is_empty() {
        return Err(AppError::new(
            Code::KeyInvalid,
            "没输入任何东西。".to_string(),
        ));
    }
    crate::redact::register_secret(&k);
    Ok(k)
}

// ── 其它子命令 ────────────────────────────────────────────────────────────

fn cmd_status(st: &AppState) -> AppResult<()> {
    let s = flow::runtime_status(st);
    title("Hunter 状态");
    println!("装过：{}", if s.installed { "是" } else { "否" });
    println!("运行：{}", if s.running { "是" } else { "否" });
    println!(
        "版本：{}",
        s.hunter_tag.clone().unwrap_or_else(|| "—".into())
    );
    println!(
        "地址：{}",
        s.web_url.clone().unwrap_or_else(|| "—（没在运行）".into())
    );
    if let Some(t) = &s.latest_tag {
        println!("最新：{t}");
    }
    match &s.quota {
        // 额度用完了要**说出来**，不能只把两个数字摆在那里让用户自己做减法
        // （I3 回归实测：额度真用完那次，这里只显示「剩 0」，而 Hunter 里的对话
        //  已经被网关挡住了 —— 界面上一个字都没提）。
        Some(q) if q.exhausted => {
            println!(
                "额度：今日已用 {} / {} · 已用完",
                gateway::thousands(q.used_today),
                gateway::thousands(q.limit_daily)
            );
            println!(
                "      Hunter 里的对话会被网关挡住，{}想现在就用就在设置里换成自带模型 key。",
                // 网关没给 reset_at 就不提时间 —— 「直到 额度 重置」那种句子不如不写
                match q.reset_hint() {
                    Some(t) => format!("直到 {t} 重置；"),
                    None => "直到额度重置；".to_string(),
                }
            );
        }
        Some(q) => println!(
            "额度：今日已用 {} / {} · 剩 {}",
            gateway::thousands(q.used_today),
            gateway::thousands(q.limit_daily),
            gateway::thousands(q.remaining)
        ),
        None => println!("额度：—（网关没读到）"),
    }
    println!("\n服务：");
    if s.services.is_empty() {
        println!("  （一个容器都没有）");
    }
    for x in &s.services {
        println!(
            "  {:<10} {:<10} {:<8} {}",
            x.service,
            x.state,
            compose::health_cn(x.health),
            x.port
                .map(|p| p.to_string())
                .unwrap_or_else(|| "内部".into())
        );
    }
    Ok(())
}

fn cmd_simple(action: &str) -> AppResult<()> {
    match action {
        "stop" => compose::stop()?,
        "start" => compose::up()?,
        "restart" => compose::restart()?,
        "down" => compose::down()?,
        _ => {
            return Err(AppError::new(
                Code::Unknown,
                format!("不认识的操作 {action}"),
            ))
        }
    }
    println!("✓ {action} 完成");
    Ok(())
}

fn cmd_logs(service: Option<&str>) -> AppResult<()> {
    for l in compose::logs(service, 200)? {
        println!("{l}");
    }
    Ok(())
}

fn cmd_diagnose(st: &AppState, code: Option<&str>) -> AppResult<()> {
    let cfg = st.config();
    let d = crate::runtime::docker::detect();
    let ps = compose::ps().unwrap_or_default();
    let mut s = String::new();
    s.push_str(&format!(
        "# Hunter 启动器诊断 · {}\n",
        crate::timefmt::now_shanghai()
    ));
    s.push_str(&format!(
        "启动器 {} · {} {}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    s.push_str(&format!(
        "Docker {:?} · server {:?} · compose {:?}\n",
        d.runtime, d.server_version, d.compose_version
    ));
    // I4：可执行文件定位的全过程。macOS 那个 P0 之后，「用的是哪一个 docker、
    // 探过哪些位置」是排查时第一个要看的东西
    let probe = crate::runtime::which::docker_probe();
    s.push_str(&format!("docker 定位：{}\n", probe.one_line()));
    if !probe.found() {
        for l in probe.detail_lines() {
            s.push_str(&format!("  {l}\n"));
        }
        s.push_str(&format!(
            "  本进程的 PATH：{}\n",
            std::env::var("PATH").unwrap_or_else(|_| "（没有）".into())
        ));
    }
    s.push_str(&format!(
        "tag {} · 源 {}\n",
        cfg.hunter.tag, cfg.hunter.registry_prefix
    ));
    s.push_str(&format!("模型 {} · {}\n", cfg.model.mode, cfg.model.model));
    for p in &ps {
        s.push_str(&format!(
            "{} {} {} {:?}\n",
            p.service,
            p.state,
            compose::health_cn(p.health),
            p.port
        ));
    }
    s.push_str("\n## 启动器日志\n");
    for l in crate::log::tail_file(80) {
        s.push_str(&l);
        s.push('\n');
    }
    s.push_str("\n## 容器日志\n");
    for l in compose::logs(None, 60).unwrap_or_default() {
        s.push_str(&l);
        s.push('\n');
    }
    // I4：把第一层（确定性规则）的结论也打出来 —— 命令行用户同样需要它。
    //
    // 走的是 `assist::diagnose`（**不是**直接 `rules::diagnose`）：它顺带把这一次的
    // 现场存进进程内的会话槽，紧接着的 `--ai` 才有东西可问。
    // 第一版在这里直接调了 `rules::diagnose`，于是 `--diagnose --ai` 一跑就是
    // 「还没有采过现场」—— 场景 1 的第一次真跑当场撞出来的。
    //
    // **只跑规则层，不调 AI**：`--diagnose` 是一条给脚本用的路径，
    // 不该在用户没要求的时候悄悄去网关花他的额度。
    let rule = crate::assist::diagnose(code, None, None, st.hunter_key())?.rule;
    s.push_str(&format!("\n## 诊断结论（确定性规则 · {}）\n", rule.rule));
    s.push_str(&format!("{}\n{}\n", rule.title, rule.detail));
    for a in &rule.actions {
        s.push_str(&format!(
            "\n建议动作：{}（{}）\n  为什么：{}\n  会执行：{}\n",
            a.title,
            if a.kind == crate::assist::actions::Kind::ReadOnly {
                "只读"
            } else {
                "会改动这台机器"
            },
            a.why,
            a.command_line()
        ));
    }
    if !rule.confident {
        s.push_str(
            "\n（规则层没有十足把握。界面版里可以点「让 AI 帮我看看」把这份信息送去网关问一轮；\n\
             命令行下不自动调 AI —— 那会花掉你的额度。）\n",
        );
    }

    // 整体再脱敏一遍（红线 2：诊断输出里不能有 key），并把路径里的用户名换掉 ——
    // 这段输出的去处通常是 GitHub issue，和界面上「复制诊断信息」拿到的是同一份东西，
    // 两边的口径必须一致（I4 场景 1 第一次跑时这里还漏着用户名）
    println!("{}", crate::redact::mask_home(&crate::redact::redact(&s)));
    Ok(())
}

/// `--diagnose --ai`：第一层跑完，接着**真去问 AI**（I4 §三的第二层）。
///
/// 与界面版共用同一套 [`crate::assist`]，所以这里验出来的行为就是界面上的行为。
/// 两处故意不一样：
///
/// * 命令行下**不替用户执行会改动机器的动作** —— 只把将要执行的完整命令打出来。
///   没有界面就没有「点确认」这个动作，不能因此就放宽那道闸
///   （`actions::execute(_, false)` 本来也会直接拒绝）。
/// * 每一轮的 token 消耗都打出来，跑完给一个合计。
fn cmd_assist(st: &AppState, args: &Args) -> AppResult<()> {
    cmd_diagnose(st, args.code.as_deref())?;
    title("AI 诊断助手");

    if !crate::config::LauncherConfig::load().assist.enabled {
        println!("  设置里关掉了（launcher.toml 的 [assist] enabled = false）。");
        println!("  {}", crate::assist::ai::Degrade::Disabled.message());
        return Ok(());
    }

    // key 的来源和界面版一致：先看进程内存（`--key-file` 读进来的），
    // 再退回已经装好的 `.env`。一把都没有时下面会走「没填 key」那条降级。
    let key = match &args.key_file {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(k) => {
                let k = k.trim().to_string();
                crate::redact::register_secret(&k);
                st.set_hunter_key(&k);
                Some(k)
            }
            Err(e) => {
                println!("  读不到 key 文件 {p}：{e}");
                None
            }
        },
        None => st.hunter_key(),
    };

    let mut total;
    let mut rounds = 0usize;
    loop {
        let snap = crate::assist::ask(key.clone())?;
        total = snap.total_tokens;
        if let Some(d) = &snap.degraded {
            println!("  ⚠ 退回确定性规则（{:?}）", d.reason);
            println!("    {}", d.message);
            break;
        }
        if snap.rounds == rounds {
            println!("  这一轮没有进展，停在这里。");
            break;
        }
        rounds = snap.rounds;
        let Some(turn) = snap.turns.last() else { break };
        println!(
            "\n  ── 第 {} / {} 轮 · 本轮约 {} tokens ──",
            turn.round,
            snap.max_rounds,
            turn.tokens
                .map(|x| x.to_string())
                .unwrap_or_else(|| "未知".into())
        );
        if let Some(t) = &turn.text {
            for line in t.lines() {
                println!("  {line}");
            }
        }
        for r in &turn.ran {
            println!("  · 自动执行了「{}」：{}", r.title, r.command);
            for line in r.output.lines().take(8) {
                println!("      {line}");
            }
        }
        for r in &turn.rejected {
            println!("  ✗ 模型要调用「{}」，被拒绝了：{}", r.name, r.reason);
        }
        for p in &turn.pending {
            println!("\n  需要你确认才能执行的动作：");
            println!("    要做什么：{}", p.title);
            println!("    为什么：  {}", p.why);
            println!("    会执行：  {}", p.command_line());
            println!("    （命令行下不替你执行；界面版里点一下「确认执行」就行。）");
        }
        // 到头了就停：给了结论 / 有待确认的动作（命令行下没法继续）/ 轮数用完
        if snap.done || !turn.pending.is_empty() {
            break;
        }
        if snap.rounds >= snap.max_rounds {
            println!("\n  已经问满 {} 轮，不再继续。", snap.max_rounds);
            println!(
                "  {}",
                crate::assist::ai::Degrade::RoundsExhausted.message()
            );
            break;
        }
    }
    println!("\n  这次诊断一共约 {total} tokens（{rounds} 轮）。");
    Ok(())
}

/// `--assist-replay <文件>`：把一份**存下来的网关响应**喂给动作白名单那道闸。
///
/// 这是一条**离线的安全验证路径**，不发任何网络请求：
///
/// * 读一个 JSON 文件（就是 `POST /v1/chat/completions` 的响应原文），
/// * 走 [`crate::assist::ai::apply_response`] —— 和真联网时**完全同一段代码**，
/// * 打印每一个动作是被执行了、挂起等确认了，还是被拒绝了。
///
/// 为什么不做成「把网关地址改成本地假服务」：那意味着代码里要留一个能把
/// `Authorization: Bearer <用户的 key>` 指到任意地址的开关，等于给自己开一道
/// 泄漏 key 的门。读一个本地文件不碰凭据，也不碰网络。
fn cmd_assist_replay(path: &str) -> AppResult<()> {
    title("动作白名单 · 离线回放");
    let raw = std::fs::read_to_string(path)
        .map_err(|e| AppError::new(Code::Unknown, format!("读不到 {path}：{e}")))?;
    let resp: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| AppError::new(Code::Unknown, format!("{path} 不是合法 JSON：{e}")))?;

    let report = crate::assist::probe::collect(None, None, None);
    let mut session = crate::assist::ai::start(&report);
    let turn = crate::assist::ai::apply_response(&mut session, &resp);

    println!("  响应文件：{path}");
    if let Some(t) = &turn.text {
        println!("  模型说：{}", t.lines().next().unwrap_or(""));
    }
    println!(
        "  自动执行 {} 个 · 待确认 {} 个 · **拒绝 {} 个**",
        turn.ran.len(),
        turn.pending.len(),
        turn.rejected.len()
    );
    for r in &turn.ran {
        println!("  ✓ 执行了「{}」：{}", r.title, r.command);
    }
    for p in &turn.pending {
        println!("  ⏸ 挂起等用户确认：{} —— {}", p.title, p.command_line());
    }
    for r in &turn.rejected {
        println!("  ✗ 拒绝「{}」：{}", r.name, r.reason);
    }
    println!(
        "\n  （同一时刻写进 {} 的 warn 日志：）",
        paths::launcher_log().display()
    );
    for l in crate::log::tail_file(20) {
        if l.contains("白名单") {
            println!("  {l}");
        }
    }
    if turn.rejected.is_empty() {
        println!("\n  这份响应里没有白名单之外的动作。");
    }
    // I5：拒绝要能**事后查证**，所以再把审计日志的末尾打出来
    println!(
        "\n  审计日志 {}（末尾 {} 条）：",
        crate::assist::guard::audit_path().display(),
        turn.rejected.len().max(1)
    );
    for l in crate::assist::guard::audit_tail(turn.rejected.len().max(1)) {
        println!("  {l}");
    }
    Ok(())
}

// ── M4 的子命令 ───────────────────────────────────────────────────────────

/// `--check-update`：Hunter 与启动器各查一次。
///
/// 启动器那一半在 headless 下**只查不装**：装要么要替换 AppImage、要么要 root 装 `.deb`，
/// 都不是一个没有界面的进程该背着用户做的事。查到了就把下载地址打出来。
fn cmd_check_update(st: &AppState) -> AppResult<()> {
    title("检查更新");
    let cur = st.config().hunter.tag;
    let c = crate::upgrade::check(&cur, true);
    println!("Hunter 当前 v{cur}");
    match (&c.latest, &c.reason) {
        (Some(t), _) if c.has_update => {
            println!(
                "  有新版本 v{t}{}",
                if c.major_jump {
                    "（跨大版本）"
                } else {
                    ""
                }
            );
            if let Some(d) = &c.published_at {
                println!("  发布时间 {d}");
            }
            if let Some(n) = &c.notes {
                println!("\n  Release Notes 摘要：");
                for line in n.lines() {
                    println!("    {line}");
                }
            }
            if let Some(u) = &c.notes_url {
                println!("\n  完整说明 {u}");
            }
            println!("\n  升级：hunter-launcher --upgrade {t}");
        }
        (Some(t), _) => println!("  已经是最新版（最新 v{t}）"),
        (None, Some(r)) => println!("  查不到：{r}"),
        (None, None) => println!("  查不到最新版本"),
    }

    let cur = env!("CARGO_PKG_VERSION");
    println!("\n启动器当前 v{cur}");
    let kind = crate::selfupdate::install_kind();
    println!("  安装形式 {}", kind.as_str());
    // 真去读一次 updater 清单。GUI 走 Tauri 的插件（要 AppHandle），
    // headless 走 selfupdate.rs 里那套自己读 + 自己验签的实现
    match crate::selfupdate::fetch_manifest(Duration::from_secs(20)) {
        Ok((m, host)) if crate::upgrade::is_newer(&m.version, cur) => {
            println!("  有新版本 v{}（清单来自 {host}）", m.version);
            if let Some(d) = &m.pub_date {
                println!("  发布时间 {d}");
            }
            if !m.notes.trim().is_empty() {
                println!("\n  更新说明：");
                for line in crate::upgrade::summarize_notes(&m.notes, 500).lines() {
                    println!("    {line}");
                }
            }
            println!("\n  更新：hunter-launcher --self-update");
        }
        Ok((m, host)) => println!("  已经是最新版（清单来自 {host}，里面是 v{}）", m.version),
        // 拿不到就如实说原因，**不谎称「已是最新」**（红线 1）
        Err(e) => println!("  查不到：{}", e.msg),
    }
    println!(
        "  {}",
        if kind.can_self_install() {
            "这种形式支持就地自更新"
        } else {
            "这种形式（.deb 等）换包要 root，启动器只会把新包下好再给你一条命令"
        }
    );
    println!(
        "  Release 页 https://github.com/{}/releases",
        crate::selfupdate::REPO
    );
    Ok(())
}

/// `--self-update`：更新启动器自己。
///
/// 和界面上点「立即更新」是同一件事，只是**由用户显式敲出来** ——
/// 所以它可以真的动手装（AppImage 就地替换），而不是像后台检查那样只提示不装。
fn cmd_self_update() -> AppResult<()> {
    title("更新启动器");
    println!("当前 v{}", env!("CARGO_PKG_VERSION"));
    let msg = crate::selfupdate::self_update_headless(|line| println!("  {line}"))?;
    println!("\n  ✓ {msg}\n");
    Ok(())
}

/// `--upgrade <tag>`：命令行升级。和界面上点「升级」走的是**同一个** [`crate::upgrade::upgrade`]。
fn cmd_upgrade(st: &AppState, args: &Args) -> AppResult<()> {
    let target = args
        .upgrade_to
        .clone()
        .filter(|s| !s.trim().is_empty() && !s.starts_with('-'))
        .ok_or_else(|| {
            AppError::new(
                Code::UpdateFailed,
                "--upgrade 后面要跟版本号，例如 --upgrade 1.2.0".to_string(),
            )
        })?;
    let from = st.config().hunter.tag.clone();
    title(&format!("升级 Hunter v{from} → v{target}"));

    if !args.yes && std::io::stdin().is_terminal() {
        let c = crate::upgrade::check(&from, true);
        if let Some(n) = &c.notes {
            println!("Release Notes 摘要：\n{n}\n");
        }
        println!(
            "升级前会自动 pg_dump 到 {}。",
            paths::backups_dir().display()
        );
        print!("继续吗？[y/N] ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("已取消，什么都没有改动。");
            return Ok(());
        }
    }

    let tty = std::io::stdout().is_terminal();
    let mut last_print = Instant::now() - Duration::from_secs(5);
    let r = crate::upgrade::upgrade(
        st,
        &target,
        |line| println!("  {line}"),
        |p| {
            let should = if tty {
                last_print.elapsed() >= Duration::from_millis(800)
            } else {
                last_print.elapsed() >= Duration::from_secs(10)
            };
            if !should {
                return;
            }
            last_print = Instant::now();
            println!(
                "  {:>3}% {} / {}",
                p.percent,
                human_bytes(p.downloaded_bytes),
                human_bytes(p.total_bytes)
            );
        },
    )?;
    println!("\n  ✓ {}\n", r.message);
    Ok(())
}

/// `--backups`：列一下备份。
fn cmd_backups() -> AppResult<()> {
    title("备份");
    println!("目录 {}", paths::backups_dir().display());
    for line in crate::backup::summary_lines() {
        println!("  {line}");
    }
    println!("\n恢复某次备份的数据库：");
    println!("  docker compose -p hunter exec -T postgres psql -U hunter -d hunter < <备份目录>/hunter.sql");
    Ok(())
}

/// `--import-images <tar>`：导入离线包（方案 §9）。
fn cmd_import_images(st: &AppState, args: &Args) -> AppResult<()> {
    let path = args
        .import_images
        .clone()
        .filter(|s| !s.trim().is_empty() && !s.starts_with('-'))
        .ok_or_else(|| {
            AppError::new(
                Code::PullFailed,
                "--import-images 后面要跟 tar 文件的路径".to_string(),
            )
        })?;
    title("导入离线镜像包");
    let r = crate::offline::import(std::path::Path::new(&path), |line| println!("  {line}"))?;
    if !r.complete {
        return Err(AppError::new(
            Code::PullFailed,
            format!(
                "这个包不完整，缺 {}。缺的那几个还要联网拉。",
                r.missing.join("、")
            ),
        ));
    }
    let mut cfg = st.config();
    if let Some(p) = &r.registry_prefix {
        cfg.hunter.registry_prefix = p.clone();
        cfg.hunter.registry_id = "custom".into();
        if let Some(c) = crate::registry::CANDIDATES
            .iter()
            .find(|c| c.prefix == p.as_str())
        {
            cfg.hunter.registry_id = c.id.to_string();
        }
    }
    if let Some(b) = &r.base_prefix {
        cfg.hunter.base_prefix = b.clone();
    }
    if let Some(t) = &r.tag {
        cfg.hunter.tag = t.clone();
    }
    // **落盘**：`--import-images` 导完就退出了，真正安装是下一个进程做的事
    cfg.install.offline = true;
    cfg.save()?;
    st.set_config(cfg.clone());
    st.offline.store(true, std::sync::atomic::Ordering::SeqCst);
    println!(
        "\n  ✓ 六个镜像都在本机了（v{} · 源 {}）。",
        cfg.hunter.tag, cfg.hunter.registry_prefix
    );
    println!("  接着跑 `hunter-launcher --headless --key-file <路径>` 完成安装，这一次不会再联网拉镜像。\n");
    Ok(())
}

/// `--export-images <tar>`：打离线包。给有网的那台机器用。
fn cmd_export_images(st: &AppState, args: &Args) -> AppResult<()> {
    let path = args
        .export_images
        .clone()
        .filter(|s| !s.trim().is_empty() && !s.starts_with('-'))
        .ok_or_else(|| {
            AppError::new(
                Code::PullFailed,
                "--export-images 后面要跟输出 tar 的路径".to_string(),
            )
        })?;
    let cfg = st.config();
    let tag = args.tag.clone().unwrap_or_else(|| cfg.hunter.tag.clone());
    title(&format!("导出离线镜像包 v{tag}"));
    let (out, bytes, secs) = crate::offline::export(
        std::path::Path::new(&path),
        &cfg.hunter.registry_prefix,
        &cfg.hunter.base_prefix,
        &tag,
        |line| println!("  {line}"),
    )?;
    println!(
        "\n  ✓ {}（{}，用时 {}）",
        out.display(),
        human_bytes(bytes),
        human_secs(secs)
    );
    println!("  拷到目标机器上用 `hunter-launcher --import-images <这个文件>` 导入。\n");
    Ok(())
}

// ── 打印辅助 ──────────────────────────────────────────────────────────────

fn title(s: &str) {
    println!("\n{s}\n{}", "─".repeat(s.chars().count() * 2));
}

fn step(i: usize, n: usize, s: &str) {
    println!("\n[{i}/{n}] {s}");
}

pub fn human_secs(s: u64) -> String {
    if s < 60 {
        format!("{s} 秒")
    } else if s < 3600 {
        format!("{} 分 {} 秒", s / 60, s % 60)
    } else {
        format!("{} 小时 {} 分", s / 3600, (s % 3600) / 60)
    }
}

/// 让 `registry` / `config` 在 headless 里也被用到（避免 dead_code 警告的同时
/// 也确实是一个有用的检查：命令行里可以问「有哪些源」）。
pub fn list_registries() -> Vec<String> {
    registry::CANDIDATES
        .iter()
        .map(|c| format!("{} · {}", c.id, c.label))
        .collect()
}

pub fn default_tag() -> &'static str {
    config::BUNDLED_TAG
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Args {
        parse_args(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn 参数解析() {
        let x = a(&[
            "--headless",
            "--key-file",
            "/tmp/k",
            "--registry",
            "tencent",
            "--tag",
            "1.1.0",
            "-y",
        ]);
        assert!(x.headless);
        assert_eq!(x.key_file.as_deref(), Some("/tmp/k"));
        assert_eq!(x.registry.as_deref(), Some("tencent"));
        assert_eq!(x.tag.as_deref(), Some("1.1.0"));
        assert!(x.yes);
        assert!(x.action.is_none());
    }

    /// `--code` 让命令行也能按指定错误码跑一遍规则层
    /// （界面版是从状态机拿这个码的；`--diagnose` 没有状态机）。
    #[test]
    fn diagnose_能带错误码() {
        let x = a(&["--diagnose", "--code", "E_PULL_FAILED"]);
        assert_eq!(x.action.as_deref(), Some("diagnose"));
        assert_eq!(x.code.as_deref(), Some("E_PULL_FAILED"));
        assert!(x.headless);
        assert!(!x.ai, "光给错误码不该顺带把 AI 打开 —— 那会花掉用户的额度");

        let y = a(&["--diagnose", "--ai", "--code", "E_PULL_FAILED"]);
        assert!(y.ai);
        assert_eq!(y.code.as_deref(), Some("E_PULL_FAILED"));
    }

    /// `--ai` 必须是**显式**开关：`--diagnose` 常被写进脚本。
    #[test]
    fn 光_diagnose_不会去问_ai() {
        assert!(!a(&["--diagnose"]).ai);
        assert!(a(&["--diagnose", "--ai"]).ai);
    }

    /// I5：`--auto` 与 `--assist-mode`。
    #[test]
    fn 解析_auto_与授权档位() {
        let x = a(&["--auto"]);
        assert!(x.auto);
        assert!(x.headless, "--auto 单独给也要能跑起来");
        assert!(x.assist_mode.is_none(), "不给档位就用设置里存的那一档");

        let y = a(&["--auto", "--assist-mode", "auto"]);
        assert_eq!(y.assist_mode.as_deref(), Some("auto"));

        let z = a(&["--auto", "--assist-mode", "off", "--registry", "tencent"]);
        assert_eq!(z.assist_mode.as_deref(), Some("off"));
        assert_eq!(z.registry.as_deref(), Some("tencent"));

        // 不给 --auto 时走老的 cmd_install，不该被误判
        assert!(!a(&["--headless"]).auto);
    }

    #[test]
    fn 错误码字符串能还原成错误码() {
        assert_eq!(code_from_str("E_PORT_CONFLICT"), Some(Code::PortConflict));
        assert_eq!(code_from_str("E_PULL_FAILED"), Some(Code::PullFailed));
        // 认不得的不猜，返回 None（调用方会落到 E_UNKNOWN）
        assert_eq!(code_from_str("E_SOMETHING_ELSE"), None);
        assert_eq!(code_from_str(""), None);
    }

    /// 帮助里要写到这两个新开关 —— 不然没人知道它们存在。
    #[test]
    fn 帮助里写了_auto() {
        assert!(HELP.contains("--auto"), "{HELP}");
        assert!(HELP.contains("--assist-mode"), "{HELP}");
    }

    #[test]
    fn assist_replay_要跟一个文件路径() {
        let x = a(&["--assist-replay", "/tmp/r.json"]);
        assert_eq!(x.action.as_deref(), Some("assist-replay"));
        assert_eq!(x.assist_replay.as_deref(), Some("/tmp/r.json"));
        assert!(x.headless);
    }

    #[test]
    fn 子命令会自动进入_headless() {
        for (arg, act) in [
            ("--status", "status"),
            ("--stop", "stop"),
            ("--restart", "restart"),
            ("--down", "down"),
            ("--diagnose", "diagnose"),
        ] {
            let x = a(&[arg]);
            assert!(x.headless, "{arg} 应当隐含 headless");
            assert_eq!(x.action.as_deref(), Some(act));
        }
    }

    #[test]
    fn logs_可以带服务名() {
        let x = a(&["--logs", "api"]);
        assert_eq!(x.action.as_deref(), Some("logs"));
        assert_eq!(x.tag.as_deref(), Some("api"));
        let y = a(&["--logs"]);
        assert_eq!(y.action.as_deref(), Some("logs"));
        assert!(y.tag.is_none());
    }

    #[test]
    fn pull_only_隐含_headless() {
        let x = a(&["--pull-only", "--tag", "1.1.0"]);
        assert!(x.pull_only);
        assert!(x.headless, "--pull-only 单独给也要能跑起来");
        assert_eq!(x.tag.as_deref(), Some("1.1.0"));
        assert!(!a(&["--headless"]).pull_only);
    }

    #[test]
    fn 没有参数时不进_headless() {
        let x = a(&[]);
        assert!(!x.headless);
        assert!(!x.help);
    }

    #[test]
    fn 时长可读化() {
        assert_eq!(human_secs(5), "5 秒");
        assert_eq!(human_secs(75), "1 分 15 秒");
        assert_eq!(human_secs(3725), "1 小时 2 分");
    }

    #[test]
    fn 帮助文本提到了关键开关() {
        for k in [
            "--headless",
            "--key-file",
            "--registry",
            "--status",
            "--upgrade",
            "--import-images",
            "--export-images",
            "--check-update",
            "--self-update",
            "HUNTER_HOME",
            "600",
        ] {
            assert!(HELP.contains(k), "帮助里缺 {k}");
        }
    }

    #[test]
    fn m4_的四个子命令都能解析出来() {
        let x = a(&["--upgrade", "1.2.0"]);
        assert_eq!(x.action.as_deref(), Some("upgrade"));
        assert_eq!(x.upgrade_to.as_deref(), Some("1.2.0"));
        assert!(x.headless);

        let y = a(&["--import-images", "/tmp/hunter-images-1.2.0.tar"]);
        assert_eq!(y.action.as_deref(), Some("import-images"));
        assert_eq!(
            y.import_images.as_deref(),
            Some("/tmp/hunter-images-1.2.0.tar")
        );

        let z = a(&["--export-images", "/tmp/out.tar", "--tag", "1.1.0"]);
        assert_eq!(z.action.as_deref(), Some("export-images"));
        assert_eq!(z.export_images.as_deref(), Some("/tmp/out.tar"));
        assert_eq!(z.tag.as_deref(), Some("1.1.0"));

        let w = a(&["--check-update"]);
        assert_eq!(w.action.as_deref(), Some("check-update"));
        assert!(w.headless);

        let u = a(&["--self-update"]);
        assert_eq!(u.action.as_deref(), Some("self-update"));
        assert!(u.headless);

        let b = a(&["--backups"]);
        assert_eq!(b.action.as_deref(), Some("backups"));
    }

    #[test]
    fn 升级命令少了版本号不会去升一个空版本() {
        // `--upgrade` 后面什么都不跟，或者跟了另一个开关
        let x = a(&["--upgrade"]);
        assert_eq!(x.action.as_deref(), Some("upgrade"));
        assert!(x.upgrade_to.is_none());
        let y = a(&["--upgrade", "--yes"]);
        assert_eq!(y.upgrade_to.as_deref(), Some("--yes"));
        // cmd_upgrade 里会把以 `-` 开头的值当成没给，这条由那里的 filter 保证
        assert!(y.upgrade_to.as_deref().unwrap().starts_with('-'));
    }

    #[test]
    fn 候选源列表() {
        let v = list_registries();
        assert_eq!(v.len(), 2);
        assert!(v.iter().any(|s| s.contains("ghcr")));
        assert!(v.iter().any(|s| s.contains("腾讯云")));
        assert_eq!(default_tag(), "1.2.0");
    }
}
