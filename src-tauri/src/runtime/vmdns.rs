//! 内置虚拟机的 DNS（I10 的 P0-1）。
//!
//! ## 现场
//!
//! 0.1.9 在用户 Mac 上（2026-09-22 21:14–22:05，证据在 `plan/mac-0.1.9-20260922/`）
//! 镜像拉下来了、`compose up` 也成了，然后 opencode 一直不健康，13 分钟后
//! `E_START_TIMEOUT`，规则层 `unknown`。本地 Claude 上机一查，根因是一句话：
//!
//! > **启动器自带的那份虚拟机镜像里，`/etc/resolv.conf` 是一条断链，
//! > 而镜像里没有 systemd-resolved 去把它填上 —— 虚拟机和所有容器都没有 DNS。**
//!
//! 更要紧的不是健康检查：容器解析不了 `hunter.agentpit.io`，
//! **就算健康检查过了也不能对话**。
//!
//! ## 为什么这份镜像没有 DNS（I10 实查，不是推测）
//!
//! 证据是把启动器真正会下发的那个文件
//! （`ubuntu-24.04-minimal-cloudimg-amd64-docker.raw.gz`，sha256 与清单一致）
//! 在测试机上解开、loop 挂载、逐条查出来的：
//!
//! | # | 事实 | 怎么查的 |
//! |---|---|---|
//! | 1 | `/etc/resolv.conf` → `../run/systemd/resolve/stub-resolv.conf`，**出厂就是断链** | `ls -la` 挂载点 |
//! | 2 | 镜像里**没有** systemd-resolved（Ubuntu 24.04 把它拆成了单独的包，minimal 镜像不装） | `dpkg -l` 里只有 `systemd` / `systemd-sysv` / `systemd-timesyncd` |
//! | 3 | 也没有 `resolvconf` / `openresolv` | 同上 |
//! | 4 | lima 写 DNS 的第一条路（cloud-init 的 `resolv_conf` 模块）**在 Ubuntu 上根本不在模块表里** | 镜像里 `/etc/cloud/cloud.cfg` 的 `cloud_config_modules` 没有 `resolv_conf` |
//! | 5 | lima 写 DNS 的第二条路（netplan 的 `nameservers:`）只把地址交给 systemd-networkd，**而 networkd 要靠 resolved 才会落到 `/etc/resolv.conf`** | lima v2.2.0 `pkg/cidata/cidata.TEMPLATE.d/network-config` |
//!
//! 结论很硬：**`colima start --dns …` / `colima.yaml` 的 `dns:` 在这份镜像上治不了本** ——
//! 它改的只是「往那两条死路里塞哪几个地址」（lima 源码 `pkg/cidata/cidata.go`
//! 的 `args.DNSAddresses`，两条路的出口都在上面第 4、5 条）。所以这里走的是
//! 任务书给的第二条：**启动器自己在虚拟机里把 `/etc/resolv.conf` 写成一个普通文件**。
//!
//! ## 写哪几个 nameserver（数值由实测决定）
//!
//! | 地址 | 为什么是它 |
//! |---|---|
//! | `192.168.5.2` | lima user-v2（gvisor-tap-vsock）的网关，**它自己就是 DNS 服务器**（lima 源码 `pkg/networks/usernet/gvproxy.go`：「GatewayIP handling all request, also answers DNS queries」）。走它等于走用户 Mac 自己的解析，公司 DNS / VPN 都跟着生效。本地 Claude 在用户机器上实测可用 |
//! | `223.6.6.6` | 阿里公共 DNS，**国内可直连**。本地 Claude 在用户机器上实测可用 |
//! | `119.29.29.29` | 腾讯 DNSPod 公共 DNS，同样国内可用，再兜一层 |
//!
//! `192.168.5.3`（传统 QEMU slirp 的 DNS 地址）**实测不通** —— vz + user-v2 这一路
//! 的 DNS 就在网关上，不在 .3。lima 自己也是这么算的（`SlirpDNS = GatewayIP`）。
//!
//! ## 怎么保证持久
//!
//! 三层，越往后越硬：
//!
//! 1. `/etc/resolv.conf` 写成**普通文件**（不是链接）。它在虚拟机的根盘上，
//!    重启不会掉 —— 而且这份镜像里没有任何东西会去重写它（上面第 2、3、4、5 条）；
//! 2. 另外装一个 **oneshot systemd 服务** `hunter-dns.service`（排在 `docker.service`
//!    前面），每次开机把 `/etc/hunter/resolv.conf` 拷回 `/etc/resolv.conf`。
//!    这样「持久」就不是推理出来的，是**每次开机都重做一遍**；
//! 3. 启动器每一次走到「检查 Docker」这一步都会重新[`probe`]一次，坏了就再修
//!    （[`ensure`]）。**不假设上一次修好了**。
//!
//! ## 边界（红线不变）
//!
//! 动的每一条命令都指名 `colima-hunter` 这一台虚拟机，`LIMA_HOME` 钉在
//! `~/.hunter/runtime/lima`。**用户 Mac 自己的 DNS、hosts、代理、防火墙一个字节都不碰** ——
//! 这一条不是靠「我们没写那一行」保证的：
//! [`crate::assist::guard::argv`] 那道总门依旧拒绝任何参数里出现 `/etc/resolv.conf`
//! 的命令（模型永远走那一道），这里走的是另开的一道窄门
//! [`crate::assist::guard::argv_vm_dns`]，门里只认代码写死的几种形状。

use std::time::Duration;

use crate::err::{AppError, AppResult, Code};

use super::builtin::{self, Progress};

/// 要写进虚拟机 `/etc/resolv.conf` 的 nameserver，以及每一条的理由。
///
/// 顺序有意义：先网关（跟着用户 Mac 的解析走），再两个国内公共 DNS 兜底。
pub const NAMESERVERS: &[(&str, &str)] = &[
    (
        "192.168.5.2",
        "lima user-v2 的网关，它自己就是 DNS 服务器，解析跟着你 Mac 走",
    ),
    ("223.6.6.6", "阿里公共 DNS，国内可直连"),
    ("119.29.29.29", "腾讯 DNSPod 公共 DNS，再兜一层"),
];

/// 虚拟机里放我们这份 resolv.conf 的目录与文件（`hunter-dns.service` 从这里拷回去）。
const VM_KEEP_DIR: &str = "/etc/hunter";
const VM_KEEP_FILE: &str = "/etc/hunter/resolv.conf";
const VM_RESOLV: &str = "/etc/resolv.conf";
const VM_UNIT: &str = "/etc/systemd/system/hunter-dns.service";
const UNIT_NAME: &str = "hunter-dns.service";
/// 往虚拟机里送文件的中转位置（`limactl copy` 的落点）。
const VM_TMP_RESOLV: &str = "/tmp/hunter-resolv.conf";
const VM_TMP_UNIT: &str = "/tmp/hunter-dns.service";

/// 在虚拟机里跑一条命令给多久。`getent` 在没有 DNS 时会一直等到解析器自己超时。
const SHELL_TIMEOUT: Duration = Duration::from_secs(40);
/// 重启虚拟机里的 dockerd 给多久。
const RESTART_TIMEOUT: Duration = Duration::from_secs(180);

/// colima 建出来的 lima 实例名（`colima-<profile>`）。
pub fn instance() -> String {
    format!("colima-{}", builtin::PROFILE)
}

/// 内置运行时那一份 `limactl` 的绝对路径。没装就说清楚 ——
/// I12 的资源监控也要走它，所以从 `fn limactl()` 上面再包一层对外的。
pub fn limactl_path() -> AppResult<String> {
    limactl().ok_or_else(|| {
        AppError::new(
            Code::NotImplemented,
            "内置运行时里没有 limactl，做不了这一步。".to_string(),
        )
    })
}

fn limactl() -> Option<String> {
    let p = crate::paths::runtime_dist()
        .join("lima")
        .join("bin")
        .join(if cfg!(windows) {
            "limactl.exe"
        } else {
            "limactl"
        });
    p.is_file().then(|| p.to_string_lossy().into_owned())
}

/// 我们在宿主机上生成那两个文件的地方（**只在 `~/.hunter/runtime` 里**）。
fn stage_dir() -> std::path::PathBuf {
    crate::paths::runtime_dir().join("vm")
}

// ── 要写进去的内容 ────────────────────────────────────────────────────────

/// `/etc/resolv.conf` 的内容。**注释里写清是谁写的、为什么** ——
/// 用户哪天自己进虚拟机看见这个文件，要能看懂。
pub fn resolv_conf_text() -> String {
    let mut s = String::new();
    s.push_str("# 由 Hunter 启动器写入（只作用于 Hunter 自己的这台虚拟机）。\n");
    s.push_str("# 这份 Ubuntu 24.04 minimal 镜像里没有 systemd-resolved，\n");
    s.push_str("# /etc/resolv.conf 出厂是一条指向它的断链 —— 虚拟机和容器因此没有任何 DNS。\n");
    s.push_str("# 你 Mac 自己的 DNS 设置没有被改动过。\n");
    for (ns, why) in NAMESERVERS {
        s.push_str(&format!("nameserver {ns}  # {why}\n"));
    }
    // 第一条不通时别把每次解析都拖到默认的 5 秒 × 2 次
    s.push_str("options timeout:2 attempts:2\n");
    s
}

/// 开机把上面那份拷回 `/etc/resolv.conf` 的 systemd 服务。
///
/// 故意写得极简：`Type=oneshot` + 一条 `cp`，**不跑 shell**，
/// 排在 `docker.service` 前面就够了。
pub fn unit_text() -> String {
    format!(
        "[Unit]\n\
         Description=Hunter: write {VM_RESOLV} (this image has no systemd-resolved)\n\
         Documentation=https://github.com/agentpit-io/HunterLauncher\n\
         Before=docker.service containerd.service\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         RemainAfterExit=yes\n\
         ExecStart=/bin/cp --remove-destination {VM_KEEP_FILE} {VM_RESOLV}\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

// ── 现在是什么状态 ────────────────────────────────────────────────────────

/// 虚拟机里 `/etc/resolv.conf` 长什么样。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "state", content = "detail")]
pub enum ResolvState {
    /// 读不到（断链、或者根本没有这个文件）—— **0.1.9 用户 Mac 上就是这一档**
    Broken(String),
    /// 读到了，但里面一条 `nameserver` 都没有
    Empty(String),
    /// 读到了，有 nameserver（原文）
    File(String),
    /// 没法查（虚拟机没跑 / 没装 limactl）
    Unknown(String),
}

impl ResolvState {
    pub fn cn(&self) -> String {
        match self {
            ResolvState::Broken(d) => format!("读不到（{d}）"),
            ResolvState::Empty(_) => "有这个文件，但一条 nameserver 都没有".to_string(),
            ResolvState::File(t) => format!(
                "是一个普通文件，nameserver：{}",
                nameservers_in(t).join("、")
            ),
            ResolvState::Unknown(d) => format!("没法查（{d}）"),
        }
    }
}

/// 一次探测的结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VmDns {
    pub instance: String,
    pub resolv: ResolvState,
    /// 在虚拟机里解析 `hunter.agentpit.io` 成没成（`None` = 没查成）
    pub resolves: Option<bool>,
    /// 解析那一步的原话（成功时是解析到的地址）
    pub resolve_detail: String,
    /// 我们那个开机服务装了吗
    pub unit_installed: Option<bool>,
}

impl VmDns {
    /// 现在这台虚拟机的 DNS 能用吗。**判据是「真的解析出来了」**，
    /// 不是「resolv.conf 看着像对的」—— 文件对不对不等于解析得动。
    pub fn healthy(&self) -> bool {
        self.resolves == Some(true)
    }
    /// 一行人话（事件流 / 日志 / 诊断报文都用它）。
    pub fn one_line(&self) -> String {
        let r = match self.resolves {
            Some(true) => "解析 hunter.agentpit.io 成功".to_string(),
            Some(false) => format!("解析 hunter.agentpit.io 失败（{}）", self.resolve_detail),
            None => format!("没能做解析测试（{}）", self.resolve_detail),
        };
        format!(
            "虚拟机 {} 的 DNS：{} · /etc/resolv.conf {}",
            self.instance,
            r,
            self.resolv.cn()
        )
    }
}

/// 从 resolv.conf 原文里取出 nameserver 地址。
pub fn nameservers_in(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("nameserver"))
        .map(|r| r.split_whitespace().next().unwrap_or_default().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 这条路现在走得通吗。走不通就返回**一句说得清的原因**，
/// 调用方据此跳过（而不是报错，更不是当成「不通」）。
///
/// 三个条件，缺一不可：
///
/// 1. 内置运行时的虚拟机在跑；
/// 2. **它正是当前生效的那一个** —— 机器上留着一台我们的旧虚拟机、
///    而这次安装走的是用户自己的 Docker 时，那台虚拟机的 DNS 与这次安装无关。
///    不判这一条的话，一台用不上的虚拟机能把整次安装判成 `E_RUNTIME_NO_DNS`；
/// 3. limactl 在（没有它什么都做不了）。
pub fn applicable() -> Result<(), String> {
    if !builtin::is_running() {
        return Err("内置运行时的虚拟机没在跑".to_string());
    }
    if !crate::runtime::effective::builtin_is_effective() {
        return Err("这次用的不是内置运行时（那台虚拟机与这次安装无关）".to_string());
    }
    if limactl().is_none() {
        return Err("内置运行时里没有 limactl".to_string());
    }
    Ok(())
}

// ── 在虚拟机里跑一条命令 ──────────────────────────────────────────────────

fn env_pairs() -> Vec<(String, String)> {
    builtin::env_pairs()
}

/// `limactl <args>`，带我们自己的 `LIMA_HOME`。
///
/// 每一条都先过[`crate::assist::guard::argv_vm_dns`]那道窄门；
/// `proc` 里的 `isolation_guard` 随后还会再过一次 `colima_call`
/// （那一条要求 `LIMA_HOME` 必须落在 `~/.hunter/runtime` 里）。
fn limactl_run(args: &[&str], timeout: Duration) -> AppResult<crate::proc::Ran> {
    let prog = limactl().ok_or_else(|| {
        AppError::new(
            Code::NotImplemented,
            "内置运行时里没有 limactl，做不了这一步。".to_string(),
        )
    })?;
    let mut argv = vec![prog.clone()];
    argv.extend(args.iter().map(|s| (*s).to_string()));
    crate::assist::guard::argv_vm_dns(&argv)?;
    let pairs = env_pairs();
    let env: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    crate::proc::run_timeout_env(&prog, args, timeout, &env)
}

/// 在虚拟机里跑一条命令（`limactl shell --workdir / colima-hunter <cmd…>`）。
fn vm_run(cmd: &[&str], timeout: Duration) -> AppResult<crate::proc::Ran> {
    let inst = instance();
    let mut args: Vec<&str> = vec!["shell", "--workdir", "/", &inst];
    args.extend_from_slice(cmd);
    limactl_run(&args, timeout)
}

// ── 探测 ──────────────────────────────────────────────────────────────────

/// 查一遍虚拟机的 DNS。**只读**，不改任何东西。
pub fn probe() -> AppResult<VmDns> {
    if let Err(why) = applicable() {
        return Ok(VmDns {
            instance: instance(),
            resolv: ResolvState::Unknown(why.clone()),
            resolves: None,
            resolve_detail: why,
            unit_installed: None,
        });
    }
    let inst = instance();

    // ① /etc/resolv.conf 读得到吗。断链的话 `cat` 会直接报 No such file or directory
    let resolv = match vm_run(&["cat", VM_RESOLV], SHELL_TIMEOUT) {
        Ok(r) if r.ok() => {
            let text = r.stdout.clone();
            if nameservers_in(&text).is_empty() {
                ResolvState::Empty(text)
            } else {
                ResolvState::File(text)
            }
        }
        Ok(r) => ResolvState::Broken(one_line(&r.err_line())),
        Err(e) => ResolvState::Unknown(e.msg),
    };

    // ② 真的解析得动吗。**这一条才是判据**
    let (resolves, detail) = match vm_run(
        &["getent", "hosts", crate::gateway::GATEWAY_HOST],
        SHELL_TIMEOUT,
    ) {
        Ok(r) if r.ok() && !r.stdout.trim().is_empty() => (Some(true), one_line(&r.stdout)),
        Ok(r) => (
            Some(false),
            if r.err_line().trim().is_empty() {
                format!("getent hosts 退出码 {:?}，没有输出", r.status)
            } else {
                one_line(&r.err_line())
            },
        ),
        Err(e) => (None, e.msg),
    };

    // ③ 开机那个服务装了吗（只影响「持久」这句话说得硬不硬，不影响判定）
    let unit_installed = match vm_run(&["test", "-f", VM_UNIT], Duration::from_secs(20)) {
        Ok(r) => Some(r.ok()),
        Err(_) => None,
    };

    Ok(VmDns {
        instance: inst,
        resolv,
        resolves,
        resolve_detail: detail,
        unit_installed,
    })
}

// ── 修 ────────────────────────────────────────────────────────────────────

/// 把 `/etc/resolv.conf` 写好，并把开机服务装上。
///
/// 返回给用户看的那一句（**真实做了什么**）。任何一步做不成都如实报错，
/// 不吞掉、不假装成功。
pub fn fix(p: &mut Progress) -> AppResult<String> {
    applicable().map_err(|why| {
        AppError::new(
            Code::RuntimeNoDns,
            format!("现在修不了虚拟机的 DNS：{why}。"),
        )
    })?;

    // ① 在 ~/.hunter/runtime/vm 里生成两个文件。**先建目录再过守卫** ——
    //    守卫解析路径要看真实的文件系统，目录不在的话它只能按字符串比
    let dir = stage_dir();
    std::fs::create_dir_all(&dir).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!(
                "建 {} 失败：{e}",
                crate::redact::mask_home(&dir.to_string_lossy())
            ),
        )
    })?;
    let real_dir = crate::assist::guard::writable_path(&dir)?;
    let host_resolv = real_dir.join("resolv.conf");
    let host_unit = real_dir.join("hunter-dns.service");
    write_file(&host_resolv, &resolv_conf_text())?;
    write_file(&host_unit, &unit_text())?;

    (p.say)(&format!(
        "正在给 Hunter 自己那台虚拟机写 DNS（{}）—— 你 Mac 的网络设置一个字节都不动",
        NAMESERVERS
            .iter()
            .map(|(ns, _)| *ns)
            .collect::<Vec<_>>()
            .join(" · ")
    ));

    let inst = instance();
    let host_resolv_s = host_resolv.to_string_lossy().into_owned();
    let host_unit_s = host_unit.to_string_lossy().into_owned();
    let to_resolv = format!("{inst}:{VM_TMP_RESOLV}");
    let to_unit = format!("{inst}:{VM_TMP_UNIT}");

    // ② 送进虚拟机。**固定 scp 后端**：这份 minimal 镜像里没有 rsync，
    //    让它自己选后端只会多一次失败重试
    must(
        limactl_run(
            &["copy", "--backend=scp", &host_resolv_s, &to_resolv],
            Duration::from_secs(120),
        ),
        "把 resolv.conf 送进虚拟机",
    )?;
    must(
        limactl_run(
            &["copy", "--backend=scp", &host_unit_s, &to_unit],
            Duration::from_secs(120),
        ),
        "把开机服务送进虚拟机",
    )?;

    // ③ 落位。`cp --remove-destination` 会**先把那条断链删掉**再写普通文件 ——
    //    直接 `cp` 会顺着链接去写 /run/systemd/resolve/，那个目录根本不存在
    must(
        vm_run(&["sudo", "mkdir", "-p", VM_KEEP_DIR], SHELL_TIMEOUT),
        "在虚拟机里建 /etc/hunter",
    )?;
    must(
        vm_run(
            &[
                "sudo",
                "cp",
                "--remove-destination",
                VM_TMP_RESOLV,
                VM_KEEP_FILE,
            ],
            SHELL_TIMEOUT,
        ),
        "把 resolv.conf 存一份到 /etc/hunter",
    )?;
    must(
        vm_run(
            &[
                "sudo",
                "cp",
                "--remove-destination",
                VM_TMP_RESOLV,
                VM_RESOLV,
            ],
            SHELL_TIMEOUT,
        ),
        "写 /etc/resolv.conf",
    )?;

    // ④ 开机服务。装不上**不算整件事失败** —— resolv.conf 已经写好了，
    //    少的只是「开机再写一遍」这层保险，如实说一句就行
    let mut persist = String::new();
    let unit_steps = [
        (
            vec!["sudo", "cp", "--remove-destination", VM_TMP_UNIT, VM_UNIT],
            "装开机服务",
        ),
        (
            vec!["sudo", "systemctl", "daemon-reload"],
            "让 systemd 重读",
        ),
        (
            vec!["sudo", "systemctl", "enable", UNIT_NAME],
            "把开机服务设成自启",
        ),
    ];
    let mut unit_ok = true;
    for (cmd, what) in unit_steps {
        match vm_run(&cmd, SHELL_TIMEOUT) {
            Ok(r) if r.ok() => {}
            Ok(r) => {
                unit_ok = false;
                persist = format!("{what}没成（{}）", one_line(&r.err_line()));
                break;
            }
            Err(e) => {
                unit_ok = false;
                persist = format!("{what}没成（{}）", e.msg);
                break;
            }
        }
    }

    let mut line = format!(
        "已给虚拟机 {inst} 写好 DNS（{}）",
        NAMESERVERS
            .iter()
            .map(|(ns, _)| *ns)
            .collect::<Vec<_>>()
            .join(" · ")
    );
    if unit_ok {
        line.push_str("，并装上了开机自动重写的服务 hunter-dns.service（重启也在）");
    } else {
        line.push_str(&format!(
            "；开机自动重写那层保险没装上（{persist}）—— 启动器每次启动仍会重新检查并修"
        ));
    }
    (p.say)(&line);
    Ok(line)
}

/// 重启虚拟机里的 dockerd。
///
/// 为什么要重启：已经起着的容器里那份 `/etc/resolv.conf` 是**容器创建时**
/// 由 dockerd 按宿主（这里是虚拟机）的 resolv.conf 生成的。
/// 虚拟机的 DNS 修好之后，那些容器手里还攥着旧的那一份 ——
/// 本地 Claude 在用户 Mac 上实测过：`systemctl restart docker` 之后
/// `compose up -d`，6/6 健康、容器里外网全通。
///
/// **动的是虚拟机里的 dockerd，不是用户 Mac 上的任何东西。**
pub fn restart_docker(p: &mut Progress) -> AppResult<String> {
    applicable().map_err(|why| {
        AppError::new(
            Code::RuntimeNoDns,
            format!("现在重启不了虚拟机里的 docker：{why}。"),
        )
    })?;
    (p.say)("正在重启虚拟机里的 docker，让容器重新拿到 DNS");
    must(
        vm_run(&["sudo", "systemctl", "restart", "docker"], RESTART_TIMEOUT),
        "重启虚拟机里的 docker",
    )?;
    // socket 还在不在（重启之后 colima 的转发 socket 应当照常）
    let mut waited = 0u64;
    while waited < 60 && !builtin::is_running() {
        std::thread::sleep(Duration::from_secs(2));
        waited += 2;
    }
    if !builtin::is_running() {
        return Err(AppError::new(
            Code::BuiltinRuntimeDown,
            "重启虚拟机里的 docker 之后，它的 socket 没有回来。".to_string(),
        ));
    }
    Ok("虚拟机里的 docker 已重启".to_string())
}

/// 一次完整的「查 → 该修就修 → 再查」。
///
/// 返回给用户看的那一句。**已经是好的就什么都不做**（用户 Mac 上现在那份
/// 是本地 Claude 手工改好的，不该被我们推倒重来），只在
/// 「开机那层保险还没装」时补一次。
pub fn ensure(p: &mut Progress) -> AppResult<VmDns> {
    if let Err(why) = applicable() {
        return Ok(VmDns {
            instance: instance(),
            resolv: ResolvState::Unknown(why.clone()),
            resolves: None,
            resolve_detail: why,
            unit_installed: None,
        });
    }
    let before = probe()?;
    if before.healthy() {
        // 解析得动 —— 不碰那份文件。只有「开机保险没装」时补一手，
        // 补不上也不算失败（下一次启动照样会查）
        if before.unit_installed == Some(false) {
            crate::linfo!(
                "虚拟机 DNS 现在是好的，但开机自动重写的服务还没装 —— 补上它（重启后也不会掉）"
            );
            if let Err(e) = fix(p) {
                crate::lwarn!("补装开机服务没成（不影响这一次安装）：{}", e.msg);
            }
            return probe();
        }
        return Ok(before);
    }

    crate::lwarn!(
        "虚拟机 {} 没有可用的 DNS（{}）—— 自动修",
        before.instance,
        before.resolv.cn()
    );
    (p.say)(&format!(
        "发现 Hunter 自己那台虚拟机没有 DNS（{}），正在修",
        before.resolv.cn()
    ));
    fix(p)?;
    let mid = probe()?;
    if !mid.healthy() {
        // 写好了还解析不动：多半是 dockerd / 解析器还攥着旧的那一份，重启它再看
        restart_docker(p)?;
        return probe();
    }
    // **容器已经起着的话，它们手里那份 resolv.conf 是修之前生成的**
    //（容器里那份文件由 dockerd 在容器创建那一刻写死，之后不会自己变）。
    // 重启虚拟机里的 dockerd 会把容器一起重起，容器因此重新拿一份 ——
    // 本地 Claude 在用户 Mac 上实测走的就是这一步（之后 6/6 健康）。
    if crate::compose::is_up() {
        (p.say)("容器是在 DNS 修好之前起的，手里那份 DNS 已经过时 —— 重启一次让它们重新拿");
        restart_docker(p)?;
    }
    probe()
}

// ── 小工具 ────────────────────────────────────────────────────────────────

fn must(r: AppResult<crate::proc::Ran>, what: &str) -> AppResult<()> {
    match r {
        Ok(x) if x.ok() => Ok(()),
        Ok(x) => Err(AppError::new(
            Code::RuntimeNoDns,
            format!(
                "{what}失败（退出码 {:?}）：{}",
                x.status,
                one_line(&x.err_line())
            ),
        )),
        Err(e) => Err(AppError::new(
            Code::RuntimeNoDns,
            format!("{what}失败：{}", e.msg),
        )),
    }
}

fn write_file(p: &std::path::Path, text: &str) -> AppResult<()> {
    crate::assist::guard::writable_path(p.parent().unwrap_or(p))?;
    std::fs::write(p, text).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!(
                "写 {} 失败：{e}",
                crate::redact::mask_home(&p.to_string_lossy())
            ),
        )
    })
}

fn one_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(200)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 写进去的_resolv_conf_里有网关和国内公共_dns() {
        let t = resolv_conf_text();
        let ns = nameservers_in(&t);
        assert!(ns.contains(&"192.168.5.2".to_string()), "{t}");
        // **国内网络也要能用**（任务书 P0-1 的硬要求）
        assert!(
            ns.iter().any(|n| n == "223.6.6.6" || n == "119.29.29.29"),
            "{t}"
        );
        // 192.168.5.3 在 vz + user-v2 上实测不通，不许再出现
        assert!(!ns.contains(&"192.168.5.3".to_string()), "{t}");
        assert!(
            ns.len() <= 3,
            "resolv.conf 最多只认 3 个 nameserver：{ns:?}"
        );
    }

    #[test]
    fn nameserver_解析忽略注释和空行() {
        let t = "# 注释\n\nnameserver 1.2.3.4  # 带尾注\n nameserver 5.6.7.8\noptions timeout:2\n";
        assert_eq!(nameservers_in(t), vec!["1.2.3.4", "5.6.7.8"]);
    }

    #[test]
    fn 断链和空文件都不算好() {
        let broken = VmDns {
            instance: instance(),
            resolv: ResolvState::Broken("No such file or directory".into()),
            resolves: Some(false),
            resolve_detail: "".into(),
            unit_installed: Some(false),
        };
        assert!(!broken.healthy());
        let empty = VmDns {
            resolv: ResolvState::Empty(String::new()),
            ..broken.clone()
        };
        assert!(!empty.healthy());
        // **解析得动才算好**：文件看着对但解析不动的，一样不算
        let looks_ok = VmDns {
            resolv: ResolvState::File("nameserver 1.2.3.4\n".into()),
            resolves: Some(false),
            ..broken.clone()
        };
        assert!(!looks_ok.healthy());
        let good = VmDns {
            resolves: Some(true),
            ..looks_ok.clone()
        };
        assert!(good.healthy());
    }

    #[test]
    fn 开机服务排在_docker_前面且不跑_shell() {
        let u = unit_text();
        assert!(u.contains("Before=docker.service"), "{u}");
        assert!(u.contains("Type=oneshot"), "{u}");
        assert!(u.contains("WantedBy=multi-user.target"), "{u}");
        assert!(
            u.contains(&format!(
                "ExecStart=/bin/cp --remove-destination {VM_KEEP_FILE} {VM_RESOLV}"
            )),
            "{u}"
        );
        for bad in ["/bin/sh", "/bin/bash", " -c "] {
            assert!(!u.contains(bad), "开机服务里不该出现 {bad}：{u}");
        }
    }

    #[test]
    fn 实例名跟着_profile_走() {
        assert_eq!(instance(), format!("colima-{}", builtin::PROFILE));
    }
}
