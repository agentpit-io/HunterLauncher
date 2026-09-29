//! 「现状复查」：**只读地**问一句「Hunter 现在到底在不在跑」（I11 · U1 / U2）。
//!
//! ## 为什么要有这一层
//!
//! 0.1.9 在用户 Mac 上的现场（2026-09-22）：
//!
//! ```text
//! 21:39  E_START_TIMEOUT（opencode 不健康，根因是虚拟机没有 DNS）→ 界面进「出错了」
//! 22:05  本地 Claude 在外部把 DNS 修好 → 6/6 健康、web 127.0.0.1:3100 返回 200
//! 22:42  AI 又读了一遍 opencode 日志，规则层 unknown —— 界面还停在「出错了」
//! 22:46  用户点「重试」→ 重新取 compose、重写 .env、开始重新拉 849 MB
//! ```
//!
//! 两件事都错了：
//!
//! 1. **界面不看现状。** 服务在 41 分钟前就好了，那张失败卡片却一直挂着。
//! 2. **「重试」等于「从头装一遍」。** 而这台机器上该有的东西一件不缺 ——
//!    重拉 + `up -d` 只会把正在跑的容器重建一遍，白下几百兆、白占几个 G。
//!
//! 这个模块回答的就是那句该先问的话。**它只看、不改**：`docker compose ps`、
//! 一次本机 HTTP GET、（深查时）一个 `--rm` 的一次性容器。
//! 本项目的容器、卷、配置文件一个字节都不碰。
//!
//! ## 六种结论，对应六种做法
//!
//! **`Stopped` 是 I16 补的，来自客户 2026-09-26 那份 Windows 诊断包。**
//! 他那台机器上 `docker compose ps` 六个容器全是 `exited`，
//! 0.1.15 却判成 `Partial` 并且说「Hunter 在跑，但 web、api…还没就绪、网页打不开」。
//! `exited`（退出了）和「起来了但健康检查没过」是两回事，**处置也完全不同**：
//! 前者该点「启动」，后者该等一等或者看日志。把前者说成后者，
//! 等于把用户往错误的方向引 —— 而界面上还同时出现了
//! 「Hunter 运行中」＋「v1.2.2 · 容器已停止」这种自相矛盾的一屏。
//!
//! **`RuntimeDown` 是 R1 补的，来自用户 2026-09-28 报的一幕**（它同时也是
//! I15 任务书的 P0-1，2026-09-24 在另一台 Mac 上真实发生过）：
//!
//! ```text
//! 不起 Docker 打开软件 → 又走了一遍安装流程 → 重新填 key
//! ```
//!
//! 磁盘上 `docker-compose.yml` 与 `.env` 都在（`installed_on_disk()` 刚返回 `true`），
//! 只是当时内置虚拟机 / Docker Desktop 没在跑，`docker compose ps` 问不出来。
//! 0.1.16 及之前，问不出来就被判成 `Absent` —— 而 `Absent` 的下游是**完整安装**。
//! 「问不出来」和「根本没有」是两回事：前者只是我们此刻看不到，
//! 后者是磁盘上真的什么都没有。**把前者说成后者，用户就得重新填 key、甚至重建数据卷。**
//!
//! | 结论 | 现场 | 该做什么 |
//! |---|---|---|
//! | [`Posture::Absent`] | 磁盘上没有这一套，本项目一个容器都没有 | 完整安装 |
//! | [`Posture::Incomplete`] | 六个服务只有一部分建出来过 | 完整安装（镜像本机已有的话拉取那步是空跑） |
//! | [`Posture::Stopped`] | 六个容器都建齐了，但一个在跑的都没有（**问得出来**） | **点「启动」就行**（不用装、不用拉、不用修） |
//! | [`Posture::RuntimeDown`] | 磁盘上装过，但运行时**问不出来** | **点「启动运行时」**；绝不重装、绝不重填 key、绝不碰 `.env` 与数据卷 |
//! | [`Posture::Partial`] | 有容器在跑，但有的没就绪 / web 打不开 | **只修不正常的那部分** |
//! | [`Posture::Healthy`] | 6/6 就绪、web 打得开（深查时还要容器连得上网关） | **什么都不做**，直接用 |

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::compose::{self, ServiceStatus};
use crate::config::{LauncherConfig, PROJECT};

/// compose 里应该有的六个服务。和 [`crate::compose::wait_healthy`] 用的是同一份名单。
pub const EXPECTED: [&str; 6] = ["web", "api", "opencode", "llm-shim", "postgres", "redis"];

/// 本机 web 探测给多久。打不开就是打不开，不值得等。
const WEB_TIMEOUT: Duration = Duration::from_secs(6);

/// 复查的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Posture {
    /// 本项目在这台机器上根本不存在
    Absent,
    /// 存在，但六个服务没建齐 —— 上一次装到一半
    Incomplete,
    /// 六个容器都建齐了，**但一个在跑的都没有**（I16 · P0-4）
    Stopped,
    /// 磁盘上装过，**但运行时问不出来**（R1）—— 内置虚拟机 / Docker 当时没在跑，
    /// 或者 `hunter` 这个项目名正被别人占着。
    ///
    /// **绝不等同于 [`Posture::Absent`]**：这一档的下游一律不安装、不重填 key、
    /// 不碰 `.env` 与数据卷（见 [`posture_before_containers`]）。
    RuntimeDown,
    /// 六个都在、至少有一个在跑，但有的不正常
    Partial,
    /// 全好
    Healthy,
}

impl Posture {
    pub fn cn(self) -> &'static str {
        match self {
            Posture::Absent => "这台机器上还没有 Hunter",
            Posture::Incomplete => "上一次只装了一半",
            Posture::Stopped => "装好了，但容器都停着",
            Posture::RuntimeDown => "装过，但运行时问不出来",
            Posture::Partial => "Hunter 在跑，但有服务不正常",
            Posture::Healthy => "Hunter 已经在正常运行",
        }
    }
}

/// 复查开头那两问里，「运行时问不问得出来」这一问的三种结果。
///
/// [`Runtime::NotAsked`] 是**故意不问**：项目名不是我们的、或者磁盘上压根没有这一套时，
/// 去问 `docker compose ps` 要么是在问别人的容器，要么没有任何意义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    /// 问了，答上来了
    Answers,
    /// 问了，没答上来（daemon 连不上）
    Silent,
    /// 没问
    NotAsked,
}

/// **还没看容器之前，光凭前两问能得出的结论。**
///
/// 做成纯函数，理由和 `commands::route_of` 一样：`docker compose ps` 问不出来这种现场，
/// 在开发机与测试机上根本造不出来（这两台机器上的 Hunter 一直好好跑着，
/// 而红线不许我们去动别人的容器）—— 而它正是 2026-09-28 用户报的那一幕。
///
/// 返回 `None` = 前两问没给出终局结论，接着去看容器现状。
///
/// 两条硬规矩，顺序不能换：
///
/// 1. **项目名不是我们的 ⇒ `RuntimeDown`。** 那是 `E_PROJECT_CONFLICT`，
///    运行时好好的、只是它服务的是另一套；这里既不该说 `Absent`（会去装，
///    把别人的配置和端口顶掉），也不该说「运行时没在跑」；
/// 2. **只有磁盘上真的什么都没有，才许说 `Absent`。**
///    `Absent` 的下游是完整安装，说错一次用户就得重填 key、甚至重建数据卷。
///    磁盘上有、而运行时问不出来 ⇒ [`Posture::RuntimeDown`]。
pub fn posture_before_containers(
    installed_on_disk: bool,
    project_owned_by_us: bool,
    runtime: Runtime,
) -> Option<Posture> {
    if !project_owned_by_us {
        return Some(Posture::RuntimeDown);
    }
    if !installed_on_disk {
        // 真的没装过 —— 磁盘上两份文件一份都不在。哪怕 Docker 没起来也是这一档：
        // 全新机器上 Docker 没装/没起，本来就该走完整流程去把运行时装起来
        // （`E_DAEMON_DOWN` / `E_BUILTIN_DOWN` 那条路）。
        return Some(Posture::Absent);
    }
    if runtime == Runtime::Silent {
        return Some(Posture::RuntimeDown);
    }
    debug_assert_eq!(
        runtime,
        Runtime::Answers,
        "走到这里只可能是问过了、答上来了"
    );
    None
}

/// 一次复查的全部结果。**每一项都是实测值，拿不到就是 `None` + 原因**（红线 1）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub posture: Posture,
    /// `docker compose ps --all` 原样给出的六（或更少）个服务
    pub services: Vec<ServiceStatus>,
    /// 已就绪的个数
    pub ready: usize,
    /// 应该有几个（恒为 6，界面上的「x / 6」用它，不要在前端写死）
    pub total: usize,
    /// 没就绪的服务名（Partial 时就是要修的那几个）
    pub unready: Vec<String>,
    /// 六个里压根没有容器的那些
    pub missing: Vec<String>,
    /// 网页地址。没在跑就是 `None`
    pub web_url: Option<String>,
    /// 本机 GET 那一下真实拿到的状态码
    pub web_status: Option<u16>,
    /// 拿不到状态码时的原话
    pub web_reason: Option<String>,
    /// 容器能不能连上模型网关（只有深查才做；浅查恒为 `None`）
    pub container_net: Option<crate::runtime::netcheck::Outcome>,
    /// 本项目的容器里有没有把端口绑到本机之外（I11 · U5）
    pub drift: Vec<compose::BindDrift>,
    /// 一句人话的结论，界面直接显示
    pub headline: String,
    /// 逐条证据（都是实测原话），折叠在「详情」里
    pub lines: Vec<String>,
    /// 这次复查本身花了多久
    pub elapsed_ms: u64,
}

impl Review {
    /// 「什么都不用做」。
    pub fn all_good(&self) -> bool {
        self.posture == Posture::Healthy
    }

    /// 六个服务本身全就绪了吗（不看 web、不看容器联网）。
    ///
    /// 界面用它决定「关闭」要不要当主按钮：容器全绿就说明这台机器上没有异常，
    /// 哪怕 web 那一下因为别的原因没探成。
    pub fn services_all_ready(&self) -> bool {
        self.total > 0 && self.ready == self.total && self.missing.is_empty()
    }
}

/// 这个容器状态算不算「跑过、然后停了」。
///
/// compose 报的状态就那么几种：`running` / `exited` / `created` / `restarting` /
/// `paused` / `dead` / `removing`。`created` 由上面「装到一半的残骸」那一档先接走；
/// `restarting` **不算** —— 那是正在挣扎，不是停着，处置也不同（要看日志）。
pub fn is_down(state: &str) -> bool {
    matches!(state, "exited" | "dead")
}

/// 六个容器都在的前提下，**光看容器状态**能得出的三种结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByContainers {
    /// 有容器「建出来过但一次都没跑起来」—— 上一次装到一半留下的残骸
    Stale,
    /// 一个在跑的都没有（全 `exited` / `dead`）
    AllDown,
    /// 其余：至少有一个在跑
    Mixed,
}

/// [`judge_containers`] 的结果。名单都带出来，界面要逐个说。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerVerdict {
    pub kind: ByContainers,
    /// `created`（建出来过但没跑过）的那几个
    pub stale: Vec<String>,
    /// 退出了的那几个
    pub down: Vec<String>,
    /// 没就绪的那几个（**包含** `down` 里的）
    pub unready: Vec<String>,
}

/// 光看容器状态判一次。**做成纯函数**，理由和 `commands::route_of` 一样：
/// 「六个全 exited」这种现场在开发机与测试机上要造出来很麻烦，
/// 而它正是客户 2026-09-26 那台 Windows 上的真实样子。
///
/// 三档的顺序不能换：
///
/// 1. `created` 先接走（I5 §六.5）—— 那些容器身上冻着**上一次那组端口**，
///    当成「只差起一下」去 `up`，起来的还是旧端口，照样撞车；
/// 2. 再看「一个在跑的都没有」（I16 · P0-4）；
/// 3. 剩下的才是「有在跑的，但有的不正常」。
pub fn judge_containers(ours: &[ServiceStatus]) -> ContainerVerdict {
    let stale: Vec<String> = ours
        .iter()
        .filter(|s| s.state == "created")
        .map(|s| s.service.clone())
        .collect();
    let down: Vec<String> = ours
        .iter()
        .filter(|s| is_down(&s.state))
        .map(|s| s.service.clone())
        .collect();
    let unready: Vec<String> = ours
        .iter()
        .filter(|s| !compose::service_ready(s))
        .map(|s| s.service.clone())
        .collect();
    let kind = if !stale.is_empty() {
        ByContainers::Stale
    } else if !ours.is_empty() && down.len() == ours.len() {
        ByContainers::AllDown
    } else {
        ByContainers::Mixed
    };
    ContainerVerdict {
        kind,
        stale,
        down,
        unready,
    }
}

/// 配置里记的 web 端口。容器全停着的时候 docker 不报端口，只能退回配置。
fn cfg_web_port() -> u16 {
    LauncherConfig::load().hunter.ports.web
}

/// 磁盘上有没有这一套的配置。两份文件缺一个就不算「装过」。
pub fn installed_on_disk() -> bool {
    crate::paths::compose_file().is_file() && crate::paths::env_file().is_file()
}

/// 配置（`.env` 的 `HUNTER_VERSION`）那一版还缺哪几个镜像。
///
/// 返回 `Some((配置版本, 缺的服务名))`；一个不缺、或者读不到版本号时返回 `None`
/// （读不到就什么都不说 —— 不猜）。
///
/// **只读**：`docker image inspect`，不碰容器、不碰卷、不拉任何东西。
/// 这一问存在的理由（I17 · P0-3）：容器全停的时候，光看容器状态得不出
/// 「点启动就能用」这个结论 —— 配置那一版的镜像可能压根不齐
/// （客户 2026-09-29 就是这样，点下去又卡一次）。
pub fn missing_config_images() -> Option<(String, Vec<String>)> {
    let cfg = crate::config::LauncherConfig::load();
    let tag = crate::config::parse_env_file(&crate::paths::env_file())
        .get("HUNTER_VERSION")
        .cloned()
        .unwrap_or_else(|| cfg.hunter.tag.clone());
    let tag = tag.trim().to_string();
    if tag.is_empty() {
        return None;
    }
    let specs = crate::config::images(&cfg.hunter.registry_prefix, &cfg.hunter.base_prefix, &tag);
    let (_ok, missing) = crate::offline::all_present(&specs);
    if missing.is_empty() {
        None
    } else {
        Some((tag, missing))
    }
}

/// 只读地复查一遍。
///
/// `deep` 为真时多做一件事：起一个 `--rm` 的一次性容器问「解析得动模型网关吗」。
/// 那一下要几秒到几十秒，所以错误页上的低频复查用浅查，
/// 安装前的预检用深查（**容器连不上网关的话，六个绿灯也没有意义** —— I10 的结论）。
/// [`Posture::RuntimeDown`] 那一档的 headline，**按「装没装」两态分开**（I18 · P0-2）。
///
/// 两态给用户的下一步完全不同：
///
/// * **装了、只是没在跑** → 点「启动运行时」；I18 起那个按钮在 Windows 上真的能把
///   Docker Desktop 拉起来（P0-1，用户 2026-09-29 拍板的口径 A），所以这里可以放心地
///   把它当出路说出来；
/// * **没装 / 读不到** → 照旧只说清「装过、不用重装、不用重填 key」这一件事 ——
///   装 Docker 要管理员、要重启，不在产品能替用户做的范围里（别在这里许一个做不到的诺）。
///
/// `d` 为 `None` 时（没去问）走第二句：**读不到就不说**，不猜。
fn runtime_down_headline(d: Option<&crate::runtime::docker::DockerInfo>) -> String {
    match d {
        Some(d) if d.installed && !d.daemon_running => format!(
            "Hunter 装在这台电脑上，{} 也装着、只是没在跑 —— 点「启动运行时」让启动器替你把它拉起来；不用重新安装，也不用重新填 key",
            d.runtime_label
                .clone()
                .unwrap_or_else(|| "Docker".to_string())
        ),
        _ => "Hunter 装在这台电脑上，只是运行时没在跑 —— 不用重新安装，也不用重新填 key"
            .to_string(),
    }
}

pub fn review(deep: bool) -> Review {
    let t0 = Instant::now();
    let mut lines: Vec<String> = Vec::new();

    // ① 这个项目名现在是谁的。不是我们的就别往下看了 —— 那是 E_PROJECT_CONFLICT 的事
    let owned_by_us = match compose::guard_project_owner() {
        Ok(()) => true,
        Err(e) => {
            lines.push(format!("compose 项目名 {PROJECT} 归属检查没过：{}", e.msg));
            false
        }
    };
    let disk = installed_on_disk();
    if owned_by_us && !disk {
        lines.push(format!(
            "{} 或 {} 不在，这台机器上没有启动器装过的那一套",
            crate::redact::mask_home(&crate::paths::compose_file().to_string_lossy()),
            crate::redact::mask_home(&crate::paths::env_file().to_string_lossy()),
        ));
    }

    // ② 容器现状。**只在这一问有意义的时候才问**：项目名不是我们的时去问，
    //    问回来的是别人的容器；磁盘上什么都没有时，问了也不知道该比什么
    let (runtime, services) = if !owned_by_us || !disk {
        (Runtime::NotAsked, Vec::new())
    } else {
        match compose::ps() {
            Ok(v) => (Runtime::Answers, v),
            Err(e) => {
                lines.push(format!("docker compose ps 问不出来：{}", e.msg));
                (Runtime::Silent, Vec::new())
            }
        }
    };

    // 前两问就定了局的（含 R1 新增的 RuntimeDown），不必再往下看容器
    if let Some(p) = posture_before_containers(disk, owned_by_us, runtime) {
        let mut r = finish_full(
            p,
            services,
            None,
            None,
            lines,
            t0,
            deep,
            Vec::new(),
            Vec::new(),
            None,
        );
        if p == Posture::RuntimeDown && !owned_by_us {
            // **项目名被别人占着，这不是「运行时没在跑」** —— 运行时好好的，
            // 只是它服务的是另一套。说法必须跟着事实走（红线 1）。
            r.headline = format!(
                "另一个位置的 Hunter 正占着「{PROJECT}」这个项目名，启动器现在管不了这一套 —— \
                 你的数据和配置都没动，这里也不会重新安装"
            );
        } else if p == Posture::RuntimeDown {
            // **I18 · P0-2：这一档要分两态说。**
            //
            // 「装了没在跑」（客户 2026-09-29 那台，`installed=true daemon=false`）
            // 与「根本没装」给用户的下一步完全不同：前者点「启动运行时」就能拉起来，
            // 后者才要去装 —— 而启动器**装不了**（要管理员、要重启）。
            //
            // 这次检测**只在这一档做**：运行时都问不出来了，多跑一次 `docker version`
            // 是值得的；别的档位一个字节都不多花。
            let d = crate::runtime::docker::detect();
            r.headline = runtime_down_headline(Some(&d));
        }
        return r;
    }
    let ours: Vec<ServiceStatus> = services
        .iter()
        .filter(|s| EXPECTED.contains(&s.service.as_str()))
        .cloned()
        .collect();
    if ours.is_empty() {
        lines.push("compose 项目 hunter 下一个容器都没有".into());
        return finish(Posture::Absent, ours, None, None, lines, t0, deep, None);
    }
    for s in &ours {
        lines.push(format!(
            "{} · {} · {}",
            s.service,
            s.state,
            compose::health_cn(s.health)
        ));
    }
    let missing: Vec<String> = EXPECTED
        .iter()
        .filter(|e| !ours.iter().any(|s| s.service == **e))
        .map(|e| (*e).to_string())
        .collect();
    if !missing.is_empty() {
        lines.push(format!("还没有容器的服务：{}", missing.join("、")));
        return finish(
            Posture::Incomplete,
            ours,
            None,
            None,
            lines,
            t0,
            deep,
            Some(missing),
        );
    }

    // 六个容器都在了，**光看容器状态**能得出什么结论 —— 见 [`judge_containers`]
    let verdict = judge_containers(&ours);
    let unready = verdict.unready.clone();
    match verdict.kind {
        ByContainers::Stale => {
            lines.push(format!(
                "这几个容器建出来过但一次都没跑起来（上一次装到一半留下的）：{}",
                verdict.stale.join("、")
            ));
            return finish(Posture::Incomplete, ours, None, None, lines, t0, deep, None);
        }
        ByContainers::AllDown => {
            lines.push(format!(
                "{} 个容器全部已退出 —— 装是装好了，只是没在跑",
                ours.len()
            ));
            // **全停的时候还要再问一句「配置那一版的镜像齐不齐」**（I17 · P0-3）。
            // 齐了才是真的「点启动就能用」；不齐就得说清 —— 客户 2026-09-29 那台
            // 就是在这里被告知「点『启动』就能用」，而 v1.2.3 的镜像本机还不齐。
            let gap = missing_config_images();
            if let Some((tag, lack)) = &gap {
                lines.push(format!(
                    "但配置那一版 v{tag} 的镜像本机还缺 {} 个：{}",
                    lack.len(),
                    lack.join("、")
                ));
                lines.push(
                    "所以现在点「启动」会先去拉这几个镜像 —— 上一次升级多半就是卡在这一步。"
                        .to_string(),
                );
            }
            // 这一档**不去探 web**：那个端口上本来就不会有人应答，
            // 探它只是白等 6 秒，再多报一句「网页打不开」把话说重了
            let mut r = finish_full(
                Posture::Stopped,
                ours,
                None,
                None,
                lines,
                t0,
                deep,
                missing,
                unready,
                Some(cfg_web_port()),
            );
            if let Some((tag, lack)) = gap {
                r.headline = format!(
                    "Hunter 装好了，但 v{tag} 的镜像本机还缺 {} 个 —— 先补齐再启动，别急着点「启动」",
                    lack.len()
                );
            }
            return r;
        }
        ByContainers::Mixed => {
            if !verdict.down.is_empty() {
                lines.push(format!(
                    "这几个容器退出了（不是「还没就绪」）：{}",
                    verdict.down.join("、")
                ));
            }
        }
    }

    // ③ web 打得开吗。端口以 docker 报的为准（配置只是意图）
    let cfg = LauncherConfig::load();
    let web_port = ours
        .iter()
        .find(|s| s.service == "web")
        .and_then(|s| s.port)
        .unwrap_or(cfg.hunter.ports.web);
    let url = format!("http://127.0.0.1:{web_port}/");
    let (status, reason) = match crate::http::get(&url, &[], WEB_TIMEOUT) {
        Ok(r) => {
            lines.push(format!("GET {url} → HTTP {}", r.status));
            (Some(r.status), None)
        }
        Err(e) => {
            lines.push(format!("GET {url} → {}", e.msg));
            (None, Some(e.msg))
        }
    };
    // 任何一个 5xx 以下的状态码都证明「这个端口后面确实有 Hunter 在应答」。
    // 不强求 200：未登录时上游会 302 到 /login，那同样是活着的证据。
    let web_ok = matches!(status, Some(s) if s < 500);

    if !unready.is_empty() || !web_ok {
        return finish_full(
            Posture::Partial,
            ours,
            status,
            reason,
            lines,
            t0,
            deep,
            missing,
            unready,
            Some(web_port),
        );
    }

    finish_full(
        Posture::Healthy,
        ours,
        status,
        reason,
        lines,
        t0,
        deep,
        missing,
        unready,
        Some(web_port),
    )
}

#[allow(clippy::too_many_arguments)]
fn finish(
    posture: Posture,
    services: Vec<ServiceStatus>,
    web_status: Option<u16>,
    web_reason: Option<String>,
    lines: Vec<String>,
    t0: Instant,
    deep: bool,
    missing: Option<Vec<String>>,
) -> Review {
    let m = missing.unwrap_or_default();
    finish_full(
        posture,
        services,
        web_status,
        web_reason,
        lines,
        t0,
        deep,
        m,
        Vec::new(),
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_full(
    mut posture: Posture,
    services: Vec<ServiceStatus>,
    web_status: Option<u16>,
    web_reason: Option<String>,
    mut lines: Vec<String>,
    t0: Instant,
    deep: bool,
    missing: Vec<String>,
    unready: Vec<String>,
    web_port: Option<u16>,
) -> Review {
    let ready = services
        .iter()
        .filter(|s| compose::service_ready(s))
        .count();

    // 绑定漂移（I11 · U5）。只读，顺手查一次
    let drift = compose::bind_drift(&services);
    for d in &drift {
        lines.push(d.human());
    }

    // 深查：容器连得上模型网关吗。六个绿灯 + 容器上不了网 = 用户一句话都问不出来
    let mut container_net = None;
    if deep && matches!(posture, Posture::Healthy | Posture::Partial) {
        // 注意：`Stopped` 不在这里 —— 容器都停着的时候起一个一次性容器去探网关，
        // 探出来的既不是「Hunter 能不能上网」也不是用户此刻关心的事。
        // `RuntimeDown` 更不在这里：运行时都没在跑，一次性容器根本起不出来
        let o = crate::runtime::netcheck::probe();
        lines.push(o.one_line());
        if !o.ok && o.fail != crate::runtime::netcheck::Fail::NotRun {
            posture = Posture::Partial;
        }
        container_net = Some(o);
    }

    // 停着的时候不给地址：那个端口上没有人应答，给出去就是一个点不开的链接。
    // `RuntimeDown` 同样不给 —— 运行时都问不出来，端口是哪来的都不知道
    let web_url = web_port
        .filter(|_| {
            !matches!(
                posture,
                Posture::Absent | Posture::Stopped | Posture::RuntimeDown
            )
        })
        .map(|p| format!("http://localhost:{p}"));

    let headline = match posture {
        Posture::Absent => "这台机器上还没有装过 Hunter".to_string(),
        Posture::Incomplete if missing.is_empty() => {
            "上一次装到一半就断了：容器建出来过，但一次都没跑起来".to_string()
        }
        Posture::Incomplete => format!("上一次只装了一半：{} 还没有容器", missing.join("、")),
        Posture::Stopped => format!(
            "Hunter 装好了，但 {} 个容器都停着 —— 点「启动」就能用",
            services.len()
        ),
        // **R1**：装过是磁盘说的事实，问不出来只是问不出来。
        // 这句要同时说清三件事：装过、不用重装、不用重填 key。
        // I18 · P0-2：再按「装没装」分两态 —— 见 [`runtime_down_headline`]
        Posture::RuntimeDown => {
            "Hunter 装在这台电脑上，只是运行时没在跑 —— 不用重新安装，也不用重新填 key".to_string()
        }
        Posture::Partial => {
            let mut what: Vec<String> = Vec::new();
            // **停着的和没就绪的分开说**（I16 · P0-4）。混在一起说成「还没就绪」，
            // 用户会去等、去翻日志，而他实际上只要点一下「启动」
            let down: Vec<&str> = services
                .iter()
                .filter(|s| is_down(&s.state))
                .map(|s| s.service.as_str())
                .collect();
            let not_ready: Vec<&str> = unready
                .iter()
                .map(String::as_str)
                .filter(|n| !down.contains(n))
                .collect();
            if !down.is_empty() {
                what.push(format!("{} 停着（需要启动）", down.join("、")));
            }
            if !not_ready.is_empty() {
                what.push(format!("{} 还没就绪", not_ready.join("、")));
            }
            if web_status.is_none() {
                what.push("网页打不开".into());
            }
            if container_net.as_ref().is_some_and(|o| !o.ok) {
                what.push("容器连不上模型网关".into());
            }
            if what.is_empty() {
                what.push("有服务不正常".into());
            }
            // **网页都不应答的时候不能说「在跑」**（I17 · P0-1 顺手补的一处）。
            //
            // 客户 HL-GFV764 那份日志里 09:52:35 那一行就是这句话：
            // 「开机判定：Hunter 在跑，但 web、api、opencode、llm-shim、redis 停着（需要启动）、网页打不开」
            // —— 六个容器里只有 postgres 活着、网页一个字节都回不来，却把话说成「在跑」。
            // 这和面板上那个把假「运行中」点亮的绿点是同一个毛病：
            // **拿不到应答就说「没在跑」**，宁严不宽（`flow::running_now` 是同一条口径）。
            if web_status.is_none() {
                format!("Hunter 没跑起来：{}", what.join("、"))
            } else {
                format!("Hunter 在跑，但 {}", what.join("、"))
            }
        }
        Posture::Healthy => format!(
            "Hunter 已经在正常运行（{ready} / {} 健康{}）",
            EXPECTED.len(),
            match web_status {
                Some(s) => format!("，网页返回 HTTP {s}"),
                None => String::new(),
            }
        ),
    };

    if posture == Posture::Healthy {
        note_healthy();
    }

    Review {
        posture,
        services,
        ready,
        total: EXPECTED.len(),
        unready,
        missing,
        web_url,
        web_status,
        web_reason,
        container_net,
        drift,
        headline,
        lines,
        elapsed_ms: t0.elapsed().as_millis() as u64,
    }
}

/// 「刚才确实看到 6/6 健康」→ 刷 `[install] last_healthy_at`（I12 · R1 第 3 条）。
///
/// **带节流**：错误页上的复查是 20 秒一次，运行面板还会更频繁 ——
/// 每次都往 `launcher.toml` 里写一遍纯粹是在磨用户的盘。
/// 一分钟最多写一次；进程刚起来时先写一次（`None`），那一次最有价值。
fn note_healthy() {
    use std::sync::Mutex;
    use std::time::Instant;
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    const EVERY: Duration = Duration::from_secs(60);

    let Ok(mut g) = LAST.lock() else { return };
    if g.is_some_and(|t| t.elapsed() < EVERY) {
        return;
    }
    *g = Some(Instant::now());
    drop(g);

    let mut cfg = LauncherConfig::load();
    // 现状是健康的 → 这台机器上装过，这件事没有可争的（R1）
    let adopted = !cfg.install.done;
    cfg.mark_installed(adopted);
    cfg.touch_healthy();
    if let Err(e) = cfg.save() {
        crate::lwarn!("刷新 last_healthy_at 没写成：{}", e.msg);
    } else if adopted {
        crate::linfo!(
            "复查看到 Hunter 已在正常运行，已把它记为已安装（adopted_from_running=true）"
        );
    }
}

// ── 只修不正常的那一部分（I11 · U2）───────────────────────────────────────

/// 「只修不正常的那部分」做了什么。
#[derive(Debug, Clone, Default)]
pub struct FixLog {
    /// 没有容器 / 停着的，`up -d --no-recreate` 起起来
    pub started: Vec<String>,
    /// 在跑但没就绪的，`restart` 一下
    pub restarted: Vec<String>,
}

impl FixLog {
    pub fn is_empty(&self) -> bool {
        self.started.is_empty() && self.restarted.is_empty()
    }

    pub fn human(&self) -> String {
        let mut v: Vec<String> = Vec::new();
        if !self.started.is_empty() {
            v.push(format!("起了 {}", self.started.join("、")));
        }
        if !self.restarted.is_empty() {
            v.push(format!("重启了 {}", self.restarted.join("、")));
        }
        v.join("；")
    }
}

/// 只碰 `r` 里说没就绪的那几个服务。
///
/// **这里没有任何一步会碰到健康的容器**：`up -d` 带 `--no-recreate`，
/// `restart` 只重启进程。不取 compose、不写 `.env`、不拉镜像 —— 那三件事
/// 是「完整安装」的活，不是「修一个服务」的活。
pub fn fix_unready(r: &Review) -> crate::err::AppResult<FixLog> {
    let mut log = FixLog::default();
    let mut to_start: Vec<&str> = Vec::new();
    let mut to_restart: Vec<&str> = Vec::new();
    for name in &r.unready {
        match r.services.iter().find(|s| &s.service == name) {
            // 在跑但健康检查不过：容器本身是好的，换掉它只会更慢
            Some(s) if s.state == "running" => to_restart.push(name.as_str()),
            _ => to_start.push(name.as_str()),
        }
    }
    for name in &r.missing {
        to_start.push(name.as_str());
    }
    if !to_start.is_empty() {
        compose::start_services(&to_start)?;
        log.started = to_start.iter().map(|s| (*s).to_string()).collect();
    }
    if !to_restart.is_empty() {
        compose::restart_services(&to_restart)?;
        log.restarted = to_restart.iter().map(|s| (*s).to_string()).collect();
    }
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(name: &str, state: &str, health: compose::Health, port: Option<u16>) -> ServiceStatus {
        ServiceStatus {
            service: name.into(),
            state: state.into(),
            health,
            port,
            bind: Some("127.0.0.1".into()),
            exit_code: None,
            image: None,
        }
    }

    fn all_healthy() -> Vec<ServiceStatus> {
        EXPECTED
            .iter()
            .map(|n| svc(n, "running", compose::Health::Healthy, Some(3100)))
            .collect()
    }

    // ── R1：装过但运行时问不出来 ──────────────────────────────────────────
    //
    // 这一组钉的是用户 2026-09-28 报的那一幕（也是 I15 任务书的 P0-1）：
    // 不起 Docker 打开软件 → 被当成「没装过」→ 重复进入安装界面、重新填 key。
    //
    // 现场造不出来（这两台机器上的 Hunter 一直好好跑着，而且红线不许我们
    // 去动别人的容器），所以判据被抽成了纯函数 `posture_before_containers`。

    /// **第 1 条**：磁盘上装过（`installed_on_disk() == true`）而 `docker compose ps`
    /// 报错 ⇒ 永远不是 `Absent`。
    ///
    /// `Absent` 的下游是完整安装（`route_of` 把 `Absent` 送进 Welcome / DataFound），
    /// 所以这一条等价于「运行时不跑的时候绝不再装一遍」。
    #[test]
    fn 装过但运行时问不出来时永远不算没装过() {
        let p = posture_before_containers(true, true, Runtime::Silent);
        assert_eq!(
            p,
            Some(Posture::RuntimeDown),
            "磁盘上有 + ps 报错 ⇒ RuntimeDown"
        );
        assert_ne!(p, Some(Posture::Absent), "★ 这一档绝不许是 Absent");

        // 说法也要对：说清「装过、不用重装、不用重填 key」
        let r = finish_full(
            Posture::RuntimeDown,
            Vec::new(),
            None,
            None,
            Vec::new(),
            Instant::now(),
            false,
            Vec::new(),
            Vec::new(),
            None,
        );
        assert_eq!(r.posture, Posture::RuntimeDown);
        assert!(!r.all_good());
        assert!(
            r.headline.contains("装在这台电脑上") && r.headline.contains("运行时没在跑"),
            "要说清「装过、只是没起来」：{}",
            r.headline
        );
        assert!(
            r.headline.contains("不用重新安装") && r.headline.contains("不用重新填 key"),
            "要说清不用重装、不用重填 key：{}",
            r.headline
        );
        assert!(
            !r.headline.contains("还没有装过"),
            "不能再说成「没装过」：{}",
            r.headline
        );
        // 运行时都问不出来，网页地址当然给不出来
        assert!(r.web_url.is_none(), "{:?}", r.web_url);
        assert_eq!(Posture::RuntimeDown.cn(), "装过，但运行时问不出来");
    }

    /// **第 2 条**：磁盘上真的什么都没有 ⇒ 仍然是 `Absent`。
    ///
    /// 别把「真的没装过」也一起改了 —— 全新机器上 Docker 没装/没起，
    /// 本来就该走完整流程把运行时装起来（`E_DAEMON_DOWN` / `E_BUILTIN_DOWN` 那条路）。
    /// 这一条同时是上面那条的**反向保险**：改宽了当场就红。
    #[test]
    fn 磁盘上什么都没有时仍然是没装过() {
        assert_eq!(
            posture_before_containers(false, true, Runtime::Answers),
            Some(Posture::Absent)
        );
        // 磁盘上没有、Docker 也没起来 —— 依然是「没装过」：这两件事互不冒充
        assert_eq!(
            posture_before_containers(false, true, Runtime::Silent),
            Some(Posture::Absent),
            "唯一允许判 Absent 的判据是磁盘，不是运行时"
        );
        assert_eq!(
            posture_before_containers(false, true, Runtime::NotAsked),
            Some(Posture::Absent)
        );
    }

    /// **第 3 条**：`guard_project_owner()` 失败 ⇒ 不是 `Absent`。
    ///
    /// 那是 `E_PROJECT_CONFLICT`：另一处的 Hunter 正占着 `hunter` 这个项目名。
    /// 判成 `Absent` 会去完整安装，把别人的配置和端口一起顶掉（M2 实测撞到过）。
    #[test]
    fn 项目名被别人占着时不算没装过() {
        let p = posture_before_containers(true, false, Runtime::NotAsked);
        assert_ne!(p, Some(Posture::Absent), "★ 项目名冲突绝不许是 Absent");
        assert_eq!(p, Some(Posture::RuntimeDown));
        // 我们这一份磁盘上还没有（用户换了工作目录）时同样不许说 Absent
        assert_ne!(
            posture_before_containers(false, false, Runtime::NotAsked),
            Some(Posture::Absent)
        );
    }

    /// **第 4 条**：`RuntimeDown` 与 `Stopped` 的**区别**要钉死。
    ///
    /// - `Stopped`：容器**问得出来**，六个全 `exited` —— 缺的只是「起一下」；
    /// - `RuntimeDown`：**根本问不出来** —— 连它建没建齐都不知道。
    ///
    /// 两者处置不同，说法也必须不同：前者说「N 个容器都停着」，
    /// 后者说「装过、只是运行时没在跑」。混成一档会让用户照着错的方向去点。
    #[test]
    fn 全停着与问不出来是两回事() {
        // `Stopped` 那一边：问得出来，六个全 exited
        let all_exit: Vec<ServiceStatus> = EXPECTED
            .iter()
            .map(|n| svc(n, "exited", compose::Health::Pending, None))
            .collect();
        assert_eq!(
            judge_containers(&all_exit).kind,
            ByContainers::AllDown,
            "六个全 exited 才叫「都停着」—— 前提是**问得出来**"
        );
        // 问得出来的时候 `posture_before_containers` 不插话，交给容器那一档判
        assert_eq!(
            posture_before_containers(true, true, Runtime::Answers),
            None,
            "问得出来时该往下看容器，不该在这里定局"
        );

        // `RuntimeDown` 那一边：根本问不出来，连服务列表都是空的
        let down = finish_full(
            Posture::RuntimeDown,
            Vec::new(), // ← ps 报错，一个服务都拿不到
            None,
            None,
            Vec::new(),
            Instant::now(),
            false,
            Vec::new(),
            Vec::new(),
            None,
        );
        assert!(down.services.is_empty(), "问不出来时服务列表是空的");
        assert!(down.missing.is_empty(), "连缺哪几个都不知道，不该编");
        assert_eq!(down.ready, 0);
        assert_eq!(
            down.total,
            EXPECTED.len(),
            "总数是常量 6，不是从 ps 数出来的"
        );

        // 停着的那一边：六个服务都在手上
        let stopped = finish_full(
            Posture::Stopped,
            all_exit.clone(),
            None,
            None,
            Vec::new(),
            Instant::now(),
            false,
            Vec::new(),
            judge_containers(&all_exit).unready.clone(),
            Some(3100),
        );
        assert_eq!(stopped.services.len(), 6, "问得出来就有六个服务可以逐个说");
        assert_eq!(stopped.ready, 0);

        // 说法不同（处置不同，说法就不能一样）
        assert_ne!(down.headline, stopped.headline);
        assert!(
            stopped.headline.contains("都停着"),
            "Stopped 说「都停着」：{}",
            stopped.headline
        );
        assert!(
            !down.headline.contains("都停着") && !down.headline.contains("个容器"),
            "RuntimeDown **不知道有几个容器**，不许说「N 个容器」：{}",
            down.headline
        );
        assert!(!down.all_good() && !stopped.all_good());
    }

    /// 三种现场，**三种说法必须互不相同**（I16 · P0-4）。
    ///
    /// 客户 2026-09-26 那台 Windows 上六个容器全是 `exited`，
    /// 0.1.15 说的却是「Hunter 在跑，但 web、api、opencode、llm-shim、postgres、redis
    /// 还没就绪、网页打不开」—— 把「退出了」说成了「还没就绪」，
    /// 而这两件事该做的动作完全不同（前者点启动，后者等一等或看日志）。
    #[test]
    fn 全_exited_与_一半_exited_与_全健康_要说三种话() {
        let all_exit: Vec<ServiceStatus> = EXPECTED
            .iter()
            .map(|n| svc(n, "exited", compose::Health::Pending, None))
            .collect();
        let v = judge_containers(&all_exit);
        assert_eq!(
            v.kind,
            ByContainers::AllDown,
            "六个全 exited 就是「都停着」"
        );
        assert_eq!(v.down.len(), 6);

        let mut half = all_healthy();
        for i in 0..3 {
            half[i] = svc(EXPECTED[i], "exited", compose::Health::Pending, None);
        }
        let v2 = judge_containers(&half);
        assert_eq!(
            v2.kind,
            ByContainers::Mixed,
            "还有三个在跑，就不是「都停着」"
        );
        assert_eq!(v2.down.len(), 3);

        let v3 = judge_containers(&all_healthy());
        assert_eq!(v3.kind, ByContainers::Mixed);
        assert!(v3.down.is_empty() && v3.unready.is_empty());

        // 三句话
        let say = |posture, services: Vec<ServiceStatus>, unready: Vec<String>, web| {
            finish_full(
                posture,
                services,
                web,
                None,
                Vec::new(),
                Instant::now(),
                false,
                Vec::new(),
                unready,
                Some(3100),
            )
            .headline
        };
        let a = say(Posture::Stopped, all_exit.clone(), v.unready.clone(), None);
        let b = say(
            Posture::Partial,
            half.clone(),
            v2.unready.clone(),
            Some(200),
        );
        let c = say(Posture::Healthy, all_healthy(), Vec::new(), Some(200));
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
        assert!(a.contains("都停着"), "全停着要说「都停着」：{a}");
        assert!(
            !a.contains("还没就绪") && !a.contains("网页打不开"),
            "全停着不该说成「还没就绪 / 网页打不开」：{a}"
        );
        assert!(
            b.contains("停着（需要启动）") && b.contains("web"),
            "一半停着要点名说哪几个停了：{b}"
        );
        assert!(c.contains("已经在正常运行"), "{c}");
    }

    /// `restarting` 不是「停着」——它正在挣扎，处置是看日志，不是点启动。
    #[test]
    fn restarting_不算停着() {
        assert!(!is_down("restarting"));
        assert!(is_down("exited") && is_down("dead"));
        let v = judge_containers(
            &EXPECTED
                .iter()
                .map(|n| svc(n, "restarting", compose::Health::Pending, None))
                .collect::<Vec<_>>(),
        );
        assert_eq!(v.kind, ByContainers::Mixed);
    }

    /// `created` 那一档要**排在**「全停着」前面：那些容器身上冻着上一次那组端口。
    #[test]
    fn created_的残骸优先于全停着() {
        let mut v: Vec<ServiceStatus> = EXPECTED
            .iter()
            .map(|n| svc(n, "exited", compose::Health::Pending, None))
            .collect();
        v[0] = svc("web", "created", compose::Health::Pending, None);
        assert_eq!(judge_containers(&v).kind, ByContainers::Stale);
    }

    /// 全停着的时候**不给网页地址** —— 那个端口上没有人应答。
    #[test]
    fn 全停着不给一个点不开的网址() {
        let all_exit: Vec<ServiceStatus> = EXPECTED
            .iter()
            .map(|n| svc(n, "exited", compose::Health::Pending, None))
            .collect();
        let r = finish_full(
            Posture::Stopped,
            all_exit,
            None,
            None,
            Vec::new(),
            Instant::now(),
            false,
            Vec::new(),
            Vec::new(),
            Some(3100),
        );
        assert!(r.web_url.is_none(), "{:?}", r.web_url);
        assert!(!r.all_good());
    }

    #[test]
    fn 六个全健康且网页应答就是无需重装() {
        let r = finish_full(
            Posture::Healthy,
            all_healthy(),
            Some(200),
            None,
            Vec::new(),
            Instant::now(),
            false,
            Vec::new(),
            Vec::new(),
            Some(3100),
        );
        assert!(r.all_good(), "{r:?}");
        assert!(r.services_all_ready());
        assert_eq!(r.ready, 6);
        assert!(
            r.headline.contains("已经在正常运行"),
            "结论要是一句人话：{}",
            r.headline
        );
    }

    /// 「只修不正常的那部分」的分诊：在跑但不健康 → 重启；停着 / 没有容器 → 起起来。
    /// **健康的那几个一个都不许出现在名单里**（I11 · U2 的硬要求）。
    #[test]
    fn 只把不正常的那几个列进要动的名单() {
        let mut services = all_healthy();
        services[2] = svc(
            "opencode",
            "running",
            compose::Health::Unhealthy,
            Some(3921),
        );
        services[4] = svc("postgres", "exited", compose::Health::Pending, None);
        let r = finish_full(
            Posture::Partial,
            services,
            Some(200),
            None,
            Vec::new(),
            Instant::now(),
            false,
            Vec::new(),
            vec!["opencode".into(), "postgres".into()],
            Some(3100),
        );
        let mut to_start: Vec<&str> = Vec::new();
        let mut to_restart: Vec<&str> = Vec::new();
        for name in &r.unready {
            match r.services.iter().find(|s| &s.service == name) {
                Some(s) if s.state == "running" => to_restart.push(name),
                _ => to_start.push(name),
            }
        }
        assert_eq!(to_restart, vec!["opencode"]);
        assert_eq!(to_start, vec!["postgres"]);
        assert!(!r.all_good());
        assert!(!r.services_all_ready());
    }

    #[test]
    fn 容器连不上网关时六个绿灯也不算好() {
        let lines = vec!["先前的证据".to_string()];
        let r = finish_full(
            Posture::Healthy,
            all_healthy(),
            Some(200),
            None,
            lines,
            Instant::now(),
            false,
            Vec::new(),
            Vec::new(),
            Some(3100),
        );
        // 浅查不做这一项，所以这里只钉住「浅查不会假装做过」
        assert!(r.container_net.is_none());
        assert!(r.all_good());
    }

    /// 上一次起到一半留下的 `Created` 容器**不算「只差起一下」**（I5 §六.5）。
    ///
    /// 它们身上冻着的是当时那一组端口，直接 `up` 起来的还是旧端口。
    /// 这一档必须回到完整流程（清残骸 → 重算端口 → 重起）。
    #[test]
    fn 已创建但没跑起来的容器要走完整流程() {
        let services: Vec<ServiceStatus> = EXPECTED
            .iter()
            .map(|n| svc(n, "created", compose::Health::Pending, None))
            .collect();
        let stale: Vec<&ServiceStatus> = services.iter().filter(|s| s.state == "created").collect();
        assert_eq!(stale.len(), 6, "六个都是残骸");
        let r = finish(
            Posture::Incomplete,
            services,
            None,
            None,
            Vec::new(),
            Instant::now(),
            false,
            None,
        );
        assert_eq!(r.posture, Posture::Incomplete);
        assert!(!r.all_good());
        assert!(r.headline.contains("一次都没跑起来"), "{}", r.headline);
    }

    #[test]
    fn 网页打不开时不会说一切正常() {
        let r = finish_full(
            Posture::Partial,
            all_healthy(),
            None,
            Some("connection refused".into()),
            Vec::new(),
            Instant::now(),
            false,
            Vec::new(),
            Vec::new(),
            Some(3100),
        );
        assert!(!r.all_good());
        // 容器本身是全绿的 —— 这一档下「关闭」仍然该是主按钮
        assert!(r.services_all_ready());
        assert!(r.headline.contains("网页打不开"), "{}", r.headline);
    }

    /// **网页不应答就不许说「在跑」**（I17 · P0-1）。
    ///
    /// 客户 HL-GFV764 那份日志里 09:52:35 那一行是：
    /// 「Hunter 在跑，但 web、api、opencode、llm-shim、redis 停着（需要启动）、网页打不开」——
    /// 六个容器里只有 postgres 活着，网页一个字节都回不来。
    /// 面板那边已经改说话了，这一句也得跟上，否则日志里两个地方两套说法。
    #[test]
    fn 网页打不开时不说在跑() {
        // 只有 postgres 活着（客户 2026-09-29 的现场）
        let services: Vec<ServiceStatus> = EXPECTED
            .iter()
            .map(|n| {
                let state = if *n == "postgres" {
                    "running"
                } else {
                    "exited"
                };
                svc(n, state, compose::Health::Pending, None)
            })
            .collect();
        let r = finish_full(
            Posture::Partial,
            services,
            None,
            Some("Connection refused".into()),
            Vec::new(),
            Instant::now(),
            false,
            Vec::new(),
            Vec::new(),
            Some(3100),
        );
        assert_eq!(r.posture, Posture::Partial);
        assert!(
            !r.headline.contains("在跑"),
            "网页打不开就不许说「在跑」：{}",
            r.headline
        );
        assert!(r.headline.contains("没跑起来"), "{}", r.headline);
        assert!(!r.all_good());

        // 反面对照：网页应答得了、只是有服务没就绪 —— 这时候说「在跑」是对的
        let r2 = finish_full(
            Posture::Partial,
            EXPECTED
                .iter()
                .map(|n| svc(n, "running", compose::Health::Healthy, None))
                .collect(),
            Some(200),
            None,
            Vec::new(),
            Instant::now(),
            false,
            Vec::new(),
            vec!["opencode".to_string()],
            Some(3100),
        );
        assert!(
            r2.headline.contains("在跑"),
            "网页打得开、只是有服务没就绪，说「在跑」没错：{}",
            r2.headline
        );
    }

    // ── I18 · P0-2：`RuntimeDown` 的两态 ────────────────────────────────

    /// 造一份「装没装 / 跑没跑」的检测结果。
    fn docker_info(installed: bool, daemon_running: bool) -> crate::runtime::docker::DockerInfo {
        let mut d = crate::runtime::docker::DockerInfo::empty();
        d.installed = installed;
        d.daemon_running = daemon_running;
        d.runtime_label = Some("Docker Desktop".to_string());
        d
    }

    /// **A2-2**：`installed=true, daemon=false`（客户 2026-09-29 那台）——
    /// headline 要说清「Docker Desktop 也装着、只是没在跑」，并指出那条出路。
    ///
    /// 原来那一句（「只是运行时没在跑」）对**没装**的人是对的，
    /// 对装了没在跑的人少说了一半 —— 他不知道启动器能不能替他拉起来。
    #[test]
    fn 运行时没在跑时_headline_要说清是装了没跑() {
        let h = runtime_down_headline(Some(&docker_info(true, false)));
        assert!(h.contains("装在这台电脑上"), "{h}");
        assert!(h.contains("Docker Desktop"), "{h}");
        assert!(h.contains("没在跑"), "{h}");
        assert!(h.contains("启动运行时"), "要指出那条出路：{h}");
        // 这两句一个都不能丢（R1 的原始要求）
        assert!(
            h.contains("不用重新安装") && h.contains("不用重新填 key"),
            "{h}"
        );
        assert!(!h.contains("还没有装"), "不能再说成「没装」：{h}");
    }

    /// 没装 / 读不到时走原来那一句 —— **读不到就不说**，不猜成「装了没在跑」。
    #[test]
    fn 问不出来时不猜是装了没跑() {
        for d in [
            Some(docker_info(false, false)), // 没装
            Some(docker_info(true, true)),   // 装着也在跑（走到这一档说明是别的问题）
            None,                            // 压根没去问
        ] {
            let h = runtime_down_headline(d.as_ref());
            assert!(
                h.contains("装在这台电脑上") && h.contains("运行时没在跑"),
                "{d:?}：{h}"
            );
            assert!(!h.contains("启动运行时"), "问不出来就别指那条按钮：{h}");
        }
    }
}
