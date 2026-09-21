//! 端口占用探测（I5 · P0）。
//!
//! ## 为什么要单独开一个模块，而不是留着那一行 `TcpListener::bind`
//!
//! 0.1.4 在用户的 Mac 上把**四个已经被占死的端口判成了空闲**，`docker compose up`
//! 随后报 `Bind for 0.0.0.0:8100 failed: port is already allocated`。根因不是逻辑写反了，
//! 是 `std::net::TcpListener::bind` 在 Unix 上**默认给监听套接字开 `SO_REUSEADDR`**，
//! 而 macOS 的 BSD 语义下这个选项会让「绑 `127.0.0.1:P`」在「`*:P` 已被别人占着」时**照样成功**：
//!
//! ```text
//! 用户 Mac 实测（OrbStack 以 *:8100 监听着另一套 Hunter）
//!   带 SO_REUSEADDR 绑 127.0.0.1:8100 → 成功  ← 0.1.4 走的就是这条，于是误判空闲
//!   带 SO_REUSEADDR 绑 0.0.0.0:8100   → 失败  ← web 走的是这条，所以只有 web 查对了
//!   不带 SO_REUSEADDR 绑 127.0.0.1:8100 → 失败
//! ```
//!
//! Linux 内核对 `SO_REUSEADDR` 的处理不一样（只影响 TIME_WAIT，不允许活跃监听共存），
//! 所以同一份代码在测试机上四个场景全过、一上真机就炸。**这类「只有某个平台才错」的判断，
//! 不能只靠一次绑定**。
//!
//! ## 三重确认
//!
//! | # | 手段 | 抓得住什么 | 抓不住什么 |
//! |---|---|---|---|
//! | 1 | [`bind_probe`]：`socket2` 显式构造，**不设 `SO_REUSEADDR`**，`0.0.0.0` 与 `[::]` 各绑一次 | 本机任何进程的 TCP 监听 | 占用者是谁 |
//! | 2 | [`docker_published`]：`docker ps` 的已发布端口 + 所属 compose 项目 | Docker/OrbStack 转发的端口，**且知道是谁** | Docker 之外的进程 |
//! | 3 | [`listeners`]：`lsof -nP -iTCP -sTCP:LISTEN`（Windows 用 `netstat -ano`） | 占用者的进程名 | 没装 lsof 的精简系统 |
//!
//! **任何一路说「占用」就算占用**。三路互为补充：漏判的代价是装完起不来（0.1.4 的现场），
//! 误判的代价只是多换一个端口 —— 两者不对称，所以这里一律往保守里判。

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use serde::Serialize;

use crate::runtime::which;

/// 跑 `docker ps` / `lsof` 的超时。daemon 半死时 `docker ps` 会一直挂着。
const CMD_TIMEOUT: Duration = Duration::from_secs(12);

/// 占用一个端口的东西是什么。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Occupant {
    /// 绑定探测失败，但不知道是谁（第 1 路只能给到这个程度）
    Bind {
        /// `0.0.0.0` 或 `[::]`
        addr: String,
        /// 系统给的原因（`Address already in use` 之类）
        reason: String,
    },
    /// Docker 已发布的端口。**知道是哪个容器、哪个 compose 项目**
    Docker {
        container: String,
        /// `com.docker.compose.project` 标签；不是 compose 起的就是空
        project: String,
        /// `docker ps` 原样给出的那一段映射，例如 `0.0.0.0:8100->8000/tcp`
        mapping: String,
    },
    /// 系统监听表里的进程
    Process { name: String, pid: Option<u32> },
}

impl Occupant {
    /// 给用户看的一句话。**不编内容**：读到什么写什么。
    pub fn human(&self) -> String {
        match self {
            Occupant::Bind { addr, reason } => format!("绑 {addr} 时被系统拒绝（{reason}）"),
            Occupant::Docker {
                container,
                project,
                mapping,
            } => {
                if project.is_empty() {
                    format!("Docker 容器 {container}（{mapping}）")
                } else {
                    format!("Docker 容器 {container}（compose 项目 {project} · {mapping}）")
                }
            }
            Occupant::Process { name, pid } => match pid {
                Some(p) => format!("进程 {name}（pid {p}）"),
                None => format!("进程 {name}"),
            },
        }
    }

    /// 这个占用者属于哪个 compose 项目（只有 Docker 那一路知道）。
    pub fn project(&self) -> Option<&str> {
        match self {
            Occupant::Docker { project, .. } if !project.is_empty() => Some(project),
            _ => None,
        }
    }
}

/// 一个端口的裁定结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Verdict {
    pub port: u16,
    /// **三路都说空闲**才是 true
    pub free: bool,
    /// 每一路查到的占用者。`free` 为 true 时是空的
    pub occupants: Vec<Occupant>,
}

impl Verdict {
    pub fn free(port: u16) -> Self {
        Self {
            port,
            free: true,
            occupants: Vec::new(),
        }
    }

    /// 占用者里有没有属于某个 compose 项目的。
    pub fn owned_by(&self, project: &str) -> bool {
        self.occupants.iter().any(|o| o.project() == Some(project))
    }

    /// 一行人话：`8100 被占用 · Docker 容器 hunter-fresh-api-1（compose 项目 hunter-fresh · …）`
    pub fn human(&self) -> String {
        if self.free {
            return format!("{} 空闲", self.port);
        }
        let who: Vec<String> = self.occupants.iter().map(Occupant::human).collect();
        format!("{} 被占用 · {}", self.port, who.join("；"))
    }
}

// ── 第 1 路：绑定探测 ─────────────────────────────────────────────────────

/// 绑一次，成功返回 `None`，失败返回系统给的原因。
///
/// **关键：不调 `set_reuse_address(true)`**。`socket2` 默认不设这个选项
/// （`std::net::TcpListener` 会设，这正是 0.1.4 的 bug 来源），所以这里显式构造。
/// 也不 `listen()` —— `bind` 成功就够判断了，少一次状态变更。
fn bind_once(addr: SocketAddr) -> Option<String> {
    use socket2::{Domain, Protocol, Socket, Type};
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let sock = match Socket::new(domain, Type::STREAM, Some(Protocol::TCP)) {
        Ok(s) => s,
        // 建不出套接字（IPv6 被系统关掉之类）不能算「端口被占」，只能算「这一路查不了」
        Err(_) => return None,
    };
    // IPv6 探测只探 v6，不要因为双栈套接字顺带把 v4 也占上导致判断串味
    if addr.is_ipv6() {
        let _ = sock.set_only_v6(true);
    }
    match sock.bind(&addr.into()) {
        Ok(()) => None,
        Err(e) => Some(e.to_string()),
    }
}

/// 第 1 路：`0.0.0.0` 与 `[::]` 各绑一次，任一失败就算占用。
///
/// 为什么探通配地址而不是 `127.0.0.1`：通配绑定和**任何**具体地址上的监听都冲突，
/// 是三种绑法里最保守的一种 —— 别人占着 `127.0.0.1:P` 或 `192.168.x.x:P`，
/// 我们绑 `0.0.0.0:P` 都会失败。红线 4 要求除 web 外的容器端口绑 `127.0.0.1`，
/// 那是**发布**时的事；**探测**一律按通配来，宁可多换一个端口，不可漏判。
pub fn bind_probe(port: u16) -> Vec<Occupant> {
    let mut v = Vec::new();
    if let Some(r) = bind_once(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))) {
        v.push(Occupant::Bind {
            addr: "0.0.0.0".into(),
            reason: r,
        });
    }
    if let Some(r) = bind_once(SocketAddr::from((Ipv6Addr::UNSPECIFIED, port))) {
        v.push(Occupant::Bind {
            addr: "[::]".into(),
            reason: r,
        });
    }
    v
}

// ── 第 2 路：docker ps ────────────────────────────────────────────────────

/// `docker ps` 里的一个容器及它发布的端口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub container: String,
    pub project: String,
    pub image: String,
    /// 宿主端口 → 原样的那一段映射文本
    pub ports: BTreeMap<u16, String>,
}

/// 解析 `docker ps --format '{{.Names}}\t{{.Image}}\t{{.Label "com.docker.compose.project"}}\t{{.Ports}}'`。
///
/// `.Ports` 的真实形状（测试机实测）：
/// ```text
/// 0.0.0.0:3100->3000/tcp, [::]:3100->3000/tcp
/// 127.0.0.1:8101->8000/tcp
/// 3999/tcp                      ← 没发布，不算占用宿主端口
/// ```
pub fn parse_ps(stdout: &str) -> Vec<Published> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 4 {
            continue;
        }
        let mut ports = BTreeMap::new();
        for seg in f[3].split(',') {
            let seg = seg.trim();
            // 没有 `->` 的是容器内端口（没发布到宿主），不占宿主端口
            let Some((host, _)) = seg.split_once("->") else {
                continue;
            };
            // host 形如 `0.0.0.0:3100` / `127.0.0.1:8101` / `[::]:3100`
            let Some((_, p)) = host.rsplit_once(':') else {
                continue;
            };
            if let Ok(n) = p.trim().parse::<u16>() {
                // 同一个宿主端口常常出现两次（`0.0.0.0:3100->…` 与 `[::]:3100->…`）。
                // 留第一条 —— 它是 IPv4 那一条，用户更认得
                ports.entry(n).or_insert_with(|| seg.to_string());
            }
        }
        out.push(Published {
            container: f[0].to_string(),
            image: f[1].to_string(),
            project: f[2].to_string(),
            ports,
        });
    }
    out
}

const PS_FORMAT: &str =
    "{{.Names}}\t{{.Image}}\t{{.Label \"com.docker.compose.project\"}}\t{{.Ports}}";

/// 第 2 路：问 Docker 要所有**正在运行**的容器的已发布端口。
///
/// Docker 是端口这件事上的最终裁判 —— OrbStack / Docker Desktop 的端口转发是
/// 由虚拟机里的进程做的，宿主上的绑定探测未必看得见（这正是用户 Mac 上的情形）。
/// daemon 没起来时返回空表，**不是把它当成「都空闲」**：调用方拿三路结果合并，
/// 这一路为空只代表这一路没话说。
pub fn docker_published() -> Vec<Published> {
    let bin = which::docker_bin();
    match crate::proc::run_timeout(&bin, &["ps", "--format", PS_FORMAT], CMD_TIMEOUT) {
        Ok(r) if r.ok() => parse_ps(&r.stdout),
        Ok(r) => {
            crate::lwarn!("docker ps 查已发布端口失败（这一路跳过）：{}", r.err_line());
            Vec::new()
        }
        Err(e) => {
            crate::lwarn!("docker ps 起不来（这一路跳过）：{}", e.msg);
            Vec::new()
        }
    }
}

/// 本机上**另一套** Hunter（compose 项目名不是我们的 `hunter`，镜像名里带
/// `hunter-community-`）。
///
/// 用户 Mac 上的 `hunter-fresh` 就是这么一套：40 小时前手工部署的，占着 5 个端口。
/// 0.1.4 完全不知道它的存在，于是 AI 把端口冲突解释成了「残留容器」。
/// I5 的默认处置是**并存换端口**，并在界面上如实告诉用户它在那儿。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OtherInstall {
    /// compose 项目名。手工 `docker run` 起的没有项目名，这里给容器名
    pub project: String,
    pub containers: Vec<String>,
    /// 它占着的宿主端口
    pub ports: Vec<u16>,
}

pub fn other_hunter_installs(published: &[Published]) -> Vec<OtherInstall> {
    // 第一趟：哪些 compose 项目是 Hunter。判据是「至少有一个容器用 hunter-community-* 镜像」
    let mut projects: Vec<String> = Vec::new();
    let mut loners: Vec<&Published> = Vec::new();
    for p in published {
        if !p.image.contains("hunter-community-") {
            continue;
        }
        if p.project == crate::config::PROJECT {
            continue;
        }
        if p.project.is_empty() {
            loners.push(p);
        } else if !projects.contains(&p.project) {
            projects.push(p.project.clone());
        }
    }

    // 第二趟：把这些项目的**全部**容器算进来。
    // postgres / redis 用的是官方镜像（`postgres:16`、`redis:7`），
    // 名字里没有 hunter-community-，但它们正是占着 5442 / 6479 的那两个 ——
    // 只按镜像名筛会漏报一半端口，用户看到的就成了「占着 3 个端口」（首轮实测撞到）
    let mut by: BTreeMap<String, OtherInstall> = BTreeMap::new();
    for p in published {
        let key = if !p.project.is_empty() && projects.contains(&p.project) {
            p.project.clone()
        } else if loners.iter().any(|l| l.container == p.container) {
            format!("（不是 compose 起的）{}", p.container)
        } else {
            continue;
        };
        let e = by.entry(key.clone()).or_insert_with(|| OtherInstall {
            project: key,
            containers: Vec::new(),
            ports: Vec::new(),
        });
        e.containers.push(p.container.clone());
        for k in p.ports.keys() {
            if !e.ports.contains(k) {
                e.ports.push(*k);
            }
        }
    }
    let mut v: Vec<OtherInstall> = by.into_values().collect();
    for i in &mut v {
        i.ports.sort_unstable();
        i.containers.sort();
    }
    v
}

// ── 第 3 路：系统监听表 ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    pub port: u16,
    pub name: String,
    pub pid: Option<u32>,
}

/// 解析 `lsof -nP -iTCP -sTCP:LISTEN`（mac / Linux）。真实一行：
/// ```text
/// docker-pr 1234 root    4u  IPv4 0x…      0t0  TCP *:3100 (LISTEN)
/// ```
pub fn parse_lsof(stdout: &str) -> Vec<Listener> {
    let mut v = Vec::new();
    for line in stdout.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 9 {
            continue;
        }
        // 倒数第二列是地址（最后一列是 `(LISTEN)`）
        let addr = f[f.len() - 2];
        let Some((_, p)) = addr.rsplit_once(':') else {
            continue;
        };
        let Ok(port) = p.parse::<u16>() else { continue };
        v.push(Listener {
            port,
            name: f[0].to_string(),
            pid: f[1].parse::<u32>().ok(),
        });
    }
    v
}

/// 解析 `netstat -ano`（Windows）。真实一行：
/// ```text
///   TCP    0.0.0.0:3100           0.0.0.0:0              LISTENING       4321
/// ```
/// netstat 只给 pid 不给进程名，所以 `name` 写成 `pid <n>` —— **不去猜名字**。
pub fn parse_netstat(stdout: &str) -> Vec<Listener> {
    let mut v = Vec::new();
    for line in stdout.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 5 || !f[0].eq_ignore_ascii_case("TCP") {
            continue;
        }
        if !f[3].eq_ignore_ascii_case("LISTENING") {
            continue;
        }
        let Some((_, p)) = f[1].rsplit_once(':') else {
            continue;
        };
        let Ok(port) = p.parse::<u16>() else { continue };
        let pid = f[4].parse::<u32>().ok();
        v.push(Listener {
            port,
            name: match pid {
                Some(n) => format!("pid {n}"),
                None => "（netstat 没给 pid）".into(),
            },
            pid,
        });
    }
    v
}

/// 第 3 路：系统监听表。查不到（没装 lsof）就返回空表 —— 这一路没话说，不影响另两路。
pub fn listeners() -> Vec<Listener> {
    #[cfg(windows)]
    {
        match crate::proc::run_timeout("netstat", &["-ano"], CMD_TIMEOUT) {
            Ok(r) if r.ok() => parse_netstat(&r.stdout),
            _ => Vec::new(),
        }
    }
    #[cfg(not(windows))]
    {
        // lsof 在 /usr/sbin（mac）或 /usr/bin（Linux）。GUI 程序的 PATH 里未必有
        // /usr/sbin（I4 那个 macOS P0 就是 PATH 的事），所以走统一的定位器
        let bin = which::resolve("lsof")
            .resolved
            .unwrap_or_else(|| "lsof".to_string());
        match crate::proc::run_timeout(&bin, &["-nP", "-iTCP", "-sTCP:LISTEN"], CMD_TIMEOUT) {
            // lsof 没有匹配项时退出码是 1，stdout 为空 —— 那也是一个有效答案
            Ok(r) => parse_lsof(&r.stdout),
            Err(_) => Vec::new(),
        }
    }
}

// ── 合并：一次 survey 查一批端口 ──────────────────────────────────────────

/// 一次调查的现场。`docker ps` 与 `lsof` 各跑一次，**不要每个端口各跑一次**
/// （5 个端口跑 10 次子进程，慢且没必要）。
pub struct Survey {
    pub published: Vec<Published>,
    pub listeners: Vec<Listener>,
}

impl Survey {
    /// 真去采一次。
    pub fn collect() -> Self {
        Self {
            published: docker_published(),
            listeners: listeners(),
        }
    }

    /// 不跑子进程的空现场。单元测试与「只想要绑定探测」时用。
    pub fn empty() -> Self {
        Self {
            published: Vec::new(),
            listeners: Vec::new(),
        }
    }

    /// 裁定一个端口。三路合并，任一说占用即占用。
    ///
    /// `ignore_projects` 里的 compose 项目**不算冲突** —— 我们自己那一套
    /// （项目 `hunter`）第二次打开启动器时本来就占着这些端口，
    /// 把它算成冲突的话端口每开一次就往上挪一格（M2 用例 9 的老坑）。
    pub fn verdict(&self, port: u16, ignore_projects: &[&str]) -> Verdict {
        let mut occ = Vec::new();

        // 第 2 路先跑：它知道占用者是谁，也知道该不该忽略
        let mut ignored_owner = false;
        for p in &self.published {
            let Some(mapping) = p.ports.get(&port) else {
                continue;
            };
            if ignore_projects.contains(&p.project.as_str()) {
                ignored_owner = true;
                continue;
            }
            occ.push(Occupant::Docker {
                container: p.container.clone(),
                project: p.project.clone(),
                mapping: mapping.clone(),
            });
        }

        // 端口正被**我们自己**的容器占着时，第 1、3 路当然也会说「占用」——
        // 那不是冲突，是我们自己。这时直接判空闲（对我们可用）。
        if ignored_owner && occ.is_empty() {
            return Verdict::free(port);
        }

        occ.extend(bind_probe(port));
        for l in &self.listeners {
            if l.port == port {
                occ.push(Occupant::Process {
                    name: l.name.clone(),
                    pid: l.pid,
                });
            }
        }

        Verdict {
            port,
            free: occ.is_empty(),
            occupants: occ,
        }
    }
}

/// 单个端口的裁定（自己采一次现场）。调用方只问一个端口时用。
pub fn verdict_now(port: u16, ignore_projects: &[&str]) -> Verdict {
    Survey::collect().verdict(port, ignore_projects)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析测试机上_docker_ps_的真实输出() {
        // 2026-09-21 在测试机（34.133.8.3）上原样抓下来的四行
        let s = "hunter-web-1\tghcr.io/agentpit-io/hunter-community-web:1.2.0\thunter\t0.0.0.0:3101->3000/tcp, [::]:3101->3000/tcp\n\
                 hunter-api-1\tghcr.io/agentpit-io/hunter-community-api:1.2.0\thunter\t127.0.0.1:8101->8000/tcp\n\
                 hunter-llm-shim-1\tghcr.io/agentpit-io/hunter-community-llm-shim:1.2.0\thunter\t3999/tcp\n\
                 hunter-community-api-1\tghcr.io/agentpit-io/hunter-community-api:1.2.0\thunter-community\t0.0.0.0:8100->8000/tcp, [::]:8100->8000/tcp\n";
        let v = parse_ps(s);
        assert_eq!(v.len(), 4);
        assert_eq!(v[0].ports.keys().collect::<Vec<_>>(), vec![&3101u16]);
        assert_eq!(v[1].ports.keys().collect::<Vec<_>>(), vec![&8101u16]);
        // 没有 `->` 的是容器内端口，不占宿主
        assert!(v[2].ports.is_empty(), "3999/tcp 没发布，不该算宿主端口");
        assert_eq!(v[3].project, "hunter-community");
        assert_eq!(v[3].ports[&8100], "0.0.0.0:8100->8000/tcp");
    }

    #[test]
    fn 认得出本机另一套_hunter() {
        let s = "hunter-web-1\tghcr.io/agentpit-io/hunter-community-web:1.2.0\thunter\t0.0.0.0:3101->3000/tcp\n\
                 hunter-fresh-web-1\tghcr.io/agentpit-io/hunter-community-web:1.2.0\thunter-fresh\t0.0.0.0:3100->3000/tcp\n\
                 hunter-fresh-api-1\tghcr.io/agentpit-io/hunter-community-api:1.2.0\thunter-fresh\t0.0.0.0:8100->8000/tcp\n\
                 nginx\tnginx:latest\t\t0.0.0.0:80->80/tcp\n";
        let v = other_hunter_installs(&parse_ps(s));
        assert_eq!(v.len(), 1, "只该报出 hunter-fresh 那一套：{v:?}");
        assert_eq!(v[0].project, "hunter-fresh");
        assert_eq!(v[0].ports, vec![3100, 8100]);
        assert_eq!(v[0].containers.len(), 2);
    }

    /// 首轮实测撞到的漏报：`hunter-community` 的 postgres 与 redis 用的是官方镜像，
    /// 只按镜像名筛的话它们不算「Hunter 的一部分」，于是界面上会说
    /// 「占着 3 个端口」而不是 5 个 —— 用户看到的数字就是错的。
    #[test]
    fn 同一个项目里用官方镜像的容器也要算进来() {
        let s = "hunter-fresh-web-1\tghcr.io/agentpit-io/hunter-community-web:1.2.0\thunter-fresh\t0.0.0.0:3100->3000/tcp\n\
                 hunter-fresh-api-1\tghcr.io/agentpit-io/hunter-community-api:1.2.0\thunter-fresh\t0.0.0.0:8100->8000/tcp\n\
                 hunter-fresh-postgres-1\tpostgres:16-alpine\thunter-fresh\t0.0.0.0:5442->5432/tcp\n\
                 hunter-fresh-redis-1\tredis:7-alpine\thunter-fresh\t0.0.0.0:6479->6379/tcp\n\
                 hunter-fresh-opencode-1\tghcr.io/agentpit-io/hunter-community-opencode:1.2.0\thunter-fresh\t0.0.0.0:3921->3901/tcp\n\
                 some-other-pg\tpostgres:16\tsomeoneelse\t0.0.0.0:5555->5432/tcp\n";
        let v = other_hunter_installs(&parse_ps(s));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(
            v[0].ports,
            vec![3100, 3921, 5442, 6479, 8100],
            "五个端口都要报出来"
        );
        assert_eq!(v[0].containers.len(), 5);
        // 别人家的 postgres 不能被算进来
        assert!(!v[0].containers.iter().any(|c| c == "some-other-pg"));
    }

    #[test]
    fn 自己那一套不算_其他安装() {
        let s = "hunter-web-1\tghcr.io/agentpit-io/hunter-community-web:1.2.0\thunter\t0.0.0.0:3101->3000/tcp\n";
        assert!(other_hunter_installs(&parse_ps(s)).is_empty());
    }

    #[test]
    fn 解析_lsof() {
        let s = "COMMAND     PID USER   FD   TYPE             DEVICE SIZE/OFF NODE NAME\n\
                 docker-pr  1234 root    4u  IPv4 0x1234567890abcdef      0t0  TCP *:3100 (LISTEN)\n\
                 OrbStack   2345 me      7u  IPv6 0xfedcba0987654321      0t0  TCP [::1]:8100 (LISTEN)\n";
        let v = parse_lsof(s);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].port, 3100);
        assert_eq!(v[0].name, "docker-pr");
        assert_eq!(v[0].pid, Some(1234));
        assert_eq!(v[1].port, 8100);
        assert_eq!(v[1].name, "OrbStack");
    }

    #[test]
    fn 解析_netstat() {
        let s = "\r\nActive Connections\r\n\r\n  Proto  Local Address          Foreign Address        State           PID\r\n\
                   TCP    0.0.0.0:3100           0.0.0.0:0              LISTENING       4321\r\n\
                   TCP    [::]:8100              [::]:0                 LISTENING       4321\r\n\
                   TCP    127.0.0.1:5442         127.0.0.1:60123        ESTABLISHED     999\r\n";
        let v = parse_netstat(s);
        assert_eq!(v.len(), 2, "只要 LISTENING 的：{v:?}");
        assert_eq!(v[0].port, 3100);
        assert_eq!(v[1].port, 8100);
        assert_eq!(v[0].pid, Some(4321));
    }

    /// **本轮 P0 的回归**：真起一个带 `SO_REUSEADDR` 的 `*:port` 监听
    /// （这正是 OrbStack / Docker 的端口转发进程在宿主上的样子），
    /// 断言我们的探测判它为「占用」。
    ///
    /// 0.1.4 的 `std::net::TcpListener::bind("127.0.0.1", port)` 在 macOS 上会**通过**
    /// 这个断言的反面 —— 也就是判成空闲。CI 的 macOS runner 上跑这一条，
    /// 以后再有「只有 mac 才炸」的探测缺陷就能当场抓住（设计文档 §9.3）。
    #[test]
    fn 带_so_reuseaddr_的通配监听必须被判为占用() {
        use socket2::{Domain, Protocol, Socket, Type};
        // 先要一个空闲端口（这一步用 std 没问题，它只是帮我们挑个号）
        let tmp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = tmp.local_addr().unwrap().port();
        drop(tmp);

        // 模拟 OrbStack：带 SO_REUSEADDR 绑 *:port 并真的 listen
        let s = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
        s.set_reuse_address(true).unwrap();
        s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())
            .unwrap();
        s.listen(16).unwrap();

        let occ = bind_probe(port);
        assert!(
            !occ.is_empty(),
            "端口 {port} 上有一个带 SO_REUSEADDR 的 *:{port} 监听，探测必须判为占用（\
             这条在 macOS 上失败就说明 0.1.4 的那个 P0 回来了）"
        );

        // 同一个现场，Survey 也必须判占用
        let v = Survey::empty().verdict(port, &[]);
        assert!(!v.free, "{}", v.human());
        drop(s);
    }

    #[test]
    fn 真空闲的端口判为空闲() {
        let tmp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = tmp.local_addr().unwrap().port();
        drop(tmp);
        let v = Survey::empty().verdict(port, &[]);
        assert!(v.free, "刚释放的端口应该是空闲的：{}", v.human());
        assert_eq!(v.human(), format!("{port} 空闲"));
    }

    #[test]
    fn 自己那一套占着的端口不算冲突() {
        let s = "hunter-web-1\timg/hunter-community-web:1.2.0\thunter\t0.0.0.0:3101->3000/tcp\n";
        let sv = Survey {
            published: parse_ps(s),
            listeners: Vec::new(),
        };
        // 忽略 hunter 项目时判空闲；不忽略时判占用并说出是谁
        assert!(sv.verdict(3101, &["hunter"]).free);
        let v = sv.verdict(3101, &[]);
        assert!(!v.free);
        assert!(v.owned_by("hunter"));
        assert!(v.human().contains("hunter-web-1"), "{}", v.human());
    }

    #[test]
    fn 占用者里认得出_compose_项目() {
        let s = "hunter-fresh-api-1\timg/hunter-community-api:1.2.0\thunter-fresh\t0.0.0.0:8100->8000/tcp\n";
        let sv = Survey {
            published: parse_ps(s),
            listeners: Vec::new(),
        };
        let v = sv.verdict(8100, &["hunter"]);
        assert!(!v.free);
        assert!(v.owned_by("hunter-fresh"));
        assert!(
            v.human().contains("compose 项目 hunter-fresh"),
            "{}",
            v.human()
        );
    }
}
