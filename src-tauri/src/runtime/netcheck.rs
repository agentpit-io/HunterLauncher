//! 「容器能不能上网」这件事，当成必检项（I10 的 P0-2）。
//!
//! ## 为什么要有这一层
//!
//! 0.1.9 在用户 Mac 上的失败是这样的：镜像拉下来了、`compose up` 成了，
//! 然后 opencode 一直不健康 —— 因为它的健康检查 `/` 要去外网取 UI，
//! 而虚拟机没有 DNS。13 分钟后报 `E_START_TIMEOUT`，规则层 `unknown`。
//!
//! 可**真正致命的不是健康检查**：容器解析不了 `hunter.agentpit.io`，
//! 就算六个服务全绿，用户也一句话都问不出来。
//! 所以「容器能不能连上模型网关」不能靠健康检查顺带证明，**它自己就是一项必检**。
//!
//! ## 怎么探（不额外下任何东西）
//!
//! 用一个**已经拉下来的** Hunter 镜像起一个一次性容器，`--rm`，
//! 把 entrypoint 换成 `curl`，请求一次模型网关：
//!
//! ```text
//! docker run --rm --entrypoint curl <api 镜像> -sS -o /dev/null
//!        -w %{http_code} --max-time 20 https://hunter.agentpit.io/api/saas/llm/v1/models
//! ```
//!
//! 三件事一次说清：镜像里有没有 curl（有，实测）、DNS 通不通、到网关的 TLS 通不通。
//! **不带 key**（红线 2）——没有 key 时网关返回 401，那照样证明这条路是通的：
//! 任何一个 HTTP 状态码都意味着 DNS + TCP + TLS 全部走通了。
//!
//! ## 失败长什么样（实测原话，规则层就靠这几句做确定性判断）
//!
//! | 现场 | curl 退出码 | 原话 |
//! |---|---|---|
//! | 容器里一个 nameserver 都没有 | 6 | `curl: (6) Could not resolve host: hunter.agentpit.io` |
//! | nameserver 写着但不应答 | 28 | `curl: (28) Resolving timed out after 15001 milliseconds` |
//!
//! 两条都在测试机上用真容器跑出来过（见 I10 报告第四节），不是抄来的。

use std::time::Duration;

use serde::Serialize;

/// 探测给多久（比 curl 自己的 `--max-time` 多留一点余量）。
const RUN_TIMEOUT: Duration = Duration::from_secs(60);
/// 交给 curl 的 `--max-time`。
const CURL_MAX_TIME: &str = "20";

/// 容器那一次探测是怎么失败的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fail {
    /// 没失败
    None,
    /// **解析不了域名** —— 这一档是可以自动修的（虚拟机 DNS）
    NoDns,
    /// 解析得动，但连不上（代理、防火墙、网关自己挂了）
    Unreachable,
    /// 这一次根本没探成（没有镜像、docker 跑不起来）
    NotRun,
}

impl Fail {
    pub fn cn(self) -> &'static str {
        match self {
            Fail::None => "通",
            Fail::NoDns => "容器解析不了域名（没有可用的 DNS）",
            Fail::Unreachable => "域名解析得动，但连不上网关",
            Fail::NotRun => "这一次没能做这项探测",
        }
    }
}

/// 一次容器联网探测的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub ok: bool,
    pub fail: Fail,
    /// 真实跑过的那条命令（已脱敏，给人看）
    pub command: String,
    /// 原话（HTTP 状态码 / curl 的报错）
    pub detail: String,
    pub elapsed_ms: u64,
}

impl Outcome {
    pub fn one_line(&self) -> String {
        if self.ok {
            format!(
                "容器能连上模型网关（HTTP {} · {} 毫秒）",
                self.detail, self.elapsed_ms
            )
        } else {
            format!("容器连不上模型网关：{}（{}）", self.fail.cn(), self.detail)
        }
    }
    fn not_run(why: impl Into<String>) -> Self {
        Outcome {
            ok: false,
            fail: Fail::NotRun,
            command: String::new(),
            detail: why.into(),
            elapsed_ms: 0,
        }
    }
}

/// 这一段原话说的是「本机没有这个镜像」吗（实测原话，`--pull=never` 下的那一句）。
///
/// 这一档必须和「连不上」分开：**没探成不是不通**（红线 1）。
fn image_missing(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    ["no such image", "unable to find image", "image not found"]
        .iter()
        .any(|k| t.contains(k))
}

/// 把一段原话归类。**只认实测见过的那几句**，认不出来就是「连不上」而不是瞎猜。
pub fn classify(text: &str) -> Fail {
    if image_missing(text) {
        return Fail::NotRun;
    }
    let t = text.to_ascii_lowercase();
    const DNS: &[&str] = &[
        "could not resolve host",
        "could not resolve",
        "temporary failure in name resolution",
        "name or service not known",
        "resolving timed out",
        "no such host",
        "name resolution",
        "nodename nor servname provided",
        "server misbehaving",
    ];
    if DNS.iter().any(|k| t.contains(k)) {
        return Fail::NoDns;
    }
    Fail::Unreachable
}

/// 从合并后的 compose 配置里挑一个**本机已经有**的镜像来做这次探测。
///
/// 优先 `api`（实测镜像里有 `curl` 与 `getent`），其次 `llm-shim`。
/// **不去 pull 一个 busybox**：为了做一次探测再下一个镜像，
/// 在国内网络上可能比探测本身还贵。
fn image_from_compose() -> Option<String> {
    let json = crate::compose::config_check_json().ok()?;
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    let services = v.get("services")?.as_object()?;
    for name in ["api", "llm-shim"] {
        if let Some(img) = services
            .get(name)
            .and_then(|s| s.get("image"))
            .and_then(|i| i.as_str())
        {
            if !img.trim().is_empty() {
                return Some(img.to_string());
            }
        }
    }
    None
}

/// 这一次探测用哪个镜像。
///
/// 先问合并后的 compose 配置（那是**现在真的会起的**那一个）；
/// compose 文件还没落地时按 `launcher.toml` 里的源与 tag 算出 api 镜像的全名 ——
/// 算出来的这一个不一定在本机，探不到就会如实说「这一次没探成」，
/// **不会**被说成「不通」。
pub fn probe_image() -> Option<String> {
    if let Some(i) = image_from_compose() {
        return Some(i);
    }
    let cfg = crate::config::LauncherConfig::load();
    crate::config::images(
        &cfg.hunter.registry_prefix,
        &cfg.hunter.base_prefix,
        &cfg.hunter.tag,
    )
    .into_iter()
    .find(|i| i.service == "api")
    .map(|i| i.reference)
}

/// 探一次。`image` 是要用的镜像（[`probe_image`] 挑出来的那一个）。
pub fn probe_with_image(image: &str) -> Outcome {
    let url = format!("{}/v1/models", crate::gateway::GATEWAY_BASE);
    let docker = crate::runtime::which::docker_bin();
    let args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        // **绝不让这一步去拉镜像**（I10 实测补的）。
        //
        // 不带这个参数时，本机没有这个镜像的话 `docker run` 会先去 pull ——
        // 于是「本机还没有这个镜像」会被报成「容器连不上网关」，
        // 而且在没有 DNS 的机器上连 pull 都失败，原话看起来更像是 DNS 的事。
        // 两件完全不同的事混成一条结论，正是这一轮要消灭的那一类错误。
        "--pull=never".into(),
        "--entrypoint".into(),
        "curl".into(),
        image.to_string(),
        "-sS".into(),
        "-o".into(),
        "/dev/null".into(),
        "-w".into(),
        "%{http_code}".into(),
        "--max-time".into(),
        CURL_MAX_TIME.into(),
        url.clone(),
    ];
    let mut argv = vec![docker.clone()];
    argv.extend(args.clone());
    // 这条命令也要过守卫（`docker run --rm` 起的是一次性容器，不动任何已有的东西）
    if let Err(e) = crate::assist::guard::argv(&argv) {
        return Outcome::not_run(e.msg);
    }
    let shown = crate::redact::mask_home(&argv.join(" "));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let t = std::time::Instant::now();
    let r = crate::proc::run_timeout(&docker, &refs, RUN_TIMEOUT);
    let elapsed_ms = t.elapsed().as_millis() as u64;
    match r {
        Ok(x) if x.ok() => {
            let code = x.stdout.trim().to_string();
            // curl 拿不到任何响应时 `%{http_code}` 是 000，那不算通
            let ok = code.chars().all(|c| c.is_ascii_digit()) && !code.is_empty() && code != "000";
            if ok {
                Outcome {
                    ok: true,
                    fail: Fail::None,
                    command: shown,
                    detail: code,
                    elapsed_ms,
                }
            } else {
                Outcome {
                    ok: false,
                    fail: classify(&format!("{} {}", x.stdout, x.stderr)),
                    command: shown,
                    detail: first_line(&format!("{} {}", x.stderr.trim(), x.stdout.trim())),
                    elapsed_ms,
                }
            }
        }
        Ok(x) => {
            let text = format!("{} {}", x.stderr.trim(), x.stdout.trim());
            Outcome {
                ok: false,
                fail: classify(&text),
                command: shown,
                detail: first_line(&text),
                elapsed_ms,
            }
        }
        Err(e) => Outcome {
            ok: false,
            fail: Fail::NotRun,
            command: shown,
            detail: e.msg,
            elapsed_ms,
        },
    }
}

/// 探一次，镜像自己挑。挑不到就是「这一次没探成」，**不是「不通」**。
pub fn probe() -> Outcome {
    match probe_image() {
        Some(img) => probe_with_image(&img),
        None => Outcome::not_run(
            "本机还没有可以用来做这次探测的 Hunter 镜像（compose 配置里读不到 api / llm-shim 的镜像名）"
                .to_string(),
        ),
    }
}

// ── 已经起着的那些容器手里的 DNS（I10 的 P0-3）────────────────────────────

/// 正跑着的某个服务，它容器里那份 `/etc/resolv.conf` 写着什么。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerDns {
    pub service: String,
    pub nameservers: Vec<String>,
}

/// 读一遍本项目正跑着的容器里那份 `/etc/resolv.conf`。
///
/// ## 为什么要读它
///
/// 容器里那份文件是**容器创建那一刻**由 dockerd 按它所在那台机器的
/// `/etc/resolv.conf` 生成的，之后就不会自己变了。所以会出现这么一种现场：
/// 虚拟机的 DNS 已经修好、新起一个容器一切正常，**而修之前就起着的那几个容器
/// 手里还攥着修之前那份空的**（用户 Mac 上 0.1.9 结束时正是这个状态）。
///
/// 只读，而且只读本项目的服务名（compose 命令本身带着 `--project-name hunter`）。
pub fn running_container_dns() -> Vec<ContainerDns> {
    let running: Vec<String> = crate::compose::ps()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.state == "running")
        .map(|s| s.service)
        .filter(|s| crate::assist::guard::OWN_SERVICES.contains(&s.as_str()))
        .collect();
    let mut out = Vec::new();
    for svc in running {
        // `exec -T` 不要 TTY；`cat` 在任何镜像里都有（这个文件本身也一定在）
        let r = crate::compose::run(
            &["exec", "-T", &svc, "cat", "/etc/resolv.conf"],
            Duration::from_secs(20),
        );
        if let Ok(x) = r {
            if x.ok() {
                out.push(ContainerDns {
                    service: svc,
                    nameservers: crate::runtime::vmdns::nameservers_in(&x.stdout),
                });
            }
        }
    }
    out
}

/// 哪几个**正跑着的**容器手里那份 DNS 已经过时了。
///
/// 判据是确定性的：它一条 nameserver 都没有，或者它那几条和现在该有的
/// （`want`）**一条都对不上** —— 那只可能是在 DNS 修好之前创建的。
/// 重建一次就好（`compose down` + `up -d`，数据卷一个都不动）。
pub fn stale_dns_containers(want: &[String]) -> Vec<String> {
    running_container_dns()
        .into_iter()
        .filter(|c| c.nameservers.is_empty() || !c.nameservers.iter().any(|n| want.contains(n)))
        .map(|c| c.service)
        .collect()
}

/// 现在「该有的」那几条 nameserver —— 就是我们写进虚拟机的那几条。
pub fn want_nameservers() -> Vec<String> {
    crate::runtime::vmdns::NAMESERVERS
        .iter()
        .map(|(ns, _)| (*ns).to_string())
        .collect()
}

fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("（没有输出）")
        .chars()
        .take(200)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 这几句都是**测试机上用真容器跑出来的原话**（I10 第四节）。
    #[test]
    fn 实测见过的那几句都归成没有_dns() {
        for s in [
            "curl: (6) Could not resolve host: hunter.agentpit.io",
            "curl: (28) Resolving timed out after 15001 milliseconds",
            "dial tcp: lookup hunter.agentpit.io: no such host",
            "getaddrinfo: Temporary failure in name resolution",
            "ping: bad address 'hunter.agentpit.io': Name or service not known",
        ] {
            assert_eq!(classify(s), Fail::NoDns, "{s}");
        }
    }

    #[test]
    fn 连得上但被拒不算_dns_问题() {
        for s in [
            "curl: (7) Failed to connect to hunter.agentpit.io port 443: Connection refused",
            "curl: (35) OpenSSL SSL_connect: Connection reset by peer",
            "curl: (56) Received HTTP code 403 from proxy after CONNECT",
        ] {
            assert_eq!(classify(s), Fail::Unreachable, "{s}");
        }
    }

    /// **「本机没有这个镜像」不是「连不上」。**
    ///
    /// 这一条是实测补的：第一次跑 `--check-net` 时本机没有那个镜像，
    /// `docker run` 先去 pull、又因为没有 DNS 而失败，
    /// 于是一条「镜像不在本机」被报成了「容器解析不了域名」——
    /// 正是这一轮要消灭的那一类「把两件事混成一条结论」。
    #[test]
    fn 本机没有镜像时算没探成而不是不通() {
        for s in [
            "docker: Error response from daemon: No such image: ghcr.io/x/y:1.2.0",
            "Unable to find image 'ghcr.io/x/y:1.2.0' locally",
        ] {
            assert_eq!(classify(s), Fail::NotRun, "{s}");
        }
    }

    #[test]
    fn 每一档都有中文说法() {
        for f in [Fail::None, Fail::NoDns, Fail::Unreachable, Fail::NotRun] {
            assert!(!f.cn().is_empty());
        }
    }

    #[test]
    fn 手里没有_nameserver_或者对不上的都算过时() {
        let want = want_nameservers();
        assert!(want.contains(&"192.168.5.2".to_string()));
        // 这几条是纯函数判定，单独拎出来测（真去读容器要有 docker，CI 上没有）
        let stale =
            |ns: Vec<&str>| ns.is_empty() || !ns.iter().any(|n| want.contains(&n.to_string()));
        assert!(stale(vec![]), "一条都没有 = 过时");
        assert!(stale(vec!["127.0.0.11"]), "只剩 docker 内嵌 DNS 也算对不上");
        assert!(!stale(vec!["192.168.5.2", "223.6.6.6"]), "对得上就不算过时");
        assert!(!stale(vec!["223.6.6.6"]), "对上一条就够");
    }

    #[test]
    fn 没探成不能说成不通() {
        let o = Outcome::not_run("本机还没有镜像");
        assert!(!o.ok);
        assert_eq!(o.fail, Fail::NotRun);
        // 界面上这一句必须让人看出「没测」而不是「测了，不通」
        assert!(o.one_line().contains("没能做这项探测"), "{}", o.one_line());
    }
}
