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
//! ## 四种结论，对应四种做法
//!
//! | 结论 | 现场 | 该做什么 |
//! |---|---|---|
//! | [`Posture::Absent`] | 本项目一个容器都没有 | 完整安装 |
//! | [`Posture::Incomplete`] | 六个服务只有一部分建出来过 | 完整安装（镜像本机已有的话拉取那步是空跑） |
//! | [`Posture::Partial`] | 六个容器都在，但有的没就绪 / web 打不开 | **只修不正常的那部分** |
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
    /// 六个都在，但有的不正常
    Partial,
    /// 全好
    Healthy,
}

impl Posture {
    pub fn cn(self) -> &'static str {
        match self {
            Posture::Absent => "这台机器上还没有 Hunter",
            Posture::Incomplete => "上一次只装了一半",
            Posture::Partial => "Hunter 在跑，但有服务不正常",
            Posture::Healthy => "Hunter 已经在正常运行",
        }
    }
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

/// 磁盘上有没有这一套的配置。两份文件缺一个就不算「装过」。
pub fn installed_on_disk() -> bool {
    crate::paths::compose_file().is_file() && crate::paths::env_file().is_file()
}

/// 只读地复查一遍。
///
/// `deep` 为真时多做一件事：起一个 `--rm` 的一次性容器问「解析得动模型网关吗」。
/// 那一下要几秒到几十秒，所以错误页上的低频复查用浅查，
/// 安装前的预检用深查（**容器连不上网关的话，六个绿灯也没有意义** —— I10 的结论）。
pub fn review(deep: bool) -> Review {
    let t0 = Instant::now();
    let mut lines: Vec<String> = Vec::new();

    // ① 这个项目名现在是谁的。不是我们的就别往下看了 —— 那是 E_PROJECT_CONFLICT 的事
    if let Err(e) = compose::guard_project_owner() {
        lines.push(format!("compose 项目名 {PROJECT} 归属检查：{}", e.msg));
        return finish(
            Posture::Absent,
            Vec::new(),
            None,
            None,
            lines,
            t0,
            deep,
            None,
        );
    }

    if !installed_on_disk() {
        lines.push(format!(
            "{} 或 {} 不在，这台机器上没有启动器装过的那一套",
            crate::redact::mask_home(&crate::paths::compose_file().to_string_lossy()),
            crate::redact::mask_home(&crate::paths::env_file().to_string_lossy()),
        ));
        return finish(
            Posture::Absent,
            Vec::new(),
            None,
            None,
            lines,
            t0,
            deep,
            None,
        );
    }

    // ② 容器现状
    let services = match compose::ps() {
        Ok(v) => v,
        Err(e) => {
            lines.push(format!("docker compose ps 问不出来：{}", e.msg));
            return finish(
                Posture::Absent,
                Vec::new(),
                None,
                None,
                lines,
                t0,
                deep,
                None,
            );
        }
    };
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

    // **「已创建但一次都没跑起来」的容器不算一套装好的 Hunter**（I5 §六.5）。
    //
    // 那是上一次起到一半被打断留下的残骸（用户 Mac 上 0.1.4 的现场就有 3 个），
    // 它们身上冻着的是**当时那一组端口**。把它们当成「只差起一下」去 `up`，
    // 起来的仍然是旧端口，照样撞车。这一档要走完整流程：
    // 清残骸 → 重新算端口 → 重新起（这条路 I5 起就有，别绕过它）。
    let stale: Vec<String> = ours
        .iter()
        .filter(|s| s.state == "created")
        .map(|s| s.service.clone())
        .collect();
    if !stale.is_empty() {
        lines.push(format!(
            "这几个容器建出来过但一次都没跑起来（上一次装到一半留下的）：{}",
            stale.join("、")
        ));
        return finish(Posture::Incomplete, ours, None, None, lines, t0, deep, None);
    }

    let unready: Vec<String> = ours
        .iter()
        .filter(|s| !compose::service_ready(s))
        .map(|s| s.service.clone())
        .collect();

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
        let o = crate::runtime::netcheck::probe();
        lines.push(o.one_line());
        if !o.ok && o.fail != crate::runtime::netcheck::Fail::NotRun {
            posture = Posture::Partial;
        }
        container_net = Some(o);
    }

    let web_url = web_port
        .filter(|_| !matches!(posture, Posture::Absent))
        .map(|p| format!("http://localhost:{p}"));

    let headline = match posture {
        Posture::Absent => "这台机器上还没有装过 Hunter".to_string(),
        Posture::Incomplete if missing.is_empty() => {
            "上一次装到一半就断了：容器建出来过，但一次都没跑起来".to_string()
        }
        Posture::Incomplete => format!("上一次只装了一半：{} 还没有容器", missing.join("、")),
        Posture::Partial => {
            let mut what: Vec<String> = Vec::new();
            if !unready.is_empty() {
                what.push(format!("{} 还没就绪", unready.join("、")));
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
            format!("Hunter 在跑，但 {}", what.join("、"))
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
}
