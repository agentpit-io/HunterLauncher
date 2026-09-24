//! 配置与工作目录（技术方案 §5.3、§7）。
//!
//! 三个文件：
//!
//! | 文件 | 内容 | 权限 |
//! |---|---|---|
//! | `~/.hunter/launcher.toml` | 启动器自己的设置（语言、镜像源、tag、端口、模型模式）。**不含任何 key** | 默认 |
//! | `~/.hunter/app/.env` | Hunter 的配置，含 hunter key 与数据库口令 | **600** |
//! | `~/.hunter/app/docker-compose.launcher.yml` | 端口与镜像源覆盖 | 默认 |
//!
//! 红线 2：key 只进 `.env` 与内存。`launcher.toml` 里连自带模型 key 都不存 ——
//! 自带 key 模式只把 BASE_URL 与模型名记在 toml 里，key 本身同样只落 `.env`。

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::err::{AppError, AppResult, Code};
use crate::paths;
use crate::registry::Candidate;

/// 启动器内置的同版本 compose 副本（离线 / raw 被墙时兜底）。
pub const BUNDLED_TAG: &str = "1.2.0";
const BUNDLED_COMPOSE: &str =
    include_str!("../../templates/docker-compose.hunter-community-1.2.0.yml");
/// 内置副本的 sha256。下载回来的内容与它不一致时说明链路上有人动过手脚，宁可用内置的。
pub const BUNDLED_COMPOSE_SHA256: &str =
    "4732348f491779051bfab0783ea7b552a2b35314d051c0ea7c95cf5471162ee2";

const ENV_TEMPLATE: &str = include_str!("../../templates/env.template");

/// compose 项目名。总控规则要求固定为 `hunter`，与用户手工部署的 `hunter-community` 互不干扰。
pub const PROJECT: &str = "hunter";

/// compose 在 `COMPOSE_FILE` 里用哪个分隔符（I11 · U5）。
///
/// compose 自己的默认值是平台相关的（POSIX `:`、Windows `;`），
/// 我们**连 `COMPOSE_PATH_SEPARATOR` 一起写死**，不去赌它的默认值。
pub const fn compose_path_separator() -> &'static str {
    if cfg!(windows) {
        ";"
    } else {
        ":"
    }
}

/// `.env` 里 `COMPOSE_FILE` 的值：两份文件的**文件名**，顺序就是
/// [`crate::compose`] 里 `-f` 的顺序（覆盖文件必须排在后面）。
///
/// 写文件名而不是绝对路径：compose 按当前目录解析相对路径，
/// 而这一行存在的意义正是「用户 `cd ~/.hunter/app` 之后直接跑 compose」。
pub fn compose_file_value() -> String {
    format!(
        "docker-compose.yml{}docker-compose.launcher.yml",
        compose_path_separator()
    )
}

// ── launcher.toml ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ports {
    pub web: u16,
    pub api: u16,
    pub opencode: u16,
    pub postgres: u16,
    pub redis: u16,
}

impl Default for Ports {
    /// 默认端口按 hunter-community 的 compose 来（M0 §3.1 实测），不是方案里写的 8000/3901。
    fn default() -> Self {
        Self {
            web: 3100,
            api: 8100,
            opencode: 3921,
            postgres: 5442,
            redis: 6479,
        }
    }
}

impl Ports {
    pub fn as_pairs(&self) -> [(&'static str, u16); 5] {
        [
            ("web", self.web),
            ("api", self.api),
            ("opencode", self.opencode),
            ("postgres", self.postgres),
            ("redis", self.redis),
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LauncherSection {
    pub version: String,
    pub locale: String,
    pub autostart: bool,
    pub check_update_hours: u32,
}

impl Default for LauncherSection {
    fn default() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            locale: "zh-CN".into(),
            autostart: false,
            check_update_hours: 24,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HunterSection {
    pub tag: String,
    /// 镜像源标识（`ghcr` / `tencent` / `custom`）
    pub registry_id: String,
    /// 实际前缀。`registry_id == "custom"` 时这一项才是唯一的真值来源
    pub registry_prefix: String,
    pub base_prefix: String,
    pub ports: Ports,
    /// **这一项已经不再决定任何事，只是留着认得出老配置**（I7 · 待办池 P1-20 已关闭）。
    ///
    /// ## 为什么它从「开关」退成了「遗迹」
    ///
    /// I1 实测了 web 绑 `0.0.0.0` 的后果并记在报告第七节：从另一台机器打
    /// `http://<这台机器的 IP>:3101`，不需要任何凭证就能进聊天界面、
    /// 调工具、烧掉用户当天的额度（上游 api 保护住了 `/api/setup/*` 那些**配置**接口，
    /// 但没有保护**使用**界面）。I2 把开关做出来、默认值没动，问题挂到 P1-20 等用户拍板。
    ///
    /// 用户 2026-09-21 19:05 拍板了，原话：
    /// **「目前只能本机访问，不考虑同一局域网访问，这个需要升级付费版本才可以。」**
    ///
    /// 于是「绑哪里」不再是配置能决定的事：**新生成的覆盖文件里六个服务一律 `127.0.0.1`**。
    /// 真正的取值由 [`WebBind::detect`] 从**磁盘上那份覆盖文件的现状**读出来，
    /// 不从这一项读 —— 这样「手改 launcher.toml 重新放开」这条路在代码层面就不存在
    /// （[`render_override`] 也没有任何入口能渲染出 `0.0.0.0`）。
    ///
    /// 这一项保留的唯一理由是：老机器的 `launcher.toml` 里有它，
    /// 读不回来会让整份配置解析失败、把用户的其他设置一起丢掉。
    /// 取值不是 `local` 时会在日志里记一条「已忽略」（[`LauncherConfig::warn_legacy_web_bind`]）。
    #[serde(default = "default_web_bind")]
    pub web_bind: String,
    /// 健康检查的等待上限（秒）。方案 §18 的 180 秒是默认值；
    /// 慢机器上 AI 的 `raise_timeouts` 动作会把它调长（I5 动作表 v2）
    #[serde(default = "default_start_timeout")]
    pub start_timeout_secs: u64,
}

/// 方案 §18 的 `E_START_TIMEOUT` 就是这个数。
fn default_start_timeout() -> u64 {
    180
}

/// 新配置的默认值 = 只绑本机（I7 起）。
fn default_web_bind() -> String {
    WEB_BIND_LOCAL.into()
}

/// 老配置里「绑所有网卡」的写法。**只用来认出老机器**，不再有任何地方按它渲染。
pub const WEB_BIND_ALL: &str = "all";
/// web 只绑本机回环。免费版唯一会新生成的取值。
pub const WEB_BIND_LOCAL: &str = "local";

impl HunterSection {
    /// 健康检查等多久。手改成离谱的值时夹回 [180, 900]，**不照单全收**。
    pub fn start_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.start_timeout_secs.clamp(180, 900))
    }
}

impl Default for HunterSection {
    fn default() -> Self {
        Self {
            tag: BUNDLED_TAG.into(),
            registry_id: "ghcr".into(),
            registry_prefix: "ghcr.io/agentpit-io".into(),
            base_prefix: "docker.io/library".into(),
            ports: Ports::default(),
            web_bind: default_web_bind(),
            start_timeout_secs: default_start_timeout(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelemetrySection {
    /// 默认关。M0 实测 telemetry.agentpit.io 根本不存在，开了也没地方发
    pub enabled: bool,
    pub install_id: String,
    /// 上报端点。**默认空 = 不上报**，事件只进本地队列
    /// `~/.hunter/telemetry/queue.jsonl`。界面上如实写「暂未开启上报」（红线 1）。
    /// 留成可配置项是为了日后真建好服务时改一行配置就能用，不用改代码。
    #[serde(default)]
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSection {
    /// `gateway` | `own`
    pub mode: String,
    /// 自带 key 模式下的 BASE_URL 与模型名。**key 不在这里**（红线 2）
    pub base_url: String,
    pub model: String,
    pub schema_sanitize: bool,
}

impl Default for ModelSection {
    fn default() -> Self {
        Self {
            mode: "gateway".into(),
            base_url: String::new(),
            model: String::new(),
            schema_sanitize: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InstallSection {
    /// 走完一次完整安装（配置写好 + 容器起过）才置 true。
    /// 第二次打开启动器直接进运行面板靠的就是它
    pub done: bool,
    /// 上海时间字符串
    pub at: String,
    /// 镜像是**离线包导入**来的（方案 §9）。
    ///
    /// **必须落盘**，不能只放在进程内存里：`--import-images` 导完就退出了，
    /// 真正安装是下一个进程做的事，内存里的标志根本传不过去
    /// （M4 测试 5 第一次跑就是栽在这里 —— 导入明明成功了，安装还是去测速然后失败）。
    ///
    /// 为真时 [`crate::flow::prepare`] 跳过镜像源测速、[`crate::flow::pull`] 跳过拉取。
    ///
    /// **它是一次性的**：一轮拉取结束（不管是跳过的还是真拉的）就自动清掉。
    /// 语义因此和文档一致 ——「导入离线包 → 紧接着的这一次安装跳过拉取」。
    /// 留成永久标志的话，以后每次升级都可能悄悄跳过拉取（M4 收尾时撞到过，
    /// 见 `flow::clear_offline` 的注释）。
    #[serde(default)]
    pub offline: bool,

    // ── I12 · R1 新增的六项「装好了就记住」────────────────────────────────
    //
    // 起因：2026-09-22 23:10 用户 Mac 上 Hunter 6/6 健康跑着，重开启动器却又从
    // 欢迎页开始 —— `done` 只在安装流程**自己**走完时才写（flow.rs），
    // 而那一次是在外部修好的。同一份文件里 `[launcher] version` 还停在 0.1.7。
    //
    // 这六项合起来回答三个问题：谁装的（哪一版启动器、哪一版 Hunter）、
    // 装在哪（项目名、目录、用的哪套运行时）、上一次好好的是什么时候。
    /// **每次启动器起来都写成当前版本**。
    ///
    /// 老配置里的 `[launcher] version` 从 I12 起只是遗迹：读得到就迁移过来，
    /// 之后一律以这一项为准（方案第四节）。
    #[serde(default)]
    pub launcher_version: String,
    /// 装的是哪个 Hunter 镜像 tag（写入时 `hunter.tag` 的值）
    #[serde(default)]
    pub hunter_tag: String,
    /// compose 项目名。恒为 [`PROJECT`]，落盘是为了让人一眼看出「这套归启动器管」
    #[serde(default)]
    pub project: String,
    /// 工作目录（`~/.hunter/app`），家目录打码后的形式
    #[serde(default)]
    pub app_dir: String,
    /// 用的哪套运行时：`builtin` | `user` | `none`（[`crate::runtime::effective::Kind`]）
    #[serde(default)]
    pub runtime: String,
    /// **最近一次真的看到 6/6 健康**是什么时候（上海时间）。
    /// 每次健康检查通过都刷新；界面上「上次正常运行」那一行读的就是它
    #[serde(default)]
    pub last_healthy_at: String,
    /// 这条记录是「检测到它已经在跑」补写的，不是安装流程写的（R1）。
    ///
    /// 为真时过程流里会说一句「检测到 Hunter 已在运行，已记录为已安装」——
    /// 用户有权知道启动器凭什么认为装过了。
    #[serde(default)]
    pub adopted_from_running: bool,
    /// **是用户自己在界面上点的「停止」**（I12 · R3+）。
    ///
    /// R3+ 那一条自愈（「运行环境在跑、容器没起 → 自己拉起来」）必须绕开这一档：
    /// 用户刚点完停止、关掉窗口、过一会儿又打开启动器，启动器要是把容器又起回来，
    /// 那就是在跟他对着干。点「启动」时清掉。
    #[serde(default)]
    pub stopped_by_user: bool,
}

/// 本机数据备份（方案第四节 `[backup]`、R6 · I13）。
///
/// 默认值全部来自用户 2026-09-22 的原话：**每晚 00:00、目录自动建议、保留近 3 天**。
///
/// 三件事值得单说：
///
/// 1. **`dir` 默认是空的，不是一个写死的路径。** 空 = 「用这台机器上该用的那一个」
///    （[`BackupSection::effective_dir`]）。把建议值直接灌进配置文件的话，
///    用户换了机器、改了家目录，配置里那一行就变成一个指向不存在目录的谎话。
/// 2. **目录一定在 `~/.hunter` 之外。** 删除应用（R4）会把 `~/.hunter` 整个删掉，
///    备份放在里面等于「删的时候连救命的那一份一起删」。
/// 3. **`last_error` 与 `fail_streak` 落盘**：定时备份跑在一个**没有界面**的进程里
///    （`--backup --scheduled`），它失败的时候没有人在看。这两项是下一次打开
///    启动器时唯一能知道「上次出事了」的途径。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupSection {
    /// 自动备份开关。**默认开**
    pub enabled: bool,
    /// 每天几点（本机时区），`HH:MM`。默认 `00:00`
    pub time: String,
    /// 备份目录。**空 = 按平台自动建议**，见 [`BackupSection::effective_dir`]
    #[serde(default)]
    pub dir: String,
    /// 保留最近几天（按天保留最后一份）。默认 3，可改 1–30
    pub keep_days: u32,
    /// 会话记录卷（`hunter_opencode_data`）要不要一起打包。默认要
    pub include_sessions: bool,
    /// 自建技能卷（`hunter_user_skills`）要不要一起打包。默认要
    pub include_skills: bool,
    /// 最近一次成功的时间（上海时间）。空 = 从来没成功过
    #[serde(default)]
    pub last_ok_at: String,
    /// 最近一次失败的原因。空 = 上一次是成功的
    #[serde(default)]
    pub last_error: String,
    /// 最近一次**尝试**的时间（不管成没成）。判「有没有睡过头」用它
    #[serde(default)]
    pub last_run_at: String,
    /// 连续失败了几次。**到 2 就交给诊断助手**（R6 6.3）
    #[serde(default)]
    pub fail_streak: u32,
    /// 定时任务装没装（由 [`crate::schedule`] 写，只是个备忘；
    /// 「装没装」的真值一律现查系统，不读这一项，红线 1）
    #[serde(default)]
    pub schedule_installed: bool,
}

impl Default for BackupSection {
    fn default() -> Self {
        Self {
            enabled: true,
            time: "00:00".into(),
            dir: String::new(),
            keep_days: 3,
            include_sessions: true,
            include_skills: true,
            last_ok_at: String::new(),
            last_error: String::new(),
            last_run_at: String::new(),
            fail_streak: 0,
            schedule_installed: false,
        }
    }
}

impl BackupSection {
    /// 这台机器该用哪个备份目录。用户填了就用他填的，没填就用建议值。
    pub fn effective_dir(&self) -> std::path::PathBuf {
        let s = self.dir.trim();
        if s.is_empty() {
            return suggested_backup_dir();
        }
        expand_home(s)
    }

    /// `HH:MM` → （时, 分）。认不出来就退回 00:00 —— **不报错，也不猜别的时间**。
    pub fn hhmm(&self) -> (u8, u8) {
        parse_hhmm(&self.time).unwrap_or((0, 0))
    }

    /// 保留天数收进 1–30。配置文件被手改成 0 的时候不能真的一份都不留。
    pub fn keep_days_clamped(&self) -> u32 {
        self.keep_days.clamp(1, 30)
    }
}

/// `HH:MM` 解析。只认 24 小时制。
///
/// 方案写的是「可改为任意整点 / 半点」，这里**不拦** `07:15` 这类值 ——
/// 三个平台的定时机制本来就接受任意分钟，没有理由替用户砍掉。
pub fn parse_hhmm(s: &str) -> Option<(u8, u8)> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u8 = h.trim().parse().ok()?;
    let m: u8 = m.trim().parse().ok()?;
    (h < 24 && m < 60).then_some((h, m))
}

/// 把 `~/xxx` 展开成绝对路径。配置文件里写 `~` 是很自然的事。
pub fn expand_home(s: &str) -> std::path::PathBuf {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("~/") {
        return paths::home().join(rest);
    }
    if s == "~" {
        return paths::home();
    }
    std::path::PathBuf::from(s)
}

/// 按平台自动建议的备份目录（方案 R6 6.1 那张表）。
///
/// 三个平台都落在**用户文档目录**下，理由是同一条：
/// 它在 `~/.hunter` 之外（删除应用波及不到）、用户找得到、而且是他本来就会备份的地方。
pub fn suggested_backup_dir() -> std::path::PathBuf {
    let home = paths::home();
    #[cfg(target_os = "macos")]
    {
        home.join("Documents").join("Hunter 备份")
    }
    #[cfg(target_os = "windows")]
    {
        home.join("Documents").join("Hunter 备份")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        home.join("Hunter-backups")
    }
}

/// 资源监控的阈值（方案第四节 `[monitor]`）。
///
/// I12 做了 R2「看得见」那一半：这些阈值决定运行面板上哪个数字标黄、哪个标红。
/// I13 的 R7 接上「提醒 + 一键处理 + 交给诊断助手」，方案 R7 那张表里的
/// 七类监测项各自的阈值都在这里，**一条都不写死在代码里**。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorSection {
    /// 系统盘剩余低于这个数（GB）标黄
    pub host_disk_warn_gb: u64,
    /// 低于这个数标红
    pub host_disk_crit_gb: u64,
    /// 运行环境虚拟机磁盘用量超过这个百分比标黄
    pub vm_disk_warn_pct: u8,
    /// 超过这个百分比标红（R7 表：> 95%）
    #[serde(default = "d_vm_disk_crit")]
    pub vm_disk_crit_pct: u8,
    /// 运行环境虚拟机内存用量超过这个百分比标黄
    pub vm_mem_warn_pct: u8,
    /// 一个服务在 [`Self::restart_window_min`] 分钟内重启这么多次就提醒（R7 表：≥ 3）
    #[serde(default = "d_restart_warn")]
    pub restart_warn_count: u32,
    /// 上面那个窗口有多长（分钟）。R7 表写的是 1 小时
    #[serde(default = "d_restart_window")]
    pub restart_window_min: u32,
    /// 数据库单日增长超过这么多 GB 就提醒（R7 表：> 1GB）
    #[serde(default = "d_db_growth")]
    pub db_growth_warn_gb: u64,
    /// 备份目录所在盘至少要放得下几份备份，放不下就提醒（R7 表：剩余 < 3 份备份大小）
    #[serde(default = "d_backup_copies")]
    pub backup_disk_min_copies: u32,
    /// 同一个问题多少小时内不重复打扰（R7：24 小时）
    #[serde(default = "d_quiet_hours")]
    pub quiet_hours: u32,
    /// 允不允许弹系统通知（严重级才弹）
    pub notify: bool,
}

fn d_vm_disk_crit() -> u8 {
    95
}
fn d_restart_warn() -> u32 {
    3
}
fn d_restart_window() -> u32 {
    60
}
fn d_db_growth() -> u64 {
    1
}
fn d_backup_copies() -> u32 {
    3
}
fn d_quiet_hours() -> u32 {
    24
}

impl Default for MonitorSection {
    fn default() -> Self {
        Self {
            host_disk_warn_gb: 20,
            host_disk_crit_gb: 5,
            vm_disk_warn_pct: 80,
            vm_disk_crit_pct: d_vm_disk_crit(),
            vm_mem_warn_pct: 85,
            restart_warn_count: d_restart_warn(),
            restart_window_min: d_restart_window(),
            db_growth_warn_gb: d_db_growth(),
            backup_disk_min_copies: d_backup_copies(),
            quiet_hours: d_quiet_hours(),
            notify: true,
        }
    }
}

/// 可执行文件的定位策略（I4 的 P0）。
///
/// 为什么要有这一段：macOS 上从访达 / 程序坞启动的 GUI 程序，PATH 只有
/// `/usr/bin:/bin:/usr/sbin:/sbin`，**不含 `/usr/local/bin`**，于是基于 PATH 的
/// `docker` 查找必然失败 —— 哪怕用户的 OrbStack 好好装着。详见
/// [`crate::runtime::which`] 的模块注释。
///
/// 内置的已知位置清单写在 [`crate::runtime::which::builtin_dirs`]，**只写那一处**；
/// 这一段是给用户/排障用的覆盖入口。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSection {
    /// 手动指定 docker 可执行文件的绝对路径。非空时**只认它**，找不到就如实报错，
    /// 不会悄悄回退到别处的 docker。默认空 = 自动探测
    #[serde(default)]
    pub docker_path: String,
    /// 手动指定独立 `docker-compose` 的绝对路径。默认空 = 自动探测
    /// （而且绝大多数机器上用的是 `docker compose` 插件，根本用不到这一项）
    #[serde(default)]
    pub compose_path: String,
    /// 追加到内置清单**前面**的搜索目录。支持 `~/` 开头
    #[serde(default)]
    pub search_paths: Vec<String>,
    /// 要不要用内置的已知位置清单。默认 true
    #[serde(default = "yes")]
    pub use_builtin_paths: bool,
    /// 要不要走 PATH。默认 true
    #[serde(default = "yes")]
    pub use_env_path: bool,
    /// 用 `~/.hunter/docker-config/` 里那份**不带凭据助手**的 docker 配置
    /// （通过 `DOCKER_CONFIG` 指过去）。默认 false = 用用户自己的 `~/.docker`。
    ///
    /// 只有在「PATH 补全之后仍然找不到 `docker-credential-*`」时才由
    /// `use_isolated_docker_config` 这个动作打开（I6）。**用户那份配置一个字节都不动。**
    #[serde(default)]
    pub isolated_docker_config: bool,
    /// 「这台机器上没有 Docker」时走哪条路（I7）。
    ///
    /// * `builtin`（默认）—— 内置运行时（Colima + Lima + docker CLI + compose），
    ///   全程用户态、零点击、可一键卸载
    /// * `orbstack` —— OrbStack 官方 dmg。装得更快，但**首次启动会弹系统提示**
    ///   （欢迎页 / 管理员密码装辅助程序），做不到零点击，所以不是默认
    ///
    /// 认不得的值一律按 `builtin` 处理。
    #[serde(default = "default_install_route")]
    pub install_route: String,
}

fn yes() -> bool {
    true
}

fn default_install_route() -> String {
    "builtin".into()
}

/// 没有 Docker 时装哪一套。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallRoute {
    /// 内置运行时（默认）
    Builtin,
    /// OrbStack 官方 dmg（备选）
    OrbStack,
}

impl InstallRoute {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "orbstack" => InstallRoute::OrbStack,
            _ => InstallRoute::Builtin,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            InstallRoute::Builtin => "builtin",
            InstallRoute::OrbStack => "orbstack",
        }
    }
    pub fn cn(self) -> &'static str {
        match self {
            InstallRoute::Builtin => "内置运行时（零点击）",
            InstallRoute::OrbStack => "OrbStack 官方安装包（首次启动需要你点几下）",
        }
    }
}

impl Default for RuntimeSection {
    fn default() -> Self {
        Self {
            docker_path: String::new(),
            compose_path: String::new(),
            search_paths: Vec::new(),
            use_builtin_paths: true,
            use_env_path: true,
            isolated_docker_config: false,
            install_route: default_install_route(),
        }
    }
}

impl RuntimeSection {
    pub fn route(&self) -> InstallRoute {
        InstallRoute::parse(&self.install_route)
    }
    pub fn to_policy(&self) -> crate::runtime::which::Policy {
        crate::runtime::which::Policy {
            docker_path: self.docker_path.clone(),
            compose_path: self.compose_path.clone(),
            extra_dirs: self.search_paths.clone(),
            use_builtin: self.use_builtin_paths,
            use_env_path: self.use_env_path,
        }
    }
}

/// AI 诊断助手的开关（I4 §三）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistSection {
    /// 默认**开**。关掉之后只用第一层的确定性规则，一个 token 也不花
    #[serde(default = "yes")]
    pub enabled: bool,
    /// 授权档位：`auto` 全自动 / `confirm` 逐步确认 / `off` 关闭。
    ///
    /// **I8 起默认是 `auto`。** I5–I7 的默认是 `confirm`，理由写的是
    /// 「不能靠一个默认值替用户同意自动驾驶」。用户 2026-09-21 22:35 把这个
    /// 权衡改了：
    ///
    /// > 需要支持全自动的安装、问题修改，不要让用户参与决策和点击确认和执行。
    ///
    /// 「同意」这件事没有消失，只是挪了位置：授权页仍然要用户亲手点一次
    /// 「开始安装」，那一次点击就是授权 —— 页面上写清了 AI 会做什么、绝不做什么。
    /// 换句话说，**默认值替他同意的不是「要不要装」，而是「装的过程中不再打扰他」**。
    /// 想回到逐条确认的，设置页里还有 `confirm` 那一档。
    #[serde(default = "default_assist_mode")]
    pub mode: String,
    /// 用户是哪一刻做的授权（上海时间）。没授权过就是空
    #[serde(default)]
    pub consented_at: String,
    /// 一次授权页上那一项**默认勾选**的勾（I7）：
    /// 「如果电脑上没有 Docker，允许 AI 为你安装（装在 `~/.hunter/runtime` 里，
    /// 不改系统，可一键卸载）」。
    ///
    /// I8 起它覆盖的范围扩大到**整条兜底链**：内置运行时、OrbStack 官方安装包、
    /// Homebrew。文案里写明了「可能安装 Homebrew / OrbStack，需要时会弹系统密码框」。
    ///
    /// 全自动档下动作本来就不再逐条问用户（见 [`crate::assist::guard::Mode::needs_confirm`]），
    /// 这一项在那一档下的作用是**留痕**：事件流里会出一张「为什么这一步没问你」的卡片。
    /// 「逐步确认」档下它仍然决定 `install_runtime` 要不要弹确认。
    ///
    /// 没勾 → 启动器不装任何软件，没有 Docker 就如实说装不了。
    #[serde(default = "yes")]
    pub allow_install_runtime: bool,
}

fn default_assist_mode() -> String {
    "auto".into()
}

impl AssistSection {
    /// 解析成档位。`enabled = false` 一律按 `off` 处理 —— 老配置里只有这个开关。
    pub fn mode(&self) -> crate::assist::guard::Mode {
        if !self.enabled {
            return crate::assist::guard::Mode::Off;
        }
        crate::assist::guard::Mode::parse(&self.mode)
    }
    /// 用户做过一次授权了吗（向导要据此决定跳不跳授权页）。
    pub fn consented(&self) -> bool {
        !self.consented_at.is_empty()
    }
}

impl Default for AssistSection {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: default_assist_mode(),
            consented_at: String::new(),
            allow_install_runtime: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LauncherConfig {
    #[serde(default)]
    pub launcher: LauncherSection,
    #[serde(default)]
    pub hunter: HunterSection,
    #[serde(default)]
    pub telemetry: TelemetrySection,
    #[serde(default)]
    pub model: ModelSection,
    #[serde(default)]
    pub install: InstallSection,
    #[serde(default)]
    pub runtime: RuntimeSection,
    #[serde(default)]
    pub assist: AssistSection,
    #[serde(default)]
    pub takeover: TakeoverSection,
    #[serde(default)]
    pub monitor: MonitorSection,
    #[serde(default)]
    pub backup: BackupSection,
}

/// 「直接用这台机器上已经有的那一套 Hunter」（I7 · `reuse_existing_hunter`）。
///
/// 默认是**并存**（`project` 为空）—— 新装的换一组空闲端口，两套互不影响。
/// 只有用户在「需要你」卡片上亲手点了「直接用它，不再装一套」，这里才会有值。
///
/// 接管之后启动器变成那一套的**管理面板**：看状态、看日志、打开网页。
/// 升级 / 停止 / `down` 这些改动类操作**一律先二次确认**，
/// 而且任何情况下都不删它的卷（[`crate::takeover`] 里有硬校验，不是靠自觉）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TakeoverSection {
    /// 被接管的 compose 项目名。空 = 没有接管任何东西（默认）
    #[serde(default)]
    pub project: String,
    /// 它的工作目录（compose 文件所在）。读不出来就是空 —— 那种情况降级成只读监控
    #[serde(default)]
    pub working_dir: String,
    /// 它的 compose 文件（逗号分隔的绝对路径，来自容器标签）
    #[serde(default)]
    pub config_files: String,
    /// 它的 web 端口。读不到就是 0
    #[serde(default)]
    pub web_port: u16,
    /// 什么时候接管的（上海时间）
    #[serde(default)]
    pub since: String,
}

impl TakeoverSection {
    /// 现在是接管态吗。
    pub fn active(&self) -> bool {
        !self.project.trim().is_empty()
    }
    /// 有工作目录吗 —— 没有的话只能只读监控（改动类操作一概做不了）。
    pub fn manageable(&self) -> bool {
        self.active() && !self.working_dir.trim().is_empty()
    }
}

impl LauncherConfig {
    /// 读 `launcher.toml`。不存在或读坏了都返回默认值 —— 一个坏掉的配置文件
    /// 不该让用户连界面都打不开。读坏时会写一条 warn 日志。
    pub fn load() -> Self {
        let p = paths::launcher_toml();
        match std::fs::read_to_string(&p) {
            Err(_) => Self::default(),
            Ok(s) => match toml::from_str::<Self>(&s) {
                Ok(mut c) => {
                    c.migrate();
                    c
                }
                Err(e) => {
                    crate::lwarn!("{} 解析失败，这次用默认设置：{e}", p.display());
                    Self::default()
                }
            },
        }
    }

    /// 老配置往 I12 的字段上搬（**只在内存里搬，落盘由调用方决定**）。
    ///
    /// 目前只有一条：`[launcher] version` → `[install] launcher_version`。
    /// 用户 Mac 上那份 0.1.9 的配置里前者停在 `0.1.7`，后者根本不存在 ——
    /// 合并成一项之后再也不会出现「显示 0.1.7、实际 0.1.9」（方案第四节）。
    ///
    /// **不删 `[launcher] version`**：老版本的启动器还要读它，
    /// 用户在两个版本之间来回切的时候不该被我们弄坏配置。
    pub fn migrate(&mut self) {
        if self.install.launcher_version.is_empty() && !self.launcher.version.is_empty() {
            self.install.launcher_version = self.launcher.version.clone();
        }
        if self.install.project.is_empty() && self.install.done {
            self.install.project = PROJECT.to_string();
        }
    }

    /// 每次启动器起来都记一次「现在跑的是哪一版」（R1 第 3 条）。
    ///
    /// 返回值是「有没有真的改到什么」—— 没改就不落盘，免得每次开机都写一遍文件。
    pub fn stamp_launcher_version(&mut self) -> bool {
        let v = env!("CARGO_PKG_VERSION");
        let changed = self.install.launcher_version != v || self.launcher.version != v;
        self.install.launcher_version = v.to_string();
        // `[launcher] version` 同步写一份：老版本启动器、以及诊断包里的老字段还在读它
        self.launcher.version = v.to_string();
        changed
    }

    /// 把「这台机器上装好了」这件事记全（R1 第 1、2 条）。
    ///
    /// `adopted` 为真 = 这条记录是「检测到它已经在跑」补写的，不是安装流程写的。
    pub fn mark_installed(&mut self, adopted: bool) {
        self.install.done = true;
        if self.install.at.is_empty() || adopted {
            self.install.at = crate::timefmt::now_shanghai();
        }
        self.install.launcher_version = env!("CARGO_PKG_VERSION").to_string();
        self.install.hunter_tag = self.hunter.tag.clone();
        self.install.project = PROJECT.to_string();
        self.install.app_dir = crate::redact::mask_home(&paths::app_dir().to_string_lossy());
        self.install.runtime = runtime_label();
        if adopted {
            self.install.adopted_from_running = true;
        }
    }

    /// 健康检查通过时刷一次时间戳（R1 第 3 条）。返回「值有没有变」。
    pub fn touch_healthy(&mut self) -> bool {
        let now = crate::timefmt::now_shanghai();
        let changed = self.install.last_healthy_at != now;
        self.install.last_healthy_at = now;
        changed
    }

    pub fn save(&self) -> AppResult<()> {
        paths::ensure_dirs()?;
        let p = paths::launcher_toml();
        let s = toml::to_string_pretty(self).map_err(|e| {
            AppError::new(Code::ConfigWrite, format!("序列化 launcher.toml 失败：{e}"))
        })?;
        let header = "# Hunter 启动器的设置。由启动器维护，手改了下次保存会被覆盖。\n\
                      # 这里**不存任何 key**：hunter key 与自带模型 key 只在 app/.env（权限 600）里。\n\n";
        std::fs::write(&p, format!("{header}{s}")).map_err(|e| {
            AppError::new(Code::ConfigWrite, format!("写 {} 失败：{e}", p.display()))
        })?;
        // 这里没有 secret（key 只在 600 的 .env 里），收成 600 纯粹是统一口径：
        // `~/.hunter` 下的东西一律只有属主能读（待办池 P2-11）
        paths::chmod_600(&p)?;
        Ok(())
    }

    /// 只有在真的改到什么的时候才落盘。开机那几条（版本戳、健康时间）都走它。
    pub fn save_if(&self, changed: bool) {
        if changed {
            let _ = self.save();
        }
    }

    pub fn apply_registry(&mut self, c: &Candidate) {
        self.hunter.registry_id = c.id.to_string();
        self.hunter.registry_prefix = c.prefix.to_string();
        self.hunter.base_prefix = c.base_prefix.to_string();
    }
}

// ── 端口冲突 ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortChange {
    pub service: String,
    pub wanted: u16,
    pub actual: u16,
    /// 原来那个端口被谁占着（I5）。0.1.4 只说「被占用」，AI 因此把用户
    /// 另一套 Hunter 猜成了「残留容器」—— 这一条就是为了让证据跟着结论走。
    /// 一行人话，读不到占用者时是空串（**不猜**）
    #[serde(default)]
    pub occupied_by: String,
}

/// 端口是不是能用。
///
/// **I5 起不再用 `std::net::TcpListener`**：它在 Unix 上默认开 `SO_REUSEADDR`，
/// macOS 上会把被 `*:P` 占死的端口判成空闲（0.1.4 在用户 Mac 上装不起来的根因）。
/// 现在走 [`crate::ports`] 的三重确认：不带 `SO_REUSEADDR` 的通配绑定 + `docker ps`
/// + 系统监听表，任一说占用即占用。
///
/// `all_interfaces` 这个参数**留着只为兼容老调用点**：探测一律按通配地址来，
/// 比 `127.0.0.1` 更保守，所以两种取值的结果相同 ——
/// I7 起 web 也只绑本机了，但探测仍然按通配来（宁可多报一次冲突，不要装到一半才炸）。
pub fn port_free(port: u16, _all_interfaces: bool) -> bool {
    // `usable()` 而不是 `free`：被**我们自己这一套**占着的端口对这次安装来说照样可用
    // （I11 起这两件事在 [`crate::ports::Verdict`] 里是分开的）
    crate::ports::verdict_now(port, &[PROJECT]).usable()
}

/// 从 `want` 开始往上找一个没被占的端口，最多找 200 个。
/// `taken` 里的一律跳过；`own` 里的（现在正被**我们自己这一套**占着的）一律当成可用。
fn next_free(
    sv: &crate::ports::Survey,
    want: u16,
    taken: &[u16],
    own: &[u16],
) -> AppResult<(u16, Vec<crate::ports::Occupant>)> {
    let mut p = want;
    let mut first_occ: Vec<crate::ports::Occupant> = Vec::new();
    for i in 0..200 {
        if !taken.contains(&p) {
            if own.contains(&p) {
                return Ok((p, Vec::new()));
            }
            let v = sv.verdict(p, &[PROJECT]);
            if v.usable() {
                return Ok((p, first_occ));
            }
            // 只记**原本想要的那个端口**被谁占着 —— 用户关心的是这一条
            if i == 0 {
                first_occ = v.occupants;
            }
        }
        p = p
            .checked_add(1)
            .ok_or_else(|| AppError::new(Code::PortInUse, "端口号溢出了".to_string()))?;
    }
    Err(AppError::new(
        Code::PortInUse,
        format!("从 {want} 开始连着 200 个端口都被占了"),
    ))
}

/// 解决端口冲突：逐个检查，被占的自动往上挪。返回定下来的端口与改动清单。
///
/// `own` 是**当前 `hunter` 项目自己已经在用**的端口。第二次打开启动器时，
/// 3101 正被我们自己的 web 容器占着 —— 要是把它也算成「被占用」，
/// 每开一次启动器端口就往上挪一格，用户存的书签全会失效（M2 用例 9 实测撞出来的）。
///
/// I5：`docker ps` 与 `lsof` 在这里**只跑一次**（[`crate::ports::Survey`]），
/// 五个端口共用同一份现场，不是每个端口各起两个子进程。
pub fn resolve_ports(want: &Ports, own: &[u16]) -> AppResult<(Ports, Vec<PortChange>)> {
    resolve_ports_with(&crate::ports::Survey::collect(), want, own)
}

/// 同上，但由调用方给现场。总指挥要在一次修复回合里反复算端口，采一次就够。
pub fn resolve_ports_with(
    sv: &crate::ports::Survey,
    want: &Ports,
    own: &[u16],
) -> AppResult<(Ports, Vec<PortChange>)> {
    let mut taken: Vec<u16> = Vec::new();
    let mut changes = Vec::new();
    let mut out = want.clone();

    for (name, wanted) in want.as_pairs() {
        let (actual, occ) = next_free(sv, wanted, &taken, own)?;
        taken.push(actual);
        if actual != wanted {
            changes.push(PortChange {
                service: name.to_string(),
                wanted,
                actual,
                occupied_by: occ
                    .iter()
                    .map(crate::ports::Occupant::human)
                    .collect::<Vec<_>>()
                    .join("；"),
            });
        }
        match name {
            "web" => out.web = actual,
            "api" => out.api = actual,
            "opencode" => out.opencode = actual,
            "postgres" => out.postgres = actual,
            _ => out.redis = actual,
        }
    }
    Ok((out, changes))
}

// ── 镜像清单 ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSpec {
    /// compose 里的服务名
    pub service: String,
    /// 完整引用，例如 ghcr.io/agentpit-io/hunter-community-web:1.2.0
    pub reference: String,
    /// 界面左列显示的短名
    pub short_ref: String,
    pub tag: String,
    /// registry 主机名（探 manifest 用）
    pub host: String,
    /// registry 里的仓库路径（探 manifest 用）
    pub repo: String,
}

/// 六个服务对应的镜像。`postgres` / `redis` 在国内源下也换成镜像地址
/// （`plan/国内镜像与下载源.md`：国内拉 Docker Hub 不稳，一并镜像过去了）。
pub fn images(registry_prefix: &str, base_prefix: &str, tag: &str) -> Vec<ImageSpec> {
    let mut v = Vec::new();
    for s in ["web", "api", "opencode", "llm-shim"] {
        v.push(spec(
            &format!("{registry_prefix}/hunter-community-{s}"),
            tag,
            s,
        ));
    }
    v.push(spec(
        &format!("{base_prefix}/postgres"),
        "16-alpine",
        "postgres",
    ));
    v.push(spec(&format!("{base_prefix}/redis"), "7-alpine", "redis"));
    v
}

fn spec(image: &str, tag: &str, service: &str) -> ImageSpec {
    let (host, repo) = split_ref(image);
    ImageSpec {
        service: service.to_string(),
        reference: format!("{image}:{tag}"),
        short_ref: image.rsplit('/').next().unwrap_or(image).to_string(),
        tag: tag.to_string(),
        host,
        repo,
    }
}

/// `ghcr.io/agentpit-io/hunter-community-web` → (`ghcr.io`, `agentpit-io/hunter-community-web`)
/// `docker.io/library/postgres` → (`registry-1.docker.io`, `library/postgres`)
/// 不带主机名的（`postgres`）按 Docker Hub 官方库处理。
pub fn split_ref(image: &str) -> (String, String) {
    let parts: Vec<&str> = image.split('/').collect();
    let looks_like_host = parts.len() > 1
        && (parts[0].contains('.') || parts[0].contains(':') || parts[0] == "localhost");
    if !looks_like_host {
        let repo = if parts.len() == 1 {
            format!("library/{image}")
        } else {
            image.to_string()
        };
        return ("registry-1.docker.io".to_string(), repo);
    }
    let host = if parts[0] == "docker.io" {
        "registry-1.docker.io".to_string()
    } else {
        parts[0].to_string()
    };
    (host, parts[1..].join("/"))
}

// ── .env ──────────────────────────────────────────────────────────────────

/// 写 `.env` 需要的全部输入。
pub struct EnvInput<'a> {
    pub tag: &'a str,
    pub registry_prefix: &'a str,
    pub ports: &'a Ports,
    /// hunter key。自带 key 模式下它仍然要写进 HUNTER_API_KEY（工具与数据网关要用）
    pub hunter_key: &'a str,
    /// 模型模式：gateway | own
    pub model_mode: &'a str,
    pub llm_base_url: &'a str,
    pub llm_model: &'a str,
    pub llm_api_key: &'a str,
    pub schema_sanitize: bool,
}

/// 从已有的 `.env` 里读出的、**必须原样保留**的三个值。
/// 换掉任何一个都会出真实的问题：JWT_SECRET 换了用户掉登录；
/// POSTGRES_PASSWORD 换了连不上已经初始化过的数据卷；OPENCODE_PASS 换了健康检查直接挂。
#[derive(Debug, Clone, Default)]
pub struct StickySecrets {
    pub jwt_secret: Option<String>,
    pub postgres_password: Option<String>,
    pub opencode_pass: Option<String>,
}

/// 这台机器上已有的 `POSTGRES_PASSWORD` 能不能安全地拼进 DSN。
///
/// 见 [`crate::secretgen`] 的模块头：0.1.1 及之前用标准 base64 生成口令，
/// 里面可能有 `/` 或 `+`，而上游会把它拼进
/// `postgresql://hunter:<口令>@postgres:5432/hunter` —— 一个 `/` 就让 api 起不来。
///
/// 0.1.2 起新生成的口令只含字母数字，但**已经写在 `.env` 里的那一个不会被换掉**
/// （换了就连不上已经初始化过的数据卷）。所以这里只负责**认出来并说清楚**，
/// 不悄悄改、更不删数据卷。
pub fn sticky_password_safe(pw: &str) -> bool {
    pw.bytes().all(|b| b.is_ascii_alphanumeric())
}

pub fn read_sticky(path: &Path) -> StickySecrets {
    let map = parse_env_file(path);
    let pick = |k: &str| map.get(k).filter(|v| !v.is_empty()).cloned();
    let s = StickySecrets {
        jwt_secret: pick("JWT_SECRET"),
        postgres_password: pick("POSTGRES_PASSWORD"),
        opencode_pass: pick("OPENCODE_PASS"),
    };
    // 认出 0.1.1 及之前留下的那种会把 DSN 截断的口令。**不打印口令本身**（红线 2）
    if let Some(pw) = s.postgres_password.as_deref() {
        if !sticky_password_safe(pw) {
            crate::lwarn!(
                "已有的 POSTGRES_PASSWORD 里有 URL 里的特殊字符（0.1.1 及之前生成的口令用的是标准 base64）。\
                 上游会把它拼进 postgresql://hunter:<口令>@postgres:5432/hunter，\
                 碰上 `/` 时 api 容器会一直报 invalid integer value ... for connection option \"port\" 起不来。\
                 这一版不会替你改它（换口令连不上已经初始化过的数据卷）。\
                 这套装起来过就不用管；要是本来就没装起来，最干净的办法是重装一次：\
                 docker compose -p hunter down -v 之后删掉 {} 再装。",
                path.display()
            );
        }
    }
    for v in [&s.jwt_secret, &s.postgres_password, &s.opencode_pass]
        .into_iter()
        .flatten()
    {
        crate::redact::register_secret(v);
    }
    s
}

/// `~/.hunter/app/.env` 里**留着的那把 hunter key**（I14 · F3）。
///
/// 「只删除应用，保留数据」这一档明确保留了 `.env`，界面上那句话是
/// 「以后重新安装会直接沿用，登录也不会失效」—— 而 0.1.13 的重装流程
/// 根本没去读它，`--auto -y` 照样报「标准输入不是终端，没法交互式问 key」。
/// 承诺了的事就得真做：要 key 之前先来这里看一眼。
///
/// **只在格式对得上时才返回**（`hunt_tools_` 开头、一共 43 位）。
/// 格式都不对的东西不值得拿去问网关，更不该被当成「沿用上次的 key」；
/// 这一步不联网，真假由调用方去 [`crate::gateway::check_key`] 那里问。
pub fn kept_hunter_key() -> Option<String> {
    match kept_hunter_key_state() {
        KeptKeyState::Usable(k) => Some(k),
        _ => None,
    }
}

/// 上一条的三分法。**「什么都没有」和「有一个但形状不对」不是一回事**：
///
/// 前者是干净的现场（第一次装、或者选了「删除应用和全部数据」），一个字都不用说；
/// 后者是 `.env` 里确实留着 `HUNTER_API_KEY=…`、只是它长得不像一把 hunter key
/// （手改过、截断了、粘贴时少了一截）。这一档要是也一个字都不说，
/// 用户看到的就是「我明明留着 key，它却说要我重新输」—— 和 F3 原来那个坏法同一个形状：
/// **事情做了（读了），只是没说出来（为什么没用上）**。
pub enum KeptKeyState {
    /// `.env` 里没有这一项，或者它是空的
    Absent,
    /// 有，但形状对不上（不是 `hunt_tools_` 开头的 43 位）。带着打码后的样子，好让界面说得具体
    BadShape(String),
    /// 形状对得上，可以拿去问网关
    Usable(String),
}

pub fn kept_hunter_key_state() -> KeptKeyState {
    let Some(k) = parse_env_file(&crate::paths::env_file())
        .get("HUNTER_API_KEY")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return KeptKeyState::Absent;
    };
    if !crate::gateway::key_shape_ok(&k) {
        // **这里不能 `register_secret`**（第一版就是这么写的，一跑单测炸了五个）：
        // 登记表是整个进程共用的，而形状不对的那个值完全可能就是 `hunt_tools_` 本身
        // （用户把 key 那一截删干净了）。把它登记进去，之后每一行日志里的
        // `hunt_tools_****` 都会被再替换一次，变成 `hun********` ——
        // 打码函数自己把自己的输出打成了看不出是什么的东西。
        //
        // 不登记也不漏：`redact::mask_shapes` 本来就按前缀整段打掉
        // `hunt_tools_…`，一把被截断的真 key 照样盖得住。
        return KeptKeyState::BadShape(crate::redact::mask_key(&k));
    }
    crate::redact::register_secret(&k);
    KeptKeyState::Usable(k)
}

/// 把 `.env` 解析成键值表。`export ` 前缀、注释、空行都能吃。
pub fn parse_env_file(path: &Path) -> BTreeMap<String, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => parse_env(&s),
        Err(_) => BTreeMap::new(),
    }
}

pub fn parse_env(s: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|x| x.strip_suffix('"'))
                .unwrap_or(v);
            m.insert(k.trim().to_string(), v.to_string());
        }
    }
    m
}

/// 会被原样写进 `.env` 的那几个值里不许有换行（I2 自审发现）。
///
/// `.env` 是一行一个 `KEY=值` 的格式，值里有 `\n` 就等于**又定义了一个环境变量** ——
/// compose 读 `.env` 时后定义的会盖掉先定义的，于是一个多出来的换行能把
/// `HUNTER_API_KEY` / `POSTGRES_PASSWORD` 这类关键项整个换掉。
///
/// 这些值的来源全是用户手输或粘贴的（模型名、BASE_URL、自定义镜像源），
/// 不是远程输入，所以这不是一个「别人能打的洞」；但从剪贴板里带出一个尾随换行
/// 是非常常见的事，而它造成的后果是一套**装得起来但行为莫名其妙**的栈。
/// 宁可当场报一句人话。
fn check_env_value(field: &str, value: &str) -> AppResult<()> {
    if value.contains('\n') || value.contains('\r') {
        return Err(AppError::new(
            Code::ConfigWrite,
            format!("{field} 里有换行。这一项会原样写进 .env，一行只能有一个值 —— 多半是粘贴时带进来的，去掉换行再试一次。"),
        ));
    }
    Ok(())
}

/// 渲染 `.env`。`sticky` 里有值的就沿用，没有的现生成。
pub fn render_env(input: &EnvInput, sticky: &StickySecrets) -> AppResult<String> {
    check_env_value("模型名", input.llm_model)?;
    check_env_value("模型 BASE_URL", input.llm_base_url)?;
    check_env_value("模型 key", input.llm_api_key)?;
    check_env_value("hunter key", input.hunter_key)?;
    check_env_value("镜像源地址", input.registry_prefix)?;
    check_env_value("Hunter 版本号", input.tag)?;

    let jwt = match &sticky.jwt_secret {
        Some(v) => v.clone(),
        None => crate::secretgen::random_token(48)?,
    };
    let pg = match &sticky.postgres_password {
        Some(v) => v.clone(),
        None => crate::secretgen::random_token(24)?,
    };
    let oc = match &sticky.opencode_pass {
        Some(v) => v.clone(),
        None => crate::secretgen::random_token(18)?,
    };
    crate::redact::register_secret(input.hunter_key);
    if !input.llm_api_key.is_empty() {
        crate::redact::register_secret(input.llm_api_key);
    }

    let sanitize = if input.schema_sanitize { "1" } else { "0" };
    let out = ENV_TEMPLATE
        // I11 · U5：让在 ~/.hunter/app 里直接敲 `docker compose up -d` 的人
        // 也一定带上覆盖文件。启动器自己走的是命令行 `-f`，那个优先级更高，
        // 所以这三行不会改变启动器的行为，只兜住手工操作。
        .replace("{{COMPOSE_PROJECT_NAME}}", PROJECT)
        .replace("{{COMPOSE_PATH_SEPARATOR}}", compose_path_separator())
        .replace("{{COMPOSE_FILE}}", &compose_file_value())
        .replace("{{HUNTER_VERSION}}", input.tag)
        .replace("{{HUNTER_REGISTRY}}", input.registry_prefix)
        .replace("{{WEB_HOST_PORT}}", &input.ports.web.to_string())
        .replace("{{API_HOST_PORT}}", &input.ports.api.to_string())
        .replace("{{OPENCODE_HOST_PORT}}", &input.ports.opencode.to_string())
        .replace("{{POSTGRES_HOST_PORT}}", &input.ports.postgres.to_string())
        .replace("{{REDIS_HOST_PORT}}", &input.ports.redis.to_string())
        .replace("{{POSTGRES_PASSWORD}}", &pg)
        .replace("{{JWT_SECRET}}", &jwt)
        .replace("{{OPENCODE_PASS}}", &oc)
        // HUNTER_API_KEY 与 LLM_API_KEY 在模板里是同一个占位符，
        // 自带 key 模式下要拆开，所以先都填 hunter key，再单独改 LLM 三项。
        .replace("{{HUNTER_KEY}}", input.hunter_key);

    let out = set_kv(&out, "LLM_BASE_URL", input.llm_base_url);
    let out = set_kv(&out, "LLM_DEFAULT_MODEL", input.llm_model);
    let out = set_kv(&out, "LLM_API_KEY", input.llm_api_key);
    let out = set_kv(&out, "LLM_SCHEMA_SANITIZE", sanitize);
    // 深度分析与子智能体的模型名：自带 key 模式下没有 hunter-deep 这个别名，
    // 全部落到用户自己那个模型上，否则「对话能用、深度分析是坏的」（M0 §3.3）。
    let out = if input.model_mode == "own" {
        let mut s = out;
        for k in [
            "ASSISTANT_MODEL_ROUTE",
            "ASSISTANT_MODEL_CHAT",
            "ASSISTANT_MODEL_COMPRESS",
            "AGENT_MODEL_ROUTER",
            "AGENT_MODEL_ROUTE_LITE",
            "AGENT_SUB_WL_MODEL",
            "AGENT_SUB_PORT_MODEL",
            "AGENT_SUB_EVENT_MODEL",
            "SIGNAL_ANALYSIS_MODEL",
            "AGENT_SUB_RESEARCH_MODEL",
        ] {
            s = set_kv(&s, k, input.llm_model);
        }
        s
    } else {
        out
    };

    if out.contains("{{") {
        let leftover: Vec<&str> = out.lines().filter(|l| l.contains("{{")).collect();
        return Err(AppError::new(
            Code::ConfigWrite,
            format!(".env 模板还有没替换的占位符：{leftover:?}"),
        ));
    }
    Ok(out)
}

/// 把 `KEY=...` 那一行的值换掉。找不到这个键就在末尾追加。
fn set_kv(src: &str, key: &str, value: &str) -> String {
    let prefix = format!("{key}=");
    let mut found = false;
    let mut out: Vec<String> = Vec::with_capacity(src.lines().count() + 1);
    for line in src.lines() {
        if line.starts_with(&prefix) {
            out.push(format!("{prefix}{value}"));
            found = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !found {
        out.push(format!("{prefix}{value}"));
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// 把 `.env` 写完之后**读回来逐项对账**。
///
/// I2 修「模型名里的换行会注入进 `.env`」用的是「写之前查」（[`check_env_value`]）。
/// 那一道拦得住已知的形状，但拦不住没想到的形状 —— 而这个文件里有 hunter key、
/// 数据库口令、JWT secret，写错一个字符就是一次真实的事故。
///
/// 所以再加一道**结果校验**：把刚写出去的文件按 `.env` 的语法解析回来，逐项比对
/// 「我本来要写的值」。任何形式的注入（换行、`export`、引号、同名键写两遍）都会让
/// 至少一项对不上，当场报错而不是装到一半才发现。
///
/// 三条规则：
/// 1. **每个键只能出现一次**。注入最典型的后果就是多出一行同名键 —— 而 `parse_env`
///    是后写的覆盖先写的，只看解析结果反而看不出来，所以这里单独数一遍行。
/// 2. 用户能左右的那几项值必须一字不差（含五个端口）。
/// 3. 报错里**只说键名，绝不带值**（红线 2：key 不进日志、不进任何输出）。
fn verify_env_written(text: &str, input: &EnvInput) -> AppResult<()> {
    // ① 同名键不能出现两次
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let l = l.strip_prefix("export ").unwrap_or(l);
        if let Some((k, _)) = l.split_once('=') {
            *seen.entry(k.trim()).or_insert(0) += 1;
        }
    }
    if let Some((k, n)) = seen.iter().find(|(_, n)| **n > 1) {
        return Err(AppError::new(
            Code::ConfigWrite,
            format!(".env 里 {k} 出现了 {n} 次。写完读回来对不上，没有写出这份配置 —— 请检查设置里手填的镜像源、模型名与 BASE_URL。"),
        ));
    }

    // ② 逐项对账。ports 也比一遍：端口写错等于整套连不上
    let m = parse_env(text);
    let web = input.ports.web.to_string();
    let api = input.ports.api.to_string();
    let opencode = input.ports.opencode.to_string();
    let postgres = input.ports.postgres.to_string();
    let redis = input.ports.redis.to_string();
    let compose_file = compose_file_value();
    let expect: [(&str, &str); 14] = [
        // I11 · U5：这三项写漏了，手工跑 compose 就会把端口重建成 0.0.0.0，
        // 所以它们和 key、端口一样要读回来对账
        ("COMPOSE_PROJECT_NAME", PROJECT),
        ("COMPOSE_PATH_SEPARATOR", compose_path_separator()),
        ("COMPOSE_FILE", &compose_file),
        ("HUNTER_VERSION", input.tag),
        ("HUNTER_REGISTRY", input.registry_prefix),
        ("HUNTER_API_KEY", input.hunter_key),
        ("LLM_BASE_URL", input.llm_base_url),
        ("LLM_DEFAULT_MODEL", input.llm_model),
        ("LLM_API_KEY", input.llm_api_key),
        ("WEB_HOST_PORT", &web),
        ("API_HOST_PORT", &api),
        ("OPENCODE_HOST_PORT", &opencode),
        ("POSTGRES_HOST_PORT", &postgres),
        ("REDIS_HOST_PORT", &redis),
    ];
    for (k, want) in expect {
        match m.get(k) {
            Some(got) if got == want => {}
            // 只报键名。值里可能是 key / 口令，一个字都不能进错误信息与日志（红线 2）
            Some(_) => {
                return Err(AppError::new(
                    Code::ConfigWrite,
                    format!(".env 写完读回来 {k} 和要写的值对不上，没有写出这份配置。"),
                ))
            }
            None => {
                return Err(AppError::new(
                    Code::ConfigWrite,
                    format!(".env 写完读回来少了 {k}，没有写出这份配置。"),
                ))
            }
        }
    }
    Ok(())
}

/// 写 `.env` 并把权限收成 600（红线 2）。
pub fn write_env(input: &EnvInput) -> AppResult<()> {
    paths::ensure_dirs()?;
    let path = paths::env_file();
    let sticky = read_sticky(&path);
    let content = render_env(input, &sticky)?;
    // 落盘之前先对一遍账：渲染错了就根本不要碰磁盘上那一份
    // （旧的 `.env` 还在，用户至少还能用原来那套起来）
    verify_env_written(&content, input)?;
    std::fs::write(&path, content).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("写 {} 失败：{e}", path.display()),
        )
    })?;
    paths::chmod_600(&path)?;
    // 再把磁盘上那一份读回来对一遍：盘满、被别的进程截断、编码出岔子都在这里露馅
    verify_env_written(&std::fs::read_to_string(&path).unwrap_or_default(), input)?;
    crate::linfo!(
        "已写 {}（权限 600）· JWT_SECRET {}",
        path.display(),
        if sticky.jwt_secret.is_some() {
            "沿用已有的"
        } else {
            "首次生成"
        }
    );
    Ok(())
}

// ── 覆盖文件 ──────────────────────────────────────────────────────────────

// ── web 端口绑在哪（I7 · 用户 2026-09-21 19:05 拍板） ─────────────────────

/// web 端口的宿主绑定地址。
///
/// ## 只有两种可能，而且只有一种是**新生成**得出来的
///
/// | 取值 | 谁会给出 | 渲染成 |
/// |---|---|---|
/// | [`WebBind::Local`] | 新装、收紧之后、以及任何读不出现状的情况 | `127.0.0.1:<port>:3000` |
/// | [`WebBind::LegacyLan`] | **只有** [`WebBind::detect`]，而且只在「这台机器当前确实已经对局域网开放」时 | `<port>:3000`（沿用现状） |
///
/// 免费版不提供「放开到局域网」这件事（付费版功能），所以：
/// **没有任何一条路径能从 `Local` 走回 `LegacyLan`** ——
/// `detect` 的依据是磁盘上那份覆盖文件，一旦被收紧写成 `127.0.0.1`，
/// 它以后就永远只会返回 `Local`。收紧是单向的，这一条靠代码保证，不靠文案。
///
/// 为什么不从 `launcher.toml` 的 `hunter.web_bind` 读：那是用户手改得到的东西。
/// 用户的决定是「免费版只能本机访问」，那么手改配置文件也不该能绕过去。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebBind {
    /// 只有这台电脑能打开
    Local,
    /// 老机器留下的现状：web 还绑在所有网卡上。**升级时不悄悄改动它**，
    /// 运行面板出一行提示 + 一个「一键收紧」（用户 2026-09-21 19:05 的决定）
    LegacyLan,
}

impl WebBind {
    /// 从**磁盘上那份覆盖文件的现状**判断，不看配置、不看命令行。
    ///
    /// 判断依据是覆盖文件里 web 那一行的映射有没有 `127.0.0.1:` 前缀：
    /// * 文件不存在（没装过 / 刚被删）→ `Local`（新装一律本机）
    /// * 读得到、web 那一行**没有**本机前缀 → `LegacyLan`（这台机器当前真的对外）
    /// * 读得到、有本机前缀 → `Local`
    /// * 读得到、但里面根本找不到 web 那一行（被手改坏了）→ `Local`（收紧的方向不会错）
    pub fn detect() -> Self {
        match std::fs::read_to_string(paths::override_file()) {
            Ok(s) => Self::from_override_text(&s),
            Err(_) => Self::Local,
        }
    }

    /// [`WebBind::detect`] 的纯函数内核，单测直接喂文本。
    pub fn from_override_text(text: &str) -> Self {
        // web 那一行长这样（本机）：  web:\n    ports: !override ["127.0.0.1:3101:3000"]
        // 或者这样（老机器对外）：    web:\n    ports: !override ["3101:3000"]
        let mut in_web = false;
        for line in text.lines() {
            let t = line.trim_start();
            if t.starts_with('#') {
                continue;
            }
            // 服务名那一行：缩进两格、以冒号结尾
            if line.starts_with("  ") && !line.starts_with("    ") && t.ends_with(':') {
                in_web = t == "web:";
                continue;
            }
            if in_web && t.starts_with("ports:") {
                return if t.contains("127.0.0.1:") {
                    Self::Local
                } else {
                    Self::LegacyLan
                };
            }
        }
        Self::Local
    }

    /// 界面 / 日志里要不要提「这台机器现在对局域网开着」
    pub fn lan_exposed(self) -> bool {
        self == Self::LegacyLan
    }
}

/// 记一笔「容器现在是不是用户自己停的」（I12 · R3+ 的自愈靠它绕开）。
///
/// 界面上的「停止」、托盘上的「停止」都要写它；两边的「启动」都要清它。
pub fn mark_stopped_by_user(v: bool) {
    let mut cfg = LauncherConfig::load();
    if cfg.install.stopped_by_user == v {
        return;
    }
    cfg.install.stopped_by_user = v;
    if let Err(e) = cfg.save() {
        crate::lwarn!("记「是不是你自己停的」没写成：{}", e.msg);
    }
}

/// 这台机器现在用的是哪套运行时，写进 `[install] runtime`（R1）。
///
/// 值域和 [`crate::runtime::effective::Kind`] 一致（`builtin` / `user` / `none`），
/// **读的是现状**，不是配置里的意图。
fn runtime_label() -> String {
    match crate::runtime::effective::current().kind {
        crate::runtime::effective::Kind::Builtin => "builtin".to_string(),
        crate::runtime::effective::Kind::User => "user".to_string(),
        crate::runtime::effective::Kind::None => "none".to_string(),
    }
}

/// 覆盖文件里给每个服务加的重启策略（I12 · R3+）。
///
/// 2026-09-23 在用户 Mac 上实测：内置运行时的虚拟机 `limactl stop` / `start` 之后，
/// 六个容器一个都没起来，`docker ps` 是空的。
///
/// 上游的 `docker-compose.yml` 其实**已经**写了 `restart: unless-stopped`
/// （`x-restart` 锚点，六个服务都引了；测试机上 `docker inspect` 核过，
/// `HostConfig.RestartPolicy.Name` 确实是 `unless-stopped`）。
/// 这里再写一遍不是重复劳动，是**把它钉在我们自己的文件里**：
/// 上游哪天改了锚点、或者用户拿到的是别的 tag，覆盖文件仍然保证这一条。
///
/// 它不能单独解决那个现场 —— `unless-stopped` 的语义是「除非你手动停过」，
/// 而 `compose stop` / `colima stop` 走的正是「手动停过」那条路。
/// 所以 R3+ 的第 2 条（启动器起来发现「运行环境在跑、容器没起」就自动拉起）
/// 才是真正兜底的那一道，见 [`crate::lib`] 启动时那段。
const RESTART_POLICY: &str = "unless-stopped";

/// 生成 `docker-compose.launcher.yml`。
///
/// 三件事：
/// 1. **所有发布出来的端口一律绑 `127.0.0.1`**（红线 4 · I7 起连 web 也收进来）。
///    `!override` 标签是必须的 —— compose 对 `ports` 默认做追加合并，
///    不加标签会变成「既听 0.0.0.0 又听 127.0.0.1」（M0 §3.4 实测）。
///    唯一的例外是 `bind == WebBind::LegacyLan`，那是**沿用老机器的现状**，
///    不是一个新做出来的决定（见 [`WebBind`]）。
/// 2. 用国内源时把 `postgres` / `redis` 的 `image:` 也改写过去。
///    另外四个服务的镜像地址由 `.env` 的 `HUNTER_REGISTRY` 控制，不用在这里写。
/// 3. **六个服务一律 `restart: unless-stopped`**（I12 · R3+，见 [`RESTART_POLICY`]）。
///    `llm-shim` 在这份文件里原本一个字都没有，这一条是它出现的唯一理由。
pub fn render_override(ports: &Ports, base_prefix: &str, bind: WebBind) -> String {
    let mut s = String::new();
    s.push_str(
        "# ~/.hunter/app/docker-compose.launcher.yml\n\
         # 由 Hunter 启动器生成 · 请勿手改（改了下次启动会被覆盖）\n\
         # 作用：1) 把**所有**发布端口收回 127.0.0.1（总控规则红线 4 · I7 起连 web 也收进来）\n\
         #       2) 落实端口冲突改写后的值\n\
         #       3) 用国内镜像源时改写 postgres / redis 的镜像地址\n\
         #       4) 六个服务一律 restart: unless-stopped —— 电脑或运行环境重启后容器自己回来\n\
         #\n\
         # `!override` 标签是必须的：compose 默认对 ports 做**追加**合并，不加这个标签会变成\n\
         # 「既监听 0.0.0.0 又监听 127.0.0.1」，红线 4 就白写了（M0 §3.4 已实测验证）。\n\
         services:\n",
    );
    // I7：web 也收回本机。用户 2026-09-21 19:05 的原话是
    // 「目前只能本机访问，不考虑同一局域网访问，这个需要升级付费版本才可以。」
    // 唯一会渲染成对外的情况是 LegacyLan —— 那不是一个决定，是**沿用老机器的现状**，
    // 免得升级把用户昨天还在用的地址突然关掉（同一条决定的第三点）。
    match bind {
        WebBind::Local => {
            s.push_str("  # web 也只绑本机：免费版不提供局域网访问（付费版功能）\n");
            s.push_str(&format!(
                "  web:\n    restart: {RESTART_POLICY}\n    ports: !override [\"127.0.0.1:{}:3000\"]\n",
                ports.web
            ));
        }
        WebBind::LegacyLan => {
            s.push_str("  # 沿用这台机器升级前的现状：web 还绑在所有网卡上。\n");
            s.push_str(
                "  # 新版本默认只允许本机访问 —— 运行面板上点「只允许本机访问」就能收紧（单向）。\n",
            );
            s.push_str(&format!(
                "  web:\n    restart: {RESTART_POLICY}\n    ports: !override [\"{}:3000\"]\n",
                ports.web
            ));
        }
    }
    s.push_str(&format!(
        "  api:\n    restart: {RESTART_POLICY}\n    ports: !override [\"127.0.0.1:{}:8000\"]\n",
        ports.api
    ));
    s.push_str(&format!(
        "  opencode:\n    restart: {RESTART_POLICY}\n    ports: !override [\"127.0.0.1:{}:3901\"]\n",
        ports.opencode
    ));
    // llm-shim 不发布端口，所以它在这份文件里只为了重启策略而存在（I12 · R3+）
    s.push_str(&format!("  llm-shim:\n    restart: {RESTART_POLICY}\n"));

    let pg_image = if base_prefix == "docker.io/library" {
        "postgres:16-alpine".to_string()
    } else {
        format!("{base_prefix}/postgres:16-alpine")
    };
    let redis_image = if base_prefix == "docker.io/library" {
        "redis:7-alpine".to_string()
    } else {
        format!("{base_prefix}/redis:7-alpine")
    };
    s.push_str(&format!(
        "  postgres:\n    image: {pg_image}\n    restart: {RESTART_POLICY}\n    ports: !override [\"127.0.0.1:{}:5432\"]\n",
        ports.postgres
    ));
    s.push_str(&format!(
        "  redis:\n    image: {redis_image}\n    restart: {RESTART_POLICY}\n    ports: !override [\"127.0.0.1:{}:6379\"]\n",
        ports.redis
    ));
    s
}

/// `launcher.toml` 里手写的 `hunter.web_bind` 已经不起作用了 —— 但**不能悄悄不起作用**。
/// 每次生成覆盖文件时，只要那一项不是 `local`，就在日志里记一条说清楚被忽略了、为什么。
///
/// 唯一不记的情况是 `bind == LegacyLan`：那台机器的 web 本来就还对外，
/// 「已忽略」这句话在那儿是假的（红线 1）。
fn warn_legacy_web_bind(bind: WebBind) {
    if bind == WebBind::LegacyLan {
        return;
    }
    let raw = LauncherConfig::load().hunter.web_bind;
    if raw != WEB_BIND_LOCAL {
        crate::lwarn!(
            "launcher.toml 里 hunter.web_bind = \"{raw}\" 已被忽略：\
             免费版只允许本机访问（局域网访问是付费版功能），\
             覆盖文件里六个服务一律按 127.0.0.1 生成。"
        );
    }
}

/// 写覆盖文件。**绑定地址不接受参数** —— 由 [`WebBind::detect`] 从磁盘现状读，
/// 调用方（安装、升级、改设置）都无法指定它。要收紧走 [`write_override_local`]。
pub fn write_override(ports: &Ports, base_prefix: &str) -> AppResult<()> {
    write_override_with(ports, base_prefix, WebBind::detect())
}

/// 显式收紧成「只有这台电脑」。**只有「一键收紧」这一个入口调它**，而且是单向的：
/// 写完之后 `detect()` 以后永远返回 `Local`，没有任何函数能写回去。
pub fn write_override_local(ports: &Ports, base_prefix: &str) -> AppResult<()> {
    write_override_with(ports, base_prefix, WebBind::Local)
}

fn write_override_with(ports: &Ports, base_prefix: &str, bind: WebBind) -> AppResult<()> {
    warn_legacy_web_bind(bind);
    paths::ensure_dirs()?;
    let p = paths::override_file();
    std::fs::write(&p, render_override(ports, base_prefix, bind))
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("写 {} 失败：{e}", p.display())))?;
    paths::chmod_600(&p)?;
    Ok(())
}

// ── compose 文件 ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComposeSource {
    /// 从 raw.githubusercontent 按固定 tag 下载
    Download,
    /// 用启动器内置的同版本副本
    Bundled,
}

/// 取 `docker-compose.yml`。
///
/// 方案 §5.2 写的是「从 Release 资产下载」，但 M0 §3.2 实测 hunter-community 的 Release
/// **没有任何 assets**，所以改走 raw.githubusercontent 的固定 tag 路径。
/// 下载回来先校验内容，不过关就退回内置副本，全程不静默失败。
pub fn fetch_compose(tag: &str, timeout: Duration) -> (String, ComposeSource, Option<String>) {
    let url = format!(
        "https://raw.githubusercontent.com/agentpit-io/hunter-community/v{tag}/docker-compose.yml"
    );
    match crate::http::get(&url, &[], timeout) {
        Ok(r) if r.ok() => match validate_compose(&r.body) {
            Ok(()) => {
                let body = r.body.replace("\r\n", "\n");
                let sha = sha256_hex(body.as_bytes());
                if tag == BUNDLED_TAG && sha != BUNDLED_COMPOSE_SHA256 {
                    let note = format!(
                        "下载到的 {tag} compose 校验和是 {sha}，与启动器内置副本的 {BUNDLED_COMPOSE_SHA256} 不一致。\
                         同一个 git tag 的内容不该变，这次改用内置副本。"
                    );
                    crate::lwarn!("{note}");
                    return (bundled_compose(), ComposeSource::Bundled, Some(note));
                }
                crate::linfo!(
                    "已从 raw.githubusercontent 取到 v{tag} 的 compose（{} 字节 · sha256 {}）",
                    body.len(),
                    &sha[..16]
                );
                (body, ComposeSource::Download, None)
            }
            Err(e) => {
                let note = format!(
                    "下载到的 compose 内容不合格（{}），改用启动器内置的 {BUNDLED_TAG} 副本。",
                    e.msg
                );
                crate::lwarn!("{note}");
                (bundled_compose(), ComposeSource::Bundled, Some(note))
            }
        },
        Ok(r) => {
            let note = format!(
                "取 compose 失败 HTTP {}，改用启动器内置的 {BUNDLED_TAG} 副本。",
                r.status
            );
            crate::lwarn!("{note}");
            (bundled_compose(), ComposeSource::Bundled, Some(note))
        }
        Err(e) => {
            let note = format!(
                "取 compose 失败（{}），改用启动器内置的 {BUNDLED_TAG} 副本。",
                e.msg
            );
            crate::lwarn!("{note}");
            (bundled_compose(), ComposeSource::Bundled, Some(note))
        }
    }
}

/// 国内备用的 compose 地址（`plan/国内镜像与下载源.md` 的目录约定 `/hunter/<tag>/docker-compose.yml`）。
pub fn cn_compose_url(tag: &str) -> String {
    format!("{CN_DOWNLOAD_BASE}/hunter/{tag}/docker-compose.yml")
}

/// 国内下载源前缀。与 GitHub 仓库变量 `CN_DOWNLOAD_BASE` 保持一致。
pub const CN_DOWNLOAD_BASE: &str = "https://hunter-dl-hk-1253756459.cos.ap-hongkong.myqcloud.com";

/// **升级专用**的 compose 获取：拿不到就是拿不到，**绝不退回内置副本**。
///
/// 和 [`fetch_compose`] 的区别只有这一条，但它很关键：安装时退回内置的 1.2.0 副本是合理的兜底
/// （用户要的就是"一套能跑的 Hunter"）；而升级时用户点的是**某个具体版本**，
/// 这时候悄悄换成内置的 1.2.0 就是拿另一件事冒充他要的事 —— 他会以为自己升到了 9.9.9，
/// 实际跑的是 1.2.0。宁可报错让他知道那个版本取不到（红线 1）。
///
/// 两个源按顺序试：raw.githubusercontent（主）→ COS 香港（国内备用）。
pub fn fetch_compose_strict(tag: &str, timeout: Duration) -> AppResult<(String, String)> {
    let urls = [
        format!(
            "https://raw.githubusercontent.com/agentpit-io/hunter-community/v{tag}/docker-compose.yml"
        ),
        cn_compose_url(tag),
    ];
    let mut why: Vec<String> = Vec::new();
    for url in urls {
        let host = crate::http::host_of(&url);
        match crate::http::get(&url, &[], timeout) {
            Ok(r) if r.ok() => match validate_compose(&r.body) {
                Ok(()) => {
                    let body = r.body.replace("\r\n", "\n");
                    crate::linfo!(
                        "升级：已从 {host} 取到 v{tag} 的 compose（{} 字节）",
                        body.len()
                    );
                    return Ok((body, host));
                }
                Err(e) => why.push(format!("{host} 返回的内容不合格（{}）", e.msg)),
            },
            Ok(r) => why.push(format!("{host} HTTP {}", r.status)),
            Err(e) => why.push(format!("{host} {}", e.msg)),
        }
    }
    Err(AppError::new(
        Code::ComposeFetch,
        format!(
            "取不到 v{tag} 的 docker-compose.yml：{}。这个版本可能不存在。",
            why.join("；")
        ),
    ))
}

/// 校验下载回来的 compose 是不是真的那个文件。
/// 不做完整 YAML 解析 —— `docker compose config` 稍后会替我们做，而且做得更彻底。
/// 这里拦的是「拿回来一个登录页 / 404 页 / 空文件」这类明显不对的东西。
pub fn validate_compose(s: &str) -> AppResult<()> {
    if s.len() < 2000 {
        return Err(AppError::new(
            Code::ComposeFetch,
            format!("只有 {} 字节，太小了", s.len()),
        ));
    }
    if !s.contains("services:") {
        return Err(AppError::new(
            Code::ComposeFetch,
            "里面没有 services: 段".to_string(),
        ));
    }
    for svc in [
        "web:",
        "api:",
        "opencode:",
        "llm-shim:",
        "postgres:",
        "redis:",
    ] {
        if !s.contains(svc) {
            return Err(AppError::new(
                Code::ComposeFetch,
                format!("里面找不到服务 {svc}"),
            ));
        }
    }
    if !s.contains("hunter-community-web") {
        return Err(AppError::new(
            Code::ComposeFetch,
            "里面没有 hunter-community 的镜像引用".to_string(),
        ));
    }
    Ok(())
}

pub fn write_compose(content: &str) -> AppResult<()> {
    paths::ensure_dirs()?;
    let p = paths::compose_file();
    std::fs::write(&p, content)
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("写 {} 失败：{e}", p.display())))?;
    paths::chmod_600(&p)?;
    Ok(())
}

/// 启动器内置的 compose 副本，**换行统一成 LF**。
///
/// 为什么要归一化：这份文件是 `include_str!` 编进二进制的，而 Windows 的 git checkout
/// 默认把 LF 换成 CRLF —— 同一份源码在 Linux 与 Windows 上编出来的内容就不一样，
/// 跟写死的 sha256 对不上（M2 的 CI 在 windows-latest 上实测撞到）。
/// 仓库里已经用 `.gitattributes` 强制 LF，这里再归一一次是双保险：
/// 别人用别的 checkout 设置克隆时也能编出一致的产物。
pub fn bundled_compose() -> String {
    BUNDLED_COMPOSE.replace("\r\n", "\n")
}

/// sha256 的十六进制串。
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    const FAKE_KEY: &str = "hunt_tools_q6sKaaaaaaaaaaaaaaaaaaaaaaaaQMo2";

    fn input<'a>(ports: &'a Ports, mode: &'a str) -> EnvInput<'a> {
        EnvInput {
            tag: "1.2.0",
            registry_prefix: "ghcr.io/agentpit-io",
            ports,
            hunter_key: FAKE_KEY,
            model_mode: mode,
            llm_base_url: "https://hunter.agentpit.io/api/saas/llm/v1",
            llm_model: "hunter-chat",
            llm_api_key: FAKE_KEY,
            schema_sanitize: false,
        }
    }

    #[test]
    fn 内置的_compose_副本与记录的校验和一致() {
        // 这条测试守的是「内置副本被人改过但忘了改校验和」
        assert_eq!(
            sha256_hex(bundled_compose().as_bytes()),
            BUNDLED_COMPOSE_SHA256
        );
        validate_compose(&bundled_compose()).expect("内置副本必须能通过自己的校验");
        // Windows 的 checkout 会把 LF 换成 CRLF，归一化之后不该再有 \r（M2 的 CI 实测撞过）
        assert!(!bundled_compose().contains('\r'), "内置副本里还有 CRLF");
    }

    #[test]
    fn sha256_对得上已知向量() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn 假的_compose_会被拦下() {
        assert!(validate_compose("").is_err());
        assert!(validate_compose("<html>404: Not Found</html>").is_err());
        let almost = format!("services:\n  web:\n{}", "# 填充\n".repeat(400));
        assert!(validate_compose(&almost).is_err(), "少了别的服务就该拦下");
    }

    /// **手工在 `~/.hunter/app` 里跑 compose 的人也要带上覆盖文件**（I11 · U5）。
    ///
    /// 2026-09-22 22:01 本地 Claude 在用户 Mac 上跑
    /// `docker compose -p hunter up -d` 漏了 `-f docker-compose.launcher.yml`，
    /// 容器被重建成 0.0.0.0 —— 五个端口在局域网可达了约 50 分钟。
    /// 这三行就是拿来兜住那种情形的：compose 会从项目目录的 `.env` 读它们。
    #[test]
    fn env_里带着_compose_自己的那三项() {
        let ports = Ports {
            web: 3100,
            api: 8100,
            opencode: 3921,
            postgres: 5442,
            redis: 6479,
        };
        let m =
            parse_env(&render_env(&input(&ports, "gateway"), &StickySecrets::default()).unwrap());
        assert_eq!(
            m.get("COMPOSE_PROJECT_NAME").map(String::as_str),
            Some(PROJECT)
        );
        let sep = m
            .get("COMPOSE_PATH_SEPARATOR")
            .expect("分隔符要写死，不赌 compose 的平台默认值");
        assert_eq!(sep, compose_path_separator());
        let files = m.get("COMPOSE_FILE").expect("COMPOSE_FILE 必须有");
        let parts: Vec<&str> = files.split(sep.as_str()).collect();
        assert_eq!(
            parts,
            vec!["docker-compose.yml", "docker-compose.launcher.yml"],
            "覆盖文件必须排在后面，否则 !override 覆盖不到（红线 4 就白写了）"
        );
    }

    /// 这三项要和 key、端口一样**写完读回来对账** —— 漏写一项，
    /// 手工跑一次 compose 就能把端口重新开到局域网上。
    #[test]
    fn 少了_compose_那三项就不算写成功() {
        let ports = Ports {
            web: 3100,
            api: 8100,
            opencode: 3921,
            postgres: 5442,
            redis: 6479,
        };
        let i = input(&ports, "gateway");
        let good = render_env(&i, &StickySecrets::default()).unwrap();
        verify_env_written(&good, &i).expect("正常渲染出来的要过");
        for key in [
            "COMPOSE_PROJECT_NAME",
            "COMPOSE_FILE",
            "COMPOSE_PATH_SEPARATOR",
        ] {
            let broken: String = good
                .lines()
                .filter(|l| !l.starts_with(&format!("{key}=")))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                verify_env_written(&broken, &i).is_err(),
                "{key} 被删掉了，对账应该失败"
            );
        }
    }

    #[test]
    fn env_渲染没有残留占位符且端口正确() {
        let ports = Ports {
            web: 3101,
            api: 8101,
            opencode: 3922,
            postgres: 5443,
            redis: 6480,
        };
        let s = render_env(&input(&ports, "gateway"), &StickySecrets::default()).unwrap();
        assert!(!s.contains("{{"), "还有占位符没替换：\n{s}");
        let m = parse_env(&s);
        assert_eq!(m.get("WEB_HOST_PORT").unwrap(), "3101");
        assert_eq!(m.get("API_HOST_PORT").unwrap(), "8101");
        assert_eq!(m.get("HUNTER_VERSION").unwrap(), "1.2.0");
        assert_eq!(m.get("HUNTER_REGISTRY").unwrap(), "ghcr.io/agentpit-io");
        assert_eq!(m.get("LLM_DEFAULT_MODEL").unwrap(), "hunter-chat");
        assert_eq!(m.get("HUNTER_API_KEY").unwrap(), FAKE_KEY);
        // 待办池 P0-3：留空，让 api 自己生成并写进 hunter_secrets 卷
        assert_eq!(m.get("HUNTER_INTERNAL_KEY").unwrap(), "");
    }

    #[test]
    fn jwt_secret_首次生成之后永不更换() {
        let ports = Ports::default();
        let first = render_env(&input(&ports, "gateway"), &StickySecrets::default()).unwrap();
        let jwt1 = parse_env(&first).get("JWT_SECRET").cloned().unwrap();
        let pg1 = parse_env(&first).get("POSTGRES_PASSWORD").cloned().unwrap();
        assert!(!jwt1.is_empty());

        let sticky = StickySecrets {
            jwt_secret: Some(jwt1.clone()),
            postgres_password: Some(pg1.clone()),
            opencode_pass: Some("keepme-0123456789".into()),
        };
        // 第二次渲染时连端口和模式都变了，三个密钥仍要原样保留
        let other = Ports {
            web: 3999,
            ..Ports::default()
        };
        let second = render_env(&input(&other, "gateway"), &sticky).unwrap();
        let m = parse_env(&second);
        assert_eq!(
            m.get("JWT_SECRET").unwrap(),
            &jwt1,
            "JWT_SECRET 变了用户会掉登录"
        );
        assert_eq!(
            m.get("POSTGRES_PASSWORD").unwrap(),
            &pg1,
            "改了口令连不上已初始化的数据卷"
        );
        assert_eq!(m.get("OPENCODE_PASS").unwrap(), "keepme-0123456789");
        assert_eq!(m.get("WEB_HOST_PORT").unwrap(), "3999", "端口该跟着变");
    }

    #[test]
    fn 不给_sticky_时两次生成的密钥不一样() {
        let ports = Ports::default();
        let a =
            parse_env(&render_env(&input(&ports, "gateway"), &StickySecrets::default()).unwrap());
        let b =
            parse_env(&render_env(&input(&ports, "gateway"), &StickySecrets::default()).unwrap());
        assert_ne!(a.get("JWT_SECRET"), b.get("JWT_SECRET"));
    }

    #[test]
    fn 自带_key_模式把全部模型名换成用户自己的() {
        let ports = Ports::default();
        let mut i = input(&ports, "own");
        i.llm_base_url = "https://api.deepseek.com/v1";
        i.llm_model = "deepseek-chat";
        i.llm_api_key = "sk-0123456789abcdef";
        i.schema_sanitize = true;
        let m = parse_env(&render_env(&i, &StickySecrets::default()).unwrap());
        assert_eq!(
            m.get("LLM_BASE_URL").unwrap(),
            "https://api.deepseek.com/v1"
        );
        assert_eq!(m.get("LLM_API_KEY").unwrap(), "sk-0123456789abcdef");
        assert_eq!(m.get("LLM_SCHEMA_SANITIZE").unwrap(), "1");
        // hunter key 仍然要在：工具与数据网关靠它（M0 §1.4 一把 key 通吃三个网关）
        assert_eq!(m.get("HUNTER_API_KEY").unwrap(), FAKE_KEY);
        // 深度分析的模型不能还留着 hunter-deep，那个别名在别人家网关上不存在
        assert_eq!(m.get("AGENT_SUB_RESEARCH_MODEL").unwrap(), "deepseek-chat");
        assert_eq!(m.get("ASSISTANT_MODEL_CHAT").unwrap(), "deepseek-chat");
    }

    #[test]
    fn 覆盖文件里五个发布端口全部绑本机() {
        let ports = Ports {
            web: 3101,
            api: 8101,
            opencode: 3922,
            postgres: 5443,
            redis: 6480,
        };
        let s = render_override(&ports, "docker.io/library", WebBind::Local);
        for (p, c) in [
            (3101, 3000),
            (8101, 8000),
            (3922, 3901),
            (5443, 5432),
            (6480, 6379),
        ] {
            assert!(
                s.contains(&format!("!override [\"127.0.0.1:{p}:{c}\"]")),
                "{p} 没有绑到 127.0.0.1：\n{s}"
            );
        }
        // 红线 4 的反向断言（I7 起连 web 也算）：一行裸端口映射都不许有
        // （只看真正的映射行，注释里提到 !override 的那几行不算）
        let mapping_lines: Vec<&str> = s
            .lines()
            .filter(|l| l.trim_start().starts_with("ports: !override"))
            .collect();
        assert_eq!(mapping_lines.len(), 5, "五个服务各一行：{mapping_lines:?}");
        for line in mapping_lines {
            assert!(line.contains("127.0.0.1:"), "这一行没绑本机：{line}");
        }
        // 上游那六个服务里 llm-shim 一个端口也不发布，所以「六个服务全部只绑本机」
        // 在覆盖文件里就是这五行 —— 剩下那一个没得可绑。
        //
        // I12 起 llm-shim **会**出现在这份文件里，但只为了 `restart:` 那一行：
        // 它名下不许有任何 `ports:`（有的话就是我们自己把一个内部服务发布出去了）
        let shim: Vec<&str> = s
            .lines()
            .skip_while(|l| l.trim() != "llm-shim:")
            .skip(1)
            .take_while(|l| l.starts_with("    "))
            .collect();
        assert!(
            shim.iter().all(|l| !l.contains("ports:")),
            "llm-shim 本来就不发布端口，覆盖文件里也不许给它加：{shim:?}"
        );
    }

    /// **六个服务一律 `restart: unless-stopped`**（I12 · R3+）。
    ///
    /// 2026-09-23 用户 Mac 上实测：内置运行时的虚拟机 stop / start 之后，
    /// 六个容器一个都没起来。上游的 compose 其实已经写了这一条（`x-restart` 锚点），
    /// 这里再写一遍是**把它钉在我们自己的文件里** —— 上游改了锚点也不受影响。
    #[test]
    fn 覆盖文件给六个服务都写了重启策略() {
        let ports = Ports {
            web: 3101,
            api: 8101,
            opencode: 3922,
            postgres: 5443,
            redis: 6480,
        };
        for bind in [WebBind::Local, WebBind::LegacyLan] {
            let s = render_override(&ports, "docker.io/library", bind);
            for svc in ["web", "api", "opencode", "llm-shim", "postgres", "redis"] {
                let body: Vec<&str> = s
                    .lines()
                    .skip_while(|l| l.trim() != format!("{svc}:"))
                    .skip(1)
                    .take_while(|l| l.starts_with("    "))
                    .collect();
                assert!(
                    body.iter().any(|l| l.trim() == "restart: unless-stopped"),
                    "{svc} 没有重启策略（{bind:?}）：{body:?}"
                );
            }
            // 只数**真正的映射行**：文件头的说明里也提到了这个字符串
            let policy_lines = s
                .lines()
                .filter(|l| l.trim() == "restart: unless-stopped" && l.starts_with("    "))
                .count();
            assert_eq!(policy_lines, 6, "六个服务各一行，不多不少：\n{s}");
        }
    }

    /// 用户 2026-09-21 19:05 的决定：免费版**只允许本机访问**。
    /// 这一条钉住「配置文件不再能放开它」—— `render_override` 只有 `WebBind` 一个入口，
    /// 而 `WebBind::LegacyLan` 只能由 `detect()` 从磁盘现状给出。
    #[test]
    fn 老机器的现状沿用而新装一律本机() {
        let ports = Ports {
            web: 3101,
            api: 8101,
            opencode: 3922,
            postgres: 5443,
            redis: 6480,
        };
        let local = render_override(&ports, "docker.io/library", WebBind::Local);
        assert!(
            local.contains("ports: !override [\"127.0.0.1:3101:3000\"]"),
            "{local}"
        );
        // 沿用老现状时 web 那一行是裸的，其余四行照样本机
        let legacy = render_override(&ports, "docker.io/library", WebBind::LegacyLan);
        assert!(
            legacy.contains("ports: !override [\"3101:3000\"]"),
            "{legacy}"
        );
        for (p, c) in [(8101, 8000), (3922, 3901), (5443, 5432), (6480, 6379)] {
            assert!(
                legacy.contains(&format!("!override [\"127.0.0.1:{p}:{c}\"]")),
                "{p} 没有绑到 127.0.0.1：\n{legacy}"
            );
        }

        // detect 的闭环：本机那一份读回来是 Local，老那一份读回来是 LegacyLan，
        // 于是「收紧」写出去之后就再也回不去了（单向）。
        assert_eq!(WebBind::from_override_text(&local), WebBind::Local);
        assert_eq!(WebBind::from_override_text(&legacy), WebBind::LegacyLan);
    }

    /// `detect` 认不出来的时候必须往**收紧**那一头倒。
    #[test]
    fn 读不出现状时一律按只绑本机() {
        // 空文件、没有 web 那一行、被手改坏了 —— 一律 Local
        for t in [
            "",
            "services:\n  api:\n    ports: !override [\"127.0.0.1:8101:8000\"]\n",
            "services:\n  web:\n    image: whatever\n",
            "# 只有注释\n#   web:\n#     ports: !override [\"3101:3000\"]\n",
            "乱七八糟的东西",
        ] {
            assert_eq!(
                WebBind::from_override_text(t),
                WebBind::Local,
                "认不出来时必须收紧：{t:?}"
            );
        }
        // 注释行里出现裸映射不算数（上面第四条已经覆盖），真正的那一行才算
        assert_eq!(
            WebBind::from_override_text(
                "services:\n  # web 也只绑本机\n  web:\n    ports: !override [\"127.0.0.1:3101:3000\"]\n"
            ),
            WebBind::Local
        );
    }

    /// I2 的 GUI 回归撞到的那个 P0（详见 `secretgen` 的模块头）：
    /// 0.1.1 及之前生成的 `POSTGRES_PASSWORD` 里可能有 `/`，会把上游拼出来的 DSN 截断。
    #[test]
    fn 认得出会把_dsn_截断的旧口令() {
        assert!(sticky_password_safe("aB3xYz09"));
        // 下面这两个是标准 base64 会产出的形状
        assert!(!sticky_password_safe("ab/cdEF012"));
        assert!(!sticky_password_safe("ab+cdEF012"));
        assert!(!sticky_password_safe("abcdEF012="));
        // 真出现在 I2 回归里的那一类：一个 `/` 就够。
        // URL 的 authority 到**第一个** `/` 为止（RFC 3986），所以口令里的 `/`
        // 会让解析器在那里就收尾，真正的主机与端口全被甩进 path ——
        // 实测的报错就是 `invalid integer value ... for connection option "port"`。
        let bad = "K7mQ1wZ/u3s9CkgP2vTnR4xLbJhA6eYd";
        assert!(!sticky_password_safe(bad));
        let authority = |dsn: &str| {
            dsn.split_once("://")
                .unwrap()
                .1
                .split(['/', '?', '#'])
                .next()
                .unwrap()
                .to_string()
        };
        assert_eq!(
            authority(&format!("postgresql://hunter:{bad}@postgres:5432/hunter")),
            "hunter:K7mQ1wZ",
            "带 `/` 的口令必须把 authority 截断 —— 这正是 api 起不来的原因"
        );
        // 换成只含字母数字的口令，authority 就是对的
        let good = "K7mQ1wZu3s9CkgP2vTnR4xLbJhA6eYd";
        assert!(sticky_password_safe(good));
        assert_eq!(
            authority(&format!("postgresql://hunter:{good}@postgres:5432/hunter")),
            format!("hunter:{good}@postgres:5432")
        );
    }

    /// I3（I2 报告第十节第 4 条的建议）：`.env` 写完要读回来逐项对账。
    /// 「写之前查」拦的是已知形状，「写完对账」拦的是**所有**形状。
    #[test]
    fn 写完的_env_读回来要逐项对得上() {
        let ports = Ports::default();
        let i = input(&ports, "gateway");
        let text = render_env(&i, &StickySecrets::default()).expect("正常输入要渲染得出来");
        verify_env_written(&text, &i).expect("自己渲染出来的当然要对得上");
    }

    #[test]
    fn 对账能抓到多出来的同名键() {
        let ports = Ports::default();
        let i = input(&ports, "gateway");
        let mut text = render_env(&i, &StickySecrets::default()).expect("渲染");
        // 模拟「不知怎么多写了一行」：解析器是后写覆盖先写，光看解析结果看不出来
        text.push_str(&format!("HUNTER_API_KEY={FAKE_KEY}\n"));
        let e = verify_env_written(&text, &i).expect_err("多一行同名键必须被抓到");
        assert!(e.msg.contains("HUNTER_API_KEY"), "{}", e.msg);
        assert!(e.msg.contains("2 次"), "{}", e.msg);
        assert!(
            !e.msg.contains(FAKE_KEY),
            "报错里不许带 key 的明文：{}",
            e.msg
        );
    }

    #[test]
    fn 对账能抓到被改过的值_并且报错里不带明文() {
        let ports = Ports::default();
        let i = input(&ports, "gateway");
        let text = render_env(&i, &StickySecrets::default())
            .expect("渲染")
            .replace(FAKE_KEY, "hunt_tools_别的东西");
        let e = verify_env_written(&text, &i).expect_err("值被换掉必须被抓到");
        assert!(e.msg.contains("HUNTER_API_KEY"), "{}", e.msg);
        assert!(
            !e.msg.contains(FAKE_KEY),
            "报错里不许带 key 的明文：{}",
            e.msg
        );
        assert!(
            !e.msg.contains("别的东西"),
            "读回来那个值也不许进报错：{}",
            e.msg
        );
    }

    #[test]
    fn 对账能抓到端口写漏或写错() {
        let ports = Ports {
            web: 3101,
            ..Default::default()
        };
        let i = input(&ports, "gateway");
        let text = render_env(&i, &StickySecrets::default()).expect("渲染");
        verify_env_written(&text, &i).expect("端口没动时要对得上");

        let 改坏 = text.replace("WEB_HOST_PORT=3101", "WEB_HOST_PORT=3100");
        let e = verify_env_written(&改坏, &i).expect_err("端口被改必须被抓到");
        assert!(e.msg.contains("WEB_HOST_PORT"), "{}", e.msg);

        let 删掉: String = text
            .lines()
            .filter(|l| !l.starts_with("API_HOST_PORT="))
            .collect::<Vec<_>>()
            .join("\n");
        let e2 = verify_env_written(&删掉, &i).expect_err("少一项必须被抓到");
        assert!(e2.msg.contains("API_HOST_PORT"), "{}", e2.msg);
        assert!(e2.msg.contains("少了"), "{}", e2.msg);
    }

    /// I7：`web_bind` 退成遗迹之后，**它的取值不该再影响任何输出**。
    #[test]
    fn 手改_web_bind_也放不开局域网() {
        let h = HunterSection::default();
        assert_eq!(h.web_bind, WEB_BIND_LOCAL, "新配置的默认值是只绑本机");

        // 老的 launcher.toml 里根本没有这一项 / 写着 all / 写着 0.0.0.0 / 拼错了 ——
        // 四种都要读得回来（读不回来会把用户的其他设置一起丢掉），
        // 而且**四种渲染出来的东西一模一样**：web 也绑 127.0.0.1。
        let base = "[hunter]\ntag = \"1.2.0\"\nregistry_id = \"ghcr\"\nregistry_prefix = \"ghcr.io/agentpit-io\"\nbase_prefix = \"docker.io/library\"\n";
        let tail = "[hunter.ports]\nweb = 3100\napi = 8100\nopencode = 3921\npostgres = 5442\nredis = 6479\n";
        let expect = render_override(&Ports::default(), "docker.io/library", WebBind::Local);
        for line in [
            "".to_string(),
            format!("web_bind = \"{WEB_BIND_ALL}\"\n"),
            "web_bind = \"0.0.0.0\"\n".to_string(),
            "web_bind = \"loacl\"\n".to_string(),
        ] {
            let c: LauncherConfig =
                toml::from_str(&format!("{base}{line}{tail}")).expect("老配置要读得回来");
            assert_eq!(
                render_override(&c.hunter.ports, &c.hunter.base_prefix, WebBind::Local),
                expect,
                "web_bind = {line:?} 不该改变渲染结果"
            );
        }

        // 而且 `HunterSection` 上**不再有**任何「是不是只绑本机」的方法可供调用 ——
        // 唯一的来源是 `WebBind::detect()`，它读的是磁盘现状。这一条靠编译保证：
        // 如果哪天有人把 `web_local_only()` 加回来，上面那些调用点会重新出现。
    }

    /// I2 自审：会原样写进 `.env` 的值里不许有换行 —— 一个换行等于多定义一个环境变量。
    #[test]
    fn 模型名里有换行时写_env_要报错而不是悄悄注入() {
        let ports = Ports::default();
        let bad = EnvInput {
            tag: "1.2.0",
            registry_prefix: "ghcr.io/agentpit-io",
            ports: &ports,
            hunter_key: "hunt_tools_x",
            model_mode: "own",
            llm_base_url: "https://api.deepseek.com/v1",
            llm_model: "deepseek-chat\nHUNTER_API_KEY=injected",
            llm_api_key: "sk-abc",
            schema_sanitize: false,
        };
        let e = render_env(&bad, &StickySecrets::default()).expect_err("带换行的模型名必须被拒");
        assert!(e.msg.contains("换行"), "{}", e.msg);

        // 正常的值照样能渲染出来，而且渲染结果里只有一行 HUNTER_API_KEY
        let ok = EnvInput {
            llm_model: "deepseek-chat",
            ..bad
        };
        let out = render_env(&ok, &StickySecrets::default()).expect("正常的值应当能渲染");
        assert_eq!(
            out.lines()
                .filter(|l| l.starts_with("HUNTER_API_KEY="))
                .count(),
            1
        );
    }

    #[test]
    fn 国内源下_postgres_与_redis_也换成镜像地址() {
        let ports = Ports::default();
        let s = render_override(&ports, "hkccr.ccs.tencentyun.com/agentpit", WebBind::Local);
        assert!(
            s.contains("image: hkccr.ccs.tencentyun.com/agentpit/postgres:16-alpine"),
            "{s}"
        );
        assert!(
            s.contains("image: hkccr.ccs.tencentyun.com/agentpit/redis:7-alpine"),
            "{s}"
        );
        // GHCR 下则保持官方库的写法
        let s2 = render_override(&ports, "docker.io/library", WebBind::Local);
        assert!(s2.contains("image: postgres:16-alpine"), "{s2}");
        assert!(
            !s2.contains("docker.io/library/postgres"),
            "官方库不该写全路径：{s2}"
        );
    }

    #[test]
    fn 镜像清单() {
        let v = images("ghcr.io/agentpit-io", "docker.io/library", "1.2.0");
        assert_eq!(v.len(), 6);
        let web = v.iter().find(|i| i.service == "web").unwrap();
        assert_eq!(
            web.reference,
            "ghcr.io/agentpit-io/hunter-community-web:1.2.0"
        );
        assert_eq!(web.short_ref, "hunter-community-web");
        assert_eq!(web.host, "ghcr.io");
        assert_eq!(web.repo, "agentpit-io/hunter-community-web");
        let pg = v.iter().find(|i| i.service == "postgres").unwrap();
        assert_eq!(pg.reference, "docker.io/library/postgres:16-alpine");
        assert_eq!(pg.host, "registry-1.docker.io");
        assert_eq!(pg.repo, "library/postgres");

        let cn = images(
            "hkccr.ccs.tencentyun.com/agentpit",
            "hkccr.ccs.tencentyun.com/agentpit",
            "1.2.0",
        );
        let pg = cn.iter().find(|i| i.service == "postgres").unwrap();
        assert_eq!(
            pg.reference,
            "hkccr.ccs.tencentyun.com/agentpit/postgres:16-alpine"
        );
        assert_eq!(pg.host, "hkccr.ccs.tencentyun.com");
    }

    #[test]
    fn 镜像引用拆主机名() {
        assert_eq!(
            split_ref("postgres"),
            ("registry-1.docker.io".into(), "library/postgres".into())
        );
        assert_eq!(
            split_ref("docker.io/library/redis"),
            ("registry-1.docker.io".into(), "library/redis".into())
        );
        assert_eq!(split_ref("ghcr.io/a/b"), ("ghcr.io".into(), "a/b".into()));
        assert_eq!(
            split_ref("localhost:5000/x"),
            ("localhost:5000".into(), "x".into())
        );
    }

    #[test]
    fn 端口冲突时自动往上挪且不撞车() {
        // 真占住一个端口，再看解析结果。
        // **绑通配地址**：三个平台对「通配 + 具体地址共存」的态度不一样
        // （Windows 的 SO_REUSEADDR 甚至允许抢占），绑 0.0.0.0 才是三平台一致的「真占住」。
        // 这一条要考的是 resolve_ports 的逻辑，不是各家内核的绑定语义。
        let a = TcpListener::bind("0.0.0.0:0").unwrap();
        let pa = a.local_addr().unwrap().port();
        let want = Ports {
            web: 3100,
            api: pa,
            opencode: 3921,
            postgres: 5442,
            redis: 6479,
        };
        let sv = crate::ports::Survey::empty();
        let (got, changes) = resolve_ports_with(&sv, &want, &[]).unwrap();
        assert_ne!(got.api, pa, "被占的端口必须换掉");
        assert!(changes.iter().any(|c| c.service == "api" && c.wanted == pa));
        // 五个端口互不相同
        let mut v = vec![got.web, got.api, got.opencode, got.postgres, got.redis];
        v.sort_unstable();
        v.dedup();
        assert_eq!(v.len(), 5);
        drop(a);
    }

    #[test]
    fn 端口没冲突时不产生改动记录() {
        let l1 = TcpListener::bind("127.0.0.1:0").unwrap();
        let free = l1.local_addr().unwrap().port();
        drop(l1);
        let want = Ports {
            web: free,
            api: free + 1,
            opencode: free + 2,
            postgres: free + 3,
            redis: free + 4,
        };
        let sv = crate::ports::Survey::empty();
        let (got, changes) = resolve_ports_with(&sv, &want, &[]).unwrap();
        if changes.is_empty() {
            assert_eq!(got, want);
        }
    }

    /// 第二次打开启动器时，端口正被**我们自己的容器**占着。
    /// 要是把它也算成冲突，每开一次就往上挪一格，用户存的书签全失效（M2 用例 9 实测撞出来的）。
    #[test]
    fn 自己这一套占着的端口不算冲突() {
        let l = TcpListener::bind("0.0.0.0:0").unwrap();
        let p = l.local_addr().unwrap().port();
        let want = Ports {
            web: 3100,
            api: p,
            opencode: 3921,
            postgres: 5442,
            redis: 6479,
        };

        // 不告诉它这是自己的 → 换端口
        let sv = crate::ports::Survey::empty();
        let (got, changes) = resolve_ports_with(&sv, &want, &[]).unwrap();
        assert_ne!(got.api, p);
        assert!(!changes.is_empty());

        // 告诉它这是自己的 → 原样保留，也不产生「端口已改」的提示
        let (got, changes) = resolve_ports_with(&sv, &want, &[p]).unwrap();
        assert_eq!(got.api, p, "自己占着的端口应当原样保留");
        assert!(
            changes.iter().all(|c| c.service != "api"),
            "不该报 api 换过端口"
        );
        drop(l);
    }

    #[test]
    fn env_解析能吃注释与_export() {
        let m = parse_env("# 注释\n\nexport A=1\nB = 2 \nC=\"带引号\"\n坏行\n");
        assert_eq!(m.get("A").unwrap(), "1");
        assert_eq!(m.get("B").unwrap(), "2");
        assert_eq!(m.get("C").unwrap(), "带引号");
        assert!(!m.contains_key("坏行"));
    }

    #[test]
    fn set_kv_替换已有键也能追加新键() {
        let s = "A=1\nB=2\n";
        assert_eq!(set_kv(s, "A", "9"), "A=9\nB=2\n");
        assert_eq!(set_kv(s, "C", "3"), "A=1\nB=2\nC=3\n");
    }

    #[test]
    fn launcher_toml_能来回序列化() {
        let mut c = LauncherConfig::default();
        c.hunter.ports.web = 3101;
        c.model.mode = "own".into();
        c.model.base_url = "https://api.deepseek.com/v1".into();
        c.install.done = true;
        let s = toml::to_string_pretty(&c).unwrap();
        // 红线 2：toml 里绝不能出现 key
        assert!(!s.contains("hunt_tools_"), "{s}");
        assert!(!s.to_lowercase().contains("api_key"), "{s}");
        let back: LauncherConfig = toml::from_str(&s).unwrap();
        assert_eq!(back.hunter.ports.web, 3101);
        assert_eq!(back.model.mode, "own");
        assert!(back.install.done);
    }

    #[test]
    fn 缺字段的_toml_用默认值补齐() {
        let c: LauncherConfig = toml::from_str("[launcher]\nlocale = \"en\"\nversion = \"0.1.0\"\nautostart = false\ncheck_update_hours = 24\n").unwrap();
        // `install.offline` 是 M4 新加的字段：老的 launcher.toml 里没有它，
        // 读回来必须是 false 而不是报错（#[serde(default)] 保证这一点）。
        assert!(!c.install.offline);
        assert_eq!(c.launcher.locale, "en");
        assert_eq!(c.hunter.ports.web, 3100, "没写的段要落到默认值");
        assert!(!c.install.done);
    }

    /// 待办池 P2-11 + P2-16：`~/.hunter` 下由启动器写出来的文件一律 600，不只是 `.env`。
    /// I3 把最后那个漏网的 `app/VERSION` 也收进来了（原来跟着 umask 走，测试机上是 644）。
    #[test]
    #[cfg(unix)]
    fn 启动器写出来的五个文件都是_600() {
        use std::os::unix::fs::PermissionsExt;
        // 全进程只有一把（`paths::env_lock`）。这里原来是模块私有的一把 ——
        // 于是 dockercfg 那边可以同时把 HUNTER_HOME 指到别处、再把那个目录删掉，
        // 本条测试就会在 write 成功之后 chmod 报「文件不存在」（macOS runner 上真撞到了）
        let _g = crate::paths::env_lock();

        let old = std::env::var("HUNTER_HOME").ok();
        let dir = std::env::temp_dir().join(format!("hunter-perm600-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("HUNTER_HOME", &dir);

        let ports = Ports::default();
        LauncherConfig::default().save().expect("写 launcher.toml");
        write_env(&input(&ports, "gateway")).expect("写 .env");
        write_override(&ports, "docker.io/library").expect("写覆盖文件");
        write_compose("services: {}\n").expect("写 compose");
        crate::flow::write_version_file("1.2.0");

        for p in [
            paths::launcher_toml(),
            paths::env_file(),
            paths::override_file(),
            paths::compose_file(),
            paths::version_file(),
        ] {
            let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{} 应当是 600，实际 {mode:o}", p.display());
        }

        let _ = std::fs::remove_dir_all(&dir);
        match old {
            Some(v) => std::env::set_var("HUNTER_HOME", v),
            None => std::env::remove_var("HUNTER_HOME"),
        }
    }

    /// I14 · F3：「只删除应用，保留数据」留下来的 `.env` 里那把 key 要读得出来。
    ///
    /// 0.1.13 的重装流程根本没去读它，`--auto -y` 照样报
    /// `E_KEY_INVALID: 标准输入不是终端…用 --key-file` ——
    /// 而界面上刚刚才承诺过「以后重新安装会直接沿用」。
    #[test]
    fn 保留下来的_env_里那把_key_读得出来() {
        let _h = paths::test_home("kept-key");
        assert_eq!(kept_hunter_key(), None, "什么都没有时不该编一把出来");

        std::fs::write(
            paths::env_file(),
            format!("# 注释\nPOSTGRES_PASSWORD=abc\nHUNTER_API_KEY={FAKE_KEY}\nJWT_SECRET=x\n"),
        )
        .unwrap();
        assert_eq!(kept_hunter_key().as_deref(), Some(FAKE_KEY));
    }

    /// 格式都不对的东西**不算**「上次保留的 key」——
    /// 拿它去问网关只会换回一句「格式不对」，而界面上会先说一句
    /// 「沿用上次保留的 key」，那就是在骗人。
    #[test]
    fn 格式不对的不算保留下来的_key() {
        let _h = paths::test_home("kept-key-bad");
        for bad in [
            "",
            "   ",
            "sk-0123456789abcdef",
            "hunt_tools_",
            // 少一位
            "hunt_tools_q6sKaaaaaaaaaaaaaaaaaaaaaaaaQMo",
            // 前缀不对
            "hunt_tool_q6sKaaaaaaaaaaaaaaaaaaaaaaaaQMo2",
        ] {
            std::fs::write(paths::env_file(), format!("HUNTER_API_KEY={bad}\n")).unwrap();
            assert_eq!(
                kept_hunter_key(),
                None,
                "这一条不该被当成可沿用的 key：{bad}"
            );
        }
    }

    /// 「什么都没有」和「有一个但形状不对」要分得开（I14 收尾时自己撞出来的）。
    ///
    /// 两者 `kept_hunter_key()` 都回 `None`，可是该对用户说的话完全不一样：
    /// 前者一个字都不用说，后者不说就成了「我明明留着 key，它却让我重新输」——
    /// 和 F3 原来那个坏法是同一个形状（**事情做了，只是没说出来**）。
    #[test]
    fn 什么都没有和形状不对要分得开() {
        let _h = paths::test_home("kept-key-state");

        assert!(
            matches!(kept_hunter_key_state(), KeptKeyState::Absent),
            ".env 都不存在时是 Absent"
        );

        std::fs::write(paths::env_file(), "POSTGRES_PASSWORD=abc\n").unwrap();
        assert!(
            matches!(kept_hunter_key_state(), KeptKeyState::Absent),
            "有 .env 但没有 HUNTER_API_KEY 这一项，也是 Absent"
        );

        std::fs::write(paths::env_file(), "HUNTER_API_KEY=   \n").unwrap();
        assert!(
            matches!(kept_hunter_key_state(), KeptKeyState::Absent),
            "空白值等于没有"
        );

        // 真实的坏法：粘贴时少了一截
        let 截断 = &FAKE_KEY[..FAKE_KEY.len() - 3];
        std::fs::write(paths::env_file(), format!("HUNTER_API_KEY={截断}\n")).unwrap();
        match kept_hunter_key_state() {
            KeptKeyState::BadShape(masked) => {
                // 红线 2：形状不对的也照样打码 —— 它很可能就是一把真 key 被截断了
                assert!(
                    !masked.contains(&截断[11..]),
                    "打码后不该露出 key 的中段：{masked}"
                );
                assert!(masked.contains("hunt_tools_"), "打码后仍要看得出它是什么：{masked}");
            }
            _ => panic!("留着一个形状不对的 key，应当是 BadShape"),
        }

        std::fs::write(paths::env_file(), format!("HUNTER_API_KEY={FAKE_KEY}\n")).unwrap();
        assert!(
            matches!(kept_hunter_key_state(), KeptKeyState::Usable(k) if k == FAKE_KEY),
            "格式对得上的才是 Usable"
        );
    }

    /// 看一眼形状不对的那个值，**不许把打码函数自己弄坏**。
    ///
    /// 第一版在 `BadShape` 那一支里顺手 `register_secret` 了一下，理由是
    /// 「它可能是一把真 key 被截断了」。可登记表是整个进程共用的，
    /// 而最常见的坏法恰恰是 `HUNTER_API_KEY=hunt_tools_`（key 正文被删干净）——
    /// 于是 `hunt_tools_` 这个**前缀本身**进了登记表，此后每一行
    /// `hunt_tools_****` 都会被再替换一次，变成 `hun********`。
    /// 一跑单测当场红了五个（`redact` / `err` / `log` / `feedback` 各一）。
    #[test]
    fn 看过形状不对的值之后打码函数还是好的() {
        let _h = paths::test_home("kept-key-no-poison");
        for bad in ["hunt_tools_", "sk-0123456789abcdef", "hunt_tool_abcdefghij"] {
            std::fs::write(paths::env_file(), format!("HUNTER_API_KEY={bad}\n")).unwrap();
            assert!(matches!(kept_hunter_key_state(), KeptKeyState::BadShape(_)));
        }
        let line = format!("LLM_API_KEY={FAKE_KEY} 写入 .env");
        let r = crate::redact::redact(&line);
        assert!(
            r.contains("hunt_tools_****"),
            "登记表被弄脏了，打码结果成了：{r}"
        );
        assert!(!r.contains("q6sK"), "{r}");
    }
}
