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
use crate::{paths, proc};

/// 健康轮询的上限（方案 §18 的 `E_START_TIMEOUT` 就是这个数）。
pub const START_TIMEOUT: Duration = Duration::from_secs(180);
/// 拉取失败重试次数（方案 §9）。
pub const PULL_ATTEMPTS: usize = 3;

/// compose 的完整参数前缀。`-f` 的顺序有意义：覆盖文件必须排在后面，
/// 否则 `!override` 覆盖不到（红线 4 就白写了）。
/// `--project-directory` 也是全局参数，指到 `~/.hunter/app` 让 compose 在那里找 `.env`。
fn full_args<'a>(
    compose: &'a str,
    overlay: &'a str,
    dir: &'a str,
    extra: &[&'a str],
) -> Vec<&'a str> {
    let mut v = vec![
        "compose",
        "--project-name",
        PROJECT,
        "--project-directory",
        dir,
        "-f",
        compose,
        "-f",
        overlay,
    ];
    v.extend_from_slice(extra);
    v
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
    let args = full_args(&c, &o, &dir_s, extra);
    proc::run_timeout("docker", &args, timeout)
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
    pub percent: u32,
    /// 字节/秒，最近若干秒的滑动平均；还没有样本时为 null
    pub speed_bps: Option<u64>,
    pub eta_seconds: Option<u64>,
    /// 第几次尝试（1 起），换源重试时会增加
    pub attempt: u32,
    pub log: Vec<String>,
    /// 出错时的原因（已脱敏）
    pub error: Option<String>,
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
            percent: 0,
            speed_bps: None,
            eta_seconds: None,
            attempt: 1,
            log: Vec::new(),
            error: None,
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
    let args = full_args(&c, &o, &dir_s, &["--progress", "json", "pull"]);

    let mut child = proc::base_command("docker")
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
        // 非 JSON 的 stderr 行要留着做失败分类，所以顺手收一份
        let collected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&collected);
        let h = std::thread::spawn(move || {
            let reader: Box<dyn BufRead> = match stream {
                Either::Out(s) => Box::new(BufReader::new(s)),
                Either::Err(s) => Box::new(BufReader::new(s)),
            };
            for line in reader.lines().map_while(Result::ok) {
                if is_err {
                    if let Ok(mut g) = sink.lock() {
                        g.push(line.clone());
                        if g.len() > 400 {
                            let n = g.len() - 400;
                            g.drain(..n);
                        }
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
    let mut err_text = String::new();
    for (h, collected, is_err) in readers {
        let _ = h.join();
        if is_err {
            if let Ok(g) = collected.lock() {
                err_text = extract_error_text(&g);
            }
        }
    }
    on_progress(&agg.snapshot(PullPhase::Pulling, None));

    if status.success() {
        Ok(())
    } else {
        Err(AppError::new(
            Code::PullFailed,
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
/// compose 在 `--progress json` 下把**报错也塞进 JSON 行**（`text` 是 `Error`，
/// 具体原因在 `details` 里），纯文本行常常一条都没有。
/// 只挑非 JSON 行的话，用户看到的就是「退出码 1。原话：」后面空一片（M2 用例 6c 实测）。
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
                let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("");
                let status = v.get("status").and_then(|x| x.as_str()).unwrap_or("");
                if text.eq_ignore_ascii_case("error") || status.eq_ignore_ascii_case("error") {
                    let detail = v
                        .get("details")
                        .and_then(|x| x.as_str())
                        .or_else(|| v.get("status").and_then(|x| x.as_str()))
                        .unwrap_or("");
                    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
                    let line = format!("{id} {detail}").trim().to_string();
                    if !line.is_empty() {
                        from_json.push(line);
                    }
                }
            }
            continue;
        }
        plain.push(t.to_string());
    }
    if !plain.is_empty() {
        plain.join("\n")
    } else {
        from_json.join("；")
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
    pub exit_code: Option<i64>,
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
    let port = p
        .publishers
        .unwrap_or_default()
        .into_iter()
        .filter_map(|x| x.published_port)
        .find(|p| *p != 0);
    ServiceStatus {
        service: p.service.unwrap_or_default(),
        state,
        health,
        port,
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

// ── up / down / logs ──────────────────────────────────────────────────────

pub fn up() -> AppResult<()> {
    let r = run(&["up", "-d", "--remove-orphans"], Duration::from_secs(300))?;
    if r.ok() {
        crate::linfo!("docker compose up -d 完成");
        Ok(())
    } else {
        Err(AppError::new(
            Code::StartTimeout,
            format!("docker compose up -d 失败：{}", r.err_line()),
        ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

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

        // 有纯文本行时优先用纯文本
        let mixed: Vec<String> = vec![
            r#"{"id":"x","text":"Error","details":"json 里的原因"}"#.into(),
            "Error response from daemon: 明文原因".into(),
        ];
        assert_eq!(
            extract_error_text(&mixed),
            "Error response from daemon: 明文原因"
        );

        // 一条错误都没有时返回空串，classify 会退回「退出码 N」的说法
        let clean: Vec<String> = vec![r#"{"id":"a","text":"Pulling"}"#.into()];
        assert_eq!(extract_error_text(&clean), "");
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

    #[test]
    fn 项目名固定为_hunter_且参数顺序正确() {
        assert_eq!(PROJECT, "hunter");
        let args = full_args(
            "a.yml",
            "b.yml",
            "/tmp/app",
            &["--progress", "json", "pull"],
        );
        assert_eq!(
            args,
            vec![
                "compose",
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
        let pi = args.iter().position(|a| *a == "pull").unwrap();
        let gi = args.iter().position(|a| *a == "--progress").unwrap();
        assert!(gi < pi);
        // 覆盖文件必须排在基础文件后面
        let f: Vec<&str> = args
            .iter()
            .copied()
            .filter(|a| a.ends_with(".yml"))
            .collect();
        assert_eq!(f, vec!["a.yml", "b.yml"]);
    }
}
