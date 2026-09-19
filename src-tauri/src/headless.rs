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
//! hunter-launcher --diagnose                  打印脱敏诊断
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
  hunter-launcher --status              看当前状态
  hunter-launcher --start | --stop | --restart | --down
  hunter-launcher --logs [服务名]       看容器日志（已脱敏）
  hunter-launcher --diagnose            打印脱敏诊断信息
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
        Some("diagnose") => cmd_diagnose(&st),
        Some("check-update") => cmd_check_update(&st),
        Some("self-update") => cmd_self_update(),
        Some("upgrade") => cmd_upgrade(&st, args),
        Some("backups") => cmd_backups(),
        Some("import-images") => cmd_import_images(&st, args),
        Some("export-images") => cmd_export_images(&st, args),
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

    // 1) Docker
    step(1, steps, "检查 Docker");
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

    // 2) key
    step(2, steps, "hunter key");
    let key = read_key(args)?;
    let check = gateway::check_key(&key, Duration::from_secs(25));
    if !check.valid {
        println!("  ✗ {}", check.message.clone().unwrap_or_default());
        let code = match check.reason {
            gateway::KeyReason::Exhausted => Code::QuotaExhausted,
            gateway::KeyReason::Network => Code::ProxyBlock,
            _ => Code::KeyInvalid,
        };
        return Err(AppError::new(code, "key 没通过校验。".to_string()));
    }
    st.set_hunter_key(&key);
    match &check.quota {
        Some(q) => println!(
            "  ✓ key 有效 · 今日已用 {} / {} · 剩 {} · 每分钟 {} 次 · 并发 {}",
            q.used_today,
            q.limit_daily,
            q.remaining,
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
        Some(q) => println!(
            "额度：今日已用 {} / {} · 剩 {}",
            q.used_today, q.limit_daily, q.remaining
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

fn cmd_diagnose(st: &AppState) -> AppResult<()> {
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
    // 整体再脱敏一遍（红线 2：诊断输出里不能有 key）
    println!("{}", crate::redact::redact(&s));
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
