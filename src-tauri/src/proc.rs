//! 子进程调用。**一律参数数组，不拼 shell 字符串**（总控规则红线 3）。
//!
//! 这个模块只暴露「给一个程序名 + 一个参数数组」的接口，
//! 连一个接受整串命令行的函数都不提供 —— 想拼字符串也没有地方拼。
//!
//! ## 两种超时（I16）
//!
//! | | 谁在管 | 什么时候用 |
//! |---|---|---|
//! | **总超时** | [`run_timeout`] | 短命令。「这条命令最多允许跑 N 秒」 |
//! | **静默超时** | [`run_streaming`] 的 [`StreamOpts::silence`] | 长跑且有进度流的：拉镜像、起容器、导入离线包 |
//!
//! 为什么必须有第二种：长跑的命令给不出一个合理的总超时 ——
//! 748 MB 在好网上 52 秒、在慢网上二十分钟，写死哪个数都是错的。
//! 而**「多久没有任何新进展」是一个与网速无关的量**。
//!
//! 现场（2026-09-25，客户的 Mac，0.1.15）：升级时 `docker compose pull`
//! 把六个镜像的 manifest 全读到了（界面上真的算出了「合计约 748 MB」），
//! 然后拉层那一步 TCP 连上了、进程活着、**一个字节都不传**，
//! 界面「正在拉取新版本的镜像…」挂了一个多小时，最后用户自己强退。
//! `compose pull` 不报错，所以既有的「报错 → 换源重试」一次都没触发。
//!
//! 同一个病在 Hunter 那边（0.1.22 修的 npm 直连卡死）刚犯过一次。
//! 这一层是**在 `proc` 上治它**，而不是在拉镜像那一处打补丁。

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use crate::err::{AppError, AppResult, Code};

// ── 子进程输出的解码（I16 · P0-3） ────────────────────────────────────────

/// 把子进程的一段输出字节解成字符串。
///
/// **不能一律 `String::from_utf8_lossy`。** 客户 2026-09-26 的 Windows 诊断包里，
/// 定时备份装不上那一行长这样：
///
/// ```text
/// 装完挂定时备份没成（不影响安装）：schtasks /Create 失败：����: δָ���Ĵ���
/// ```
///
/// 那串东西不是乱码而已 —— 它是 GBK 字节被当成 UTF-8 解的结果。
/// 把它按 GBK 解回来是 **`错误: 未指定的错误`**
/// （`b4 ed ce f3 3a 20 ce b4 d6 b8 b6 a8 b5 c4 b4 ed ce f3`，
/// 单测 `gbk_的_schtasks_报错要解成中文` 逐字节钉住了这件事）。
///
/// 后果有两层，第二层更严重：真正的原因我们**从来没看见过**；
/// 而客户以为自己有每日自动备份，**实际上一次都没跑过**。
///
/// 规则（按平台走，不猜）：
///
/// 1. 先按 UTF-8 严格解。成了就用它 —— docker / git / 我们自己的程序都吐 UTF-8；
/// 2. 解不动，才按这台机器的控制台代码页重解一遍（中文 Windows 是 936/GBK）；
/// 3. 代码页不认识（例如 OEM 437，`encoding_rs` 只实现 WHATWG 那套编码）
///    就退回 `from_utf8_lossy` —— **退回去也要退得明明白白**，不自己编码表。
pub fn decode_output(bytes: &[u8]) -> String {
    decode_console(bytes, ansi_codepage())
}

/// [`decode_output`] 的**纯函数**版：代码页由调用方给。
///
/// 拆出来只为一件事 —— 让「给一段 GBK 字节流，断言解出来是中文」这条单测
/// 能在 Linux 上跑。我们手上一台 Windows 都没有，
/// 把这段逻辑关进 `cfg(windows)` 就等于永远不测它。
pub fn decode_console(bytes: &[u8], codepage: Option<u32>) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    match codepage.and_then(encoding_of_codepage) {
        Some(enc) => enc.decode(bytes).0.into_owned(),
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// 代码页号 → `encoding_rs` 的编码。认不出来返回 `None`（退回 lossy，不瞎猜）。
fn encoding_of_codepage(cp: u32) -> Option<&'static encoding_rs::Encoding> {
    Some(match cp {
        936 | 54936 => encoding_rs::GBK, // 简体中文（客户那台就是它）
        950 => encoding_rs::BIG5,        // 繁体中文
        932 => encoding_rs::SHIFT_JIS,   // 日文
        949 => encoding_rs::EUC_KR,      // 韩文
        874 => encoding_rs::WINDOWS_874, // 泰文
        1250 => encoding_rs::WINDOWS_1250,
        1251 => encoding_rs::WINDOWS_1251,
        1252 => encoding_rs::WINDOWS_1252,
        1253 => encoding_rs::WINDOWS_1253,
        1254 => encoding_rs::WINDOWS_1254,
        1255 => encoding_rs::WINDOWS_1255,
        1256 => encoding_rs::WINDOWS_1256,
        1257 => encoding_rs::WINDOWS_1257,
        1258 => encoding_rs::WINDOWS_1258,
        65001 => encoding_rs::UTF_8,
        _ => return None,
    })
}

/// 这台机器上「非 UTF-8 的那一路」该按哪个代码页解。非 Windows 一律 `None`。
///
/// 先问**控制台输出代码页**（子进程真正用来写 stdout/stderr 的那个），
/// 问不到（GUI 程序没有控制台时 `GetConsoleOutputCP` 会返回 0）再问 ANSI 代码页。
#[cfg(windows)]
fn ansi_codepage() -> Option<u32> {
    extern "system" {
        fn GetConsoleOutputCP() -> u32;
        fn GetACP() -> u32;
    }
    // SAFETY: 两个都是 kernel32 里无参数、无副作用、返回一个整数的函数
    let cp = unsafe { GetConsoleOutputCP() };
    if cp != 0 {
        return Some(cp);
    }
    let cp = unsafe { GetACP() };
    (cp != 0).then_some(cp)
}

#[cfg(not(windows))]
fn ansi_codepage() -> Option<u32> {
    // macOS 与 Linux 上子进程的输出就是 UTF-8（locale 再怪也不会是 GBK 的双字节流），
    // 真解不动时 lossy 是对的：那说明它吐的是二进制，不是另一种文字编码
    None
}

#[derive(Debug)]
pub struct Ran {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Ran {
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }
    /// 失败时给人看的一行说明（会进日志，所以由 AppError 那边统一脱敏）
    pub fn err_line(&self) -> String {
        let s = self.stderr.trim();
        let s = if s.is_empty() { self.stdout.trim() } else { s };
        s.lines().next_back().unwrap_or("（没有输出）").to_string()
    }
}

/// 跑一个程序并等它结束。程序**起不来**（没装 / 不在 PATH）与**跑了但失败**是两种不同的返回：
/// 前者 `Err`，后者 `Ok(Ran{status: Some(非 0)})`。M0 §5.4 的 Docker 判定就靠这个区分。
pub fn run(program: &str, args: &[&str]) -> AppResult<Ran> {
    run_with_env(program, args, &[])
}

pub fn run_with_env(program: &str, args: &[&str], env: &[(&str, &str)]) -> AppResult<Ran> {
    isolation_guard(program, args, env)?;
    let mut cmd = base_command(program);
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out: Output = cmd
        .stdin(Stdio::null())
        .output()
        .map_err(|e| AppError::new(Code::Unknown, format!("无法执行 {program}：{e}")))?;
    Ok(Ran {
        status: out.status.code(),
        stdout: decode_output(&out.stdout),
        stderr: decode_output(&out.stderr),
    })
}

/// 带超时的版本。docker 在 daemon 半死不活时会一直挂着，不设超时会把界面卡住。
pub fn run_timeout(program: &str, args: &[&str], timeout: Duration) -> AppResult<Ran> {
    run_timeout_env(program, args, timeout, &[])
}

/// 带超时 **且带额外环境变量** 的版本（I7 的内置运行时要给 colima / docker
/// 传 `LIMA_HOME` / `COLIMA_HOME` / `DOCKER_HOST`）。
///
/// 这些变量**只加在这一次调用上**，不写进进程环境 —— 启动器自己的环境里
/// 出现 `DOCKER_HOST` 会把「用不用内置运行时」这件事变成全局隐式状态，
/// 那正是最难查的一类 bug。
pub fn run_timeout_env(
    program: &str,
    args: &[&str],
    timeout: Duration,
    env: &[(&str, &str)],
) -> AppResult<Ran> {
    run_cmd_timeout(base_command(program), program, args, timeout, env)
}

/// 带超时，但**把我们自己那几个运行时变量摘掉**（I9）。
///
/// 用在「启动用户自己装的那个运行时」这条路上：OrbStack / Docker Desktop /
/// 他自己的 colima / systemd 的 docker 服务，都是**他的**东西，
/// 我们的 `COLIMA_HOME` / `LIMA_HOME` / `DOCKER_HOST` / `DOCKER_CONFIG`
/// 在那里一点道理都没有。
///
/// 不摘会出真问题：机器上留着内置运行时的残骸时，[`crate::runtime::env`] 会把
/// `COLIMA_HOME` 钉在 `~/.hunter/runtime` 上，于是「起用户原有的那个 profile」
/// 这条命令会带着**我们的家**去找**他的 profile** —— 守卫当场拒掉，
/// 而那条命令本来是完全正当的。
///
/// PATH 补全与代理照常保留：那两样是「让程序跑得起来」，不是「指向谁的东西」。
pub fn run_timeout_user_runtime(program: &str, args: &[&str], timeout: Duration) -> AppResult<Ran> {
    let mut cmd = base_command(program);
    for k in crate::runtime::env::OURS_ONLY {
        cmd.env_remove(k);
    }
    run_cmd_timeout(cmd, program, args, timeout, &[])
}

/// `run_timeout_env` 与 `run_timeout_bare` 共用的那一段。
/// 差别只有一个：`cmd` 是 [`base_command`] 建的还是 [`base_command_bare`] 建的。
fn run_cmd_timeout(
    mut cmd: Command,
    program: &str,
    args: &[&str],
    timeout: Duration,
    env: &[(&str, &str)],
) -> AppResult<Ran> {
    isolation_guard(program, args, env)?;
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::new(Code::Unknown, format!("无法执行 {program}：{e}")))?;

    // **必须一边跑一边读**（I7 实测撞出来的）。
    //
    // 管道的内核缓冲区只有 64 KB 左右。原先的写法是「先等它退出，再
    // `wait_with_output()` 把输出读出来」—— 子进程写满 64 KB 之后就阻塞在 `write` 上
    // 永远不退出，而我们在等它退出。**双方互等，只能靠超时收场。**
    //
    // 现场：`docker compose logs --tail 200` 对着一套 6 个容器的栈
    // （postgres 的日志行很长），60 秒超时报「超过 60 秒没有返回」；
    // 同一条命令在终端里 0.24 秒就跑完了。
    //
    // 这不是 I7 才有的问题 —— `compose::logs` 走的是同一个函数，
    // 只是我们自己那一套的日志一直没满过 64 KB。
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let h_out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = std::io::Read::read_to_end(p, &mut buf);
        }
        buf
    });
    let h_err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = std::io::Read::read_to_end(p, &mut buf);
        }
        buf
    });

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    // 杀掉它，两个读线程就会拿到 EOF 结束 —— 否则 join 会挂住
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = h_out.join();
                    let _ = h_err.join();
                    return Err(AppError::new(
                        Code::Unknown,
                        format!(
                            "{program} {} 超过 {} 秒没有返回",
                            args.join(" "),
                            timeout.as_secs()
                        ),
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = h_out.join();
                let _ = h_err.join();
                return Err(AppError::new(
                    Code::Unknown,
                    format!("等待 {program} 失败：{e}"),
                ));
            }
        }
    };
    let stdout = h_out.join().unwrap_or_default();
    let stderr = h_err.join().unwrap_or_default();
    Ok(Ran {
        status: status.code(),
        stdout: decode_output(&stdout),
        stderr: decode_output(&stderr),
    })
}

// ── 静默超时：一边跑一边看有没有进展（I16） ────────────────────────────────

/// 默认的静默阈值：**90 秒没有任何新进展就算卡死**。
///
/// 为什么是 90 秒：拉镜像时 docker 每读到一块就报一次进度，正常拉取里
/// 两条进度之间是亚秒级的。真实的对照是 2026-09-25 另一台 Mac 上那一次
/// —— 同样是 1.2.0 → 1.2.2、同样走腾讯云香港，**整整 52 秒**就全部拉完了
/// （08:27:48 开始 → 08:28:41 完成）。也就是说 90 秒的静默比**整次拉取**还长：
/// 一条正常的连接不可能在中间空这么久，而一条卡死的连接会一直空下去。
///
/// 这个数是可调的（`[hunter] pull_stall_secs`）—— 真撞上极慢的网时，
/// 调大它比让用户对着一个死掉的连接等一个多小时强。
pub const DEFAULT_SILENCE: Duration = Duration::from_secs(90);

/// 轮询间隔。静默判定、取消判定、`Ev::Poll` 都按这个节奏走。
const POLL: Duration = Duration::from_millis(200);

/// 每个流最多留多少行原文（给调用方拼「原话」用）。
const DEFAULT_TAIL: usize = 400;

/// 子进程给出的一个事件。
pub enum Ev<'a> {
    /// 从 stdout（`is_err = false`）或 stderr（`is_err = true`）读到的一整行
    Line { text: &'a str, is_err: bool },
    /// 没有新行时每 200 毫秒来一次。用来刷界面，也给调用方一个
    /// 「我从别的地方知道有进展」的机会
    Poll,
}

/// 处理完一个事件之后，调用方回答一句：**这算不算进展**。
///
/// 回 [`Fresh::Yes`] 就把静默计时器归零。
///
/// 为什么这件事要交给调用方判断，而不是「管道上有字节就算」：
/// `docker compose --progress json pull` 在拉不动的时候**照样会重复吐
/// 一模一样的进度行**（同一层、同一个 `current`）。按「管道有字节」算，
/// 那种现场永远判不出卡死；按「聚合出来的进度有没有变」算才判得准。
/// 不关心这件事的调用方一律回 `Yes` 就行（等价于「管道有字节 = 有进展」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fresh {
    Yes,
    No,
}

/// 子进程是怎么结束的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// 自己退出的，带退出码（拿不到码时为 `None`，例如被信号打死）
    Exited(Option<i32>),
    /// **静默超时**：`silent_secs` 秒没有任何新进展，已经把它杀了
    Stalled { silent_secs: u64 },
    /// 总时长超过上限，已经把它杀了
    TimedOut { secs: u64 },
    /// 调用方的 `cancel` 置位，已经把它杀了
    Cancelled,
}

impl Ended {
    pub fn ok(self) -> bool {
        self == Ended::Exited(Some(0))
    }
}

/// [`run_streaming`] 的可选项。
pub struct StreamOpts<'a> {
    /// 多久没有进展算卡死。`None` = 不判静默（老行为）
    pub silence: Option<Duration>,
    /// 总时长上限。`None` = 不限（长跑命令就该不限，靠静默超时收场）
    pub total: Option<Duration>,
    /// 置位就杀掉子进程并返回 [`Ended::Cancelled`]
    pub cancel: Option<&'a Arc<AtomicBool>>,
    pub cwd: Option<&'a std::path::Path>,
    pub env: &'a [(&'a str, &'a str)],
    /// 每个流各留多少行原文
    pub tail: usize,
}

impl Default for StreamOpts<'_> {
    fn default() -> Self {
        Self {
            silence: Some(DEFAULT_SILENCE),
            total: None,
            cancel: None,
            cwd: None,
            env: &[],
            tail: DEFAULT_TAIL,
        }
    }
}

/// 一次流式运行的结果。
pub struct Streamed {
    pub ended: Ended,
    /// stdout 的最后若干行原文
    pub stdout: Vec<String>,
    /// stderr 的最后若干行原文
    pub stderr: Vec<String>,
    /// 从起进程到结束一共多久
    pub elapsed: Duration,
}

/// 起一个长跑的子进程，**一边跑一边把每一行交给调用方**，并在
/// 「太久没有进展」时主动把它杀掉。
///
/// 和 [`run_cmd_timeout`] 的区别有两处，都是长跑命令必需的：
///
/// 1. 输出是**逐行推给调用方**的，不是等它跑完再一次性给 ——
///    进度条要靠这个才动得起来；
/// 2. 超时是**静默超时**，不是总超时（见模块注释那张表）。
///
/// 管道照样是两个线程各读各的（I7 的 64 KB 死锁在这里同样成立）。
pub fn run_streaming(
    program: &str,
    args: &[&str],
    opts: &StreamOpts,
    mut on_event: impl FnMut(Ev) -> Fresh,
) -> AppResult<Streamed> {
    isolation_guard(program, args, opts.env)?;
    let mut cmd = base_command(program);
    cmd.args(args);
    for (k, v) in opts.env {
        cmd.env(k, v);
    }
    if let Some(d) = opts.cwd {
        cmd.current_dir(d);
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::new(Code::Unknown, format!("无法执行 {program}：{e}")))?;

    let started = Instant::now();
    let (tx, rx) = mpsc::channel::<(bool, String)>();
    let mut readers = Vec::new();
    for (stream, is_err) in [
        (child.stdout.take().map(Pipe::Out), false),
        (child.stderr.take().map(Pipe::Err), true),
    ] {
        let Some(stream) = stream else { continue };
        let tx = tx.clone();
        let tail = opts.tail;
        let collected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&collected);
        let h = std::thread::spawn(move || {
            let mut reader: Box<dyn BufRead> = match stream {
                Pipe::Out(s) => Box::new(BufReader::new(s)),
                Pipe::Err(s) => Box::new(BufReader::new(s)),
            };
            // **按字节读，自己切行，再按 [`decode_output`] 解**（I16 · P0-3）。
            //
            // 原先是 `reader.lines().map_while(Result::ok)`：`Lines` 只吐
            // `io::Result<String>`，碰到一行不是合法 UTF-8 就返回 `Err`，
            // 而 `map_while(Result::ok)` 见 `Err` 即**整条流就此停读** ——
            // 在中文 Windows 上，子进程随便吐一句 GBK 的错误提示，
            // 后面所有输出（包括拉取进度）就全丢了，而且丢得悄无声息。
            let mut buf: Vec<u8> = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
                while matches!(buf.last(), Some(b'\n') | Some(b'\r')) {
                    buf.pop();
                }
                let line = decode_output(&buf);
                if let Ok(mut g) = sink.lock() {
                    g.push(line.clone());
                    if g.len() > tail {
                        let n = g.len() - tail;
                        g.drain(..n);
                    }
                }
                if tx.send((is_err, line)).is_err() {
                    break;
                }
            }
        });
        readers.push((h, collected, is_err));
    }
    drop(tx); // 两个读线程都结束之后 rx 才会断开

    let mut last_fresh = Instant::now();
    let ended = loop {
        match rx.recv_timeout(POLL) {
            Ok((is_err, line)) => {
                if on_event(Ev::Line {
                    text: &line,
                    is_err,
                }) == Fresh::Yes
                {
                    last_fresh = Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if on_event(Ev::Poll) == Fresh::Yes {
                    last_fresh = Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break None,
        }
        if opts.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            break Some(Ended::Cancelled);
        }
        if let Some(limit) = opts.silence {
            let idle = last_fresh.elapsed();
            if idle >= limit {
                break Some(Ended::Stalled {
                    silent_secs: idle.as_secs(),
                });
            }
        }
        if let Some(limit) = opts.total {
            if started.elapsed() >= limit {
                break Some(Ended::TimedOut {
                    secs: started.elapsed().as_secs(),
                });
            }
        }
    };

    // 非正常结束：先杀
    let killed = ended.is_some();
    if killed {
        kill_hard(&mut child);
    }
    let status = child.wait().ok();

    // **被杀的那一路不 join 读线程。**
    //
    // 杀掉的是我们 spawn 的那一个进程，但它可能已经 fork 出孙子进程
    // （`sh -c '… ; sleep 600'` 就是，`docker compose` 拉起 buildkit 辅助进程同理）。
    // 孙子进程继承着同一个管道写端 —— 父进程死了，管道**不会**关，
    // 读线程于是永远等不到 EOF。join 它就等于把「杀掉卡死的进程」
    // 换成「自己也卡死」，那比原来的病还糟。
    //
    // 原文是从 `Arc<Mutex<Vec<String>>>` 里取的，不需要线程结束就拿得到；
    // 读线程自己会在 `tx.send` 失败（rx 已经丢掉）时退出，最迟在下一行输出时。
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    drop(rx); // 让还活着的读线程在下一次 send 时退出
    for (h, collected, is_err) in readers {
        if !killed {
            let _ = h.join();
        }
        if let Ok(g) = collected.lock() {
            if is_err {
                stderr = g.clone();
            } else {
                stdout = g.clone();
            }
        }
    }
    Ok(Streamed {
        ended: ended.unwrap_or_else(|| Ended::Exited(status.and_then(|s| s.code()))),
        stdout,
        stderr,
        elapsed: started.elapsed(),
    })
}

/// 杀掉子进程并等它收尸。
///
/// **Unix 上先 `SIGTERM` 再 `SIGKILL`**：`docker compose pull` 收到 TERM
/// 会自己去把已经起的拉取协程收掉；直接 KILL 它来不及做这件事，
/// 会在 daemon 那边留下半截的下载任务。给它 2 秒，不走再硬杀。
fn kill_hard(child: &mut Child) {
    #[cfg(unix)]
    {
        // 只用一个 extern 声明，不为这一行把 libc 拖进依赖树（main.rs 里同样的写法）
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        const SIGTERM: i32 = 15;
        let pid = child.id() as i32;
        // SAFETY: pid 来自我们自己刚 spawn 的子进程，还没有被收尸
        unsafe {
            kill(pid, SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// 两个管道类型不同，又想用同一段读取代码，包一层。
enum Pipe {
    Out(std::process::ChildStdout),
    Err(std::process::ChildStderr),
}

/// **虚拟机隔离守卫**（I9 的 P0-2），挂在真正 `spawn` 之前的那一刻。
///
/// 为什么挂在这里而不是只挂在动作表里：动作表那一道
/// （[`crate::assist::guard::argv`]）只看得到 AI / 规则层提出来的动作，
/// 而 0.1.8 那条闯祸的 `colima start` 是**动作表自己规划出来的**
/// （`start_runtime_argv("colima")`）—— 提案那一侧没有人觉得它有问题。
/// 挂在 `proc` 这一层，意味着**不管这条命令是谁写的、走的哪条路**，
/// 只要它最终要变成一个进程，就必须先回答「你用的是谁的 COLIMA_HOME、
/// 动的是哪一台虚拟机」。
///
/// 只对 colima / limactl 生效，别的程序零开销（一次 basename 比对）。
fn isolation_guard(program: &str, args: &[&str], env: &[(&str, &str)]) -> AppResult<()> {
    let base = std::path::Path::new(program)
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| program.to_ascii_lowercase());
    let base = base.trim_end_matches(".exe");
    if base != "colima" && base != "limactl" {
        return Ok(());
    }
    crate::assist::guard::colima_call(program, args, env)
}

/// 判断一个程序在不在 PATH 上。
pub fn exists(program: &str) -> bool {
    base_command(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// 读系统代理那两条命令专用：**不套 [`crate::runtime::env`]**（I8）。
///
/// ## 为什么要单开一条路
///
/// `netproxy::current()` 要跑 `scutil --proxy`（macOS）或 `reg query`（Windows）。
/// 走 [`base_command`] 的话，链条是这样的：
///
/// ```text
/// netproxy::current()  ← 缓存还空着
///   → detect() → read_scutil() → proc::run_timeout() → base_command()
///   → runtime::env::apply() → env::current()  ← 缓存也还空着
///   → env::compute() → netproxy::current()    ← 回到第一行
/// ```
///
/// **两个缓存都还没写，所以它会一直绕下去。** CI 的 macOS runner 上就是这么
/// `fatal runtime error: stack overflow` 的 —— Linux 上 `detect()` 根本不走
/// `scutil` 那一支（`cfg!(target_os = "macos")`），所以本地 482 条全绿。
/// 又一条「只有 macOS 才炸」的。
///
/// 断链的办法是**这两条命令不需要那份环境**：程序用的是绝对路径 `/usr/sbin/scutil`
/// （不需要 PATH 补全），读一份本机设置也不需要代理变量。所以干脆不套。
pub fn base_command_bare(program: &str) -> Command {
    // `mut` 只有 Windows 那一支用得上；不加 allow 的话非 Windows 上 clippy 会红
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 带超时、**不套 [`crate::runtime::env`]** 的版本。只给 [`crate::netproxy`] 用，
/// 理由见 [`base_command_bare`]。
pub fn run_timeout_bare(program: &str, args: &[&str], timeout: Duration) -> AppResult<Ran> {
    run_cmd_timeout(base_command_bare(program), program, args, timeout, &[])
}

/// 建 Command。**这是全项目唯一建子进程的地方**，两件事在这里统一做掉：
///
/// 1. Windows 上加 `CREATE_NO_WINDOW`，否则每调一次 docker 都会闪一个黑框；
/// 2. 套上 [`crate::runtime::env`] 算出来的环境 —— PATH 补全 + 必要时的
///    `DOCKER_CONFIG`。**I6 的 P0 就修在这一行**：I4 解决了「启动器找得到 docker」，
///    但 docker 自己去 `$PATH` 上找 `docker-credential-osxkeychain` 时用的是
///    子进程继承的那份受限 PATH，照样找不到（用户 Mac 上 0.1.5 的现场）。
pub fn base_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    crate::runtime::env::apply(&mut cmd);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 不存在的程序返回_err_而不是非零退出码() {
        let r = run("hunter-definitely-not-a-real-program", &["--version"]);
        assert!(
            r.is_err(),
            "起不来的程序必须是 Err，M0 §5.4 的判定依赖这一点"
        );
    }

    #[test]
    #[cfg(unix)]
    fn 跑得起来但失败的返回非零退出码() {
        let r = run("sh", &["-c", "exit 3"]).expect("sh 应当存在");
        assert_eq!(r.status, Some(3));
        assert!(!r.ok());
    }

    #[test]
    #[cfg(unix)]
    fn 参数是数组不经过_shell() {
        // 如果实现里偷偷拼了 shell 字符串，这里的 `;` 会被当成命令分隔符
        let r = run("echo", &["a; touch /tmp/hunter-should-not-exist"]).expect("echo 应当存在");
        assert!(
            r.stdout.contains("a; touch"),
            "参数被 shell 解释了：{}",
            r.stdout
        );
        assert!(!std::path::Path::new("/tmp/hunter-should-not-exist").exists());
    }

    #[test]
    #[cfg(unix)]
    fn 超时会把进程杀掉() {
        let r = run_timeout("sleep", &["10"], Duration::from_millis(300));
        assert!(r.is_err());
        assert!(r.unwrap_err().msg.contains("没有返回"));
    }

    /// **输出超过管道缓冲区（64 KB）时不许挂住。**
    ///
    /// I7 实测撞出来的：原先的写法先等进程退出、再 `wait_with_output()` 读输出 ——
    /// 子进程写满 64 KB 之后阻塞在 `write` 上，而我们在等它退出，双方互等。
    /// 现场是 `docker compose logs --tail 200` 对着一套 6 个容器的栈。
    ///
    /// 这条测试造 1 MB 输出（远超缓冲区），限时 20 秒 —— 修之前必然超时。
    #[test]
    fn 输出超过管道缓冲区也不会挂住() {
        // 1 MB：`yes` 打 16 个字符一行，打 65536 行
        let r = run_timeout(
            "/usr/bin/env",
            &[
                "sh",
                "-c",
                "i=0; while [ $i -lt 65536 ]; do echo 0123456789abcde; i=$((i+1)); done",
            ],
            Duration::from_secs(20),
        );
        // 这台机器上没有 /usr/bin/env 或 sh 的话跳过（Windows）
        let Ok(r) = r else {
            if cfg!(windows) {
                return;
            }
            panic!("起不来：{:?}", r.err().map(|e| e.msg));
        };
        assert_eq!(r.status, Some(0));
        assert!(
            r.stdout.len() > 1_000_000,
            "只读到 {} 字节，管道那一头被截断了",
            r.stdout.len()
        );
        assert_eq!(r.stdout.lines().count(), 65536);
    }

    // ── 静默超时（I16） ──────────────────────────────────────────────────

    /// **「输出一行就再也不吐字节」的假子进程必须被杀掉，而且结论是「卡住」不是「失败」。**
    ///
    /// 这就是客户 Mac 上那个现场的最小复现：进程活着、管道开着、一个字节都不来。
    /// 阈值在测试里调到 1 秒（生产是 [`DEFAULT_SILENCE`] 90 秒）。
    #[test]
    #[cfg(unix)]
    fn 吐一行之后就不动的进程会被静默超时杀掉() {
        let mut lines = Vec::new();
        let t0 = Instant::now();
        let r = run_streaming(
            "/usr/bin/env",
            &["sh", "-c", "echo 开始拉取; sleep 600"],
            &StreamOpts {
                silence: Some(Duration::from_secs(1)),
                ..Default::default()
            },
            |ev| {
                if let Ev::Line { text, .. } = ev {
                    lines.push(text.to_string());
                    return Fresh::Yes;
                }
                Fresh::No
            },
        )
        .expect("起得来");
        // 判定是「卡住」，不是「失败」—— 两者的处置完全不同
        match r.ended {
            Ended::Stalled { silent_secs } => assert!(silent_secs >= 1),
            other => panic!("应当判成卡住，实际是 {other:?}"),
        }
        assert!(!r.ended.ok());
        assert_eq!(lines, vec!["开始拉取".to_string()]);
        // 真的被杀了：600 秒的 sleep 不可能 10 秒内回来
        assert!(t0.elapsed() < Duration::from_secs(10), "没杀掉，等满了");
    }

    /// **一直有新进展的进程不许被误杀。** 阈值 1 秒，每 200 毫秒吐一行，跑 3 秒。
    #[test]
    #[cfg(unix)]
    fn 一直有进展的进程不会被误判为卡住() {
        let mut n = 0usize;
        let r = run_streaming(
            "/usr/bin/env",
            &[
                "sh",
                "-c",
                "i=0; while [ $i -lt 15 ]; do echo tick $i; sleep 0.2; i=$((i+1)); done",
            ],
            &StreamOpts {
                silence: Some(Duration::from_secs(1)),
                ..Default::default()
            },
            |ev| match ev {
                Ev::Line { .. } => {
                    n += 1;
                    Fresh::Yes
                }
                Ev::Poll => Fresh::No,
            },
        )
        .expect("起得来");
        assert_eq!(r.ended, Ended::Exited(Some(0)), "跑完的进程不该被杀");
        assert_eq!(n, 15);
    }

    /// **「一直在吐字节、但吐的全是同一句」照样算卡住。**
    ///
    /// 这一条是 `compose pull` 的真实形状：拉不动的时候它会把同一条进度行
    /// 一遍遍重复出来。按「管道上有没有字节」判永远判不出来 ——
    /// 判据必须是调用方给的 [`Fresh`]。
    #[test]
    #[cfg(unix)]
    fn 重复同一行也算卡住() {
        let mut seen: Option<String> = None;
        let r = run_streaming(
            "/usr/bin/env",
            &[
                "sh",
                "-c",
                "while true; do echo '{\"id\":\"layer\",\"current\":100}'; sleep 0.1; done",
            ],
            &StreamOpts {
                silence: Some(Duration::from_secs(1)),
                ..Default::default()
            },
            // 只有「和上一行不一样」才算进展
            |ev| match ev {
                Ev::Line { text, .. } => {
                    let same = seen.as_deref() == Some(text);
                    seen = Some(text.to_string());
                    if same {
                        Fresh::No
                    } else {
                        Fresh::Yes
                    }
                }
                Ev::Poll => Fresh::No,
            },
        )
        .expect("起得来");
        assert!(
            matches!(r.ended, Ended::Stalled { .. }),
            "应当判成卡住，实际是 {:?}",
            r.ended
        );
    }

    /// 取消置位时立刻杀掉，结论是 `Cancelled` 而不是 `Stalled`。
    #[test]
    #[cfg(unix)]
    fn 取消会立刻杀掉子进程() {
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = Arc::clone(&cancel);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            c2.store(true, Ordering::SeqCst);
        });
        let t0 = Instant::now();
        let r = run_streaming(
            "/usr/bin/env",
            &["sh", "-c", "sleep 600"],
            &StreamOpts {
                silence: Some(Duration::from_secs(600)),
                cancel: Some(&cancel),
                ..Default::default()
            },
            |_| Fresh::No,
        )
        .expect("起得来");
        assert_eq!(r.ended, Ended::Cancelled);
        assert!(t0.elapsed() < Duration::from_secs(10));
    }

    /// 静默超时不影响正常退出码的传递，两个流的原文也都收得到。
    #[test]
    #[cfg(unix)]
    fn 正常退出时两个流的原文都收得到() {
        let r = run_streaming(
            "/usr/bin/env",
            &["sh", "-c", "echo 到标准输出; echo 到标准错误 >&2; exit 7"],
            &StreamOpts::default(),
            |_| Fresh::Yes,
        )
        .expect("起得来");
        assert_eq!(r.ended, Ended::Exited(Some(7)));
        assert_eq!(r.stdout, vec!["到标准输出".to_string()]);
        assert_eq!(r.stderr, vec!["到标准错误".to_string()]);
    }

    /// 输出远超管道缓冲区（64 KB）时，流式这一路同样不许挂住（I7 的那条病）。
    #[test]
    #[cfg(unix)]
    fn 流式读取超过管道缓冲区也不会挂住() {
        let mut n = 0usize;
        let r = run_streaming(
            "/usr/bin/env",
            &[
                "sh",
                "-c",
                "i=0; while [ $i -lt 65536 ]; do echo 0123456789abcde; i=$((i+1)); done",
            ],
            &StreamOpts {
                silence: Some(Duration::from_secs(20)),
                ..Default::default()
            },
            |ev| {
                if matches!(ev, Ev::Line { .. }) {
                    n += 1;
                }
                Fresh::Yes
            },
        )
        .expect("起得来");
        assert_eq!(r.ended, Ended::Exited(Some(0)));
        assert_eq!(n, 65536);
    }

    /// stderr 那一路同样要一边跑一边读。
    #[test]
    fn stderr_超过缓冲区也不会挂住() {
        let r = run_timeout(
            "/usr/bin/env",
            &[
                "sh",
                "-c",
                "i=0; while [ $i -lt 65536 ]; do echo 0123456789abcde >&2; i=$((i+1)); done",
            ],
            Duration::from_secs(20),
        );
        let Ok(r) = r else {
            if cfg!(windows) {
                return;
            }
            panic!("起不来");
        };
        assert_eq!(r.status, Some(0));
        assert!(r.stderr.len() > 1_000_000, "只读到 {} 字节", r.stderr.len());
    }
    // ── 子进程输出的解码（I16 · P0-3） ────────────────────────────────────

    /// 客户 2026-09-26 那台中文 Windows 上真实发生过的一行。
    ///
    /// `schtasks` 用 GBK 吐了 `错误: 未指定的错误`，0.1.15 把它当 UTF-8 解，
    /// 于是日志里只剩 `����: δָ���Ĵ���` —— 真正的原因我们一次都没看见过。
    /// 这条单测**两个方向都钉住**：按 936 解得回中文，按 UTF-8 lossy 解就是那串东西。
    #[test]
    fn gbk_的_schtasks_报错要解成中文() {
        // “错误: 未指定的错误” 的 GBK 字节，逐字节写死，不靠运行环境的 locale
        let gbk: &[u8] = &[
            0xb4, 0xed, 0xce, 0xf3, 0x3a, 0x20, 0xce, 0xb4, 0xd6, 0xb8, 0xb6, 0xa8, 0xb5, 0xc4,
            0xb4, 0xed, 0xce, 0xf3,
        ];
        assert_eq!(
            decode_console(gbk, Some(936)),
            "错误: 未指定的错误",
            "按 936（GBK）要解得回中文"
        );
        // 对照：0.1.15 的做法。这串就是客户诊断包里那一行的原文
        assert_eq!(
            String::from_utf8_lossy(gbk),
            "����: δָ���Ĵ���",
            "这是 0.1.15 的结果 —— 诊断包里一字不差就是它"
        );
        // 解出来的东西必须是**能看的**：一个替换字符都不许剩
        assert!(!decode_console(gbk, Some(936)).contains('\u{FFFD}'));
    }

    /// UTF-8 的输出一个字节都不许动（docker / git / 我们自己的程序都吐 UTF-8）。
    #[test]
    fn utf8_的输出不走回退() {
        let s = "六个镜像合计约 748 MB · ok";
        assert_eq!(decode_console(s.as_bytes(), Some(936)), s);
        assert_eq!(decode_console(s.as_bytes(), None), s);
    }

    /// 代码页不认识（例如 OEM 437，`encoding_rs` 没有它）就退回 lossy，**不瞎猜**。
    #[test]
    fn 认不出来的代码页退回_lossy() {
        let gbk: &[u8] = &[0xb4, 0xed];
        assert_eq!(decode_console(gbk, Some(437)), "��");
        assert_eq!(decode_console(gbk, None), "��");
    }

    /// 非 Windows 上一律不做代码页回退。
    #[test]
    fn 非_windows_不猜代码页() {
        if cfg!(windows) {
            return;
        }
        assert_eq!(ansi_codepage(), None);
    }

    /// **一行不是合法 UTF-8，不许把整条流读断**（I16 · P0-3）。
    ///
    /// 原先 `reader.lines().map_while(Result::ok)` 见到 `Err` 就停：
    /// 中文 Windows 上子进程随便吐一句 GBK，后面所有输出（含拉取进度）全没了，
    /// 而且没有任何迹象。
    #[test]
    fn 中间夹一行非_utf8_后面的行照样收得到() {
        if cfg!(windows) {
            return;
        }
        let mut got: Vec<String> = Vec::new();
        let r = run_streaming(
            "/usr/bin/env",
            &[
                "sh",
                "-c",
                // 第二行是 GBK 的“错误”两个字，第三行必须照样收到
                // 八进制转义：dash 的 printf 不认 \\xHH，认 \\nnn
                "echo one; printf '\\264\\355\\316\\363\\n'; echo three",
            ],
            &StreamOpts {
                silence: Some(Duration::from_secs(30)),
                ..Default::default()
            },
            |ev| {
                if let Ev::Line { text, .. } = ev {
                    got.push(text.to_string());
                }
                Fresh::Yes
            },
        )
        .expect("起不来");
        assert!(r.ended.ok(), "{:?}", r.ended);
        assert_eq!(
            got,
            vec!["one".to_string(), "����".to_string(), "three".to_string()],
            "第三行丢了就说明又被 map_while 截断了"
        );
    }
}
