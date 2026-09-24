//! `docker compose` 的封装（技术方案 §5.2、§9）。
//!
//! 所有命令都带 `--project-name hunter`（总控规则），参数一律数组（红线 3）。
//!
//! 拉取进度按 M0 §5.1 的实测结论做：
//! `docker compose --progress json pull` 逐行吐 JSON，按 `parent_id`（形如 `"Image <完整引用>"`）
//! 分组到镜像，层级的 `current`/`total` 求和得每镜像进度。
//! **分母不用观测到的 total 之和**，而是拉取前从 registry manifest 算好的压缩大小 ——
//! 那样进度条从第一秒起就是对的，不用等所有层都报上来。
//!
//! 两个已知的坑（M0 记过）：
//! * `Extracting` 行**只有 `current` 没有 `total`**，算不出解压百分比 → 这一段显示「解压中」；
//! * `docker pull`（不带 compose）在非 TTY 下完全没有逐层进度，所以不能走那条路。

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::{ImageSpec, PROJECT};
use crate::err::{AppError, AppResult, Code};
use crate::runtime::which;
use crate::{paths, proc};

/// 健康轮询的上限（方案 §18 的 `E_START_TIMEOUT` 就是这个数）。
pub const START_TIMEOUT: Duration = Duration::from_secs(180);
/// 拉取失败重试次数（方案 §9）。
pub const PULL_ATTEMPTS: usize = 3;

/// compose 的完整参数前缀。`-f` 的顺序有意义：覆盖文件必须排在后面，
/// 否则 `!override` 覆盖不到（红线 4 就白写了）。
/// `--project-directory` 也是全局参数，指到 `~/.hunter/app` 让 compose 在那里找 `.env`。
///
/// 开头那一截由 [`which::ComposeInfo::argv_prefix`] 给：
/// `docker compose` 插件形态是 `["compose"]`，独立 `docker-compose` 形态是空的（I4）。
fn full_args(compose: &str, overlay: &str, dir: &str, extra: &[&str]) -> (String, Vec<String>) {
    let (program, mut v) = which::compose_info().argv_prefix();
    v.extend(
        [
            "--project-name",
            PROJECT,
            "--project-directory",
            dir,
            "-f",
            compose,
            "-f",
            overlay,
        ]
        .into_iter()
        .map(str::to_string),
    );
    v.extend(extra.iter().map(|s| s.to_string()));
    (program, v)
}

fn file_paths() -> (String, String) {
    (
        paths::compose_file().to_string_lossy().into_owned(),
        paths::override_file().to_string_lossy().into_owned(),
    )
}

/// 跑一条 compose 命令（不需要流式输出的那些）。
pub fn run(extra: &[&str], timeout: Duration) -> AppResult<proc::Ran> {
    let (c, o) = file_paths();
    let dir = paths::app_dir();
    let dir_s = dir.to_string_lossy().into_owned();
    let (program, args) = full_args(&c, &o, &dir_s, extra);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    proc::run_timeout(&program, &refs, timeout)
}

/// 同一条命令的**程序名 + 完整参数（带所有权）**。
///
/// 给需要自己接管子进程 stdin/stdout 的调用方用 —— 目前是 [`crate::backup`]：
/// `pg_dump` 的输出要直接落盘，不能先在内存里攒成一个 String
/// （用户的库可能有几百 MB，攒在内存里就是等着 OOM）。
pub fn argv(extra: &[&str]) -> (String, Vec<String>) {
    let (c, o) = file_paths();
    let dir = paths::app_dir();
    let dir_s = dir.to_string_lossy().into_owned();
    full_args(&c, &o, &dir_s, extra)
}

// ── 进度解析 ──────────────────────────────────────────────────────────────

/// `--progress json` 的一行。字段全集见 M0 §5.1，这里只取用得上的。
#[derive(Debug, Deserialize)]
pub struct ProgressLine {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub status: Option<String>,
    pub text: Option<String>,
    pub current: Option<u64>,
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PullState {
    Pending,
    Downloading,
    Extracting,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagePull {
    pub service: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub short_ref: String,
    pub tag: String,
    /// 压缩后的总字节。拉取前从 manifest 算好；算不到时为 0，界面显示「—」
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub state: PullState,
    /// manifest 没读到时为 true，界面要标明这一条的分母是估的
    pub size_unknown: bool,
    /// 这个镜像从第一个字节到 `Pulled` 花了多少秒；还没完成时为 None。
    /// 成果文档要记「各镜像耗时」，靠的就是它。
    pub seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PullPhase {
    /// 选源、取 compose、写配置
    Preparing,
    Pulling,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullProgress {
    pub phase: PullPhase,
    /// 镜像源的前缀，界面上显示这个
    pub registry: String,
    pub registry_label: String,
    pub images: Vec<ImagePull>,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    /// 这一次**真的走了网络**的字节数（I5）。
    ///
    /// `downloaded_bytes` 把「本机已有的层」也算进去了 —— 它是进度条的分子，
    /// 用户关心的是整体完成度。但拿它去说「下载了 849 MB，用时 1 秒」就是在骗人：
    /// 那 849 MB 一个字节都没过网。这一项只累计 `Already exists` 之外的层。
    #[serde(default)]
    pub net_bytes: u64,
    pub percent: u32,
    /// 字节/秒，最近若干秒的滑动平均；还没有样本时为 null
    pub speed_bps: Option<u64>,
    pub eta_seconds: Option<u64>,
    /// 第几次尝试（1 起），换源重试时会增加
    pub attempt: u32,
    pub log: Vec<String>,
    /// 出错时的原因（已脱敏）
    pub error: Option<String>,
    /// 出错时的**真实错误码**（`E_PULL_FAILED` / `E_PROJECT_CONFLICT` / `E_COMPOSE_FETCH` …）。
    ///
    /// I4 之前这个字段不存在，前端把安装阶段的**任何**失败都当成 `PULL_FAILED` 发给状态机，
    /// 于是「另一个工作目录占着 compose 项目名」会显示成「镜像拉取失败」——
    /// 标题说拉取失败，正文却在讲项目名冲突，而 I4 的诊断助手据此给出「换个镜像源」这种
    /// 完全跑偏的建议（本轮取错误页截图时当场撞出来的）。
    pub error_code: Option<String>,
}

impl PullProgress {
    pub fn empty() -> Self {
        Self {
            phase: PullPhase::Preparing,
            registry: String::new(),
            registry_label: String::new(),
            images: Vec::new(),
            total_bytes: 0,
            downloaded_bytes: 0,
            net_bytes: 0,
            percent: 0,
            speed_bps: None,
            eta_seconds: None,
            attempt: 1,
            log: Vec::new(),
            error: None,
            error_code: None,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct Layer {
    current: u64,
    total: u64,
    complete: bool,
    /// 这一层是不是真的走了网络。
    /// 本机内容存储里已经有的层会直接报 `Already exists`，一瞬间就「完成」了 ——
    /// 把它们算进速度，进度条右边就会显示 67 MB/s 这种一看就假的数字。
    /// 进度百分比要算上它们（用户关心的是整体完成度），**速度和剩余时间不算**。
    downloaded: bool,
}

/// 把一条条进度行累积成可以直接上屏的 [`PullProgress`]。
pub struct PullAggregator {
    /// service → 镜像
    images: Vec<ImagePull>,
    /// 完整引用 → images 下标
    by_ref: BTreeMap<String, usize>,
    /// (镜像下标, 层 id) → 层状态
    layers: BTreeMap<(usize, String), Layer>,
    log: Vec<String>,
    started: Instant,
    /// (时刻, 已下载字节) 的采样，算速度用
    samples: Vec<(Instant, u64)>,
    /// 每个镜像第一次出现进度的时刻，用来算单镜像耗时
    first_seen: BTreeMap<usize, Instant>,
    registry: String,
    registry_label: String,
    attempt: u32,
}

impl PullAggregator {
    pub fn new(
        specs: &[ImageSpec],
        sizes: &BTreeMap<String, u64>,
        registry: &str,
        registry_label: &str,
    ) -> Self {
        let mut images = Vec::new();
        let mut by_ref = BTreeMap::new();
        for (i, s) in specs.iter().enumerate() {
            let total = sizes.get(&s.service).copied().unwrap_or(0);
            images.push(ImagePull {
                service: s.service.clone(),
                reference: s.reference.clone(),
                short_ref: s.short_ref.clone(),
                tag: s.tag.clone(),
                total_bytes: total,
                downloaded_bytes: 0,
                state: PullState::Pending,
                size_unknown: total == 0,
                seconds: None,
            });
            by_ref.insert(s.reference.clone(), i);
        }
        Self {
            images,
            by_ref,
            layers: BTreeMap::new(),
            log: Vec::new(),
            started: Instant::now(),
            samples: Vec::new(),
            first_seen: BTreeMap::new(),
            registry: registry.to_string(),
            registry_label: registry_label.to_string(),
            attempt: 1,
        }
    }

    pub fn set_attempt(&mut self, n: u32) {
        self.attempt = n;
    }

    /// 吃一行原始输出。**解析不了的行直接丢弃**，不报错 ——
    /// 以后 compose 加了新的行型不该把启动器搞崩（M0 §5.1 定的）。
    pub fn feed(&mut self, raw: &str) {
        let line = raw.trim();
        if line.is_empty() {
            return;
        }
        let Ok(p) = serde_json::from_str::<ProgressLine>(line) else {
            // 不是 JSON 的行（compose 偶尔会掺一句人话）留给日志框
            self.push_log(line);
            return;
        };
        let text = p.text.as_deref().unwrap_or("");

        // 镜像级的行：id 形如 "Image ghcr.io/…:1.2.0"，没有 parent_id
        if p.parent_id.is_none() {
            if let Some(idx) = p.id.as_deref().and_then(|i| self.index_of(i)) {
                match text {
                    "Pulling" => {
                        if self.images[idx].state == PullState::Pending {
                            self.images[idx].state = PullState::Downloading;
                        }
                    }
                    "Pulled" => {
                        self.images[idx].state = PullState::Done;
                        // 完成时把已下载补齐到总量，免得进度条停在 99%
                        let t = self.images[idx].total_bytes;
                        if t > 0 {
                            self.images[idx].downloaded_bytes = t;
                        }
                        if self.images[idx].seconds.is_none() {
                            let since = self.first_seen.get(&idx).copied().unwrap_or(self.started);
                            self.images[idx].seconds = Some(since.elapsed().as_secs());
                        }
                    }
                    "Error" => self.images[idx].state = PullState::Failed,
                    _ => {}
                }
                self.push_log(&format!(
                    "{} {}",
                    short_of(&self.images[idx].short_ref),
                    text
                ));
            }
            return;
        }

        // 层级的行
        let Some(idx) = p.parent_id.as_deref().and_then(|i| self.index_of(i)) else {
            return;
        };
        let Some(layer_id) = p.id.clone() else { return };
        self.first_seen.entry(idx).or_insert_with(Instant::now);
        let key = (idx, layer_id.clone());
        let entry = self.layers.entry(key).or_default();

        match text {
            "Downloading" => {
                entry.downloaded = true;
                if let Some(c) = p.current {
                    entry.current = c;
                }
                if let Some(t) = p.total {
                    entry.total = t;
                }
                if self.images[idx].state != PullState::Done {
                    self.images[idx].state = PullState::Downloading;
                }
            }
            // M0 §5.1：Extracting 行只有 current，没有 total，算不出百分比
            "Extracting" => {
                entry.complete = true;
                if self.images[idx].state != PullState::Done {
                    self.images[idx].state = PullState::Extracting;
                }
            }
            "Download complete" | "Pull complete" | "Already exists" => {
                entry.complete = true;
                if entry.total > 0 {
                    entry.current = entry.total;
                }
            }
            _ => {}
        }
        self.recompute(idx);
        if matches!(text, "Pull complete" | "Already exists") {
            self.push_log(&format!(
                "{} layer {} {}",
                short_of(&self.images[idx].short_ref),
                &layer_id[..layer_id.len().min(12)],
                text
            ));
        }
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        // id 形如 "Image ghcr.io/agentpit-io/hunter-community-web:1.2.0"
        let r = id.strip_prefix("Image ").unwrap_or(id);
        if let Some(i) = self.by_ref.get(r) {
            return Some(*i);
        }
        // compose 有时会省掉 docker.io/library/ 前缀（postgres:16-alpine）
        self.by_ref
            .iter()
            .find(|(k, _)| k.ends_with(r) || r.ends_with(k.as_str()))
            .map(|(_, v)| *v)
    }

    fn recompute(&mut self, idx: usize) {
        let sum: u64 = self
            .layers
            .iter()
            .filter(|((i, _), _)| *i == idx)
            .map(|(_, l)| l.current)
            .sum();
        let img = &mut self.images[idx];
        if img.state == PullState::Done {
            return;
        }
        // 分母是 manifest 算好的；万一 manifest 没读到，退而用观测到的层 total 之和
        if img.total_bytes == 0 {
            let observed: u64 = self
                .layers
                .iter()
                .filter(|((i, _), _)| *i == idx)
                .map(|(_, l)| l.total)
                .sum();
            img.total_bytes = observed;
        }
        img.downloaded_bytes = if img.total_bytes > 0 {
            sum.min(img.total_bytes)
        } else {
            sum
        };
    }

    fn push_log(&mut self, s: &str) {
        let line = format!(
            "{} {}",
            crate::timefmt::hms_shanghai(crate::timefmt::now_unix()),
            crate::redact::redact(s)
        );
        self.log.push(line);
        if self.log.len() > 200 {
            let n = self.log.len() - 200;
            self.log.drain(..n);
        }
    }

    pub fn snapshot(&mut self, phase: PullPhase, error: Option<String>) -> PullProgress {
        let total: u64 = self.images.iter().map(|i| i.total_bytes).sum();
        let done: u64 = self.images.iter().map(|i| i.downloaded_bytes).sum();
        // 只有真的走过网络的层才计入速度（见 Layer::downloaded 的注释）
        let transferred: u64 = self
            .layers
            .values()
            .filter(|l| l.downloaded)
            .map(|l| l.current)
            .sum();

        let now = Instant::now();
        self.samples.push((now, transferred));
        // 只留最近 20 秒的采样
        self.samples
            .retain(|(t, _)| now.duration_since(*t) <= Duration::from_secs(20));
        let speed = match (self.samples.first(), self.samples.last()) {
            (Some((t0, b0)), Some((t1, b1))) if t1 > t0 && b1 > b0 => {
                let secs = t1.duration_since(*t0).as_secs_f64();
                if secs >= 1.0 {
                    Some(((b1 - b0) as f64 / secs) as u64)
                } else {
                    None
                }
            }
            _ => None,
        };
        // 剩余时间同样按「还要过网络的字节」算：已经在本机的层不用等
        let remaining_net: u64 = self
            .layers
            .values()
            .filter(|l| l.downloaded && l.total > l.current)
            .map(|l| l.total - l.current)
            .sum();
        let eta = match speed {
            Some(s) if s > 0 && remaining_net > 0 => Some(remaining_net / s),
            _ => None,
        };

        PullProgress {
            phase,
            registry: self.registry.clone(),
            registry_label: self.registry_label.clone(),
            images: self.images.clone(),
            total_bytes: total,
            downloaded_bytes: done,
            net_bytes: transferred,
            percent: if total > 0 {
                ((done as f64 / total as f64) * 100.0).round().min(100.0) as u32
            } else {
                0
            },
            speed_bps: speed,
            eta_seconds: eta,
            attempt: self.attempt,
            log: self.log.clone(),
            error,
            // 聚合器本身不知道错误码（它只管进度）。真正的码由 commands.rs
            // 那一层从 `AppError` 上取出来填进去。
            error_code: None,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// 每个镜像各花了多久（成果文档要记这个）——按「首次出现」到「Pulled」的时间差没有记，
    /// 这里给的是最终每镜像下载的字节数，耗时由调用方按整体计时。
    pub fn per_image_bytes(&self) -> Vec<(String, u64, u64)> {
        self.images
            .iter()
            .map(|i| (i.service.clone(), i.downloaded_bytes, i.total_bytes))
            .collect()
    }
}

fn short_of(s: &str) -> &str {
    s.strip_prefix("hunter-community-").unwrap_or(s)
}

// ── 拉取 ──────────────────────────────────────────────────────────────────

/// 流式跑 `compose pull`，每收到一批进度就调一次 `on_progress`。
/// `cancel` 置位时杀掉子进程并返回 `Err`。
///
/// **进度流在 stderr 上**。M0 §5.1 记的是 stdout，M2 实测下来是 stderr ——
/// 按 stdout 读的话拉取过程中一行都收不到，所有进度会在进程结束后一次性涌出来
/// （表现为「进度条一直 0%，最后瞬间 100%」）。这里两个流都读，谁有内容都吃得下。
pub fn pull_streaming(
    agg: &mut PullAggregator,
    cancel: &Arc<AtomicBool>,
    mut on_progress: impl FnMut(&PullProgress),
) -> AppResult<()> {
    let (c, o) = file_paths();
    let dir = paths::app_dir();
    let dir_s = dir.to_string_lossy().into_owned();
    // --progress 是 `docker compose` 的**全局**参数，必须写在 pull 前面（M0 §5.1）
    let (program, args) = full_args(&c, &o, &dir_s, &["--progress", "json", "pull"]);

    let mut child = proc::base_command(&program)
        .args(&args)
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            AppError::new(
                Code::PullFailed,
                format!("起 docker compose pull 失败：{e}"),
            )
        })?;

    let (tx, rx) = mpsc::channel::<String>();
    let mut readers = Vec::new();
    for (stream, is_err) in [
        (child.stdout.take().map(Either::Out), false),
        (child.stderr.take().map(Either::Err), true),
    ] {
        let Some(stream) = stream else { continue };
        let tx = tx.clone();
        // **两个流都收**。原来只收 stderr —— 而「原话」是给用户看的唯一一句实话，
        // 少收一个流就可能让它是空的（I6：0.1.5 那次「原话：」后面什么都没有）
        let collected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&collected);
        let h = std::thread::spawn(move || {
            let reader: Box<dyn BufRead> = match stream {
                Either::Out(s) => Box::new(BufReader::new(s)),
                Either::Err(s) => Box::new(BufReader::new(s)),
            };
            for line in reader.lines().map_while(Result::ok) {
                if let Ok(mut g) = sink.lock() {
                    g.push(line.clone());
                    if g.len() > 400 {
                        let n = g.len() - 400;
                        g.drain(..n);
                    }
                }
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        readers.push((h, collected, is_err));
    }
    drop(tx); // 两个读线程都结束后，rx 才会断开

    let mut last_emit = Instant::now() - Duration::from_secs(1);
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(line) => agg.feed(&line),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::new(
                Code::PullFailed,
                "用户取消了拉取".to_string(),
            ));
        }
        // 每 300ms 推一次，别把前端淹了
        if last_emit.elapsed() >= Duration::from_millis(300) {
            last_emit = Instant::now();
            on_progress(&agg.snapshot(PullPhase::Pulling, None));
        }
    }

    let status = child
        .wait()
        .map_err(|e| AppError::new(Code::PullFailed, format!("等 compose pull 结束失败：{e}")))?;
    // stderr 优先，它空了才看 stdout
    let mut from_err = String::new();
    let mut from_out = String::new();
    for (h, collected, is_err) in readers {
        let _ = h.join();
        if let Ok(g) = collected.lock() {
            let t = extract_error_text(&g);
            if is_err {
                from_err = t;
            } else {
                from_out = t;
            }
        }
    }
    let err_text = if from_err.trim().is_empty() {
        from_out
    } else {
        from_err
    };
    on_progress(&agg.snapshot(PullPhase::Pulling, None));

    if status.success() {
        Ok(())
    } else {
        if err_text.trim().is_empty() {
            // 两个流都没说话。**如实说「一个字都没说」**，别留一句半截话给用户
            crate::lwarn!(
                "docker compose pull 退出码 {:?}，但 stdout / stderr 都没有可识别的错误行",
                status.code()
            );
        }
        Err(AppError::new(
            pull_error_code(&err_text),
            classify_pull_error(&err_text, status.code()),
        ))
    }
}

/// 两个管道类型不同，又想用同一段读取代码，包一层。
enum Either {
    Out(std::process::ChildStdout),
    Err(std::process::ChildStderr),
}

/// 从 stderr 的全部行里挑出「能说明失败原因」的部分。
///
/// compose 在 `--progress json` 下把**报错也塞进 JSON 行**，纯文本行常常一条都没有。
/// 只挑非 JSON 行的话，用户看到的就是「退出码 1。原话：」后面空一片（M2 用例 6c 实测）。
///
/// ## 三种 JSON 形状都要认（I6）
///
/// 0.1.5 在用户 Mac 上失败时，「原话：」后面**真的是空的** —— 因为 compose v5 把
/// 顶层错误写成的是第三种形状，而这里当时只认头两种。测试机上 `docker compose
/// --progress json pull` 的实测原文（`DOCKER_CONFIG` 里写了个找不到的 credsStore）：
///
/// ```text
/// {"id":"Image ghcr.io/…:1.2.0","status":"Working","text":"Pulling"}
/// {"error":true,"message":"error getting credentials - err: exec: \"docker-credential-fakehelper\": executable file not found in $PATH, out: ``"}
/// ```
///
/// | 形状 | 取哪里 | 谁会发 |
/// |---|---|---|
/// | `{"text":"Error","details":"…"}` / `{"status":"Error",…}` | `details`，退而取 `status` | compose 的逐镜像进度事件 |
/// | `{"errorDetail":{"message":"…"}}` / `{"error":"…"}` | 里面的 message | `docker pull` 的原生流 |
/// | `{"error":true,"message":"…"}` | `message` | **compose v5 的顶层错误** ← 0.1.5 漏的就是它 |
///
/// 纯文本行与 JSON 里挑出来的**都要**（纯文本排在前面）—— 只要谁说了一句实话，
/// 就不能让「原话」是空的。
pub fn extract_error_text(lines: &[String]) -> String {
    let mut plain: Vec<String> = Vec::new();
    let mut from_json: Vec<String> = Vec::new();
    for l in lines {
        let t = l.trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
                if let Some(line) = json_error_line(&v) {
                    if !from_json.contains(&line) {
                        from_json.push(line);
                    }
                }
            }
            continue;
        }
        plain.push(t.to_string());
    }
    let mut out = plain;
    for j in from_json {
        if !out.iter().any(|p| p.contains(&j) || j.contains(p)) {
            out.push(j);
        }
    }
    out.join("\n")
}

/// 一条 JSON 进度行里的错误原话。不是错误行就返回 `None`。
fn json_error_line(v: &serde_json::Value) -> Option<String> {
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
    // ③ compose v5 的顶层错误：`{"error":true,"message":"…"}`
    match v.get("error") {
        Some(serde_json::Value::Bool(true)) => {
            let m = v.get("message").and_then(|x| x.as_str()).unwrap_or("");
            let line = format!("{id} {m}").trim().to_string();
            if !line.is_empty() {
                return Some(line);
            }
        }
        // ② docker 原生流：`{"error":"…"}`
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => {
            return Some(format!("{id} {s}").trim().to_string());
        }
        _ => {}
    }
    // ② docker 原生流：`{"errorDetail":{"message":"…"}}`
    if let Some(m) = v
        .get("errorDetail")
        .and_then(|x| x.get("message"))
        .and_then(|x| x.as_str())
    {
        if !m.trim().is_empty() {
            return Some(format!("{id} {m}").trim().to_string());
        }
    }
    // ① compose 的逐镜像进度事件
    let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("");
    let status = v.get("status").and_then(|x| x.as_str()).unwrap_or("");
    if text.eq_ignore_ascii_case("error") || status.eq_ignore_ascii_case("error") {
        let detail = v
            .get("details")
            .and_then(|x| x.as_str())
            .or_else(|| v.get("message").and_then(|x| x.as_str()))
            .or_else(|| v.get("status").and_then(|x| x.as_str()))
            .unwrap_or("");
        let line = format!("{id} {detail}").trim().to_string();
        if !line.is_empty() {
            return Some(line);
        }
    }
    None
}

/// 本机凭据助手缺失的判据（I6）。
///
/// 三条任一命中就算 —— 用户 Mac 上的原话同时命中了前两条：
/// `error getting credentials - err: exec: "docker-credential-osxkeychain":
/// executable file not found in $PATH, out: ``。
///
/// **这是本机配置的问题，和下载源没有半点关系**，换源永远修不好它
/// （0.1.5 在用户机器上换了三次源，白花 268 秒）。
pub fn is_cred_helper_error(text: &str) -> bool {
    let s = text.to_lowercase();
    s.contains("error getting credentials")
        || s.contains("docker-credential-")
        || (s.contains("executable file not found") && !s.contains("docker-compose"))
}

/// 拉取失败该归到哪个错误码。
pub fn pull_error_code(text: &str) -> Code {
    if is_cred_helper_error(text) {
        Code::CredHelper
    } else {
        Code::PullFailed
    }
}

/// 把 compose 的报错翻成一句能照着办的中文。
pub fn classify_pull_error(stderr: &str, code: Option<i32>) -> String {
    let s = stderr.to_lowercase();
    let tail = stderr
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();
    // 凭据助手这一条要排在网络那几条前面 —— 它的原话里常常也有别的关键词
    if is_cred_helper_error(stderr) {
        return format!(
            "Docker 要用本机的凭据助手去取登录信息，但在这个程序看得到的 PATH 上找不到它。\
             这和下载源无关，换源修不好。原话：{tail}"
        );
    }
    if tail.is_empty() {
        // 半截话比没有话更糟：「原话：」后面空一片，用户与 AI 都无从下手
        return format!(
            "docker compose pull 退出码 {}，而且 stdout / stderr 里一个字的错误都没有。",
            code.map(|c| c.to_string()).unwrap_or_else(|| "未知".into())
        );
    }
    if s.contains("proxyconnect") || s.contains("proxy") && s.contains("refused") {
        return format!("代理把镜像源挡住了。检查 HTTP_PROXY / HTTPS_PROXY，或者在代理里放行镜像源。原话：{tail}");
    }
    if s.contains("no such host") || s.contains("dns") {
        return format!("DNS 解析不了镜像源的域名。原话：{tail}");
    }
    if s.contains("timeout")
        || s.contains("timed out")
        || s.contains("deadline exceeded")
        || s.contains("i/o timeout")
    {
        return format!("连镜像源超时。网络不通或者被墙。原话：{tail}");
    }
    if s.contains("pull access denied") {
        return format!(
            "镜像源说这个仓库不让拉（pull access denied）。多半是 tag 写错了或者仓库不是公开的。原话：{tail}"
        );
    }
    if s.contains("unauthorized") || s.contains("denied") {
        return format!("镜像源拒绝访问（仓库可能不是公开的）。原话：{tail}");
    }
    if s.contains("no space left") {
        return format!("磁盘满了。腾出空间后重试（六个镜像解压后大约要 3 GB）。原话：{tail}");
    }
    if s.contains("connection refused") || s.contains("connection reset") || s.contains("eof") {
        return format!("连接被中断。网络不稳定或者被中间设备切断。原话：{tail}");
    }
    format!(
        "docker compose pull 退出码 {}。原话：{tail}",
        code.map(|c| c.to_string()).unwrap_or_else(|| "未知".into())
    )
}

// ── ps / 健康 ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Healthy,
    Starting,
    Unhealthy,
    /// 容器在跑但没有健康检查
    None,
    /// 容器还没起来
    Pending,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub service: String,
    pub state: String,
    pub health: Health,
    /// 宿主端口。llm-shim 不发布端口时为 null（M0 §5.3 坑 3：它的 PublishedPort 是 0）
    pub port: Option<u16>,
    /// 这个端口**实际**绑在哪个地址上（`docker compose ps` 的 `Publishers[].URL`）。
    ///
    /// I7 加的，用来回答「这台机器现在是不是对局域网开着」——
    /// 答案只能来自 docker 本身，不能来自我们自己写的配置（红线 1：配置是意图，这是现状）。
    /// 空串 / 拿不到就是 `None`，界面显示「还没读到」。
    pub bind: Option<String>,
    pub exit_code: Option<i64>,
}

impl ServiceStatus {
    /// 这个服务的端口是不是**不止本机**能连。
    /// 读不到绑定地址时返回 `false` —— 不知道就不喊狼来了（红线 1）。
    pub fn lan_exposed(&self) -> bool {
        match self.bind.as_deref() {
            Some(b) if !b.is_empty() => b != "127.0.0.1" && b != "::1" && b != "localhost",
            _ => false,
        }
    }
}

/// `docker compose ps --format json` 的一行。
/// **注意不是一个 JSON 数组**，是每个容器一行（M0 §5.3）。
#[derive(Debug, Deserialize)]
struct PsLine {
    #[serde(rename = "Service")]
    service: Option<String>,
    #[serde(rename = "State")]
    state: Option<String>,
    #[serde(rename = "Health")]
    health: Option<String>,
    #[serde(rename = "ExitCode")]
    exit_code: Option<i64>,
    #[serde(rename = "Publishers")]
    publishers: Option<Vec<Publisher>>,
}

#[derive(Debug, Deserialize)]
struct Publisher {
    #[serde(rename = "PublishedPort")]
    published_port: Option<u16>,
    /// 宿主侧的绑定地址。实测取值有 `127.0.0.1`、`0.0.0.0`、`::`，
    /// 老版本 compose 里也可能整个字段都没有
    #[serde(rename = "URL")]
    url: Option<String>,
}

/// 解析 `ps --format json` 的输出。
/// **空输入是合法的**（没有容器时 stdout 是空串、退出码仍是 0，M0 §5.3 坑 2），返回空列表。
pub fn parse_ps(stdout: &str) -> Vec<ServiceStatus> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // compose 某些版本会把整批包成数组，两种都吃
        if line.starts_with('[') {
            if let Ok(arr) = serde_json::from_str::<Vec<PsLine>>(line) {
                out.extend(arr.into_iter().map(to_status));
                continue;
            }
        }
        if let Ok(p) = serde_json::from_str::<PsLine>(line) {
            out.push(to_status(p));
        }
    }
    out.sort_by_key(|s| (display_order(&s.service), s.service.clone()));
    out
}

/// 服务在界面上的固定排序，照视觉稿第 3 张的服务网格来：
/// web / api / opencode 一行，llm-shim / postgres / redis 一行。
/// **不按字母序** —— 那样 api 会排到 web 前面，和稿子对不上。
fn display_order(service: &str) -> usize {
    const ORDER: [&str; 6] = ["web", "api", "opencode", "llm-shim", "postgres", "redis"];
    ORDER
        .iter()
        .position(|s| *s == service)
        .unwrap_or(ORDER.len())
}

fn to_status(p: PsLine) -> ServiceStatus {
    let state = p.state.unwrap_or_default();
    let health = match p
        .health
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "healthy" => Health::Healthy,
        "starting" => Health::Starting,
        "unhealthy" => Health::Unhealthy,
        _ if state == "running" => Health::None,
        _ => Health::Pending,
    };
    // M0 §5.3 坑 1：每个端口有 IPv4 / IPv6 两条，去重取第一个非 0 的
    let pubs = p.publishers.unwrap_or_default();
    let port = pubs
        .iter()
        .filter_map(|x| x.published_port)
        .find(|p| *p != 0);
    // 绑定地址取**同一个端口**那一条的 URL。
    // IPv4 / IPv6 两条的 URL 不一样（`0.0.0.0` 与 `::`），但对「是不是只有本机能连」
    // 这个问题来说，只要有一条不是本机地址就算对外 —— 所以这里挑最宽的那一条。
    let bind = port.and_then(|want| {
        let mut widest: Option<String> = None;
        for x in pubs.iter().filter(|x| x.published_port == Some(want)) {
            let u = x.url.clone().unwrap_or_default();
            if u.is_empty() {
                continue;
            }
            let lan = u != "127.0.0.1" && u != "::1" && u != "localhost";
            if lan {
                return Some(u);
            }
            widest = widest.or(Some(u));
        }
        widest
    });
    ServiceStatus {
        service: p.service.unwrap_or_default(),
        state,
        health,
        port,
        bind,
        exit_code: p.exit_code,
    }
}

pub fn ps() -> AppResult<Vec<ServiceStatus>> {
    let r = run(
        &["ps", "--format", "json", "--all"],
        Duration::from_secs(30),
    )?;
    if !r.ok() {
        return Err(AppError::new(
            Code::Unknown,
            format!("docker compose ps 失败：{}", r.err_line()),
        ));
    }
    Ok(parse_ps(&r.stdout))
}

/// 服务是不是就绪：健康检查通过，或者没有健康检查但容器在跑。
pub fn service_ready(s: &ServiceStatus) -> bool {
    matches!(s.health, Health::Healthy) || (s.health == Health::None && s.state == "running")
}

// ── 项目名归属（待办池 P0-5） ───────────────────────────────────────────────

/// 现在占着 `hunter` 这个 compose 项目名的那一套，是从哪个工作目录起的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectOwner {
    /// 随便取的一个容器名，报错时给用户一个能 `docker inspect` 的抓手
    pub container: String,
    /// `com.docker.compose.project.config_files` 标签（逗号分隔的绝对路径）
    pub config_files: String,
    /// `com.docker.compose.project.working_dir` 标签
    pub working_dir: String,
}

impl ProjectOwner {
    /// 从工作目录反推那一套的 `HUNTER_HOME`：`<home>/app` → `<home>`。
    /// 反推不出来（用户手工 compose 起的、目录结构不是我们这套）就退回原值。
    pub fn hunter_home(&self) -> String {
        match self.working_dir.strip_suffix("/app") {
            Some(h) if !h.is_empty() => h.to_string(),
            _ => match self.working_dir.strip_suffix("\\app") {
                Some(h) if !h.is_empty() => h.to_string(),
                _ => self.working_dir.clone(),
            },
        }
    }
}

/// `docker ps -a` 里 `hunter` 项目的标签行，一行一个容器。
/// 分隔符用 `|`：路径里不会有它，而制表符在 Windows 的 docker 输出里会被吃掉。
const OWNER_FORMAT: &str = concat!(
    "{{.Names}}|{{.Label \"com.docker.compose.project.config_files\"}}",
    "|{{.Label \"com.docker.compose.project.working_dir\"}}"
);

/// 比路径之前先归一化：两头的空白、结尾的 `/` 或 `\` 都不算差别。
/// `HUNTER_HOME=~/.hunter/` 与 `HUNTER_HOME=~/.hunter` 是同一个目录，
/// 但 docker 标签里存的是当时传进去的原样字符串，直接字符串比会误报成「别人的」。
fn norm_path(p: &str) -> &str {
    p.trim().trim_end_matches(['/', '\\'])
}

/// 解析上面那个格式的输出，挑出**第一个不是我们这套**的容器。
///
/// 判定「是我们的」有两条，满足任意一条就算：
/// 1. `config_files` 里有一项正好是我们要用的 `docker-compose.yml`；
/// 2. `working_dir` 正好是我们的 `~/.hunter/app`。
///
/// 两条都要，是因为用户可能只改过其中一半：比如 `--project-directory` 一样但
/// compose 文件是手工指的。只要沾上一条，`up -d` 就不会顶掉别人的配置。
pub fn foreign_owner(stdout: &str, our_compose: &str, our_app_dir: &str) -> Option<ProjectOwner> {
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.splitn(3, '|');
        let container = it.next().unwrap_or_default().to_string();
        let config_files = it.next().unwrap_or_default().to_string();
        let working_dir = it.next().unwrap_or_default().to_string();
        // 标签读不到（老版本 docker、或者容器不是 compose 起的）就不判 ——
        // 宁可漏报也不能误报，误报会把正常安装整个拦死
        if config_files.is_empty() && working_dir.is_empty() {
            continue;
        }
        let ours = config_files
            .split(',')
            .any(|f| norm_path(f) == norm_path(our_compose))
            || (!working_dir.is_empty() && norm_path(&working_dir) == norm_path(our_app_dir));
        if !ours {
            return Some(ProjectOwner {
                container,
                config_files,
                working_dir,
            });
        }
    }
    None
}

/// 安装 / 启动前的自查：**`hunter` 这个项目名有没有被另一个工作目录占着**（待办池 P0-5）。
///
/// 为什么必须拦：`docker compose -p hunter up -d` 是按项目名认容器的，
/// 换过 `HUNTER_HOME` 再装一次，compose 会把先装好的那一套**连配置带端口一起顶掉**
/// （M2 实测撞到过，数据卷没丢但得重装一次才能回来）。
///
/// docker 起不来 / 读不到标签一律放行 —— 这是一道保险，不是必经的关卡，
/// 不能因为它自己出问题就让正常安装走不下去。
pub fn guard_project_owner() -> AppResult<()> {
    let compose_path = paths::compose_file().to_string_lossy().into_owned();
    let app_dir = paths::app_dir().to_string_lossy().into_owned();
    let filter = format!("label=com.docker.compose.project={PROJECT}");
    let r = match proc::run_timeout(
        &which::docker_bin(),
        &["ps", "-a", "--filter", &filter, "--format", OWNER_FORMAT],
        Duration::from_secs(20),
    ) {
        Ok(r) if r.ok() => r,
        _ => return Ok(()),
    };
    let Some(owner) = foreign_owner(&r.stdout, &compose_path, &app_dir) else {
        return Ok(());
    };
    let other_home = owner.hunter_home();
    let here = paths::root().display().to_string();
    crate::lwarn!(
        "compose 项目名 {PROJECT} 正被 {} 占着（容器 {}）",
        owner.working_dir,
        owner.container
    );
    Err(AppError::new(
        Code::ProjectConflict,
        format!(
            "这台机器上已经有一套 Hunter 在用 compose 项目名「{PROJECT}」，它的工作目录是 {other_home}（容器 {}）。\
             现在这个工作目录是 {}。继续下去会把那一套的配置和端口一起顶掉，所以先停在这里。\n\
             \n\
             要紧的一点：**数据卷是跟着项目名 `{PROJECT}` 走的，不是跟着工作目录走的**。\
             数据库的口令在 {other_home}/app/.env 里，卷当初就是用它初始化的 —— \
             新建一个空工作目录会现生成一把新口令，api 连不上已有的库（实测报 \
             `password authentication failed for user \"hunter\"`）。\n\
             \n\
             想接着用那一套（推荐）：把工作目录改回去 —— 环境变量 HUNTER_HOME={other_home}，\
             或者干脆不设这个变量。\n\
             想把工作目录挪到现在这个位置：**整个目录搬过来**，别新建空的 —— \
             HUNTER_HOME={other_home} hunter-launcher --down，然后 mv {other_home} {}。\n\
             想从零开始（**会丢掉已有数据**）：先 HUNTER_HOME={other_home} hunter-launcher --down，\
             再 docker volume rm $(docker volume ls -q --filter label=com.docker.compose.project={PROJECT})。\n\
             \n\
             （`up -d` 与 `down` 都不会删数据卷，被顶掉的是配置与端口。）",
            owner.container,
            here,
            here
        ),
    ))
}

// ── up / down / logs ──────────────────────────────────────────────────────

/// Docker 在起容器时报的端口冲突，从原话里抠出来的一条。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindFailure {
    pub port: u16,
    /// `0.0.0.0` / `127.0.0.1` / `[::]`，读不到就是空
    pub addr: String,
}

/// 从 `docker compose up` 的输出里找出「端口已被占用」的那几条。
///
/// 真实原话（用户 Mac 上 0.1.4 的现场，以及测试机复现）：
/// ```text
/// Error response from daemon: Ports are not available: exposing port TCP 0.0.0.0:8100 -> …
/// Error response from daemon: driver failed programming external connectivity on endpoint
///   hunter-api-1 (…): Bind for 0.0.0.0:8100 failed: port is already allocated
/// listen tcp 0.0.0.0:6479: bind: address already in use
/// ```
pub fn parse_bind_failures(text: &str) -> Vec<BindFailure> {
    let mut out: Vec<BindFailure> = Vec::new();
    let low = text.to_lowercase();
    if !(low.contains("port is already allocated")
        || low.contains("address already in use")
        || low.contains("ports are not available"))
    {
        return out;
    }
    // 在整段文本里扫 `<地址>:<端口>` 形状。只认真的像地址的那些，不硬凑
    for line in text.lines() {
        let l = line.to_lowercase();
        if !(l.contains("already allocated")
            || l.contains("already in use")
            || l.contains("not available"))
        {
            continue;
        }
        for tok in line.split(|c: char| c.is_whitespace() || c == '(' || c == ')') {
            let tok = tok.trim_end_matches(&[',', ':', '.'][..]);
            let Some((addr, port)) = tok.rsplit_once(':') else {
                continue;
            };
            let Ok(port) = port.parse::<u16>() else {
                continue;
            };
            let addr_ok = addr == "0.0.0.0"
                || addr == "[::]"
                || addr == "::"
                || addr.starts_with("127.")
                || addr.chars().all(|c| c.is_ascii_digit() || c == '.');
            if !addr_ok || addr.is_empty() {
                continue;
            }
            if !out.iter().any(|b| b.port == port) {
                out.push(BindFailure {
                    port,
                    addr: addr.to_string(),
                });
            }
        }
    }
    out
}

/// 把 `docker compose up` 的失败归成一个错误码。
///
/// **0.1.4 把所有 up 失败都归成了 `E_START_TIMEOUT`**，包括那条写得清清楚楚的
/// `port is already allocated` —— 于是规则层的 ports-taken 没被匹配、AI 拿到的
/// 错误码是「启动超时」，给出的解释自然也是错的（用户 Mac 上那次失败的第 2、3 条根因）。
pub fn classify_up_error(text: &str, code: Option<i32>) -> AppError {
    let binds = parse_bind_failures(text);
    if !binds.is_empty() {
        // 端口 → 服务名：从我们自己写下去的端口表反查，查不到就不写服务名（不猜）
        let cfg = crate::config::LauncherConfig::load();
        let mut named: Vec<String> = Vec::new();
        for b in &binds {
            match cfg
                .hunter
                .ports
                .as_pairs()
                .iter()
                .find(|(_, p)| *p == b.port)
            {
                Some((svc, _)) => named.push(format!("{}（{svc}）", b.port)),
                None => named.push(b.port.to_string()),
            }
        }
        return AppError::new(
            Code::PortConflict,
            format!(
                "Docker 拒绝发布端口 {}：已经被这台机器上别的东西占着了。原话：{}",
                named.join("、"),
                last_line(text)
            ),
        );
    }
    let low = text.to_lowercase();
    // `up` 也会顺手拉镜像，所以凭据助手这一条在这里同样会出现（I6）
    if is_cred_helper_error(text) {
        return AppError::new(
            Code::CredHelper,
            format!(
                "起容器时要拉镜像，Docker 去找本机的凭据助手没找到。这和下载源无关。原话：{}",
                last_line(text)
            ),
        );
    }
    if low.contains("no space left") {
        return AppError::new(
            Code::PullFailed,
            format!(
                "磁盘满了，容器起不来。腾出空间后重试。原话：{}",
                last_line(text)
            ),
        );
    }
    if low.contains("pull access denied") {
        return AppError::new(
            Code::PullFailed,
            format!(
                "起容器时要拉的镜像不让拉（pull access denied），多半是 tag 写错了。原话：{}",
                last_line(text)
            ),
        );
    }
    if low.contains("tls handshake timeout") {
        return AppError::new(
            Code::PullFailed,
            format!("连镜像源时 TLS 握手超时。原话：{}", last_line(text)),
        );
    }
    if low.contains("i/o timeout") || low.contains("context deadline exceeded") {
        return AppError::new(
            Code::PullFailed,
            format!("连镜像源超时。原话：{}", last_line(text)),
        );
    }
    if low.contains("cannot connect to the docker daemon")
        || low.contains("is the docker daemon running")
    {
        return AppError::new(
            Code::DaemonDown,
            format!("连不上 Docker 守护进程。原话：{}", last_line(text)),
        );
    }
    AppError::new(
        Code::StartTimeout,
        format!(
            "docker compose up -d 失败（退出码 {}）：{}",
            code.map(|c| c.to_string()).unwrap_or_else(|| "未知".into()),
            last_line(text)
        ),
    )
}

fn last_line(text: &str) -> String {
    text.lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or("（没有输出）")
        .trim()
        .to_string()
}

// ── 预检闸门（I5 §六.2） ──────────────────────────────────────────────────

/// 起容器**之前**查出来的一条端口冲突。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightConflict {
    pub service: String,
    pub port: u16,
    /// 占用者，一行人话
    pub occupied_by: String,
}

/// `docker compose up` 之前，拿**合并后**的端口表再对一遍现场。
///
/// 为什么不能只信 `prepare` 那一步算好的端口：中间隔着几分钟的镜像拉取，
/// 用户完全可能在这期间起了别的东西；更要紧的是 0.1.4 那种探测本身漏判的情况 ——
/// 与其等 Docker 报错再去猜，不如在这里用同一套三重确认再判一次。
///
/// 端口表从 `docker compose config --format json` 取（**渲染后**的真实意图），
/// 不是从 `launcher.toml` 取 —— 后者只是我们以为写下去的东西。
pub fn preflight_ports() -> AppResult<Vec<PreflightConflict>> {
    let rendered = config_check_json()?;
    let v: serde_json::Value = serde_json::from_str(&rendered).map_err(|e| {
        AppError::new(
            Code::Unknown,
            format!("compose config 的 JSON 解析不了：{e}"),
        )
    })?;
    let mut want: Vec<(String, u16)> = Vec::new();
    if let Some(svcs) = v.get("services").and_then(|s| s.as_object()) {
        for (name, body) in svcs {
            let Some(ports) = body.get("ports").and_then(|p| p.as_array()) else {
                continue;
            };
            for p in ports {
                // 渲染后的 compose 里 published 可能是字符串也可能是数字
                let published = p.get("published").and_then(|x| {
                    x.as_u64()
                        .or_else(|| x.as_str().and_then(|s| s.parse::<u64>().ok()))
                });
                if let Some(n) = published {
                    if let Ok(n) = u16::try_from(n) {
                        want.push((name.clone(), n));
                    }
                }
            }
        }
    }
    if want.is_empty() {
        return Ok(Vec::new());
    }
    let sv = crate::ports::Survey::collect();
    let mut out = Vec::new();
    for (svc, port) in want {
        // 我们自己那一套正占着的不算冲突（`usable()` 把那一档单独分出来了）
        let vd = sv.verdict(port, &[PROJECT]);
        if vd.usable() {
            continue;
        }
        out.push(PreflightConflict {
            service: svc,
            port,
            occupied_by: vd
                .occupants
                .iter()
                .map(crate::ports::Occupant::human)
                .collect::<Vec<_>>()
                .join("；"),
        });
    }
    Ok(out)
}

/// 预检不过就返回 `E_PORT_CONFLICT`。总指挥接到它去 remap 再来一次。
pub fn preflight_gate() -> AppResult<()> {
    let c = preflight_ports()?;
    if c.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = c
        .iter()
        .map(|x| format!("{}（{}）被 {} 占着", x.port, x.service, x.occupied_by))
        .collect();
    Err(AppError::new(
        Code::PortConflict,
        format!(
            "起容器前的预检发现 {} 个端口冲突：{}。先换端口再起，不等 Docker 报错。",
            c.len(),
            lines.join("；")
        ),
    ))
}

// ── 绑定漂移：容器实际绑在哪 vs 覆盖文件说该绑在哪（I11 · U5）─────────────

/// 一条「容器实际绑的地址和覆盖文件说的不一样」。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindDrift {
    pub service: String,
    pub port: u16,
    /// docker 报的真实绑定地址（`0.0.0.0` / `::`）
    pub actual: String,
}

impl BindDrift {
    pub fn human(&self) -> String {
        format!(
            "{}（端口 {}）现在绑在 {} 上，局域网里的其他机器能连到它",
            self.service, self.port, self.actual
        )
    }
}

/// 本项目**正在跑的容器**里，有哪些把端口绑到了本机之外。
///
/// 只在覆盖文件说「该绑本机」时才算漂移：
/// 老机器的 web 本来就对外（[`crate::config::WebBind::LegacyLan`]），
/// 那是用户的现状，启动器有意不悄悄改它（I7 · 用户 2026-09-21 19:05 的决定）。
///
/// 这件事只能问 docker，不能问我们自己写的配置 —— 配置是意图，这里要的是现状。
/// 现场依据：2026-09-22 22:01 本地 Claude 在用户 Mac 上手工
/// `docker compose -p hunter up -d` 漏了覆盖文件，容器被重建成 0.0.0.0，
/// 五个端口在局域网可达了约 50 分钟。
pub fn bind_drift(services: &[ServiceStatus]) -> Vec<BindDrift> {
    let web_should_be_lan = crate::config::WebBind::detect().lan_exposed();
    services
        .iter()
        .filter(|s| s.state == "running")
        .filter(|s| !(s.service == "web" && web_should_be_lan))
        .filter(|s| s.lan_exposed())
        .filter_map(|s| {
            Some(BindDrift {
                service: s.service.clone(),
                port: s.port?,
                actual: s.bind.clone().unwrap_or_default(),
            })
        })
        .collect()
}

/// 查一遍绑定漂移；有就按覆盖文件把那些容器重建回来。
///
/// 返回「这次处理了哪几条」。没有漂移时返回空表并且**一个子进程都不起**。
///
/// 为什么是 `up -d --force-recreate <那几个服务>` 而不是整套 `up`：
/// 端口映射是容器创建时定死的，不重建改不掉；而没漂移的服务一个都不该碰。
pub fn fix_bind_drift(drift: &[BindDrift]) -> AppResult<()> {
    if drift.is_empty() {
        return Ok(());
    }
    let names: Vec<&str> = drift.iter().map(|d| d.service.as_str()).collect();
    up_services(&names)
}

// ── 本项目的残留容器（I5 §六.5） ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaleContainer {
    pub id: String,
    pub name: String,
    pub state: String,
}

/// 列出**本项目** `hunter` 里状态为 `created` 的残留容器。
///
/// 用户 Mac 上那次失败留下了 3 个（api / web / opencode）：`up` 起到一半被端口
/// 冲突打断，容器建出来了但没起来，还带着**旧的端口配置**。不清掉就重来的话，
/// compose 可能直接复用它们，换过的端口不生效。
pub fn stale_own_containers() -> Vec<StaleContainer> {
    let bin = which::docker_bin();
    let filter = format!("label=com.docker.compose.project={PROJECT}");
    let r = proc::run_timeout(
        &bin,
        &[
            "ps",
            "-a",
            "--filter",
            &filter,
            "--filter",
            "status=created",
            "--format",
            "{{.ID}}\t{{.Names}}\t{{.State}}",
        ],
        Duration::from_secs(20),
    );
    let Ok(r) = r else { return Vec::new() };
    if !r.ok() {
        return Vec::new();
    }
    r.stdout
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.trim().split('\t').collect();
            if f.len() < 3 || f[0].is_empty() {
                return None;
            }
            Some(StaleContainer {
                id: f[0].to_string(),
                name: f[1].to_string(),
                state: f[2].to_string(),
            })
        })
        .collect()
}

/// 删掉本项目的 `Created` 残留容器。**不带 `-v`，也绝不碰别的项目**。
///
/// 每一个都在删之前再查一遍 `com.docker.compose.project` 标签 ——
/// `--filter` 已经筛过一次，这里再核一次是因为「删容器」这个动作没有后悔药，
/// 而守卫的成本只有一次 `docker inspect`。
pub fn remove_own_stale_containers() -> AppResult<Vec<String>> {
    let bin = which::docker_bin();
    let mut removed = Vec::new();
    for c in stale_own_containers() {
        // 守卫：删之前再查一遍这个容器的 compose 项目标签（`--filter` 已经筛过一次，
        // 但删容器没有后悔药，多一次 `docker inspect` 换一个确定性很划算）
        if let Err(e) = crate::assist::guard::own_container(&c.id) {
            crate::lwarn!("不删容器 {}：{}", c.name, e.msg);
            continue;
        }
        let argv: Vec<String> = vec![bin.clone(), "rm".into(), "-f".into(), c.id.clone()];
        // 按 ID 删带不了 `--project-name`，范围保证来自上面那次 own_container
        crate::assist::guard::argv_scoped(&argv, true)?;
        match proc::run_timeout(&bin, &["rm", "-f", &c.id], Duration::from_secs(60)) {
            Ok(r) if r.ok() => {
                crate::linfo!("清掉本项目的残留容器 {}（{}）", c.name, c.state);
                removed.push(c.name);
            }
            Ok(r) => crate::lwarn!("删残留容器 {} 失败：{}", c.name, r.err_line()),
            Err(e) => crate::lwarn!("删残留容器 {} 起不来：{}", c.name, e.msg),
        }
    }
    Ok(removed)
}

pub fn up() -> AppResult<()> {
    guard_project_owner()?;
    let r = run(&["up", "-d", "--remove-orphans"], Duration::from_secs(300))?;
    if r.ok() {
        crate::linfo!("docker compose up -d 完成");
        Ok(())
    } else {
        // I5：**不再一律归成 E_START_TIMEOUT**。端口冲突有自己的错误码，
        // 否则规则层与 AI 拿到的前提就是错的（用户 Mac 上那次失败的第 2、3 条根因）
        let mut text = r.stderr.clone();
        if !r.stdout.trim().is_empty() {
            text.push('\n');
            text.push_str(&r.stdout);
        }
        Err(classify_up_error(&text, r.status))
    }
}

/// `up -d` 之后轮询健康，超时 180 秒。
/// 超时的时候**指出是哪个服务没就绪**（方案 §18 对 `E_START_TIMEOUT` 的明确要求）。
pub fn wait_healthy(
    timeout: Duration,
    mut on_tick: impl FnMut(&[ServiceStatus]),
) -> AppResult<Vec<ServiceStatus>> {
    let deadline = Instant::now() + timeout;
    let expected = ["api", "llm-shim", "opencode", "postgres", "redis", "web"];
    let mut last: Vec<ServiceStatus> = Vec::new();
    loop {
        match ps() {
            Ok(v) => {
                last = v;
                on_tick(&last);
                let ready: Vec<&str> = last
                    .iter()
                    .filter(|s| service_ready(s))
                    .map(|s| s.service.as_str())
                    .collect();
                if expected.iter().all(|e| ready.contains(e)) {
                    return Ok(last);
                }
                // 有容器直接退出了就别等了
                if let Some(dead) = last
                    .iter()
                    .find(|s| s.state == "exited" && s.exit_code.unwrap_or(0) != 0)
                {
                    return Err(AppError::new(
                        Code::StartTimeout,
                        format!(
                            "服务 {} 启动后就退出了（退出码 {}）。先看它的日志。",
                            dead.service,
                            dead.exit_code.unwrap_or(-1)
                        ),
                    ));
                }
            }
            Err(e) => crate::lwarn!("轮询 compose ps 失败（会继续重试）：{}", e.msg),
        }
        if Instant::now() >= deadline {
            let not_ready: Vec<String> = expected
                .iter()
                .filter(|e| !last.iter().any(|s| s.service == **e && service_ready(s)))
                .map(|e| {
                    let s = last.iter().find(|s| s.service == *e);
                    match s {
                        Some(s) => format!("{}（{} · {}）", e, s.state, health_cn(s.health)),
                        None => format!("{e}（还没有容器）"),
                    }
                })
                .collect();
            return Err(AppError::new(
                Code::StartTimeout,
                format!(
                    "等了 {} 秒，还有 {} 个服务没就绪：{}。用「查看日志」看它们的输出。",
                    timeout.as_secs(),
                    not_ready.len(),
                    not_ready.join("、")
                ),
            ));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

pub fn health_cn(h: Health) -> &'static str {
    match h {
        Health::Healthy => "健康",
        Health::Starting => "启动中",
        Health::Unhealthy => "不健康",
        Health::None => "无健康检查",
        Health::Pending => "未启动",
    }
}

pub fn stop() -> AppResult<()> {
    let r = run(&["stop"], Duration::from_secs(180))?;
    if r.ok() {
        Ok(())
    } else {
        Err(AppError::new(
            Code::Unknown,
            format!("docker compose stop 失败：{}", r.err_line()),
        ))
    }
}

/// `down`。**永远不带 `-v`** —— 那会删掉用户的数据卷。
pub fn down() -> AppResult<()> {
    let r = run(&["down", "--remove-orphans"], Duration::from_secs(180))?;
    if r.ok() {
        Ok(())
    } else {
        Err(AppError::new(
            Code::Unknown,
            format!("docker compose down 失败：{}", r.err_line()),
        ))
    }
}

/// 只重建指定的几个服务（切换模型模式后用）。
///
/// 为什么是 `up -d <服务…>` 而不是 `restart <服务…>`：`restart` 只是把进程重启一遍，
/// **容器里的环境变量还是老的**；`.env` 改了要生效必须让 compose 重新创建容器。
/// 这一点是真跑出来的 —— 只 `restart` 的话切完模型对话仍然走旧网关。
pub fn up_services(services: &[&str]) -> AppResult<()> {
    // `--force-recreate` 会把容器整个换掉，落到别人的那一套上就是一场事故（待办池 P0-5）
    guard_project_owner()?;
    let mut args: Vec<&str> = vec!["up", "-d", "--force-recreate"];
    args.extend_from_slice(services);
    let r = run(&args, Duration::from_secs(300))?;
    if r.ok() {
        crate::linfo!("docker compose up -d {} 完成", services.join(" "));
        Ok(())
    } else {
        Err(AppError::new(
            Code::StartTimeout,
            format!(
                "docker compose up -d {} 失败：{}",
                services.join(" "),
                r.err_line()
            ),
        ))
    }
}

/// 只把**指定的几个服务**起起来，**不重建任何已经在跑的容器**（I11 · U2）。
///
/// 和 [`up_services`] 的区别就是 `--no-recreate`：那一个是「配置变了，要换容器」，
/// 这一个是「别的都好好的，只有这一两个没起来」。
/// 「点重试不能把正在跑的东西推倒重来」这条要求，就落在这个标志上。
pub fn start_services(services: &[&str]) -> AppResult<()> {
    guard_project_owner()?;
    let mut args: Vec<&str> = vec!["up", "-d", "--no-recreate"];
    args.extend_from_slice(services);
    let r = run(&args, Duration::from_secs(300))?;
    if r.ok() {
        crate::linfo!(
            "docker compose up -d --no-recreate {} 完成",
            services.join(" ")
        );
        Ok(())
    } else {
        Err(classify_up_error(
            &format!("{}\n{}", r.stderr, r.stdout),
            r.status,
        ))
    }
}

/// 只重启**指定的几个服务**的进程（容器不换，端口映射与数据卷都不动）。
///
/// 给「容器在跑但健康检查不过」用：那种情况容器本身是好的，
/// 重建它既慢又可能把一个本来只是没准备好的服务弄成新问题。
pub fn restart_services(services: &[&str]) -> AppResult<()> {
    guard_project_owner()?;
    let mut args: Vec<&str> = vec!["restart"];
    args.extend_from_slice(services);
    let r = run(&args, Duration::from_secs(240))?;
    if r.ok() {
        crate::linfo!("docker compose restart {} 完成", services.join(" "));
        Ok(())
    } else {
        Err(AppError::new(
            Code::StartTimeout,
            format!(
                "docker compose restart {} 失败：{}",
                services.join(" "),
                r.err_line()
            ),
        ))
    }
}

pub fn restart() -> AppResult<()> {
    let r = run(&["restart"], Duration::from_secs(240))?;
    if r.ok() {
        Ok(())
    } else {
        Err(AppError::new(
            Code::Unknown,
            format!("docker compose restart 失败：{}", r.err_line()),
        ))
    }
}

/// 取日志。**返回前统一脱敏**（容器日志里可能带 key）。
pub fn logs(service: Option<&str>, tail: usize) -> AppResult<Vec<String>> {
    let tail_s = tail.to_string();
    let mut args: Vec<&str> = vec!["logs", "--no-color", "--tail", tail_s.as_str()];
    if let Some(s) = service {
        args.push(s);
    }
    let r = run(&args, Duration::from_secs(60))?;
    let mut out: Vec<String> = Vec::new();
    for l in r.stdout.lines().chain(r.stderr.lines()) {
        if !l.trim().is_empty() {
            out.push(crate::redact::redact(l));
        }
    }
    Ok(out)
}

/// `docker compose config` —— 在真启动之前验一遍文件写对了没有。
pub fn config_check() -> AppResult<String> {
    let r = run(&["config"], Duration::from_secs(60))?;
    if r.ok() {
        Ok(r.stdout)
    } else {
        Err(AppError::new(
            Code::ConfigWrite,
            format!("compose 文件校验不过：{}", r.err_line()),
        ))
    }
}

/// 同上，但要 JSON —— 红线 4 的端口绑定自查要读结构化结果，不能靠 grep 文本。
pub fn config_check_json() -> AppResult<String> {
    let r = run(&["config", "--format", "json"], Duration::from_secs(60))?;
    if r.ok() {
        Ok(r.stdout)
    } else {
        Err(AppError::new(
            Code::ConfigWrite,
            format!("compose 文件校验不过：{}", r.err_line()),
        ))
    }
}

/// 栈是不是已经起着（第二次打开启动器时判断要不要直接进运行面板）。
pub fn is_up() -> bool {
    ps().map(|v| v.iter().any(|s| s.state == "running"))
        .unwrap_or(false)
}

// ── 本项目的数据卷（I12 · R5 / R2）────────────────────────────────────────

/// 一个数据卷的现状。**名字与挂载点都来自 docker 自己**，不是我们拼出来的。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    /// docker 里的完整卷名（例如 `hunter_pg_data`）
    pub name: String,
    /// compose 文件里声明的卷名（例如 `hunter_pg_data`）。
    ///
    /// 来自 docker 的标签 `com.docker.compose.volume` —— **不是按名字前缀切出来的**。
    /// 实测（测试机 2026-09-23）：compose 项目 `hunter` 里，compose 文件声明的卷叫
    /// `hunter_pg_data`，docker 里的完整名是 `hunter_hunter_pg_data`（项目名 + 声明名）。
    /// 方案 1.3 节那张表写的是 `pg_data`，与实际差一层 —— 以实测为准（总控规则）。
    /// 标签读不到时退回按项目名前缀切一次，并在 `label_missing` 里说明
    pub short: String,
    /// 这一条的 `short` 是不是猜出来的（标签没读到）
    pub label_missing: bool,
    /// 卷在运行时里的挂载点。内置运行时下这是**虚拟机里**的路径，宿主机上看不到
    pub mountpoint: String,
    /// 占多少字节。`docker system df -v` 给不出来就是 `None`（显示「—」，不猜）
    pub size_bytes: Option<u64>,
}

/// 列出 compose 项目 `hunter` 名下的全部数据卷。
///
/// 判据是 docker 自己打的标签 `com.docker.compose.project=hunter` ——
/// **不按名字前缀猜**：用户另外装的 `hunter-fresh`、`hunter-community` 前缀也以 hunter 开头，
/// 按前缀匹配会把别人的卷算到我们头上（R4 删除应用时那就是一场事故）。
pub fn volumes_of_project() -> AppResult<Vec<VolumeInfo>> {
    let bin = which::docker_bin();
    let filter = format!("label=com.docker.compose.project={PROJECT}");
    let r = proc::run_timeout(
        &bin,
        &[
            "volume",
            "ls",
            "--filter",
            &filter,
            "--format",
            "{{.Name}}\t{{.Mountpoint}}\t{{.Labels}}",
        ],
        Duration::from_secs(30),
    )?;
    if !r.ok() {
        return Err(AppError::new(
            Code::Unknown,
            format!("docker volume ls 失败：{}", r.err_line()),
        ));
    }
    Ok(parse_volumes(&r.stdout))
}

/// `docker volume ls --format '{{.Name}}\t{{.Mountpoint}}\t{{.Labels}}'` 的输出。
pub fn parse_volumes(stdout: &str) -> Vec<VolumeInfo> {
    let prefix = format!("{PROJECT}_");
    let mut v: Vec<VolumeInfo> = stdout
        .lines()
        .filter_map(|l| {
            let mut f = l.trim_end().splitn(3, '\t');
            let name = f.next().unwrap_or("").trim();
            if name.is_empty() {
                return None;
            }
            let mount = f.next().unwrap_or("").trim().to_string();
            let labels = f.next().unwrap_or("");
            let declared = labels.split(',').find_map(|kv| {
                kv.trim()
                    .strip_prefix("com.docker.compose.volume=")
                    .map(str::to_string)
            });
            let label_missing = declared.is_none();
            let short =
                declared.unwrap_or_else(|| name.strip_prefix(&prefix).unwrap_or(name).to_string());
            Some(VolumeInfo {
                name: name.to_string(),
                short,
                label_missing,
                mountpoint: mount,
                size_bytes: None,
            })
        })
        .collect();
    v.sort_by(|a, b| a.short.cmp(&b.short));
    v
}

/// compose 里声明的那几个卷，按「丢了会怎样」分档（方案 1.3 节，名字按实测修正）。
///
/// 这张表**只用来给人话说明**（界面上「这一个是数据库」之类），
/// 判断卷在不在一律以 docker 的标签为准。
pub const VOLUME_NOTES: &[(&str, &str)] = &[
    ("hunter_pg_data", "数据库：账号、配置、自选股、历史分析"),
    ("hunter_secrets", "密钥卷：数据库里加密的配置靠它才解得开"),
    ("hunter_opencode_data", "对话引擎的会话记录"),
    ("hunter_user_skills", "你自己建的技能"),
    ("hunter_packages", "依赖缓存（可再生）"),
    ("hunter_redis_data", "缓存（可再生）"),
];

/// 数据库那个卷在 compose 里声明的名字。R5 与 R4 都按它找。
pub const VOL_DB: &str = "hunter_pg_data";
/// 密钥卷。**必须和数据库成对**处理（方案第六节第 2 条）。
pub const VOL_SECRETS: &str = "hunter_secrets";
/// 用户自建技能。备份可选项之一（I13 · R6）。
pub const VOL_SKILLS: &str = "hunter_user_skills";
/// 对话引擎的会话记录。备份可选项之一（I13 · R6）。
pub const VOL_SESSIONS: &str = "hunter_opencode_data";

/// compose 文件里**声明过**的全部数据卷（短名）。
///
/// I13 的「悬空卷」判定靠它：带着本项目标签、但**不在这张表里**的卷，
/// 就是历史上改过卷名之后留下的孤儿 —— 只有那种才允许一键清掉。
/// 这张表和 [`VOLUME_NOTES`] 是同一份来源，避免两处各写一半。
pub fn declared_volumes() -> Vec<&'static str> {
    VOLUME_NOTES.iter().map(|(k, _)| *k).collect()
}

/// 本项目的卷一共占多少（`docker system df -v`）。
///
/// 只统计**本项目的**卷：`docker system df` 那一行「Local Volumes」是全机器的总数，
/// 拿它当 Hunter 的占用就是编数字（红线 1）。
/// 拿不到就整体返回 `None` —— 界面显示「—」+ 原因。
pub fn volume_sizes() -> AppResult<BTreeMap<String, u64>> {
    let bin = which::docker_bin();
    let r = proc::run_timeout(
        &bin,
        &["system", "df", "-v", "--format", "{{json .Volumes}}"],
        Duration::from_secs(60),
    )?;
    if !r.ok() {
        return Err(AppError::new(
            Code::Unknown,
            format!("docker system df -v 失败：{}", r.err_line()),
        ));
    }
    Ok(parse_volume_sizes(&r.stdout))
}

/// 解析 `docker system df -v --format '{{json .Volumes}}'`。
///
/// 每个元素形如 `{"Name":"hunter_pg_data","Size":"312.4MB","Links":1}`。
/// **`Size` 是给人看的字符串**，docker 没有给字节数的格式化选项，所以这里得自己还原。
pub fn parse_volume_sizes(stdout: &str) -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(arr) = v.as_array() else { continue };
        for it in arr {
            let (Some(name), Some(size)) = (
                it.get("Name").and_then(|x| x.as_str()),
                it.get("Size").and_then(|x| x.as_str()),
            ) else {
                continue;
            };
            if let Some(b) = parse_human_size(size) {
                m.insert(name.to_string(), b);
            }
        }
    }
    m
}

/// `312.4MB` / `4.003GB` / `0B` → 字节。认不出来就是 `None`（**不猜**）。
///
/// docker 用的是 go-units 的十进制单位（kB = 1000），不是 1024 —— 照它的来，
/// 否则我们印出来的数字和用户敲 `docker system df` 看到的对不上。
pub fn parse_human_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let pos = s.find(|c: char| c.is_ascii_alphabetic())?;
    let (num, unit) = s.split_at(pos);
    let n: f64 = num.trim().parse().ok()?;
    let mult: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        _ => return None,
    };
    Some((n * mult) as u64)
}

/// 本项目**镜像**一共占多少字节（`docker image ls` 按 compose 标签筛）。
///
/// compose 不给镜像打项目标签，所以这里按 `.env` / compose 里那六个镜像引用去查 ——
/// 查不到的就不计入，并把「查到了几个」一起给出去，界面好说清楚这个数是哪来的。
pub fn image_disk_usage(refs: &[String]) -> (Option<u64>, usize) {
    if refs.is_empty() {
        return (None, 0);
    }
    let bin = which::docker_bin();
    let mut total = 0u64;
    let mut hit = 0usize;
    // 同一个镜像可能被多个服务引用（这里不会），按 ID 去重才不会重复计
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for r in refs {
        let Ok(out) = proc::run_timeout(
            &bin,
            &["image", "inspect", r, "--format", "{{.Id}}\t{{.Size}}"],
            Duration::from_secs(20),
        ) else {
            continue;
        };
        if !out.ok() {
            continue;
        }
        let line = out.stdout.trim();
        let mut f = line.split('\t');
        let (Some(id), Some(size)) = (f.next(), f.next()) else {
            continue;
        };
        hit += 1;
        if !seen.insert(id.to_string()) {
            continue;
        }
        if let Ok(b) = size.trim().parse::<u64>() {
            total += b;
        }
    }
    if hit == 0 {
        (None, 0)
    } else {
        (Some(total), hit)
    }
}

/// 每个镜像各自多大（I13 · R4 的删除报告要逐个摆出来）。
///
/// 和 [`image_disk_usage`] 的区别只有一个：那个给总数，这个给明细。
/// **查不到的镜像不进这张表**（界面显示「—」，不猜），所以调用方要按引用去查，
/// 查不到就是「本机没有这一个」。
pub fn image_sizes(refs: &[String]) -> BTreeMap<String, u64> {
    let mut m = BTreeMap::new();
    let bin = which::docker_bin();
    for r in refs {
        let Ok(out) = proc::run_timeout(
            &bin,
            &["image", "inspect", r, "--format", "{{.Size}}"],
            Duration::from_secs(20),
        ) else {
            continue;
        };
        if !out.ok() {
            continue;
        }
        if let Ok(b) = out.stdout.trim().parse::<u64>() {
            m.insert(r.clone(), b);
        }
    }
    m
}

/// 每个容器的重启次数与「是不是被 OOM 杀过」（R2 的服务层）。
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContainerVitals {
    pub restart_count: Option<u32>,
    pub oom_killed: Option<bool>,
    /// 容器是什么时候起来的（RFC3339 原样给出，前端只显示不解析）
    pub started_at: Option<String>,
}

/// `docker inspect` 本项目的一个服务容器，取重启次数与 OOM 标志。
pub fn vitals_of(container: &str) -> Option<ContainerVitals> {
    let bin = which::docker_bin();
    let out = proc::run_timeout(
        &bin,
        &[
            "inspect",
            container,
            "--format",
            "{{.RestartCount}}\t{{.State.OOMKilled}}\t{{.State.StartedAt}}",
        ],
        Duration::from_secs(20),
    )
    .ok()?;
    if !out.ok() {
        return None;
    }
    let line = out.stdout.trim();
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 3 {
        return None;
    }
    Some(ContainerVitals {
        restart_count: f[0].parse().ok(),
        oom_killed: match f[1] {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        started_at: (!f[2].is_empty() && f[2] != "<no value>").then(|| f[2].to_string()),
    })
}

/// 本项目**六个服务的容器名**（`docker compose ps` 报的那一份）。
pub fn container_names() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let bin = which::docker_bin();
    let filter = format!("label=com.docker.compose.project={PROJECT}");
    let Ok(r) = proc::run_timeout(
        &bin,
        &[
            "ps",
            "-a",
            "--filter",
            &filter,
            "--format",
            "{{.Label \"com.docker.compose.service\"}}\t{{.Names}}",
        ],
        Duration::from_secs(20),
    ) else {
        return m;
    };
    if !r.ok() {
        return m;
    }
    for l in r.stdout.lines() {
        let mut f = l.trim().splitn(2, '\t');
        let (Some(svc), Some(name)) = (f.next(), f.next()) else {
            continue;
        };
        if !svc.is_empty() && !name.is_empty() {
            m.insert(svc.to_string(), name.to_string());
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    // ── I12 · R3：compose 调用必带覆盖文件 ──────────────────────────────
    //
    // 2026-09-22 22:01 的事故（I11 · U5）：本地 Claude 手工跑了一条漏掉覆盖文件的
    // `docker compose -p hunter up -d`，五个端口在局域网里可达了约 50 分钟。
    // I11 兜住了「手工敲」那一路（`.env` 里写 `COMPOSE_FILE`）；这几条钉住
    // **启动器自己**那一路：不管哪个函数、哪个子命令，命令行里一定有两份 `-f`。

    /// 这条命令行里两份 compose 文件都在，而且覆盖文件排在基础文件**后面**。
    fn 断言带着覆盖文件(args: &[String]) {
        let base = paths::compose_file().to_string_lossy().into_owned();
        let overlay = paths::override_file().to_string_lossy().into_owned();
        let f: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, _)| *i > 0 && args[i - 1] == "-f")
            .map(|(_, a)| a)
            .collect();
        assert_eq!(f.len(), 2, "必须正好两份 -f：{args:?}");
        assert_eq!(f[0], &base, "第一份必须是基础 compose：{args:?}");
        assert_eq!(
            f[1], &overlay,
            "第二份必须是覆盖文件，而且排在后面（排前面 !override 就覆盖不到）：{args:?}"
        );
        // 项目名也一起钉住：漏了它 compose 会按目录名猜一个，那就是另一套
        let p: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, _)| *i > 0 && args[i - 1] == "--project-name")
            .map(|(_, a)| a)
            .collect();
        assert_eq!(
            p,
            vec![&PROJECT.to_string()],
            "必须带 --project-name：{args:?}"
        );
    }

    #[test]
    fn 每一个_compose_子命令都带着覆盖文件() {
        // 这两条测试要读 `paths::compose_file()`，而它跟着进程级的 `HUNTER_HOME` 走 ——
        // 不拿这把锁的话，别的文件里拿了锁的测试一并行，这里读到的就是**人家的**根目录
        // （2026-09-24 CI 上就是这么红的：命令行里是 hunter-unittest-<pid>，
        // 断言那一侧却成了 hunter-t-kept-key-state-<pid>）。I7、I9 各抓过一轮同样的事。
        let _h = paths::test_home("compose-argv");

        // 这一份清单覆盖启动器会发出的全部 compose 子命令。
        // 加新命令时如果没走 `compose::run` / `compose::argv`，这条测试是发现不了的 ——
        // 所以 `full_args` 是唯一的出口，别在别处自己拼 docker compose
        for extra in [
            vec!["up", "-d", "--remove-orphans"],
            vec!["up", "-d", "--no-recreate", "opencode"],
            vec!["up", "-d", "--force-recreate", "postgres", "redis"],
            vec!["stop"],
            vec!["start"],
            vec!["restart"],
            vec!["restart", "api"],
            vec!["down", "--remove-orphans"],
            vec!["ps", "--all", "--format", "json"],
            vec!["logs", "--no-color", "--tail", "40"],
            vec!["config"],
            vec!["--progress", "json", "pull"],
        ] {
            let (_, args) = argv(&extra);
            断言带着覆盖文件(&args);
            // 子命令本身也得原样在后面
            let tail: Vec<String> = args[args.len() - extra.len()..].to_vec();
            assert_eq!(tail, extra, "子命令被改动了：{args:?}");
        }
    }

    #[test]
    fn 覆盖文件的路径就是磁盘上那一份() {
        let _h = paths::test_home("compose-overlay-path");
        // 别把覆盖文件写成别的名字：`.env` 里的 `COMPOSE_FILE`（I11 · U5）与这里
        // 必须指同一个文件，否则手工敲和启动器自己跑的会是两套配置
        let (_, args) = argv(&["ps"]);
        assert!(
            args.iter()
                .any(|a| a.ends_with("docker-compose.launcher.yml")),
            "{args:?}"
        );
    }

    fn specs() -> Vec<ImageSpec> {
        config::images("ghcr.io/agentpit-io", "docker.io/library", "1.2.0")
    }

    fn sizes() -> BTreeMap<String, u64> {
        // M0 §4.1 实测的 amd64 压缩字节数
        BTreeMap::from([
            ("web".to_string(), 331_344_498u64),
            ("api".to_string(), 214_680_985),
            ("opencode".to_string(), 152_979_569),
            ("llm-shim".to_string(), 18_054_459),
            ("postgres".to_string(), 115_990_528),
            ("redis".to_string(), 16_148_070),
        ])
    }

    #[test]
    fn 数据卷的短名来自_docker_的标签而不是按前缀猜() {
        // 实测（测试机 2026-09-23）：compose 项目 `hunter` 里，声明名是 `hunter_pg_data`，
        // docker 里的完整名是 `hunter_hunter_pg_data`。按前缀切一次得到的正是声明名，
        // 但**标签才是真值** —— 方案 1.3 节写的 `pg_data` 与实际差一层
        let out = "hunter_hunter_pg_data\t/var/lib/docker/volumes/hunter_hunter_pg_data/_data\tcom.docker.compose.project=hunter,com.docker.compose.volume=hunter_pg_data\n\
                   hunter_hunter_secrets\t/var/lib/docker/volumes/hunter_hunter_secrets/_data\tcom.docker.compose.project=hunter,com.docker.compose.volume=hunter_secrets\n";
        let v = parse_volumes(out);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "hunter_hunter_pg_data");
        assert_eq!(v[0].short, VOL_DB);
        assert!(!v[0].label_missing);
        assert_eq!(v[1].short, VOL_SECRETS);
    }

    #[test]
    fn 没有标签时退回按前缀切_并且说清是猜的() {
        let out = "hunter_hunter_pg_data\t/var/lib/docker/volumes/x/_data\t\n";
        let v = parse_volumes(out);
        assert_eq!(v[0].short, "hunter_pg_data");
        assert!(v[0].label_missing, "标签没读到就要说清楚这一条是猜的");
    }

    #[test]
    fn 卷大小按_docker_的十进制单位还原() {
        let out = r#"[{"Name":"hunter_hunter_pg_data","Size":"51.05MB","Links":"1"},{"Name":"hunter_hunter_secrets","Size":"0B","Links":"1"}]"#;
        let m = parse_volume_sizes(out);
        assert_eq!(m.get("hunter_hunter_pg_data"), Some(&51_050_000));
        assert_eq!(m.get("hunter_hunter_secrets"), Some(&0));
    }

    #[test]
    fn 卷大小认不出来就不进表_不猜() {
        let out = r#"[{"Name":"hunter_hunter_pg_data","Size":"N/A"}]"#;
        assert!(parse_volume_sizes(out).is_empty());
    }

    #[test]
    fn 分母来自_manifest_而不是等层报上来() {
        let agg = PullAggregator::new(&specs(), &sizes(), "ghcr.io/agentpit-io", "GHCR · GitHub");
        let total: u64 = agg.images.iter().map(|i| i.total_bytes).sum();
        assert_eq!(total, 849_198_109, "六个镜像合计（M0 §4.1）");
        assert!(agg.images.iter().all(|i| !i.size_unknown));
    }

    #[test]
    fn 按_parent_id_分组并累加层字节() {
        let mut agg = PullAggregator::new(&specs(), &sizes(), "ghcr.io/agentpit-io", "GHCR");
        let img = "Image ghcr.io/agentpit-io/hunter-community-web:1.2.0";
        agg.feed(&format!(
            r#"{{"id":"{img}","status":"Working","text":"Pulling"}}"#
        ));
        agg.feed(&format!(r#"{{"id":"aaa","parent_id":"{img}","status":"Working","text":"Downloading","current":1000,"total":5000}}"#));
        agg.feed(&format!(r#"{{"id":"bbb","parent_id":"{img}","status":"Working","text":"Downloading","current":2000,"total":9000}}"#));
        let s = agg.snapshot(PullPhase::Pulling, None);
        let web = s.images.iter().find(|i| i.service == "web").unwrap();
        assert_eq!(web.downloaded_bytes, 3000);
        assert_eq!(web.state, PullState::Downloading);
        // 同一层再报一次更大的值应当是覆盖，不是累加
        agg.feed(&format!(r#"{{"id":"aaa","parent_id":"{img}","status":"Working","text":"Downloading","current":4000,"total":5000}}"#));
        let s = agg.snapshot(PullPhase::Pulling, None);
        assert_eq!(
            s.images
                .iter()
                .find(|i| i.service == "web")
                .unwrap()
                .downloaded_bytes,
            6000
        );
    }

    #[test]
    fn extracting_行没有_total_时只改状态不乱算进度() {
        let mut agg = PullAggregator::new(&specs(), &sizes(), "x", "x");
        let img = "Image ghcr.io/agentpit-io/hunter-community-api:1.2.0";
        agg.feed(&format!(r#"{{"id":"ccc","parent_id":"{img}","status":"Working","text":"Downloading","current":500,"total":500}}"#));
        agg.feed(&format!(r#"{{"id":"ccc","parent_id":"{img}","status":"Working","text":"Extracting","details":"1B","current":1}}"#));
        let s = agg.snapshot(PullPhase::Pulling, None);
        let api = s.images.iter().find(|i| i.service == "api").unwrap();
        assert_eq!(
            api.state,
            PullState::Extracting,
            "这一段界面要显示「解压中」而不是百分比"
        );
        assert_eq!(
            api.downloaded_bytes, 500,
            "Extracting 的 current=1 不能把已下载字节打回去"
        );
    }

    #[test]
    fn 每个镜像记下自己的耗时() {
        let mut agg = PullAggregator::new(&specs(), &sizes(), "x", "x");
        let img = "Image ghcr.io/agentpit-io/hunter-community-web:1.2.0";
        agg.feed(&format!(
            r#"{{"id":"l1","parent_id":"{img}","text":"Downloading","current":1,"total":2}}"#
        ));
        let mid = agg.snapshot(PullPhase::Pulling, None);
        assert!(
            mid.images
                .iter()
                .find(|i| i.service == "web")
                .unwrap()
                .seconds
                .is_none(),
            "没拉完不该有耗时"
        );
        agg.feed(&format!(
            r#"{{"id":"{img}","status":"Done","text":"Pulled"}}"#
        ));
        let end = agg.snapshot(PullPhase::Pulling, None);
        assert!(
            end.images
                .iter()
                .find(|i| i.service == "web")
                .unwrap()
                .seconds
                .is_some(),
            "拉完要记下耗时"
        );
        // 没出现过的镜像不该凭空有耗时
        assert!(end
            .images
            .iter()
            .find(|i| i.service == "redis")
            .unwrap()
            .seconds
            .is_none());
    }

    #[test]
    fn pulled_之后进度补齐到百分之百() {
        let mut agg = PullAggregator::new(&specs(), &sizes(), "x", "x");
        let img = "Image ghcr.io/agentpit-io/hunter-community-llm-shim:1.2.0";
        agg.feed(&format!(
            r#"{{"id":"ddd","parent_id":"{img}","text":"Downloading","current":10,"total":20}}"#
        ));
        agg.feed(&format!(
            r#"{{"id":"{img}","status":"Done","text":"Pulled"}}"#
        ));
        let s = agg.snapshot(PullPhase::Pulling, None);
        let x = s.images.iter().find(|i| i.service == "llm-shim").unwrap();
        assert_eq!(x.state, PullState::Done);
        assert_eq!(x.downloaded_bytes, x.total_bytes, "完成了就不该停在 99%");
    }

    #[test]
    fn 认得出省略了_docker_io_前缀的_postgres() {
        let mut agg = PullAggregator::new(&specs(), &sizes(), "x", "x");
        agg.feed(r#"{"id":"eee","parent_id":"Image postgres:16-alpine","text":"Downloading","current":777,"total":1000}"#);
        let s = agg.snapshot(PullPhase::Pulling, None);
        assert_eq!(
            s.images
                .iter()
                .find(|i| i.service == "postgres")
                .unwrap()
                .downloaded_bytes,
            777
        );
    }

    #[test]
    fn 解析不了的行被丢进日志框而不是让它崩() {
        let mut agg = PullAggregator::new(&specs(), &sizes(), "x", "x");
        agg.feed("这不是 JSON");
        agg.feed("{\"未来的新字段\": 1}");
        agg.feed("");
        let s = agg.snapshot(PullPhase::Pulling, None);
        assert_eq!(s.percent, 0);
        assert!(s.log.iter().any(|l| l.contains("这不是 JSON")));
    }

    #[test]
    fn 日志框里的内容也过脱敏() {
        let mut agg = PullAggregator::new(&specs(), &sizes(), "x", "x");
        agg.feed("Get https://x/?token=hunt_tools_q6sKaaaaaaaaaaaaaaaaaaaaaaaaQMo2 failed");
        let s = agg.snapshot(PullPhase::Pulling, None);
        assert!(s.log.iter().all(|l| !l.contains("q6sK")), "{:?}", s.log);
    }

    /// 本机已经有的层（`Already exists`）会瞬间「完成」。
    /// 它们要算进百分比（用户关心整体完成度），但**不能**算进速度 ——
    /// 否则进度条右边会冒出 67 MB/s 这种一看就假的数字（M2 第一版实测踩过）。
    #[test]
    fn 已在本机的层不计入速度() {
        let mut agg = PullAggregator::new(&specs(), &sizes(), "x", "x");
        let img = "Image ghcr.io/agentpit-io/hunter-community-web:1.2.0";
        // 一层是本机已有的（只有 Already exists，没有 Downloading）
        agg.feed(&format!(
            r#"{{"id":"cached","parent_id":"{img}","text":"Already exists"}}"#
        ));
        // 另一层真的在下
        agg.feed(&format!(
            r#"{{"id":"net","parent_id":"{img}","text":"Downloading","current":1000,"total":4000}}"#
        ));
        let s1 = agg.snapshot(PullPhase::Pulling, None);
        // 第一次采样还算不出速度（样本不足一秒），但不能因为缓存层就算出一个天文数字
        assert!(
            s1.speed_bps.is_none() || s1.speed_bps.unwrap() < 10_000_000,
            "{:?}",
            s1.speed_bps
        );
        // 百分比里缓存层照样算数：web 的已下载字节应当包含两层
        assert!(
            s1.images
                .iter()
                .find(|i| i.service == "web")
                .unwrap()
                .downloaded_bytes
                >= 1000
        );
    }

    #[test]
    fn 总进度与百分比() {
        let mut agg = PullAggregator::new(&specs(), &sizes(), "x", "x");
        for (svc, r) in [
            ("web", "hunter-community-web:1.2.0"),
            ("api", "hunter-community-api:1.2.0"),
        ] {
            let img = format!("Image ghcr.io/agentpit-io/{r}");
            let total = sizes()[svc];
            agg.feed(&format!(r#"{{"id":"l-{svc}","parent_id":"{img}","text":"Downloading","current":{total},"total":{total}}}"#));
        }
        let s = agg.snapshot(PullPhase::Pulling, None);
        assert_eq!(s.downloaded_bytes, 331_344_498 + 214_680_985);
        assert_eq!(s.total_bytes, 849_198_109);
        assert_eq!(s.percent, 64, "两个大镜像下完刚好 64%");
    }

    /// 这一条用的是 M2 在测试机上抓到的**真实 stderr 流**的行型
    /// （M0 §5.1 记成 stdout 是错的，M2 实测是 stderr —— 见成果文档）。
    #[test]
    fn 真实流的六个镜像都能对上号() {
        let mut agg = PullAggregator::new(&specs(), &sizes(), "ghcr.io/agentpit-io", "GHCR");
        let real = [
            r#"{"id":"Image ghcr.io/agentpit-io/hunter-community-llm-shim:1.2.0","status":"Working","text":"Pulling"}"#,
            r#"{"id":"Image postgres:16-alpine","status":"Working","text":"Pulling"}"#,
            r#"{"id":"Image redis:7-alpine","status":"Working","text":"Pulling"}"#,
            r#"{"id":"Image ghcr.io/agentpit-io/hunter-community-web:1.2.0","status":"Working","text":"Pulling"}"#,
            r#"{"id":"Image ghcr.io/agentpit-io/hunter-community-api:1.2.0","status":"Working","text":"Pulling"}"#,
            r#"{"id":"Image ghcr.io/agentpit-io/hunter-community-opencode:1.2.0","status":"Working","text":"Pulling"}"#,
        ];
        for l in real {
            agg.feed(l);
        }
        let s = agg.snapshot(PullPhase::Pulling, None);
        // 六个都从 Pending 变成了 Downloading，说明 id 全部匹配上了
        // （postgres / redis 的 id 里没有 docker.io/library 前缀，靠后缀匹配认出来）
        assert_eq!(
            s.images
                .iter()
                .filter(|i| i.state == PullState::Downloading)
                .count(),
            6,
            "{:?}",
            s.images
        );
    }

    /// 用户 Mac 上 0.1.4 的真实原话（用户抄给我们的那两行）。
    #[test]
    fn 端口冲突要归成_e_port_conflict_而不是启动超时() {
        let real = "Error response from daemon: driver failed programming external connectivity \
                    on endpoint hunter-api-1 (8f3a…): Bind for 0.0.0.0:8100 failed: port is already allocated";
        let e = classify_up_error(real, Some(1));
        assert_eq!(e.code, Code::PortConflict, "{}", e.msg);
        assert!(e.msg.contains("8100"), "{}", e.msg);

        let real2 =
            "Error response from daemon: Bind for 0.0.0.0:6479 failed: port is already allocated";
        assert_eq!(classify_up_error(real2, Some(1)).code, Code::PortConflict);
    }

    #[test]
    fn address_already_in_use_也算端口冲突() {
        let s =
            "Error starting userland proxy: listen tcp4 0.0.0.0:3921: bind: address already in use";
        let e = classify_up_error(s, Some(1));
        assert_eq!(e.code, Code::PortConflict, "{}", e.msg);
        assert!(e.msg.contains("3921"), "{}", e.msg);
    }

    #[test]
    fn 抠端口只抠报错那几行_不把别处的冒号数字当端口() {
        let s = "Creating hunter-web-1 ... done\n\
                 image sha256:abc123 pulled in 12:34\n\
                 Error response from daemon: Bind for 0.0.0.0:8100 failed: port is already allocated";
        let b = parse_bind_failures(s);
        assert_eq!(b.len(), 1, "{b:?}");
        assert_eq!(b[0].port, 8100);
        assert_eq!(b[0].addr, "0.0.0.0");
    }

    #[test]
    fn up_的其他错误各归各位() {
        assert_eq!(
            classify_up_error(
                "write /var/lib/docker/tmp: no space left on device",
                Some(1)
            )
            .code,
            Code::PullFailed
        );
        assert_eq!(
            classify_up_error(
                "Error response from daemon: pull access denied for x",
                Some(1)
            )
            .code,
            Code::PullFailed
        );
        assert_eq!(
            classify_up_error("net/http: TLS handshake timeout", Some(1)).code,
            Code::PullFailed
        );
        assert_eq!(
            classify_up_error("dial tcp 1.2.3.4:443: i/o timeout", Some(1)).code,
            Code::PullFailed
        );
        assert_eq!(
            classify_up_error(
                "Cannot connect to the Docker daemon at unix:///var/run/docker.sock.",
                Some(1)
            )
            .code,
            Code::DaemonDown
        );
        // 认不出来的仍然是启动失败，但**要带上原话**，不能只说「超时」
        let e = classify_up_error("something weird", Some(17));
        assert_eq!(e.code, Code::StartTimeout);
        assert!(e.msg.contains("something weird"), "{}", e.msg);
        assert!(e.msg.contains("17"), "{}", e.msg);
    }

    #[test]
    fn pull_归类里_pull_access_denied_有自己的一句话() {
        let s = classify_pull_error(
            "Error response from daemon: pull access denied for ghcr.io/x/y, repository does not exist",
            Some(1),
        );
        assert!(s.contains("不让拉"), "{s}");
    }

    #[test]
    fn ps_解析用的是_m0_实测的真实一行() {
        let line = r#"{"Name":"hunter-api-1","Service":"api","State":"running","Health":"healthy","ExitCode":0,"Publishers":[{"TargetPort":8000,"PublishedPort":8101,"URL":"0.0.0.0"},{"TargetPort":8000,"PublishedPort":8101,"URL":"::"}]}"#;
        let v = parse_ps(line);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].service, "api");
        assert_eq!(v[0].health, Health::Healthy);
        // M0 §5.3 坑 1：IPv4/IPv6 两条要去重成一个端口
        assert_eq!(v[0].port, Some(8101));
        assert!(service_ready(&v[0]));
    }

    #[test]
    fn 没有容器时空输出不算错() {
        assert!(parse_ps("").is_empty());
        assert!(parse_ps("\n \n").is_empty());
    }

    #[test]
    fn llm_shim_不发布端口时端口为_null() {
        let line = r#"{"Service":"llm-shim","State":"running","Health":"healthy","Publishers":[{"TargetPort":3999,"PublishedPort":0,"URL":""}]}"#;
        let v = parse_ps(line);
        assert_eq!(
            v[0].port, None,
            "PublishedPort 为 0 要显示「内部」而不是端口 0（M0 §5.3 坑 3）"
        );
    }

    #[test]
    fn 按视觉稿的顺序排而不是字母序() {
        let lines = ["redis", "postgres", "llm-shim", "opencode", "api", "web"]
            .map(|s| {
                format!("{{\"Service\":\"{s}\",\"State\":\"running\",\"Health\":\"healthy\"}}")
            })
            .join("\n");
        let v = parse_ps(&lines);
        assert_eq!(
            v.iter().map(|s| s.service.as_str()).collect::<Vec<_>>(),
            vec!["web", "api", "opencode", "llm-shim", "postgres", "redis"],
            "视觉稿第 3 张就是这个顺序"
        );
        // 不认识的服务排到最后，不会把已知的挤乱
        let extra = format!("{lines}\n{{\"Service\":\"zzz\",\"State\":\"running\"}}");
        let v = parse_ps(&extra);
        assert_eq!(v.last().unwrap().service, "zzz");
    }

    #[test]
    fn 多行与数组两种格式都能吃() {
        let multi = "{\"Service\":\"web\",\"State\":\"running\",\"Health\":\"healthy\"}\n{\"Service\":\"redis\",\"State\":\"running\",\"Health\":\"starting\"}";
        let v = parse_ps(multi);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].service, "web", "按视觉稿顺序：web 排在 redis 前面");
        let arr = r#"[{"Service":"web","State":"running","Health":"healthy"}]"#;
        assert_eq!(parse_ps(arr).len(), 1);
    }

    /// I7：绑定地址要从 docker 自己报的 `Publishers[].URL` 读出来。
    /// 下面三行的形状取自测试机上 `docker compose ps --format json` 的真实输出
    /// （旧栈 hunter-community，2026-09-21 实测）。
    #[test]
    fn 读得出端口实际绑在哪个地址() {
        // 对外：IPv4 与 IPv6 各一条
        let lan = r#"{"Service":"api","State":"running","Health":"healthy","Publishers":[{"URL":"0.0.0.0","TargetPort":8000,"PublishedPort":8100,"Protocol":"tcp"},{"URL":"::","TargetPort":8000,"PublishedPort":8100,"Protocol":"tcp"}]}"#;
        let v = parse_ps(lan);
        assert_eq!(v[0].port, Some(8100));
        assert_eq!(v[0].bind.as_deref(), Some("0.0.0.0"));
        assert!(v[0].lan_exposed(), "0.0.0.0 就是对外");

        // 收回本机之后只有一条
        let local = r#"{"Service":"api","State":"running","Health":"healthy","Publishers":[{"URL":"127.0.0.1","TargetPort":8000,"PublishedPort":8100,"Protocol":"tcp"}]}"#;
        let v = parse_ps(local);
        assert_eq!(v[0].bind.as_deref(), Some("127.0.0.1"));
        assert!(!v[0].lan_exposed());

        // llm-shim 不发布端口：URL 是空串、PublishedPort 是 0（M0 §5.3 坑 3）
        let none = r#"{"Service":"llm-shim","State":"running","Health":"healthy","Publishers":[{"URL":"","TargetPort":3999,"PublishedPort":0,"Protocol":"tcp"}]}"#;
        let v = parse_ps(none);
        assert_eq!(v[0].port, None);
        assert_eq!(v[0].bind, None);
        assert!(!v[0].lan_exposed(), "没有端口就谈不上对外");

        // 老版本 compose 里可能整个 URL 字段都没有 —— 那就是「读不到」，不猜（红线 1）
        let nourl = r#"{"Service":"web","State":"running","Health":"healthy","Publishers":[{"TargetPort":3000,"PublishedPort":3101}]}"#;
        let v = parse_ps(nourl);
        assert_eq!(v[0].port, Some(3101));
        assert_eq!(v[0].bind, None);
        assert!(!v[0].lan_exposed(), "读不到就不喊狼来了");
    }

    #[test]
    fn 没有健康检查但在跑也算就绪() {
        let v = parse_ps(r#"{"Service":"x","State":"running","Health":""}"#);
        assert_eq!(v[0].health, Health::None);
        assert!(service_ready(&v[0]));
        let v = parse_ps(r#"{"Service":"x","State":"exited","Health":""}"#);
        assert!(!service_ready(&v[0]));
    }

    /// compose 在 --progress json 下把报错也塞进 JSON 行，纯文本行可能一条都没有。
    /// 只挑非 JSON 行的话用户看到的是「原话：」后面空一片（M2 用例 6c 实测撞出来的）。
    #[test]
    fn 报错藏在_json_行里时也要能挑出来() {
        let lines: Vec<String> = vec![
            r#"{"id":"Image hkccr.ccs.tencentyun.com/agentpit/hunter-community-web:1.2.0","status":"Error","text":"Error","details":"failed to resolve reference: dial tcp: connect: connection refused"}"#.into(),
            r#"{"id":"abc","parent_id":"Image x","text":"Downloading","current":1,"total":2}"#.into(),
        ];
        let t = extract_error_text(&lines);
        assert!(t.contains("connection refused"), "{t}");

        // 纯文本与 JSON 都有时**两个都要**，纯文本排前面 ——
        // 少收哪一个都可能让「原话」是空的（I6）
        let mixed: Vec<String> = vec![
            r#"{"id":"x","text":"Error","details":"json 里的原因"}"#.into(),
            "Error response from daemon: 明文原因".into(),
        ];
        let t = extract_error_text(&mixed);
        assert!(t.starts_with("Error response from daemon: 明文原因"), "{t}");
        assert!(t.contains("json 里的原因"), "{t}");

        // 一条错误都没有时返回空串，classify 会退回「退出码 N」的说法
        let clean: Vec<String> = vec![r#"{"id":"a","text":"Pulling"}"#.into()];
        assert_eq!(extract_error_text(&clean), "");
    }

    /// **0.1.5 在用户 Mac 上那一行空白的「原话：」就是这里漏的**。
    ///
    /// 下面这三行是测试机上 `docker compose --progress json pull` 的实测原文
    /// （`DOCKER_CONFIG` 里写了一个找不到的 credsStore，compose v5.5.1 / Docker 29.8.1）：
    /// 错误在**顶层** `{"error":true,"message":"…"}` 里，既没有 `text` 也没有 `status`。
    #[test]
    fn compose_v5_的顶层错误必须挑得出来() {
        let lines: Vec<String> = vec![
            r#"{"id":"Image ghcr.io/agentpit-io/hunter-community-api:1.2.0","status":"Working","text":"Pulling"}"#.into(),
            r#"{"error":true,"message":"error getting credentials - err: exec: \"docker-credential-osxkeychain\": executable file not found in $PATH, out: ``"}"#.into(),
        ];
        let t = extract_error_text(&lines);
        assert!(!t.trim().is_empty(), "「原话」不许是空的");
        assert!(t.contains("docker-credential-osxkeychain"), "{t}");
        // 归类要落到 E_CRED_HELPER，而不是「拉取失败 → 换个源」
        assert_eq!(pull_error_code(&t), Code::CredHelper);
        let msg = classify_pull_error(&t, Some(1));
        assert!(msg.contains("凭据助手"), "{msg}");
        assert!(msg.contains("换源修不好"), "要点破这一条与源无关：{msg}");
        assert!(
            msg.contains("docker-credential-osxkeychain"),
            "原话要带上：{msg}"
        );
    }

    /// docker 原生流的两种错误形状（`errorDetail` / `error` 是字符串）也要认。
    #[test]
    fn docker_原生流的错误形状也认() {
        let a: Vec<String> = vec![
            r#"{"errorDetail":{"message":"manifest unknown"},"error":"manifest unknown"}"#.into(),
        ];
        assert!(extract_error_text(&a).contains("manifest unknown"));
        let b: Vec<String> = vec![r#"{"error":"toomanyrequests: rate limited"}"#.into()];
        assert!(extract_error_text(&b).contains("rate limited"));
    }

    /// 一个字的错误都没输出时，**不许**给用户留一句「原话：」的半截话。
    #[test]
    fn 两个流都没说话时也要说人话() {
        let msg = classify_pull_error("", Some(1));
        assert!(!msg.ends_with("原话："), "{msg}");
        assert!(msg.contains("一个字的错误都没有"), "{msg}");
        assert!(msg.contains('1'), "退出码要带上：{msg}");
    }

    /// 凭据助手的三条判据。
    #[test]
    fn 凭据助手的判据() {
        assert!(is_cred_helper_error(
            "error getting credentials - err: exec: \"docker-credential-desktop.exe\": executable file not found in %PATH%"
        ));
        assert!(is_cred_helper_error(
            "docker-credential-secretservice not installed"
        ));
        assert!(!is_cred_helper_error(
            "failed to resolve reference: dial tcp: i/o timeout"
        ));
        // 「找不到 docker-compose」是另一回事，别抢它的归类
        assert!(!is_cred_helper_error(
            "docker: 'compose' is not a docker command, executable file not found: docker-compose"
        ));
    }

    /// `up` 的时候撞上同一件事，也要归到 `E_CRED_HELPER`。
    #[test]
    fn up_撞上凭据助手也归到_cred_helper() {
        let e = classify_up_error(
            "Error response from daemon: error getting credentials - err: exec: \"docker-credential-osxkeychain\": executable file not found in $PATH",
            Some(1),
        );
        assert_eq!(e.code, Code::CredHelper);
        assert!(e.msg.contains("下载源无关"), "{}", e.msg);
    }

    #[test]
    fn 拉取失败的分类文案能照着办() {
        assert!(
            classify_pull_error("Error: dial tcp: lookup ghcr.io: no such host", Some(1))
                .contains("DNS")
        );
        assert!(classify_pull_error("net/http: TLS handshake timeout", Some(1)).contains("超时"));
        assert!(classify_pull_error(
            "denied: requested access to the resource is denied",
            Some(1)
        )
        .contains("拒绝访问"));
        assert!(
            classify_pull_error("write /var/lib/docker: no space left on device", Some(1))
                .contains("磁盘满")
        );
        assert!(classify_pull_error("unexpected EOF", Some(1)).contains("连接被中断"));
        // 认不出来的也要把原话带上，别吞掉
        let s = classify_pull_error("something weird happened", Some(17));
        assert!(s.contains("17") && s.contains("something weird"), "{s}");
    }

    #[test]
    fn 超时文案要点出是哪个服务没就绪() {
        // 直接验文案拼装：wait_healthy 里那段的等价构造
        let last = parse_ps("{\"Service\":\"web\",\"State\":\"running\",\"Health\":\"healthy\"}\n{\"Service\":\"opencode\",\"State\":\"running\",\"Health\":\"starting\"}");
        let expected = ["api", "opencode", "web"];
        let not_ready: Vec<String> = expected
            .iter()
            .filter(|e| !last.iter().any(|s| s.service == **e && service_ready(s)))
            .map(|e| e.to_string())
            .collect();
        assert_eq!(not_ready, vec!["api".to_string(), "opencode".to_string()]);
    }

    /// 安装阶段失败的原因不止「拉不下来」一种。
    ///
    /// I4 之前 `PullProgress` 只有一段错误文字，前端把**任何**安装失败都当成
    /// `PULL_FAILED` 发给状态机 —— 于是「另一个工作目录占着 compose 项目名」
    /// 在界面上显示成「镜像拉取失败」：标题和正文互相打架，
    /// I4 的诊断助手还据此给出「换个镜像源」这种完全跑偏的建议
    /// （本轮取错误页截图时当场撞出来的）。
    #[test]
    fn 进度快照带得动真实错误码() {
        let mut p = PullProgress::empty();
        assert_eq!(p.error_code, None, "没出错时不该有码");

        // commands.rs 里安装失败那一支写的就是这两行
        p.phase = PullPhase::Failed;
        p.error = Some("这台机器上已经有一套 Hunter 在用 compose 项目名「hunter」".into());
        p.error_code = Some(crate::err::Code::ProjectConflict.as_str().to_string());
        assert_eq!(p.error_code.as_deref(), Some("E_PROJECT_CONFLICT"));

        // 序列化成 camelCase 给前端（前端按 errorCode 决定发哪个事件）
        let j = serde_json::to_value(&p).expect("能序列化");
        assert_eq!(j["errorCode"], "E_PROJECT_CONFLICT");
        assert_eq!(j["phase"], "failed");
    }

    #[test]
    fn 项目名固定为_hunter_且参数顺序正确() {
        assert_eq!(PROJECT, "hunter");
        let (program, args) = full_args(
            "a.yml",
            "b.yml",
            "/tmp/app",
            &["--progress", "json", "pull"],
        );
        // I4 起程序名是**定位出来的绝对路径**（定位不到就退回裸名字）
        assert!(
            program.ends_with("docker")
                || program.ends_with("docker.exe")
                || program.ends_with("docker-compose"),
            "{program}"
        );
        // 插件形态下 compose 子命令还在；独立 docker-compose 形态下没有它。
        // 跑测试的这台机器上是哪一种不由测试决定，所以把它剥掉之后再比后面的固定部分。
        let rest: Vec<&str> = args
            .iter()
            .map(String::as_str)
            .skip_while(|a| *a == "compose")
            .collect();
        assert_eq!(
            rest,
            vec![
                "--project-name",
                "hunter",
                "--project-directory",
                "/tmp/app",
                "-f",
                "a.yml",
                "-f",
                "b.yml",
                "--progress",
                "json",
                "pull"
            ]
        );
        // --progress 必须在 pull 之前（它是 compose 的全局参数，不是 pull 的）
        let pi = args.iter().position(|a| a == "pull").unwrap();
        let gi = args.iter().position(|a| a == "--progress").unwrap();
        assert!(gi < pi);
        // 覆盖文件必须排在基础文件后面
        let f: Vec<&str> = args
            .iter()
            .map(String::as_str)
            .filter(|a| a.ends_with(".yml"))
            .collect();
        assert_eq!(f, vec!["a.yml", "b.yml"]);
    }

    // ── 项目名归属（待办池 P0-5） ─────────────────────────────────────────

    const OURS: &str = "/home/u/.hunter/app/docker-compose.yml";
    const OUR_DIR: &str = "/home/u/.hunter/app";

    fn line(name: &str, files: &str, dir: &str) -> String {
        format!("{name}|{files}|{dir}\n")
    }

    #[test]
    fn 自己这套不算冲突() {
        let out = line(
            "hunter-web-1",
            "/home/u/.hunter/app/docker-compose.yml,/home/u/.hunter/app/docker-compose.launcher.yml",
            OUR_DIR,
        );
        assert_eq!(foreign_owner(&out, OURS, OUR_DIR), None);
    }

    #[test]
    fn 另一个工作目录会被认出来() {
        let out = line(
            "hunter-web-1",
            "/home/u/.hunter-b/app/docker-compose.yml,/home/u/.hunter-b/app/docker-compose.launcher.yml",
            "/home/u/.hunter-b/app",
        );
        let o = foreign_owner(&out, OURS, OUR_DIR).expect("这是别人的，必须认出来");
        assert_eq!(o.container, "hunter-web-1");
        assert_eq!(o.working_dir, "/home/u/.hunter-b/app");
        // 报错文案里要给出对方的 HUNTER_HOME，不是 app 子目录
        assert_eq!(o.hunter_home(), "/home/u/.hunter-b");
    }

    #[test]
    fn 一个容器沾边就算我们的() {
        // compose 文件对得上、project-directory 被用户手工改过 —— 仍然是我们这套
        let out = line("hunter-api-1", OURS, "/somewhere/else");
        assert_eq!(foreign_owner(&out, OURS, OUR_DIR), None);
    }

    #[test]
    fn 路径末尾多一个斜杠不算冲突() {
        let out = line(
            "hunter-web-1",
            "/home/u/.hunter/app/docker-compose.yml",
            OUR_DIR,
        );
        assert_eq!(
            foreign_owner(
                &out,
                "/home/u/.hunter/app/docker-compose.yml",
                "/home/u/.hunter/app/"
            ),
            None
        );
        let out2 = line("hunter-web-1", "x.yml", "/home/u/.hunter/app/");
        assert_eq!(foreign_owner(&out2, OURS, OUR_DIR), None);
    }

    #[test]
    fn 没有容器时不算冲突() {
        assert_eq!(foreign_owner("", OURS, OUR_DIR), None);
        assert_eq!(foreign_owner("\n  \n", OURS, OUR_DIR), None);
    }

    #[test]
    fn 读不到标签的容器一律放行() {
        // 老版本 docker 或者不是 compose 起的容器：两个标签都是空串。
        // 宁可漏报也不能误报 —— 误报会把正常安装整个拦死
        assert_eq!(foreign_owner("某个容器||\n", OURS, OUR_DIR), None);
    }

    #[test]
    fn 混着的时候也认得出外来的那个() {
        let mut out = line("hunter-web-1", OURS, OUR_DIR);
        out.push_str(&line(
            "hunter-api-1",
            "/opt/other/docker-compose.yml",
            "/opt/other",
        ));
        let o = foreign_owner(&out, OURS, OUR_DIR).expect("第二行是外来的");
        assert_eq!(o.container, "hunter-api-1");
        // 反推不出 <home>/app 结构时退回原值，不编一个不存在的路径
        assert_eq!(o.hunter_home(), "/opt/other");
    }

    #[test]
    fn windows_路径也能反推出工作目录() {
        let o = ProjectOwner {
            container: "hunter-web-1".into(),
            config_files: "C:\\Users\\u\\.hunter\\app\\docker-compose.yml".into(),
            working_dir: "C:\\Users\\u\\.hunter\\app".into(),
        };
        assert_eq!(o.hunter_home(), "C:\\Users\\u\\.hunter");
    }

    #[test]
    fn 格式串里要同时有两个归属标签() {
        assert!(OWNER_FORMAT.contains("com.docker.compose.project.config_files"));
        assert!(OWNER_FORMAT.contains("com.docker.compose.project.working_dir"));
        // 分隔符不能换成制表符或空格：路径里有空格是常事
        assert!(OWNER_FORMAT.contains('|'));
    }

    // ── 绑定漂移（I11 · U5）────────────────────────────────────────────

    fn svc(name: &str, bind: Option<&str>, port: Option<u16>, state: &str) -> ServiceStatus {
        ServiceStatus {
            service: name.into(),
            state: state.into(),
            health: Health::Healthy,
            port,
            bind: bind.map(str::to_string),
            exit_code: None,
        }
    }

    /// 用户 Mac 上 22:01–22:50 那 50 分钟的现场：有人在 `~/.hunter/app` 里
    /// 直接 `docker compose -p hunter up -d`，漏了覆盖文件，
    /// 容器被重建成 0.0.0.0，局域网里的机器连得上 5442。
    #[test]
    fn 容器被重建成对外时要认出来() {
        let v = vec![
            svc("web", Some("127.0.0.1"), Some(3100), "running"),
            svc("api", Some("0.0.0.0"), Some(8100), "running"),
            svc("postgres", Some("0.0.0.0"), Some(5442), "running"),
        ];
        let d = bind_drift(&v);
        let names: Vec<&str> = d.iter().map(|x| x.service.as_str()).collect();
        assert_eq!(names, vec!["api", "postgres"], "{d:?}");
        assert!(d[0].human().contains("局域网"), "{}", d[0].human());
    }

    /// 没跑的容器不算 —— 它的端口没在监听，谈不上「对外开着」。
    #[test]
    fn 停着的容器不算漂移() {
        let v = vec![svc("api", Some("0.0.0.0"), Some(8100), "exited")];
        assert!(bind_drift(&v).is_empty());
    }

    /// 读不到绑定地址的时候**不喊狼来了**（红线 1：不知道就说不知道）。
    #[test]
    fn 读不到绑定地址就不算漂移() {
        let v = vec![svc("api", None, Some(8100), "running")];
        assert!(bind_drift(&v).is_empty());
    }

    /// 没有漂移的时候 [`fix_bind_drift`] 一个子进程都不起。
    #[test]
    fn 没有漂移就什么都不做() {
        fix_bind_drift(&[]).expect("空表必须是立刻成功、什么都不做");
    }
}
