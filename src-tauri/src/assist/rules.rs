//! 第一层 · 确定性规则（I4 §三）。
//!
//! **顺序不能反**：先走这一层，认出来了就不调模型。
//! 零延迟、零 token、结果可复现，而且同一个现象每次给的都是同一句话 ——
//! 这三条是 AI 那一层永远给不了的。
//!
//! 认得出的情况：
//!
//! | 现象 | 判据 | 给的动作 |
//! |---|---|---|
//! | 装了但没起来 | docker 客户端在、daemon 不在，而 OrbStack / Docker Desktop / colima / systemd 之一**装了但进程没起** | 「启动它」，`confident = true` → **不走 AI** |
//! | 没装 | 所有已知位置都探不到 docker | 安装指引 + 探测明细 |
//! | 端口被占 | 5 个端口里有被占的 | 「重新分配端口」 |
//! | 磁盘不足 | 剩余 < 5 GB | 提示清理，不给动作（我们不替用户删东西） |
//! | 项目名冲突 | 错误码是 `E_PROJECT_CONFLICT` | 三条出路，**不给动作**（有一条会删数据） |
//! | 拉取失败 | 错误码是 `E_PULL_FAILED` | 「换镜像源」 |
//! | **虚拟机没有 DNS**（I10） | 在 Hunter 自己那台虚拟机里**真的解析过一次**域名，没成 | 「给这台虚拟机写好 DNS」，`confident = true` → **不走 AI** |
//! | **容器连不上网关**（I10） | 错误码是 `E_CONTAINER_OFFLINE` | 内置运行时 → 「写好 DNS」；用户自己的 Docker → **不给动作**（改他的网络设置是禁止项） |
//!
//! 认不出来时返回一条 `confident = false` 的兜底，界面据此把
//! 「让 AI 帮我看看」那个按钮显出来。

use serde::Serialize;

use crate::assist::actions::{self, Call};
use crate::assist::probe::{human_bytes, Report};

/// 拉镜像 + 解压要的余量。低于这个数就提醒 —— 六个镜像解压后是几个 GB 的量级。
pub const MIN_DISK_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// 一条建议。规则层与 AI 层共用这个形状，界面只按 `source` 区分显示。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    /// 规则 id，稳定不变，测试与日志按它对
    pub rule: String,
    pub code: Option<String>,
    pub title: String,
    /// 正文（可能多行）
    pub detail: String,
    /// 建议的动作。界面按 `kind` 决定是直接执行还是先让用户确认
    pub actions: Vec<actions::Plan>,
    /// 规则层有把握到「不必再问 AI」的程度。
    /// 为真时界面**不显示**「让 AI 帮我看看」的主按钮（仍可从「还是不行」进去）
    pub confident: bool,
}

/// 跑一遍规则。永远返回一条 —— 认不出来也要说句话，不能留给用户一片空白。
pub fn diagnose(r: &Report) -> Suggestion {
    // **这一条排在所有 docker 规则前面**（I9 的 P0-1）：
    // 「Hunter 自己那台虚拟机没起来」与「用户的 Docker 没起来」是两个现场，
    // 顺序反了就会走成 0.1.8 在用户 Mac 上那样 —— 用裸 `colima start`
    // 去「启动用户的 Colima」，而用户根本没装过 Colima。
    if let Some(s) = builtin_runtime_down(r) {
        return s;
    }
    // **这一条紧跟在它后面**（I10 的 P0-2）：虚拟机起来了、docker 也连得上，
    // 可它一个域名都解析不了 —— 0.1.9 在用户 Mac 上就卡在这儿 13 分钟，
    // 最后报的是「启动超时」，规则层 unknown。这不是超时，是没有 DNS。
    if let Some(s) = vm_no_dns(r) {
        return s;
    }
    if let Some(s) = container_offline(r) {
        return s;
    }
    if let Some(s) = daemon_down(r) {
        return s;
    }
    if let Some(s) = docker_missing(r) {
        return s;
    }
    if let Some(s) = project_conflict(r) {
        return s;
    }
    if let Some(s) = cred_helper(r) {
        return s;
    }
    if let Some(s) = pull_failed(r) {
        return s;
    }
    if let Some(s) = port_taken(r) {
        return s;
    }
    if let Some(s) = disk_low(r) {
        return s;
    }
    unknown(r)
}

/// **Hunter 自己那套内置运行时装着、虚拟机没起来**（I9 的 P0-1）。
///
/// 0.1.8 在用户 Mac 上这个现场被判成了 `daemon-down-app-installed`，
/// 给出的动作是「启动 Colima」—— 而用户从来没装过 Colima，
/// 那份 colima 是我们自己下到 `~/.hunter/runtime/bin` 里的。
/// 执行的裸 `colima start` 在他家目录里建了一台没人要的虚拟机（1.4 GB）。
///
/// 判据只看两件事，都是确定性的：内置运行时装了点东西、它的 socket 连不上。
fn builtin_runtime_down(r: &Report) -> Option<Suggestion> {
    // daemon 已经连上了就不是这个现场（哪怕连的是用户自己的 Docker）
    if r.daemon_running {
        return None;
    }
    // **判据来自侦察员采的那份现场，不去现查**（I9 自审）。
    //
    // 原先这里直接调 `effective::current()`，两个毛病：同一次诊断里现场被采了两遍
    // （规则层看到的可能和报文里写的不是同一件事）；以及**这条规则在任何一台
    // 装着 Docker 的机器上都测不了** —— 开发机与测试机都常驻 docker，
    // `current()` 永远返回「用户那一套」，分支一次都跑不到。
    let a = r.apps.iter().find(|x| x.id == "builtin")?;
    if a.running == Some(true) {
        return None;
    }
    // 工具齐了（installed = true）→ 起它；还缺文件（false）→ 先补齐
    let tools_ready = a.installed == Some(true);
    let rt = crate::redact::mask_home(&crate::paths::runtime_dir().to_string_lossy());
    let (call, detail) = if tools_ready {
        (
            Call::new("start_builtin_runtime"),
            format!(
                "Hunter 自己那套运行时已经装好了（在 {rt} 里），只是它的虚拟机没在跑。\n\
                 这不是你电脑上的 Docker，也不是你装的 Colima —— 启动器不会去碰你的 ~/.colima。\n\
                 把 profile {} 这台虚拟机起起来就行，系统镜像本机已经有、校验过，不用下东西。\n\
                 （判据：{}）",
                crate::runtime::builtin::PROFILE,
                a.evidence
            ),
        )
    } else {
        (
            Call::new("install_runtime"),
            format!(
                "上一次安装没有装完：Hunter 自己那套运行时还缺几个文件。\n\
                 这不是你电脑上的 Docker —— 它是启动器自己下到 {rt} 里的一套，\
                 只服务 Hunter，不改你系统里的任何东西。\n\
                 把缺的那几个补齐再起虚拟机就行；已经下好并校验过的文件一个字节都不会重下。\n\
                 （判据：{}）",
                a.evidence
            ),
        )
    };
    // 规划不出来（这个平台上装不了内置运行时，例如 Linux）时**照样出这条结论** ——
    // 少一个按钮不该让整条规则失效、掉到优先级更低的规则上去。
    // 这是 I4 在 `daemon_down` 那条上踩过一次的坑，同一个教训不踩第二遍。
    let planned = actions::plan(&call);
    let detail = match &planned {
        Ok(_) => detail,
        Err(e) => format!("{detail}\n启动器在这个平台上做不了这一步：{}", e.msg),
    };
    Some(Suggestion {
        rule: "builtin-runtime-down".into(),
        code: Some("E_BUILTIN_DOWN".into()),
        title: "Hunter 自己那台虚拟机没起来".into(),
        detail,
        actions: planned.into_iter().collect(),
        // 判据全是文件与 socket，查得清清楚楚 —— 这一条不必花 token
        confident: true,
    })
}

/// **Hunter 自己那台虚拟机没有 DNS**（I10 的 P0-1 / P0-2）。
///
/// 判据只有一条，而且是确定性的：侦察员在那台虚拟机里**真的解析过一次**
/// `hunter.agentpit.io`，没解析出来。不是「日志里像是网络问题」，
/// 是「试过了，不行」—— 所以这一条 `confident = true`，一个 token 都不花。
///
/// 这条规则**必须排在 `pull-failed` 与「起不来」前面**：没有 DNS 的机器上，
/// 拉镜像（走代理）可能是成的、容器也起得来，表现出来只是「某个服务一直不健康」。
/// 顺序反了就会去换镜像源 —— 和 I6 那次「换了三次源、一个问题都没解决」一模一样。
fn vm_no_dns(r: &Report) -> Option<Suggestion> {
    let d = r.vm_dns.as_ref()?;
    // 解析得动 / 根本没查成 → 都不是这一条
    if d.resolves != Some(false) {
        return None;
    }
    let plan = actions::plan(&Call::new("fix_vm_dns")).ok()?;
    Some(Suggestion {
        rule: "vm-no-dns".into(),
        code: Some("E_RUNTIME_NO_DNS".into()),
        title: "Hunter 自己那台虚拟机没有可用的 DNS".into(),
        detail: format!(
            "启动器自己那台虚拟机（{}）里解析不了任何域名，所以它里面的容器\n\
             既过不了健康检查，也连不上 Hunter 的模型网关 —— 就算六个服务全绿也没法对话。\n\
             根子在镜像本身：这份 Ubuntu 24.04 minimal 镜像里没有 systemd-resolved，\n\
             而 /etc/resolv.conf 出厂就是一条指向它的断链，没有任何东西会去把它填上。\n\
             启动器可以直接在这台虚拟机里把它写成普通文件（{}），并装一个开机重写它的服务。\n\
             你这台电脑的 DNS、hosts、代理、防火墙一个字节都不会动。\n\
             （判据：{}）",
            d.instance,
            crate::runtime::vmdns::NAMESERVERS
                .iter()
                .map(|(ns, _)| *ns)
                .collect::<Vec<_>>()
                .join(" / "),
            d.one_line()
        ),
        actions: vec![plan],
        // 试过了、不行 —— 这一条不必花 token
        confident: true,
    })
}

/// **容器连不上模型网关**（I10 的 P0-2）。
///
/// 这条规则**一个动作都不给**，而且这是有意的：
///
/// | 现场 | 谁来管 |
/// |---|---|
/// | 虚拟机自己就解析不了域名 | 上面那条 `vm-no-dns`（它排在前面，有动作） |
/// | 虚拟机解析得动、容器却连不上 | **不是 DNS 断链那一类**，多半是代理或防火墙只放行了宿主机 —— 启动器不会去改你电脑的网络设置 |
/// | 跑在用户自己的 Docker 上 | 同上，而且更不能碰 |
///
/// 「我们修不了」这件事要说清楚，不要给一个看着像能修、其实没用的按钮 ——
/// I6 那次「换了三次源、268 秒、一个问题都没解决」就是这么来的。
fn container_offline(r: &Report) -> Option<Suggestion> {
    if r.error_code.as_deref() != Some("E_CONTAINER_OFFLINE") {
        return None;
    }
    let vm = r.vm_dns.as_ref();
    let builtin =
        vm.is_some_and(|d| !matches!(d.resolv, crate::runtime::vmdns::ResolvState::Unknown(_)));
    let head = if builtin {
        format!(
            "起了一个一次性容器去连 hunter.agentpit.io，没连上。\n\
             而 Hunter 自己那台虚拟机是解析得动域名的（{}）—— \n\
             所以这不是「虚拟机没有 DNS」那一类，启动器没有对症的修法。\n",
            vm.map(|d| d.one_line()).unwrap_or_default()
        )
    } else {
        "起了一个一次性容器去连 hunter.agentpit.io，没连上。\n\
         这一套跑在你自己装的那个 Docker 上，容器的 DNS 与出网由它和你的网络环境决定。\n"
            .to_string()
    };
    Some(Suggestion {
        rule: if builtin {
            "container-offline-builtin".into()
        } else {
            "container-offline-user-docker".into()
        },
        code: Some("E_CONTAINER_OFFLINE".into()),
        title: "容器连不上 Hunter 的模型网关".into(),
        detail: format!(
            "{head}\
             启动器不会去改你电脑的 DNS、hosts、代理或防火墙设置 —— \n\
             这是写进程序里的红线，不是一句承诺。\n\
             常见的两种原因：代理只放行了本机、没放行容器网段；或者网络挡了 443。\n\
             容器起得来但问不出话时，先从这两条查。"
        ),
        actions: Vec::new(),
        confident: true,
    })
}

/// 「装了但没起来」。这一条是 I4 点名要**不走 AI** 的那一个。
fn daemon_down(r: &Report) -> Option<Suggestion> {
    if !r.docker_installed || r.daemon_running {
        return None;
    }
    // 装了、但进程没起 —— 两个条件都要，「读不到」不算。
    // **`builtin` 排除掉**（I9）：它不是用户装的，起它走 `builtin-runtime-down`
    let candidate = r
        .apps
        .iter()
        .filter(|a| a.id != "builtin")
        .find(|a| a.installed == Some(true) && a.running == Some(false))?;

    // 启动动作规划不出来（这个平台上没法替他点）时**照样出这条建议** ——
    // 结论本身是对的，少一个按钮不该让整条规则失效、掉到优先级更低的规则上去
    // （I4 自审：原先这里是 `.ok()?`，在没有 GUI 应用的平台上会静悄悄地掉到「端口被占」）
    let planned = actions::plan(&Call::with("start_runtime", "app", &candidate.id));
    let tail = match &planned {
        // I9：全自动档下这一步由启动器自己执行（见 `assist::diagnose_and_fix`），
        // 所以话不该是「点下面那个按钮」——「逐步确认」档才会真出一个按钮。
        // 0.1.8 的用户在授权页上选的是全自动，界面却让他连点了四次。
        Ok(_) => "启动器会替你把它启动起来，再等它就绪。".to_string(),
        Err(e) => format!(
            "启动器在这个平台上没法替你启动它（{}）—— 这一步只能由 {} 自己来。",
            e.msg, candidate.label
        ),
    };
    Some(Suggestion {
        rule: "daemon-down-app-installed".into(),
        code: Some("E_DAEMON_DOWN".into()),
        title: format!("{} 装着，但没在跑", candidate.label),
        detail: format!(
            "Docker 的客户端在（{}），但连不上后台服务。\n\
             这台机器上 {} 是装了的，只是进程没起来（判据：{}）。\n\
             {tail}",
            r.docker_path.as_deref().unwrap_or("路径读不到"),
            candidate.label,
            candidate.evidence
        ),
        actions: planned.into_iter().collect(),
        // 规则认得清清楚楚，这一条不必花 token 去问模型
        confident: true,
    })
}

/// 没找到 docker。**把探过的位置全列出来** —— 这正是 0.1.3 在用户 mac 上缺的那一段。
fn docker_missing(r: &Report) -> Option<Suggestion> {
    if r.docker_installed {
        return None;
    }
    let mut detail = String::new();
    if r.os == "macos" {
        detail.push_str(
            "macOS 上「从访达或程序坞启动的程序」拿不到你终端里的 PATH\
             （GUI 程序默认只有 /usr/bin:/bin:/usr/sbin:/sbin）。\
             所以「终端里 docker version 有输出」和「启动器找得到 docker」是两件事。\n\
             启动器已经按已知安装位置挨个探过了：\n\n",
        );
    } else {
        detail.push_str("按已知位置挨个探过了，都没有可执行的 docker：\n\n");
    }
    for l in &r.probe_lines {
        detail.push_str(l);
        detail.push('\n');
    }
    detail.push_str(&format!("\n本进程拿到的 PATH：{}\n", r.env_path));

    // 装了某个运行时但 docker 命令仍然找不到：多半是 CLI 没装进来
    let installed_app = r.apps.iter().find(|a| a.installed == Some(true));
    if let Some(a) = installed_app {
        detail.push_str(&format!(
            "\n注意：这台机器上 {} 是装了的，但它的命令行工具不在上面任何一个位置。\
             要么它装在别处（可以用「让 AI 帮我看看」问一下），\
             要么它的 CLI 没有装进来。\n",
            a.label
        ));
    }

    Some(Suggestion {
        rule: "docker-not-found".into(),
        code: Some("E_DOCKER_MISSING".into()),
        title: "所有已知位置都没找到 docker".into(),
        detail,
        actions: vec![actions::plan(&Call::new("probe_docker_path")).ok()]
            .into_iter()
            .flatten()
            .collect(),
        // 真没装还是装在别处，规则层分不清 —— 这一条要留给 AI 接着问
        confident: false,
    })
}

/// 本机 docker 凭据助手找不到（I6 的 P0）。
///
/// **必须排在「拉不动 → 换源」前面**：0.1.5 在用户 Mac 上就是把它当成源不通，
/// 来回换了三次源、268 秒、一个问题都没解决。换源修不好本机配置。
fn cred_helper(r: &Report) -> Option<Suggestion> {
    if r.error_code.as_deref() != Some("E_CRED_HELPER") {
        return None;
    }
    let status = crate::dockercfg::helper_status();
    let missing: Vec<String> = status
        .iter()
        .filter(|(_, f)| f.is_none())
        .map(|(n, _)| n.clone())
        .collect();
    let sub = crate::runtime::env::current().one_line();
    let (calls, detail) = if missing.is_empty() {
        (
            vec![Call::with("retry_pull", "delay_seconds", "0")],
            format!(
                "Docker 要用本机的凭据助手去取登录信息，刚才那一次没找到它。\n\
                 现在按已知位置补全之后是找得到的（{sub}），再拉一次就行。"
            ),
        )
    } else {
        (
            vec![
                Call::new("use_isolated_docker_config"),
                Call::with("retry_pull", "delay_seconds", "0"),
            ],
            format!(
                "你的 docker 配置里写着要用 {} 去取登录信息，但这台机器上补全 PATH 之后仍然找不到它。\n\
                 （{sub}）\n\
                 Hunter 的六个镜像都是公开的，本来就不需要登录。\n\
                 启动器可以给自己另起一份不带凭据助手的 docker 配置（放在 ~/.hunter/docker-config/），\n\
                 你的 ~/.docker/config.json 一个字节都不会动。",
                missing.join("、")
            ),
        )
    };
    let actions: Vec<actions::Plan> = calls.iter().filter_map(|c| actions::plan(c).ok()).collect();
    Some(Suggestion {
        rule: "cred-helper-missing".into(),
        code: Some("E_CRED_HELPER".into()),
        title: "Docker 找不到它自己的凭据助手".into(),
        detail,
        actions,
        // 判据是确定的：配置里写了谁、那个文件在不在，两个都查得清清楚楚
        confident: true,
    })
}

fn pull_failed(r: &Report) -> Option<Suggestion> {
    if r.error_code.as_deref() != Some("E_PULL_FAILED") {
        return None;
    }
    // 现在用的不是国内源时，换源是最见效的一步
    let other = crate::registry::CANDIDATES
        .iter()
        .find(|c| c.id != r.registry_id)?;
    let plan = actions::plan(&Call::with("switch_registry", "registry", &other.id)).ok()?;
    Some(Suggestion {
        rule: "pull-failed-switch-registry".into(),
        code: Some("E_PULL_FAILED".into()),
        title: "镜像没拉下来".into(),
        detail: format!(
            "现在用的源是「{}」（{}）。\n\
             拉不动最常见的原因就是这个源在你的网络里不通 —— 国内直连 ghcr.io 经常超时。\n\
             可以换成「{}」（{}）再试一次。",
            r.registry_id, r.registry_prefix, other.label, other.prefix
        ),
        actions: vec![plan],
        // 换源大概率有用，但也可能是代理、磁盘、认证的问题，留给用户决定要不要问 AI
        confident: false,
    })
}

/// 另一个工作目录占着 compose 项目名 `hunter`（待办池 P0-5 的那道闸门）。
///
/// 这条规则**不给动作**：三条出路（改回原工作目录 / 整个目录搬过来 / 从零开始并丢数据）
/// 里选哪一条只有用户自己知道，而第三条会删数据卷 —— 不替他按。
fn project_conflict(r: &Report) -> Option<Suggestion> {
    if r.error_code.as_deref() != Some("E_PROJECT_CONFLICT") {
        return None;
    }
    Some(Suggestion {
        rule: "project-conflict".into(),
        code: Some("E_PROJECT_CONFLICT".into()),
        title: "另一个工作目录正占着 compose 项目名「hunter」".into(),
        detail: "这台机器上已经有一套 Hunter 在用 `hunter` 这个 compose 项目名，\
             而现在这个工作目录不是它。继续下去会把那一套的配置和端口一起顶掉。\n\n\
             要紧的一点：数据卷是跟着项目名走的，不是跟着工作目录走的 —— \
             新建一个空工作目录会现生成一把新的数据库口令，api 连不上已有的库。\n\n\
             三条出路（上面的错误详情里有完整命令）：\n\
             · 想接着用那一套（推荐）：把 HUNTER_HOME 改回去，或者干脆不设这个变量；\n\
             · 想把工作目录挪过来：先 --down，再把整个目录搬过来，别新建空的；\n\
             · 想从零开始：先 --down，再删掉项目名为 hunter 的数据卷（会丢数据）。\n\n\
             最后一条会删数据，启动器不替你按。"
            .to_string(),
        actions: Vec::new(),
        confident: true,
    })
}

fn port_taken(r: &Report) -> Option<Suggestion> {
    // **我们自己这一套占着的不算冲突** —— 装好之后再来诊断时，3101 本来就该被
    // 我们的 web 容器占着。不排掉的话，规则层会对着一套跑得好好的 Hunter 说
    // 「5 个端口被别的程序占着」（I4 取错误页截图时撞到的）。
    let taken: Vec<&crate::assist::probe::PortState> =
        r.ports.iter().filter(|p| !p.free && !p.ours).collect();
    if taken.is_empty() {
        return None;
    }
    let plan = actions::plan(&Call::new("remap_ports")).ok()?;
    Some(Suggestion {
        rule: "ports-taken".into(),
        code: Some("E_PORT_IN_USE".into()),
        title: format!("{} 个端口被别的程序占着", taken.len()),
        detail: format!(
            "被占的是：{}。\n\
             启动器可以自动往上挪到空闲端口并写进配置，容器重启之后用新端口访问。",
            taken
                .iter()
                .map(|p| format!("{} {}", p.service, p.port))
                .collect::<Vec<_>>()
                .join("、")
        ),
        actions: vec![plan],
        confident: true,
    })
}

fn disk_low(r: &Report) -> Option<Suggestion> {
    let free = r.disk_free?;
    if free >= MIN_DISK_BYTES {
        return None;
    }
    Some(Suggestion {
        rule: "disk-low".into(),
        code: None,
        title: "磁盘快满了".into(),
        detail: format!(
            "工作目录所在分区只剩 {}，而六个镜像解压之后要几个 GB。\n\
             先腾点地方再来 —— `docker system prune -a` 能清掉不用的镜像层，\
             但它会连别的项目的镜像一起删，所以这一条启动器不替你执行。",
            human_bytes(free)
        ),
        // 我们不替用户删东西，所以这一条没有动作
        actions: Vec::new(),
        confident: true,
    })
}

/// 规则认不出来时说的话。**不装懂** —— 如实说认不出来，并把 AI 那条路指出来。
fn unknown(r: &Report) -> Suggestion {
    Suggestion {
        rule: "unknown".into(),
        code: r.error_code.clone(),
        title: "这个情况确定性规则认不出来".into(),
        detail: "启动器内置的规则里没有和现在这个现象对得上的。\n\
             诊断信息已经收好了（下面可以整份复制）。\n\
             点「让 AI 帮我看看」会把这份信息送去 Hunter 网关问一下；\
             不想用的话，可以直接把它贴到 GitHub issue 里。"
            .to_string(),
        actions: Vec::new(),
        confident: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assist::probe::{AppPresence, PortState};

    fn base() -> Report {
        Report {
            launcher_version: "0.1.4".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            os_version: Some("15.1".into()),
            effective_runtime: "当前生效运行时：测试（测试）".into(),
            error_code: None,
            error_message: None,
            stage: Some("docker".into()),
            docker_installed: true,
            daemon_running: true,
            docker_runtime: "Orbstack".into(),
            docker_client: Some("29.4.0".into()),
            docker_server: Some("29.4.0".into()),
            compose_version: Some("v2.39.1".into()),
            compose_mode: Some("docker compose 插件 v2.39.1".into()),
            docker_path: Some("/usr/local/bin/docker".into()),
            probe_lines: vec!["✓ [已知位置] /usr/local/bin/docker — 可执行".into()],
            env_path: "/usr/bin:/bin:/usr/sbin:/sbin".into(),
            apps: Vec::new(),
            ports: vec![
                PortState {
                    service: "web".into(),
                    port: 3100,
                    free: true,
                    ours: false,
                },
                PortState {
                    service: "api".into(),
                    port: 8100,
                    free: true,
                    ours: false,
                },
            ],
            disk_free: Some(50 * 1024 * 1024 * 1024),
            disk_total: Some(100 * 1024 * 1024 * 1024),
            registry_id: "ghcr".into(),
            registry_prefix: "ghcr.io/agentpit-io".into(),
            hunter_tag: "1.2.0".into(),
            commands: Vec::new(),
            vm_dns: None,
            log_tail: Vec::new(),
        }
    }

    fn app(id: &str, installed: Option<bool>, running: Option<bool>) -> AppPresence {
        AppPresence {
            id: id.into(),
            label: actions::runtime_label(id).into(),
            installed,
            running,
            evidence: "测试".into(),
        }
    }

    /// **界面上不该出现 Markdown 的星号，也不该出现「请在终端执行」。**
    ///
    /// 前一条是 I10 在 Xvfb 截图里当场看见的：错误页上原样印着
    /// 「这一套跑在\*\*你自己装的 Docker\*\* 上」——
    /// 这些 `detail` 是直接渲染成纯文本的，写 Markdown 只会把星号显给用户看。
    /// i18n 那一侧早就有同样一条测试（`src/i18n/i18n.test.ts`），规则层这一侧原来没有。
    ///
    /// 后一条是 2026-09-21 22:10 那条产品原则：界面永远不出现
    /// 「请在终端里执行」「复制命令」这一类话。（wording-ok：这里是禁用名单本身）
    #[test]
    fn 规则层给用户看的每一句都不带星号也不支使他去敲命令() {
        let mut cases: Vec<Report> = Vec::new();
        // 逐条把每一个规则的现场造出来
        for code in [
            "E_PULL_FAILED",
            "E_CRED_HELPER",
            "E_PROJECT_CONFLICT",
            "E_CONTAINER_OFFLINE",
            "E_RUNTIME_NO_DNS",
        ] {
            let mut r = base();
            r.error_code = Some(code.into());
            cases.push(r);
        }
        // 虚拟机没有 DNS
        let mut r = base();
        r.vm_dns = Some(vm_dns(Some(false)));
        cases.push(r);
        // 容器连不上，但虚拟机是好的
        let mut r = base();
        r.error_code = Some("E_CONTAINER_OFFLINE".into());
        r.vm_dns = Some(vm_dns(Some(true)));
        cases.push(r);
        // 端口被占
        let mut r = base();
        r.ports[0].free = false;
        cases.push(r);
        // 磁盘快满
        let mut r = base();
        r.disk_free = Some(1024 * 1024 * 1024);
        cases.push(r);
        // 认不出来
        cases.push(base());

        for r in cases {
            let s = diagnose(&r);
            for (what, text) in [("标题", &s.title), ("正文", &s.detail)] {
                assert!(
                    !text.contains("**"),
                    "规则 {} 的{}里有 Markdown 星号，会原样印到界面上：{text}",
                    s.rule,
                    what
                );
                // wording-ok：这一行是禁用名单本身，不是给用户看的文案
                for bad in ["请在终端", "复制命令", "打开终端", "我执行完了"] {
                    assert!(
                        !text.contains(bad),
                        "规则 {} 的{}里在支使用户去敲命令（「{bad}」）：{text}",
                        s.rule,
                        what
                    );
                }
            }
        }
    }

    // ── I10：虚拟机没有 DNS ──────────────────────────────────────────

    fn vm_dns(resolves: Option<bool>) -> crate::runtime::vmdns::VmDns {
        crate::runtime::vmdns::VmDns {
            instance: "colima-hunter".into(),
            resolv: match resolves {
                Some(true) => {
                    crate::runtime::vmdns::ResolvState::File("nameserver 192.168.5.2\n".into())
                }
                _ => crate::runtime::vmdns::ResolvState::Broken(
                    "cat: /etc/resolv.conf: No such file or directory".into(),
                ),
            },
            resolves,
            resolve_detail: "".into(),
            unit_installed: Some(false),
        }
    }

    /// 0.1.9 用户 Mac 上那个现场：虚拟机跑着、docker 连得上，但一个域名都解析不了。
    /// 规则层必须**确定性**地认出来，而不是 unknown。
    #[test]
    fn 虚拟机解析不动域名时规则层认得出来且不必问_ai() {
        let mut r = base();
        r.vm_dns = Some(vm_dns(Some(false)));
        r.error_code = Some("E_START_TIMEOUT".into());
        let s = diagnose(&r);
        assert_eq!(s.rule, "vm-no-dns", "{s:?}");
        assert_eq!(s.code.as_deref(), Some("E_RUNTIME_NO_DNS"));
        assert!(s.confident, "试过一次就知道的事，不该再花 token");
        assert_eq!(s.actions.len(), 1);
        assert_eq!(s.actions[0].id, "fix_vm_dns");
        // 这一句是给用户看的，必须把「不动你电脑」说出来
        assert!(s.detail.contains("一个字节都不会动"), "{}", s.detail);
    }

    /// 解析得动就**不该**触发 —— 否则每次安装都会去改一遍虚拟机的文件。
    #[test]
    fn 虚拟机解析得动时这条规则不触发() {
        let mut r = base();
        r.vm_dns = Some(vm_dns(Some(true)));
        assert_ne!(diagnose(&r).rule, "vm-no-dns");
        // 「没查成」也不算「不通」（红线 1）
        r.vm_dns = Some(vm_dns(None));
        assert_ne!(diagnose(&r).rule, "vm-no-dns");
    }

    /// 跑在**用户自己的 Docker** 上时容器连不上 —— 我们没得修，
    /// 只能说清楚；**绝不给一个会去改他网络设置的动作**。
    #[test]
    fn 用户自己的_docker_上容器连不上时不给动作() {
        let mut r = base();
        r.error_code = Some("E_CONTAINER_OFFLINE".into());
        r.vm_dns = None;
        let s = diagnose(&r);
        assert_eq!(s.rule, "container-offline-user-docker");
        assert!(s.actions.is_empty(), "不该给任何动作：{:?}", s.actions);
        assert!(s.detail.contains("不会去改你电脑"), "{}", s.detail);
    }

    /// 虚拟机解析得动、容器却连不上 —— **这一类我们修不了，就别给按钮**。
    #[test]
    fn 虚拟机没问题而容器连不上时如实说修不了() {
        let mut r = base();
        r.error_code = Some("E_CONTAINER_OFFLINE".into());
        r.vm_dns = Some(vm_dns(Some(true)));
        let s = diagnose(&r);
        assert_eq!(s.rule, "container-offline-builtin");
        assert!(s.actions.is_empty(), "不该给动作：{:?}", s.actions);
        assert!(s.detail.contains("没有对症的修法"), "{}", s.detail);
    }

    /// 虚拟机本身就解析不动时，**先认「没有 DNS」那一条**（它有修法）。
    #[test]
    fn 虚拟机没有_dns_时优先走那条有修法的规则() {
        let mut r = base();
        r.error_code = Some("E_CONTAINER_OFFLINE".into());
        r.vm_dns = Some(vm_dns(Some(false)));
        let s = diagnose(&r);
        assert_eq!(s.rule, "vm-no-dns");
        assert_eq!(s.actions[0].id, "fix_vm_dns");
    }

    /// I4 测试场景 2 的期望：规则层直接识别并给「启动」动作，**不该走到 AI**。
    #[test]
    fn 装了但没起来时规则层直接给启动动作且不走_ai() {
        // 按平台挑一个这台机器上真能启动的运行时 —— 写死 systemd 的话，
        // CI 的 macOS / Windows runner 上必红（I4 第一版就是这样）
        let Some((app_id, cmd_part)) = actions::tests::startable_app() else {
            // Windows：一个都启动不了，那就验「结论照给、但说清没法替你按」那条路
            let mut r = base();
            r.daemon_running = false;
            r.apps = vec![app("docker-desktop", Some(true), Some(false))];
            let s = diagnose(&r);
            assert_eq!(s.rule, "daemon-down-app-installed");
            assert!(s.confident);
            assert!(s.actions.is_empty());
            assert!(s.detail.contains("没法替你启动它"), "{}", s.detail);
            return;
        };

        let mut r = base();
        r.daemon_running = false;
        r.apps = vec![app(app_id, Some(true), Some(false))];
        let s = diagnose(&r);
        assert_eq!(s.rule, "daemon-down-app-installed");
        assert!(
            s.confident,
            "这一条必须 confident，否则界面会把人往 AI 那边引"
        );
        assert_eq!(s.actions.len(), 1);
        assert_eq!(s.actions[0].id, "start_runtime");
        assert_eq!(
            s.actions[0].kind,
            actions::Kind::Mutating,
            "启动服务要用户确认"
        );
        // 展示的命令必须是完整的那一条
        assert!(
            s.actions[0].command_line().contains(cmd_part),
            "{}",
            s.actions[0].command_line()
        );
    }

    #[test]
    fn 装了但读不到进程状态时不硬说它没起() {
        let mut r = base();
        r.daemon_running = false;
        r.apps = vec![app("orbstack", Some(true), None)];
        let s = diagnose(&r);
        assert_ne!(
            s.rule, "daemon-down-app-installed",
            "「读不到」不等于「没起」"
        );
    }

    /// I4 测试场景 1：规则层给安装指引，并且**把探过的位置全列出来**。
    #[test]
    fn 没找到_docker_时列出探过的每一个位置() {
        let mut r = base();
        r.docker_installed = false;
        r.daemon_running = false;
        r.docker_path = None;
        r.probe_lines = vec![
            "· [已知位置] /usr/local/bin/docker — 没有这个文件".into(),
            "· [已知位置] /opt/homebrew/bin/docker — 没有这个文件".into(),
        ];
        let s = diagnose(&r);
        assert_eq!(s.rule, "docker-not-found");
        assert_eq!(s.code.as_deref(), Some("E_DOCKER_MISSING"));
        assert!(s.detail.contains("/usr/local/bin/docker"), "{}", s.detail);
        assert!(
            s.detail.contains("/opt/homebrew/bin/docker"),
            "{}",
            s.detail
        );
        // macOS 上要把 GUI 程序 PATH 这件事说清楚 —— 0.1.3 缺的就是这句
        assert!(s.detail.contains("拿不到你终端里的 PATH"), "{}", s.detail);
        assert!(
            s.detail.contains("/usr/bin:/bin:/usr/sbin:/sbin"),
            "PATH 原文要贴出来：{}",
            s.detail
        );
        assert!(
            !s.confident,
            "真没装还是装在别处，规则层分不清，要留 AI 这条路"
        );
    }

    #[test]
    fn 装了_orbstack_但找不到命令行时要点出来() {
        let mut r = base();
        r.docker_installed = false;
        r.daemon_running = false;
        r.apps = vec![app("orbstack", Some(true), Some(true))];
        let s = diagnose(&r);
        assert_eq!(s.rule, "docker-not-found");
        assert!(s.detail.contains("OrbStack 是装了的"), "{}", s.detail);
    }

    #[test]
    fn 端口被占时给出重新分配的动作() {
        let mut r = base();
        r.ports[0].free = false;
        let s = diagnose(&r);
        assert_eq!(s.rule, "ports-taken");
        assert_eq!(s.code.as_deref(), Some("E_PORT_IN_USE"));
        assert!(s.detail.contains("web 3100"), "{}", s.detail);
        assert_eq!(s.actions[0].id, "remap_ports");
        assert!(s.confident);
    }

    /// I4 测试场景 4：规则层报 E_PULL_FAILED，并建议换国内源。
    #[test]
    fn 拉取失败时建议换另一个源() {
        let mut r = base();
        r.error_code = Some("E_PULL_FAILED".into());
        let s = diagnose(&r);
        assert_eq!(s.rule, "pull-failed-switch-registry");
        assert_eq!(s.actions[0].id, "switch_registry");
        assert!(
            s.actions[0].title.contains("腾讯云"),
            "{}",
            s.actions[0].title
        );
    }

    /// I4 取错误页截图时撞到的：装好之后再来诊断，5 个端口本来就被
    /// **我们自己的容器**占着 —— 规则层不能对着一套跑得好好的 Hunter 说
    /// 「5 个端口被别的程序占着」。
    #[test]
    fn 自己这一套占着的端口不算冲突() {
        let mut r = base();
        r.ports[0].free = false;
        r.ports[0].ours = true;
        r.ports[1].free = false;
        r.ports[1].ours = true;
        assert_ne!(diagnose(&r).rule, "ports-taken", "自己占的不算");

        // 只要有一个是**别的程序**占的，就还是冲突
        r.ports[1].ours = false;
        let s = diagnose(&r);
        assert_eq!(s.rule, "ports-taken");
        assert!(s.detail.contains("api 8100"), "{}", s.detail);
        assert!(
            !s.detail.contains("web 3100"),
            "自己占的那个不该列出来：{}",
            s.detail
        );
    }

    /// 项目名冲突有自己的一条规则。
    ///
    /// 修 `PullProgress.error_code` 之前，这种失败在界面上会显示成「镜像拉取失败」，
    /// 于是规则层给出「换一个镜像源」—— 完全跑偏（I4 §五.2）。
    #[test]
    fn 项目名冲突有自己的结论而不是被当成拉取失败() {
        let mut r = base();
        r.error_code = Some("E_PROJECT_CONFLICT".into());
        let s = diagnose(&r);
        assert_eq!(s.rule, "project-conflict");
        assert!(s.confident);
        assert!(s.actions.is_empty(), "三条出路里有一条会删数据，不替用户按");
        assert!(s.detail.contains("数据卷是跟着项目名走的"), "{}", s.detail);
        // 绝不能顺手给出「换镜像源」
        assert!(!s.detail.contains("镜像源"), "{}", s.detail);
    }

    /// 项目名冲突要排在「端口被占」前面：项目名冲突时端口当然也是占着的，
    /// 先说端口会把人带偏。
    #[test]
    fn 项目名冲突排在端口之前() {
        let mut r = base();
        r.error_code = Some("E_PROJECT_CONFLICT".into());
        r.ports[0].free = false;
        assert_eq!(diagnose(&r).rule, "project-conflict");
    }

    #[test]
    fn 磁盘不足时提醒但不替用户删东西() {
        let mut r = base();
        r.disk_free = Some(1024 * 1024 * 1024);
        let s = diagnose(&r);
        assert_eq!(s.rule, "disk-low");
        assert!(
            s.actions.is_empty(),
            "prune 会删别的项目的镜像，不能替用户按"
        );
        assert!(s.detail.contains("1.0 GB"), "{}", s.detail);
    }

    #[test]
    fn 磁盘读不到时这条规则不触发() {
        let mut r = base();
        r.disk_free = None;
        assert_ne!(diagnose(&r).rule, "disk-low", "读不到不等于不足");
    }

    #[test]
    fn 一切正常又说不出问题时如实说认不出来() {
        let s = diagnose(&base());
        assert_eq!(s.rule, "unknown");
        assert!(!s.confident);
        assert!(s.actions.is_empty());
        assert!(s.detail.contains("认不出来") || s.title.contains("认不出来"));
    }

    /// 「daemon 没起」要排在「端口被占」前面 —— 两者的建议完全不同，顺序错了会把人带偏。
    #[test]
    fn 规则的优先级顺序() {
        let mut r = base();
        r.daemon_running = false;
        r.apps = vec![app("orbstack", Some(true), Some(false))];
        r.ports[0].free = false; // 同时端口也被占着
        assert_eq!(
            diagnose(&r).rule,
            "daemon-down-app-installed",
            "daemon 起不来的时候，端口占用是次要问题"
        );
    }

    /// I4 自审撞到的：`start_runtime` 在当前平台规划不出来时（跑测试的是 Linux，
    /// 而 OrbStack 只有 macOS 上能 `open -a`），这条规则**不能**因此整条失效 ——
    /// 原来的写法是 `.ok()?`，于是它悄悄掉到了「端口被占」那一条上，
    /// 用户看到的建议会完全跑偏。
    #[test]
    fn 启动动作规划不出来时结论仍然要给出来() {
        let mut r = base();
        r.daemon_running = false;
        r.apps = vec![app("orbstack", Some(true), Some(false))];
        let s = diagnose(&r);
        assert_eq!(s.rule, "daemon-down-app-installed");
        assert!(s.confident);
        if cfg!(target_os = "macos") {
            assert_eq!(s.actions.len(), 1, "mac 上应当给得出启动按钮");
        } else {
            assert!(s.actions.is_empty(), "非 mac 上给不出按钮");
            assert!(
                s.detail.contains("没法替你启动它"),
                "给不出按钮时要说清怎么手动做：{}",
                s.detail
            );
        }
    }

    #[test]
    fn 每条规则都有标题与正文() {
        let _g = crate::paths::test_home("rules-all");
        let mut cases = vec![base()];
        let mut r = base();
        r.daemon_running = false;
        r.apps = vec![app("systemd", Some(true), Some(false))];
        cases.push(r);
        let mut r = base();
        r.docker_installed = false;
        cases.push(r);
        let mut r = base();
        r.error_code = Some("E_PULL_FAILED".into());
        cases.push(r);
        let mut r = base();
        r.ports[0].free = false;
        cases.push(r);
        let mut r = base();
        r.disk_free = Some(1);
        cases.push(r);
        for c in cases {
            let s = diagnose(&c);
            assert!(!s.title.is_empty(), "{}", s.rule);
            assert!(!s.detail.is_empty(), "{}", s.rule);
        }
    }

    // ── I9 的 P0-1 ──────────────────────────────────────────────────────

    use crate::runtime::effective::make_fake_tools;

    /// **用户 Mac 上 0.1.8 那次的现场，一比一复现。**
    ///
    /// 报文里：docker「装了」（那份 docker 正是我们自己下的）、daemon 连不上、
    /// `builtin` 这一条是「装好了、没在跑」。
    ///
    /// 0.1.8 对着它给出的是 `daemon-down-app-installed` +「启动 Colima」，
    /// 执行的是一条裸 `colima start`。I9 之后必须是 `builtin-runtime-down` +
    /// `start_builtin_runtime`，而且**绝不能**出现 `start_runtime`。
    #[test]
    fn 内置运行时残骸走内置主线而不是启动用户的_colima() {
        let _g = crate::paths::test_home("rules-residue");
        make_fake_tools();

        let mut r = base();
        r.daemon_running = false;
        r.docker_installed = true;
        r.docker_path = Some(
            crate::paths::runtime_bin()
                .join("docker")
                .to_string_lossy()
                .into_owned(),
        );
        r.apps = vec![
            app_ev(
                "builtin",
                "Hunter 内置运行时",
                Some(true),
                Some(false),
                "看 ~/.hunter/runtime 下的四件工具 + socket 连不连得上（装好了，虚拟机没起来）",
            ),
            // **0.1.8 的报文里同时还有一条「Colima 装了没在跑」** ——
            // 那条正是我们自己那份 colima 被认成了用户的。规则必须先命中内置那条
            app_ev("colima", "你自己的 Colima", Some(true), Some(false), "测试"),
        ];
        let s = diagnose(&r);
        assert_eq!(s.rule, "builtin-runtime-down", "{}", s.detail);
        assert_eq!(s.code.as_deref(), Some("E_BUILTIN_DOWN"));
        assert!(s.confident, "判据全是文件与 socket，不必问模型");
        assert_eq!(s.actions.len(), 1, "{:?}", s.actions);
        assert_eq!(s.actions[0].id, "start_builtin_runtime");
        // 真正要跑的那条命令必须带 profile hunter
        let line = s.actions[0].command_line();
        assert!(line.contains("--profile"), "{line}");
        assert!(line.contains(crate::runtime::builtin::PROFILE), "{line}");
        // 用的必须是我们自己那份 colima（Windows 上分隔符是反斜杠，先归一）
        let norm = |x: &str| x.replace('\\', "/");
        assert!(
            norm(&line).contains(&norm(&crate::paths::runtime_dir().to_string_lossy())),
            "{line}"
        );
        // 话要说清楚：这不是用户装的 Docker
        assert!(s.detail.contains("不是你电脑上的 Docker"), "{}", s.detail);
        assert!(s.detail.contains("不会去碰你的 ~/.colima"), "{}", s.detail);
    }

    /// 装了一半（0.1.7 那种机器）→ 先补齐缺的那几个，而不是直接去起虚拟机。
    #[test]
    fn 内置运行时装了一半时先补齐再起() {
        let _g = crate::paths::test_home("rules-partial");
        let mut r = base();
        r.daemon_running = false;
        r.apps = vec![app_ev(
            "builtin",
            "Hunter 内置运行时",
            Some(false),
            Some(false),
            "看 ~/.hunter/runtime 下的四件工具（装了一半（还缺 colima、limactl））",
        )];
        let s = diagnose(&r);
        assert_eq!(s.rule, "builtin-runtime-down");
        assert!(s.detail.contains("还缺"), "{}", s.detail);
        if cfg!(target_os = "macos") {
            assert_eq!(
                s.actions.first().map(|a| a.id.as_str()),
                Some("install_runtime")
            );
        } else {
            // Linux / Windows 上装不了内置运行时 —— **结论照给，做不到就说清楚**
            assert!(s.actions.is_empty(), "{:?}", s.actions);
            assert!(s.detail.contains("做不了这一步"), "{}", s.detail);
        }
    }

    /// daemon 已经连上了（用户自己的 Docker 在跑）→ 这条规则必须让路，
    /// 哪怕 `~/.hunter/runtime` 里躺着一套没起来的内置运行时。
    #[test]
    fn 用户的_docker_在跑时内置那条规则让路() {
        let _g = crate::paths::test_home("rules-user-first");
        let mut r = base(); // base() 里 daemon_running = true
        r.apps = vec![app_ev(
            "builtin",
            "Hunter 内置运行时",
            Some(true),
            Some(false),
            "测试",
        )];
        assert_ne!(diagnose(&r).rule, "builtin-runtime-down");
    }

    /// 内置运行时的虚拟机在跑 → 也让路（那就不是这个现场了）。
    #[test]
    fn 内置运行时在跑时这条规则不触发() {
        let _g = crate::paths::test_home("rules-builtin-running");
        let mut r = base();
        r.daemon_running = false;
        r.apps = vec![app_ev(
            "builtin",
            "Hunter 内置运行时",
            Some(true),
            Some(true),
            "测试",
        )];
        assert_ne!(diagnose(&r).rule, "builtin-runtime-down");
    }

    /// `builtin` 这一条**不许**落进「装了但没起来」那条规则里 ——
    /// 那条规则会给出 `start_runtime`，而 `start_runtime` 碰的是用户的 colima。
    #[test]
    fn builtin_不算用户装的运行时() {
        let _g = crate::paths::test_home("rules-builtin-not-user");
        let mut r = base();
        r.daemon_running = false;
        // 只有 builtin 一条，而且它「在跑」—— 上面那条内置规则不触发，
        // 于是必须掉到别的规则上去，**但绝不能是 daemon-down-app-installed**
        r.apps = vec![app_ev(
            "builtin",
            "Hunter 内置运行时",
            Some(true),
            Some(true),
            "测试",
        )];
        // 造一个「装了但没起」的形状：installed=true、running=false
        r.apps[0].running = Some(false);
        let s = diagnose(&r);
        assert_eq!(
            s.rule, "builtin-runtime-down",
            "内置运行时不是用户装的东西，不该走 daemon-down：{}",
            s.detail
        );
        assert!(
            !s.actions.iter().any(|a| a.id == "start_runtime"),
            "{:?}",
            s.actions
        );
    }

    fn app_ev(
        id: &str,
        label: &str,
        installed: Option<bool>,
        running: Option<bool>,
        evidence: &str,
    ) -> AppPresence {
        AppPresence {
            id: id.into(),
            label: label.into(),
            installed,
            running,
            evidence: evidence.into(),
        }
    }
}
