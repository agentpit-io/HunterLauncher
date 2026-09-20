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
use std::net::TcpListener;
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
    /// web 端口绑在哪张网卡上。`all` = 所有网卡（同一网络里的人都能打开）；
    /// `local` = 只有本机。**默认 `all`**，见下。
    ///
    /// ## 为什么默认是 `all`，以及为什么这是一个开关而不是一个决定
    ///
    /// 总控规则红线 4 的原话是「除 web 之外的端口一律绑 127.0.0.1」——
    /// web **被明确排除在外**，也就是说「对外」是既定的产品设计，不是疏忽。
    ///
    /// I1 实测了它的后果并记在报告第七节：从另一台机器打
    /// `http://<这台机器的 IP>:3101`，不需要任何凭证就能进聊天界面、
    /// 调工具、烧掉用户当天的额度（上游 api 保护住了 `/api/setup/*` 那些**配置**接口，
    /// 但没有保护**使用**界面）。
    ///
    /// 改默认值属于改产品决策，本项目单方面定不了。I2 做的是**把开关做出来**：
    /// 配置项、设置页的勾选、覆盖文件里的渲染、单测全都齐了，
    /// 想收紧的用户现在点一下就能收紧；**默认行为一个字节没动**。
    /// 「默认该是哪一个、已装机器要不要在升级时自动收紧」仍然是待办池 P1-20，
    /// 等用户决定。
    #[serde(default = "default_web_bind")]
    pub web_bind: String,
}

fn default_web_bind() -> String {
    WEB_BIND_ALL.into()
}

/// web 绑所有网卡（默认）。
pub const WEB_BIND_ALL: &str = "all";
/// web 只绑本机回环。
pub const WEB_BIND_LOCAL: &str = "local";

impl HunterSection {
    /// web 端口是不是只听本机。认不出来的取值一律按默认（`all`）处理 ——
    /// 手改配置文件写错一个字不该悄悄改变安全边界的**方向**，
    /// 而「继续保持现状」比「悄悄收紧」更不容易让人摸不着头脑。
    pub fn web_local_only(&self) -> bool {
        self.web_bind == WEB_BIND_LOCAL
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
}

impl LauncherConfig {
    /// 读 `launcher.toml`。不存在或读坏了都返回默认值 —— 一个坏掉的配置文件
    /// 不该让用户连界面都打不开。读坏时会写一条 warn 日志。
    pub fn load() -> Self {
        let p = paths::launcher_toml();
        match std::fs::read_to_string(&p) {
            Err(_) => Self::default(),
            Ok(s) => match toml::from_str::<Self>(&s) {
                Ok(c) => c,
                Err(e) => {
                    crate::lwarn!("{} 解析失败，这次用默认设置：{e}", p.display());
                    Self::default()
                }
            },
        }
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
}

/// 端口是不是能用。web 要对外监听，所以按 `0.0.0.0` 试；其余按 `127.0.0.1` 试
/// （红线 4：除 web 外都只绑本机）。绑不上就算被占。
pub fn port_free(port: u16, all_interfaces: bool) -> bool {
    let addr = if all_interfaces {
        ("0.0.0.0", port)
    } else {
        ("127.0.0.1", port)
    };
    TcpListener::bind(addr).is_ok()
}

/// 从 `want` 开始往上找一个没被占的端口，最多找 200 个。
/// `taken` 里的一律跳过；`own` 里的（现在正被**我们自己这一套**占着的）一律当成可用。
fn next_free(want: u16, all_interfaces: bool, taken: &[u16], own: &[u16]) -> AppResult<u16> {
    let mut p = want;
    for _ in 0..200 {
        if !taken.contains(&p) && (own.contains(&p) || port_free(p, all_interfaces)) {
            return Ok(p);
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
pub fn resolve_ports(
    want: &Ports,
    own: &[u16],
    web_local_only: bool,
) -> AppResult<(Ports, Vec<PortChange>)> {
    let mut taken: Vec<u16> = Vec::new();
    let mut changes = Vec::new();
    let mut out = want.clone();

    for (name, wanted) in want.as_pairs() {
        // web 默认要对外监听，所以按 0.0.0.0 试；设置里收紧成「只有本机」之后
        // 它和别的服务一样只要 127.0.0.1 占得住就行（I2 加的开关）
        let all_if = name == "web" && !web_local_only;
        let actual = next_free(wanted, all_if, &taken, own)?;
        taken.push(actual);
        if actual != wanted {
            changes.push(PortChange {
                service: name.to_string(),
                wanted,
                actual,
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

/// 写 `.env` 并把权限收成 600（红线 2）。
pub fn write_env(input: &EnvInput) -> AppResult<()> {
    paths::ensure_dirs()?;
    let path = paths::env_file();
    let sticky = read_sticky(&path);
    let content = render_env(input, &sticky)?;
    std::fs::write(&path, content).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("写 {} 失败：{e}", path.display()),
        )
    })?;
    paths::chmod_600(&path)?;
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

/// 生成 `docker-compose.launcher.yml`。
///
/// 两件事：
/// 1. **除 web 之外的端口一律绑 `127.0.0.1`**（红线 4）。`!override` 标签是必须的 ——
///    compose 对 `ports` 默认做追加合并，不加标签会变成「既听 0.0.0.0 又听 127.0.0.1」（M0 §3.4 实测）。
/// 2. 用国内源时把 `postgres` / `redis` 的 `image:` 也改写过去。
///    另外四个服务的镜像地址由 `.env` 的 `HUNTER_REGISTRY` 控制，不用在这里写。
pub fn render_override(ports: &Ports, base_prefix: &str, web_local_only: bool) -> String {
    let mut s = String::new();
    s.push_str(
        "# ~/.hunter/app/docker-compose.launcher.yml\n\
         # 由 Hunter 启动器生成 · 请勿手改（改了下次启动会被覆盖）\n\
         # 作用：1) 把除 web 之外的端口全部收回 127.0.0.1（总控规则红线 4）\n\
         #       2) 落实端口冲突改写后的值\n\
         #       3) 按设置决定 web 端口绑所有网卡还是只绑本机\n\
         #       4) 用国内镜像源时改写 postgres / redis 的镜像地址\n\
         #\n\
         # `!override` 标签是必须的：compose 默认对 ports 做**追加**合并，不加这个标签会变成\n\
         # 「既监听 0.0.0.0 又监听 127.0.0.1」，红线 4 就白写了（M0 §3.4 已实测验证）。\n\
         services:\n",
    );
    // web 是唯一一个可以对外的端口（红线 4 把它明确排除在「只绑本机」之外）。
    // 设置页里勾上「只允许本机访问」之后这里会多一个 127.0.0.1: 前缀，默认不勾。
    if web_local_only {
        s.push_str("  # 设置里勾了「只允许这台电脑访问」：web 也收回本机\n");
        s.push_str(&format!(
            "  web:\n    ports: !override [\"127.0.0.1:{}:3000\"]\n",
            ports.web
        ));
    } else {
        s.push_str("  # 默认：web 绑所有网卡，同一网络里的其他设备也能打开（见设置页的说明）\n");
        s.push_str(&format!(
            "  web:\n    ports: !override [\"{}:3000\"]\n",
            ports.web
        ));
    }
    s.push_str(&format!(
        "  api:\n    ports: !override [\"127.0.0.1:{}:8000\"]\n",
        ports.api
    ));
    s.push_str(&format!(
        "  opencode:\n    ports: !override [\"127.0.0.1:{}:3901\"]\n",
        ports.opencode
    ));

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
        "  postgres:\n    image: {pg_image}\n    ports: !override [\"127.0.0.1:{}:5432\"]\n",
        ports.postgres
    ));
    s.push_str(&format!(
        "  redis:\n    image: {redis_image}\n    ports: !override [\"127.0.0.1:{}:6379\"]\n",
        ports.redis
    ));
    s
}

pub fn write_override(ports: &Ports, base_prefix: &str, web_local_only: bool) -> AppResult<()> {
    paths::ensure_dirs()?;
    let p = paths::override_file();
    std::fs::write(&p, render_override(ports, base_prefix, web_local_only))
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
    fn 覆盖文件只有_web_对外其余都绑本机() {
        let ports = Ports {
            web: 3101,
            api: 8101,
            opencode: 3922,
            postgres: 5443,
            redis: 6480,
        };
        let s = render_override(&ports, "docker.io/library", false);
        assert!(s.contains("ports: !override [\"3101:3000\"]"), "{s}");
        for (p, c) in [(8101, 8000), (3922, 3901), (5443, 5432), (6480, 6379)] {
            assert!(
                s.contains(&format!("!override [\"127.0.0.1:{p}:{c}\"]")),
                "{p} 没有绑到 127.0.0.1：\n{s}"
            );
        }
        // 红线 4 的反向断言：除 web 那一行外不能出现任何裸端口映射
        // （只看真正的映射行，注释里提到 !override 的那几行不算）
        let mapping_lines: Vec<&str> = s
            .lines()
            .filter(|l| l.trim_start().starts_with("ports: !override"))
            .collect();
        assert_eq!(mapping_lines.len(), 5, "五个服务各一行：{mapping_lines:?}");
        for line in mapping_lines {
            assert!(
                line.contains("127.0.0.1:") || line.contains("\"3101:3000\""),
                "这一行没绑本机：{line}"
            );
        }
    }

    /// 待办池 P1-20 的开关。**默认必须还是对外**（那是既定设计，改默认要用户拍板），
    /// 勾上之后 web 才多一个 127.0.0.1 前缀。
    #[test]
    fn web_只允许本机时覆盖文件多一个本机前缀() {
        let ports = Ports {
            web: 3101,
            api: 8101,
            opencode: 3922,
            postgres: 5443,
            redis: 6480,
        };
        let open = render_override(&ports, "docker.io/library", false);
        assert!(open.contains("ports: !override [\"3101:3000\"]"), "{open}");
        assert!(
            !open.contains("127.0.0.1:3101:3000"),
            "默认不该收紧：{open}"
        );

        let local = render_override(&ports, "docker.io/library", true);
        assert!(
            local.contains("ports: !override [\"127.0.0.1:3101:3000\"]"),
            "{local}"
        );
        // 收紧之后五行全是本机
        for line in local
            .lines()
            .filter(|l| l.trim_start().starts_with("ports: !override"))
        {
            assert!(line.contains("127.0.0.1:"), "这一行没绑本机：{line}");
        }
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

    #[test]
    fn web_bind_的默认值是对外() {
        let h = HunterSection::default();
        assert_eq!(h.web_bind, WEB_BIND_ALL);
        assert!(
            !h.web_local_only(),
            "默认必须是对外（总控规则红线 4 的既定设计）"
        );
        // 老的 launcher.toml 里根本没有这一项，读回来也得是默认值
        let c: LauncherConfig = toml::from_str("[hunter]\ntag = \"1.2.0\"\nregistry_id = \"ghcr\"\nregistry_prefix = \"ghcr.io/agentpit-io\"\nbase_prefix = \"docker.io/library\"\n[hunter.ports]\nweb = 3100\napi = 8100\nopencode = 3921\npostgres = 5442\nredis = 6479\n").expect("老配置要读得回来");
        assert!(!c.hunter.web_local_only(), "升级上来的机器不该被悄悄收紧");
        // 写错了也按默认走，不悄悄改变方向
        let h2 = HunterSection {
            web_bind: "loacl".into(),
            ..Default::default()
        };
        assert!(!h2.web_local_only());
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
        let s = render_override(&ports, "hkccr.ccs.tencentyun.com/agentpit", false);
        assert!(
            s.contains("image: hkccr.ccs.tencentyun.com/agentpit/postgres:16-alpine"),
            "{s}"
        );
        assert!(
            s.contains("image: hkccr.ccs.tencentyun.com/agentpit/redis:7-alpine"),
            "{s}"
        );
        // GHCR 下则保持官方库的写法
        let s2 = render_override(&ports, "docker.io/library", false);
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
        // 真占住两个端口，再看解析结果
        let a = TcpListener::bind("127.0.0.1:0").unwrap();
        let pa = a.local_addr().unwrap().port();
        let want = Ports {
            web: 3100,
            api: pa,
            opencode: 3921,
            postgres: 5442,
            redis: 6479,
        };
        let (got, changes) = resolve_ports(&want, &[], false).unwrap();
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
        let (got, changes) = resolve_ports(&want, &[], false).unwrap();
        if changes.is_empty() {
            assert_eq!(got, want);
        }
    }

    /// 第二次打开启动器时，端口正被**我们自己的容器**占着。
    /// 要是把它也算成冲突，每开一次就往上挪一格，用户存的书签全失效（M2 用例 9 实测撞出来的）。
    #[test]
    fn 自己这一套占着的端口不算冲突() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        let want = Ports {
            web: 3100,
            api: p,
            opencode: 3921,
            postgres: 5442,
            redis: 6479,
        };

        // 不告诉它这是自己的 → 换端口
        let (got, changes) = resolve_ports(&want, &[], false).unwrap();
        assert_ne!(got.api, p);
        assert!(!changes.is_empty());

        // 告诉它这是自己的 → 原样保留，也不产生「端口已改」的提示
        let (got, changes) = resolve_ports(&want, &[p], false).unwrap();
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

    /// 待办池 P2-11：`~/.hunter` 下由启动器写出来的文件一律 600，不只是 `.env`。
    #[test]
    #[cfg(unix)]
    fn 启动器写出来的四个文件都是_600() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::{Mutex, OnceLock};
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        let _g = L
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let old = std::env::var("HUNTER_HOME").ok();
        let dir = std::env::temp_dir().join(format!("hunter-perm600-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("HUNTER_HOME", &dir);

        let ports = Ports::default();
        LauncherConfig::default().save().expect("写 launcher.toml");
        write_env(&input(&ports, "gateway")).expect("写 .env");
        write_override(&ports, "docker.io/library", false).expect("写覆盖文件");
        write_compose("services: {}\n").expect("写 compose");

        for p in [
            paths::launcher_toml(),
            paths::env_file(),
            paths::override_file(),
            paths::compose_file(),
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
}
