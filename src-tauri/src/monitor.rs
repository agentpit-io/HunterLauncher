//! 运行面板上的资源监控（I12 · R2）。
//!
//! ## 为什么分成三层
//!
//! 用户的原话是「显示系统运行状态，CPU、内存、硬盘状态」。可是在 macOS 上，
//! Hunter 跑在一台**虚拟机**里 —— 这台 Mac 的内存条和虚拟机里那 4 GB 完全是两回事。
//! 把它们混成一个数字，用户看到「内存 92%」根本判断不出该关哪个程序。
//!
//! 所以这里分三层，每一层都写明**数字是从哪问来的**：
//!
//! | 层 | 问谁 | 刷新 |
//! |---|---|---|
//! | 这台电脑 | `sysinfo`（macOS 走 `host_statistics`、Linux 读 `/proc`、Windows 走 Win32） | 5 秒 |
//! | Hunter 运行环境 | `limactl shell colima-hunter -- free -b / df -B1 /var/lib/docker` | 30 秒 |
//! | Hunter 各服务 | `docker stats --no-stream` + `docker inspect` | 10 秒 |
//! | 数据卷与镜像 | `docker volume ls` + `docker system df -v` + `docker image inspect` | 5 分钟 |
//!
//! ## 三条硬规矩
//!
//! 1. **拿不到就是 `None` + 一句原因**（红线 1）。界面显示「—」，绝不拿上一次的、
//!    也绝不按比例估一个。
//! 2. **窗口看不见就不采**。这一层跑的是子进程（`docker stats` 要几百毫秒），
//!    界面不在前台时每 5 秒起一次进程纯粹是在耗用户的电（方案第六节第 5 条）。
//!    这条规矩落在前端：`document.visibilityState` 变成 `hidden` 就停掉定时器。
//! 3. **虚拟机那一层必须遵守 I9 的隔离守卫**：`limactl` 只认 `~/.hunter/runtime`
//!    里的那一份，`LIMA_HOME` / `COLIMA_HOME` 必须指到那里，否则 `proc::isolation_guard`
//!    直接拒绝。这一层新增的命令走 [`crate::assist::guard::argv_vm_metrics`]
//!    那张**只读**白名单（表里一条 `sudo` 都没有）。

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;

use crate::config::LauncherConfig;
use crate::err::AppResult;
use crate::runtime::builtin;

/// 一个指标的三档提醒。**I12 只用来决定界面上标不标色**，不发通知（R7 是 I13）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    #[default]
    Ok,
    Warn,
    Crit,
}

// ── 第一层：这台电脑 ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HostMetrics {
    /// 全机 CPU 占用百分比（0–100）
    pub cpu_pct: Option<f32>,
    /// 逻辑核数
    pub cpu_cores: Option<usize>,
    pub mem_used_bytes: Option<u64>,
    pub mem_total_bytes: Option<u64>,
    /// 内存压力。macOS 有原生的三档，Linux 用 PSI 折算，Windows 没有 → `None`
    pub mem_pressure: Option<String>,
    pub mem_pressure_level: Level,
    /// 系统盘（家目录所在的那块）
    pub disk_free_bytes: Option<u64>,
    pub disk_total_bytes: Option<u64>,
    pub disk_mount: Option<String>,
    pub disk_level: Level,
    /// 每一项拿不到的原因，按字段名索引。**有值就一定有解释**
    pub reasons: BTreeMap<String, String>,
}

/// `sysinfo::System` 要留在进程里才算得出 CPU 占用（它是两次采样之间的差）。
static SYS: Mutex<Option<sysinfo::System>> = Mutex::new(None);

/// 采一次这台电脑的指标。
pub fn host() -> HostMetrics {
    let mut m = HostMetrics::default();
    let cfg = LauncherConfig::load();

    {
        let mut g = match SYS.lock() {
            Ok(g) => g,
            Err(e) => {
                m.reasons
                    .insert("cpu".into(), format!("采样器被别的线程弄坏了：{e}"));
                return m;
            }
        };
        let first = g.is_none();
        let sys = g.get_or_insert_with(sysinfo::System::new);
        sys.refresh_memory();
        if first {
            // 第一次没有参照点，CPU 占用一定是 0 —— 那是**假数字**。
            // 隔一个最小采样间隔再取一次，让第一屏也是真的（红线 1）
            sys.refresh_cpu_usage();
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        }
        sys.refresh_cpu_usage();
        m.cpu_pct = Some(sys.global_cpu_usage());
        m.cpu_cores = Some(sys.cpus().len());
        let total = sys.total_memory();
        if total > 0 {
            m.mem_total_bytes = Some(total);
            m.mem_used_bytes = Some(sys.used_memory());
        } else {
            m.reasons
                .insert("mem".into(), "sysinfo 报的内存总量是 0".into());
        }
    }

    match mem_pressure() {
        Ok((label, level)) => {
            m.mem_pressure = Some(label);
            m.mem_pressure_level = level;
        }
        Err(why) => {
            m.reasons.insert("memPressure".into(), why);
        }
    }

    match home_disk() {
        Some((mount, free, total)) => {
            m.disk_mount = Some(mount);
            m.disk_free_bytes = Some(free);
            m.disk_total_bytes = Some(total);
            let gb = free / 1_000_000_000;
            m.disk_level = if gb < cfg.monitor.host_disk_crit_gb {
                Level::Crit
            } else if gb < cfg.monitor.host_disk_warn_gb {
                Level::Warn
            } else {
                Level::Ok
            };
        }
        None => {
            m.reasons.insert(
                "disk".into(),
                "没找到家目录所在的那块盘（sysinfo 的挂载点列表里没有能对上的）".into(),
            );
        }
    }
    m
}

/// 家目录落在哪块盘上 → （挂载点, 可用字节, 总字节）。
fn home_disk() -> Option<(String, u64, u64)> {
    disk_for(&crate::paths::home())
}

/// 某个路径落在哪块盘上 → （挂载点, 可用字节, 总字节）。
///
/// 按**挂载点最长匹配**选盘：`/` 和 `/System/Volumes/Data` 同时列出来时，
/// 家目录真正落在后者上（macOS 的 APFS 布局），选错了给出来的剩余空间是另一个数。
///
/// I13 起备份目录也走它 —— 备份可能被放到外接盘上，那是另一块盘的余量。
pub fn disk_for(path: &std::path::Path) -> Option<(String, u64, u64)> {
    // 目录还不存在时（第一次设备份目录）往上找到第一个存在的祖先
    let mut home = path.to_path_buf();
    while !home.exists() {
        match home.parent() {
            Some(p) if p != home => home = p.to_path_buf(),
            _ => break,
        }
    }
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut best: Option<(usize, String, u64, u64)> = None;
    for d in disks.list() {
        let mp = d.mount_point();
        if !home.starts_with(mp) {
            continue;
        }
        let len = mp.as_os_str().len();
        if best.as_ref().map(|b| len > b.0).unwrap_or(true) {
            best = Some((
                len,
                mp.to_string_lossy().into_owned(),
                d.available_space(),
                d.total_space(),
            ));
        }
    }
    best.map(|(_, mp, free, total)| (mp, free, total))
}

/// 内存压力。
///
/// * macOS：`sysctl -n kern.memorystatus_vm_pressure_level` —— 1 正常 / 2 吃紧 / 4 危急。
///   这就是「活动监视器」里那个内存压力图的来源，不是我们自己按占用率折算的。
/// * Linux：`/proc/pressure/memory` 的 `some avg10`（PSI）。内核 4.20 起才有，
///   没有这个文件就如实说没有。
/// * Windows：没有对应概念 → 说清楚，不编。
fn mem_pressure() -> Result<(String, Level), String> {
    #[cfg(target_os = "macos")]
    {
        let r = crate::proc::run_timeout(
            "/usr/sbin/sysctl",
            &["-n", "kern.memorystatus_vm_pressure_level"],
            Duration::from_secs(5),
        )
        .map_err(|e| format!("sysctl 起不来：{}", e.msg))?;
        if !r.ok() {
            return Err(format!("sysctl 读不到内存压力：{}", r.err_line()));
        }
        return Ok(match r.stdout.trim() {
            "1" => ("正常".to_string(), Level::Ok),
            "2" => ("吃紧".to_string(), Level::Warn),
            "4" => ("危急".to_string(), Level::Crit),
            other => return Err(format!("sysctl 报了个认不出的值：{other}")),
        });
    }
    #[cfg(target_os = "linux")]
    {
        let txt = std::fs::read_to_string("/proc/pressure/memory")
            .map_err(|e| format!("读不到 /proc/pressure/memory（内核 4.20 起才有）：{e}"))?;
        let avg10 = txt
            .lines()
            .find(|l| l.starts_with("some "))
            .and_then(|l| {
                l.split_whitespace()
                    .find_map(|kv| kv.strip_prefix("avg10="))
            })
            .and_then(|v| v.parse::<f32>().ok())
            .ok_or_else(|| "/proc/pressure/memory 里没有 some avg10 这一项".to_string())?;
        Ok(if avg10 >= 10.0 {
            (format!("危急（PSI avg10 {avg10:.1}%）"), Level::Crit)
        } else if avg10 >= 1.0 {
            (format!("吃紧（PSI avg10 {avg10:.1}%）"), Level::Warn)
        } else {
            (format!("正常（PSI avg10 {avg10:.1}%）"), Level::Ok)
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Err("Windows 没有「内存压力」这个指标，这里不拿占用率冒充它".to_string())
    }
}

// ── 第二层：Hunter 运行环境（内置运行时的虚拟机）──────────────────────────

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMetrics {
    /// 这一层适不适用。用户自己的 Docker / 没装内置运行时 → false，界面整张卡不显示
    pub applicable: bool,
    /// 不适用的原因（适用时为空）
    pub reason: String,
    /// 分配给虚拟机的核数与内存（`colima start` 时定的，见 `builtin::vm_params`）
    pub cpus: Option<u32>,
    pub mem_total_bytes: Option<u64>,
    pub mem_used_bytes: Option<u64>,
    pub mem_level: Level,
    /// 虚拟机里 `/var/lib/docker` 那块盘
    pub disk_used_bytes: Option<u64>,
    pub disk_total_bytes: Option<u64>,
    pub disk_level: Level,
    pub reasons: BTreeMap<String, String>,
}

/// 采一次虚拟机的指标。**只读**：三条命令都在
/// [`crate::assist::guard::VM_METRIC_COMMANDS`] 那张表里逐字写死。
pub fn runtime() -> RuntimeMetrics {
    let mut m = RuntimeMetrics::default();
    if let Err(why) = crate::runtime::vmdns::applicable() {
        m.reason = why;
        return m;
    }
    m.applicable = true;
    let cfg = LauncherConfig::load();

    // 分配值来自我们自己启动虚拟机时定的参数（`colima start --cpu N --memory G`），
    // 不是猜的 —— 那一份就是 builtin::vm_params 的返回值
    let (cpus, mem_gib, _disk_gib) = builtin::vm_params();
    m.cpus = Some(cpus);

    match vm_free() {
        Ok((total, used)) => {
            m.mem_total_bytes = Some(total);
            m.mem_used_bytes = Some(used);
            if let Some(pct) = (used * 100).checked_div(total) {
                m.mem_level = if pct >= u64::from(cfg.monitor.vm_mem_warn_pct) {
                    Level::Warn
                } else {
                    Level::Ok
                };
            }
        }
        Err(why) => {
            // 分配量是知道的（我们自己定的），实际用量没问到 —— 这两件事要分开说
            m.mem_total_bytes = Some(u64::from(mem_gib) * 1024 * 1024 * 1024);
            m.reasons.insert("mem".into(), why);
        }
    }

    match vm_df() {
        Ok((total, used)) => {
            m.disk_total_bytes = Some(total);
            m.disk_used_bytes = Some(used);
            if let Some(pct) = (used * 100).checked_div(total) {
                m.disk_level = if pct >= u64::from(cfg.monitor.vm_disk_warn_pct) {
                    Level::Warn
                } else {
                    Level::Ok
                };
            }
        }
        Err(why) => {
            m.reasons.insert("disk".into(), why);
        }
    }
    m
}

/// 虚拟机里跑一条**只读**命令。参数先过 `argv_vm_metrics`，
/// 之后 `proc::isolation_guard` 还会再核一遍 `LIMA_HOME`（I9）。
fn vm_run(cmd: &[&str], timeout: Duration) -> AppResult<crate::proc::Ran> {
    let prog = crate::runtime::vmdns::limactl_path()?;
    let inst = crate::runtime::vmdns::instance();
    let mut args: Vec<&str> = vec!["shell", "--workdir", "/", &inst];
    args.extend_from_slice(cmd);
    let mut argv = vec![prog.clone()];
    argv.extend(args.iter().map(|s| (*s).to_string()));
    crate::assist::guard::argv_vm_metrics(&argv)?;
    let pairs = builtin::env_pairs();
    let env: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    crate::proc::run_timeout_env(&prog, &args, timeout, &env)
}

const VM_TIMEOUT: Duration = Duration::from_secs(20);

/// `free -b` → （总字节, 已用字节）。
fn vm_free() -> Result<(u64, u64), String> {
    let r = vm_run(&["free", "-b"], VM_TIMEOUT).map_err(|e| e.msg)?;
    if !r.ok() {
        return Err(format!("虚拟机里 free -b 没跑通：{}", r.err_line()));
    }
    parse_free(&r.stdout).ok_or_else(|| format!("free -b 的输出看不懂：{}", first_line(&r.stdout)))
}

/// `free -b` 的输出：第二行 `Mem: <total> <used> <free> …`。
pub fn parse_free(stdout: &str) -> Option<(u64, u64)> {
    for l in stdout.lines() {
        let l = l.trim();
        if !l.starts_with("Mem:") {
            continue;
        }
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 3 {
            return None;
        }
        let total = f[1].parse::<u64>().ok()?;
        let used = f[2].parse::<u64>().ok()?;
        return Some((total, used));
    }
    None
}

/// `df -B1 /var/lib/docker` → （总字节, 已用字节）。
fn vm_df() -> Result<(u64, u64), String> {
    let r = vm_run(&["df", "-B1", "/var/lib/docker"], VM_TIMEOUT).map_err(|e| e.msg)?;
    if !r.ok() {
        return Err(format!("虚拟机里 df 没跑通：{}", r.err_line()));
    }
    parse_df(&r.stdout).ok_or_else(|| format!("df 的输出看不懂：{}", first_line(&r.stdout)))
}

/// `df -B1` 的输出：表头之后那一行 `<设备> <1B-blocks> <Used> <Available> …`。
pub fn parse_df(stdout: &str) -> Option<(u64, u64)> {
    for l in stdout.lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        let total = f[1].parse::<u64>().ok()?;
        let used = f[2].parse::<u64>().ok()?;
        return Some((total, used));
    }
    None
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

// ── 第三层：Hunter 各服务 ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServiceUsage {
    pub service: String,
    pub cpu_pct: Option<f32>,
    pub mem_bytes: Option<u64>,
    pub mem_limit_bytes: Option<u64>,
    pub restart_count: Option<u32>,
    pub oom_killed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServiceMetrics {
    pub services: Vec<ServiceUsage>,
    /// 一个都采不到时的原因
    pub reason: String,
}

/// `docker stats --no-stream` + `docker inspect`，只统计本项目的容器。
pub fn services() -> ServiceMetrics {
    let mut out = ServiceMetrics::default();
    let names = crate::compose::container_names();
    if names.is_empty() {
        out.reason = "这台机器上还没有本项目的容器".into();
        return out;
    }
    let bin = crate::runtime::which::docker_bin();
    let ids: Vec<&str> = names.values().map(String::as_str).collect();
    let mut args: Vec<&str> = vec![
        "stats",
        "--no-stream",
        "--format",
        "{{.Name}}\t{{.CPUPerc}}\t{{.MemUsage}}",
    ];
    args.extend_from_slice(&ids);
    let stats = match crate::proc::run_timeout(&bin, &args, Duration::from_secs(45)) {
        Ok(r) if r.ok() => parse_stats(&r.stdout),
        Ok(r) => {
            out.reason = format!("docker stats 没跑通：{}", r.err_line());
            BTreeMap::new()
        }
        Err(e) => {
            out.reason = format!("docker stats 起不来：{}", e.msg);
            BTreeMap::new()
        }
    };
    for (svc, container) in &names {
        let s = stats.get(container);
        let v = crate::compose::vitals_of(container).unwrap_or_default();
        out.services.push(ServiceUsage {
            service: svc.clone(),
            cpu_pct: s.map(|x| x.0),
            mem_bytes: s.map(|x| x.1),
            mem_limit_bytes: s.and_then(|x| x.2),
            restart_count: v.restart_count,
            oom_killed: v.oom_killed,
        });
    }
    out.services.sort_by_key(|s| {
        crate::selfcheck::EXPECTED
            .iter()
            .position(|e| *e == s.service)
    });
    out
}

/// `docker stats --format '{{.Name}}\t{{.CPUPerc}}\t{{.MemUsage}}'` 的输出。
///
/// `MemUsage` 形如 `135.2MiB / 3.822GiB`。docker 这里用的是 **1024 进制**（MiB/GiB），
/// 和 `system df` 的十进制不是一套 —— 两边各自按各自的来，别混。
pub fn parse_stats(stdout: &str) -> BTreeMap<String, (f32, u64, Option<u64>)> {
    let mut m = BTreeMap::new();
    for l in stdout.lines() {
        let f: Vec<&str> = l.trim().split('\t').collect();
        if f.len() < 3 || f[0].is_empty() {
            continue;
        }
        let Some(cpu) = f[1].trim().trim_end_matches('%').parse::<f32>().ok() else {
            continue;
        };
        let mut parts = f[2].split('/');
        let Some(used) = parts.next().and_then(parse_binary_size) else {
            continue;
        };
        let limit = parts.next().and_then(parse_binary_size);
        m.insert(f[0].to_string(), (cpu, used, limit));
    }
    m
}

/// `135.2MiB` / `3.822GiB` / `0B` → 字节（1024 进制）。认不出来就是 `None`。
pub fn parse_binary_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let pos = s.find(|c: char| c.is_ascii_alphabetic())?;
    let (num, unit) = s.split_at(pos);
    let n: f64 = num.trim().parse().ok()?;
    let mult: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "b" => 1.0,
        "kib" | "kb" => 1024.0,
        "mib" | "mb" => 1024.0 * 1024.0,
        "gib" | "gb" => 1024.0 * 1024.0 * 1024.0,
        "tib" | "tb" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * mult) as u64)
}

// ── 第四层：数据卷与镜像 ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StorageMetrics {
    pub volumes: Vec<crate::compose::VolumeInfo>,
    /// 本项目全部数据卷合计（能算出大小的那些）
    pub volumes_total_bytes: Option<u64>,
    /// 数据库那一个卷（`hunter_pg_data`）单独拎出来 —— 用户最关心的就是它
    pub db_bytes: Option<u64>,
    /// 本项目六个镜像合计
    pub images_bytes: Option<u64>,
    /// 六个里查到了几个（查不到的不计入，界面要说清楚）
    pub images_found: usize,
    pub reasons: BTreeMap<String, String>,
}

/// 数据卷与镜像的占用。这一层最慢（`docker system df -v` 要遍历所有卷），
/// 所以刷新周期是 5 分钟。
pub fn storage() -> StorageMetrics {
    let mut m = StorageMetrics::default();
    match crate::compose::volumes_of_project() {
        Ok(v) => m.volumes = v,
        Err(e) => {
            m.reasons.insert("volumes".into(), e.msg);
        }
    }
    match crate::compose::volume_sizes() {
        Ok(sizes) => {
            let mut total = 0u64;
            let mut any = false;
            for v in &mut m.volumes {
                if let Some(b) = sizes.get(&v.name) {
                    v.size_bytes = Some(*b);
                    total += b;
                    any = true;
                    if v.short == crate::compose::VOL_DB {
                        m.db_bytes = Some(*b);
                    }
                }
            }
            if any {
                m.volumes_total_bytes = Some(total);
            } else if !m.volumes.is_empty() {
                m.reasons.insert(
                    "volumeSizes".into(),
                    "docker system df -v 里没有本项目这几个卷的大小".into(),
                );
            }
        }
        Err(e) => {
            m.reasons.insert("volumeSizes".into(), e.msg);
        }
    }

    let cfg = LauncherConfig::load();
    let refs: Vec<String> = crate::config::images(
        &cfg.hunter.registry_prefix,
        &cfg.hunter.base_prefix,
        &cfg.hunter.tag,
    )
    .iter()
    .map(|i| i.reference.clone())
    .collect();
    let (bytes, found) = crate::compose::image_disk_usage(&refs);
    m.images_bytes = bytes;
    m.images_found = found;
    if bytes.is_none() {
        m.reasons.insert(
            "images".into(),
            "docker image inspect 一个都没查到（镜像可能还没拉下来）".into(),
        );
    }
    m
}

// ── R7 · 异常监测与智能分析（I13）────────────────────────────────────────
//
// I12 把七类指标采齐了、阈值也写进了 `[monitor]`，但只用来给界面标色。
// R7 接上的是后面那三件事：**给结论、给一键处理、判不了就交给诊断助手**。
//
// ## 三条规矩
//
// 1. **提醒里的每一个数字都是实测的。** 「系统盘只剩 3.2 GB」里那个 3.2
//    就是 `sysinfo` 那一刻报的值；「api 一小时内重启了 4 次」里那个 4
//    是两次采样之间 `RestartCount` 的真实差值，不是估的。
// 2. **能自己做的只有三样**（[`crate::cleanup`]）：清本项目旧镜像、悬空卷、
//    过期备份。要删用户文件、给虚拟机扩容、调内存的**一律只给建议**。
// 3. **同一个问题 24 小时内只说一次**（`[monitor] quiet_hours`）。
//    判据存在 `~/.hunter/monitor-state.json` 里 —— 启动器关掉再开，
//    不该把昨天说过的话重说一遍。

/// 一条提醒。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Alert {
    /// 稳定的规则 id（「24 小时内不重复」按它去重，测试也按它对）
    pub id: String,
    pub level: Level,
    pub title: String,
    /// 结论，人话一两句
    pub detail: String,
    /// 逐条实测数字（界面折在「详情」里）
    pub facts: Vec<String>,
    /// 一键处理：动作 id（目前只有 `cleanup_project_space` 与 `prune_backups`）
    pub actions: Vec<String>,
    /// 只能建议、不替他做的事
    pub advice: Vec<String>,
    /// 规则层判不了，要把脱敏快照交给诊断助手
    pub needs_ai: bool,
}

/// 一次体检的结果。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Alerts {
    pub at: String,
    pub alerts: Vec<Alert>,
    /// 这一轮**新出现**、该弹通知的那几条（已经按 24 小时去过重）
    pub notify: Vec<String>,
    /// 采不到的那几层（界面要说清楚「这一项没查」而不是「这一项没事」）
    pub reasons: BTreeMap<String, String>,
}

impl Alerts {
    pub fn worst(&self) -> Level {
        if self.alerts.iter().any(|a| a.level == Level::Crit) {
            Level::Crit
        } else if self.alerts.is_empty() {
            Level::Ok
        } else {
            Level::Warn
        }
    }
}

/// 跨进程记下来的那几件事。**只有这个文件能让「一小时内重启了几次」
/// 和「同一个问题别重复说」在启动器重开之后还算得准。**
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// 服务名 → [(unix 秒, 那一刻的 RestartCount)]，只留窗口内的
    #[serde(default)]
    pub restarts: BTreeMap<String, Vec<(i64, u32)>>,
    /// `YYYYMMDD` → 那一天最后一次看到的数据库卷大小
    #[serde(default)]
    pub db_size: BTreeMap<String, u64>,
    /// 提醒 id → 上次弹通知的 unix 秒
    #[serde(default)]
    pub notified: BTreeMap<String, i64>,
}

pub fn state_path() -> std::path::PathBuf {
    crate::paths::root().join("monitor-state.json")
}

pub fn load_state() -> State {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_state(s: &State) {
    if let Ok(j) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(state_path(), j);
    }
}

/// 窗口内重启了几次 = 窗口内**最后一次**的计数 − **第一次**的计数。
///
/// `RestartCount` 是累计值，直接拿它当「最近重启次数」是错的 ——
/// 一台跑了半年的机器上它可能是 200，而这半年里一次问题都没有。
pub fn restarts_in_window(samples: &[(i64, u32)], now: i64, window_secs: i64) -> u32 {
    let inside: Vec<&(i64, u32)> = samples
        .iter()
        .filter(|(t, _)| now - *t <= window_secs)
        .collect();
    match (inside.first(), inside.last()) {
        (Some((_, a)), Some((_, b))) => b.saturating_sub(*a),
        _ => 0,
    }
}

/// 把这一轮的采样并进 state，并把窗口外的丢掉。
fn push_restart_samples(st: &mut State, svcs: &[ServiceUsage], now: i64, window_secs: i64) {
    for s in svcs {
        let Some(c) = s.restart_count else { continue };
        let v = st.restarts.entry(s.service.clone()).or_default();
        // 计数没变就只更新最后一条的时间戳，免得文件无限长
        if let Some(last) = v.last_mut() {
            if last.1 == c {
                last.0 = now;
                v.retain(|(t, _)| now - *t <= window_secs * 2);
                continue;
            }
        }
        v.push((now, c));
        v.retain(|(t, _)| now - *t <= window_secs * 2);
    }
}

/// 跑一遍 R7 的七类监测。**采一次现场，给一组结论。**
pub fn alerts() -> Alerts {
    let cfg = LauncherConfig::load();
    let now = crate::timefmt::now_unix();
    let mut out = Alerts {
        at: crate::timefmt::now_shanghai(),
        ..Default::default()
    };
    let mut st = load_state();

    let h = host();
    let rt = runtime();
    let sv = services();
    let store = storage();

    // ① 系统盘剩余（< 20GB 提醒 / < 5GB 严重）
    match (h.disk_free_bytes, h.disk_total_bytes) {
        (Some(free), Some(total)) => {
            let gb = free / 1_000_000_000;
            let lvl = if gb < cfg.monitor.host_disk_crit_gb {
                Level::Crit
            } else if gb < cfg.monitor.host_disk_warn_gb {
                Level::Warn
            } else {
                Level::Ok
            };
            if lvl != Level::Ok {
                let cl = crate::cleanup::plan_cached();
                let mut facts = vec![
                    format!(
                        "系统盘（{}）剩 {} / 共 {}",
                        h.disk_mount.clone().unwrap_or_default(),
                        crate::flow::human_bytes(free),
                        crate::flow::human_bytes(total)
                    ),
                    format!(
                        "阈值：低于 {} GB 提醒、低于 {} GB 严重",
                        cfg.monitor.host_disk_warn_gb, cfg.monitor.host_disk_crit_gb
                    ),
                ];
                facts.extend(cl.lines.clone());
                let mut actions = Vec::new();
                if !cl.is_empty() {
                    actions.push("cleanup_project_space".to_string());
                    facts.push(format!(
                        "Hunter 自己能安全清掉的：旧镜像 {} + 悬空卷 {} + 过期备份 {} = {}",
                        crate::flow::human_bytes(cl.old_images_bytes),
                        crate::flow::human_bytes(cl.orphan_volumes_bytes),
                        crate::flow::human_bytes(cl.expired_backups_bytes),
                        crate::flow::human_bytes(cl.total_bytes)
                    ));
                }
                out.alerts.push(Alert {
                    id: "host-disk".into(),
                    level: lvl,
                    title: if lvl == Level::Crit {
                        "系统盘快没空间了".into()
                    } else {
                        "系统盘空间偏紧".into()
                    },
                    detail: format!(
                        "这块盘只剩 {}。Hunter 的镜像与数据加起来有几个 GB，\
                         盘满了容器会起不来、数据库也可能写不进去。",
                        crate::flow::human_bytes(free)
                    ),
                    facts,
                    actions,
                    advice: vec![
                        "系统盘上别的文件（下载、废纸篓、旧的虚拟机镜像）要不要清，由你自己决定 —— 启动器不会去动 Hunter 之外的任何文件。".into(),
                    ],
                    needs_ai: false,
                });
            }
        }
        _ => {
            if let Some(w) = h.reasons.get("disk") {
                out.reasons.insert("hostDisk".into(), w.clone());
            }
        }
    }

    // ② 运行环境虚拟机磁盘（> 80% 提醒 / > 95% 严重）
    if rt.applicable {
        match (rt.disk_used_bytes, rt.disk_total_bytes) {
            (Some(u), Some(t)) if t > 0 => {
                let pct = (u * 100).checked_div(t).unwrap_or(0);
                let lvl = if pct >= u64::from(cfg.monitor.vm_disk_crit_pct) {
                    Level::Crit
                } else if pct >= u64::from(cfg.monitor.vm_disk_warn_pct) {
                    Level::Warn
                } else {
                    Level::Ok
                };
                if lvl != Level::Ok {
                    let cl = crate::cleanup::plan_cached();
                    let mut actions = Vec::new();
                    if !cl.is_empty() {
                        actions.push("cleanup_project_space".to_string());
                    }
                    out.alerts.push(Alert {
                        id: "vm-disk".into(),
                        level: lvl,
                        title: "Hunter 运行环境的磁盘快满了".into(),
                        detail: format!(
                            "那台虚拟机的磁盘用了 {pct}%（{} / {}）。它满了之后镜像拉不下来、\
                             数据库也写不进去 —— 而这块磁盘是 Hunter 独占的，和你的系统盘是两回事。",
                            crate::flow::human_bytes(u),
                            crate::flow::human_bytes(t)
                        ),
                        facts: vec![
                            format!("虚拟机内 df -B1 /var/lib/docker：已用 {u} / 共 {t} 字节"),
                            format!(
                                "阈值：超过 {}% 提醒、超过 {}% 严重",
                                cfg.monitor.vm_disk_warn_pct, cfg.monitor.vm_disk_crit_pct
                            ),
                        ],
                        actions,
                        advice: vec![
                            "还不够的话可以给虚拟机扩盘，但那要先停掉运行环境、而且是不可逆的（只能扩不能缩），所以要你自己点头之后才会做。".into(),
                        ],
                        needs_ai: false,
                    });
                }
            }
            _ => {
                if let Some(w) = rt.reasons.get("disk") {
                    out.reasons.insert("vmDisk".into(), w.clone());
                }
            }
        }

        // ⑤ 运行环境内存（> 85% 提醒；出现 OOM 由 ⑥ 负责报严重）
        if let (Some(u), Some(t)) = (rt.mem_used_bytes, rt.mem_total_bytes) {
            if let Some(pct) = (u * 100).checked_div(t) {
                if pct >= u64::from(cfg.monitor.vm_mem_warn_pct) {
                    let mut facts = vec![format!(
                        "虚拟机内 free -b：已用 {} / 共 {}（{pct}%）",
                        crate::flow::human_bytes(u),
                        crate::flow::human_bytes(t)
                    )];
                    let mut top: Vec<&ServiceUsage> = sv
                        .services
                        .iter()
                        .filter(|s| s.mem_bytes.is_some())
                        .collect();
                    top.sort_by_key(|s| std::cmp::Reverse(s.mem_bytes.unwrap_or(0)));
                    for s in top.iter().take(3) {
                        facts.push(format!(
                            "{} 占 {}",
                            s.service,
                            crate::flow::human_bytes(s.mem_bytes.unwrap_or(0))
                        ));
                    }
                    let host_free = match (h.mem_total_bytes, h.mem_used_bytes) {
                        (Some(t), Some(u)) => Some(t.saturating_sub(u)),
                        _ => None,
                    };
                    let mut advice = vec![format!(
                        "占得最多的是 {}。",
                        top.first().map(|s| s.service.as_str()).unwrap_or("—")
                    )];
                    // 这台电脑自己还有富余时，才建议给虚拟机多分一点
                    if let Some(f) = host_free {
                        if f > 4_000_000_000 {
                            advice.push(format!(
                                "这台电脑还空着 {}，可以把虚拟机的内存从 {} 调大一点；\
                                 改了要重启运行环境，所以要你点头之后才会做。",
                                crate::flow::human_bytes(f),
                                crate::flow::human_bytes(t)
                            ));
                        } else {
                            advice.push(
                                "这台电脑自己也不宽裕，调大虚拟机内存会让别的程序更卡 —— 先关几个别的程序更划算。"
                                    .into(),
                            );
                        }
                    }
                    out.alerts.push(Alert {
                        id: "vm-mem".into(),
                        level: Level::Warn,
                        title: "Hunter 运行环境的内存吃紧".into(),
                        detail: format!(
                            "那台虚拟机分到 {}，现在用了 {pct}%。再高下去服务会被系统杀掉重启。",
                            crate::flow::human_bytes(t)
                        ),
                        facts,
                        actions: Vec::new(),
                        advice,
                        needs_ai: false,
                    });
                }
            }
        }
    }

    // ③ 备份目录所在盘：放得下几份
    let bdir = cfg.backup.effective_dir();
    match (disk_for(&bdir), crate::backup::typical_bytes()) {
        (Some((mount, free, _)), Some(one)) if one > 0 => {
            let copies = free / one;
            let want = u64::from(cfg.monitor.backup_disk_min_copies);
            if copies < want {
                let lvl = if copies < 1 { Level::Crit } else { Level::Warn };
                out.alerts.push(Alert {
                    id: "backup-disk".into(),
                    level: lvl,
                    title: if lvl == Level::Crit {
                        "备份目录所在的盘放不下一份备份了".into()
                    } else {
                        "备份目录所在的盘快放不下了".into()
                    },
                    detail: format!(
                        "{} 还剩 {}，而一份备份大约 {} —— 只够再放 {copies} 份。",
                        crate::redact::mask_home(&bdir.to_string_lossy()),
                        crate::flow::human_bytes(free),
                        crate::flow::human_bytes(one)
                    ),
                    facts: vec![
                        format!("挂载点 {mount}，剩 {free} 字节"),
                        format!("最近一份备份 {one} 字节"),
                        format!("保留策略：最近 {} 天", cfg.backup.keep_days_clamped()),
                    ],
                    actions: vec!["prune_backups".to_string()],
                    advice: vec![
                        "也可以把备份目录换到另一块盘（外接盘更保险），或者把保留天数改小 —— 两个都在设置页里。".into(),
                    ],
                    needs_ai: false,
                });
            }
        }
        (None, _) => {
            out.reasons.insert(
                "backupDisk".into(),
                format!(
                    "查不到 {} 落在哪块盘上",
                    crate::redact::mask_home(&bdir.to_string_lossy())
                ),
            );
        }
        _ => {
            out.reasons.insert(
                "backupDisk".into(),
                "还没有任何备份，算不出「一份备份有多大」—— 这一项等第一次备份之后才判得了".into(),
            );
        }
    }

    // ④ 这台电脑的内存压力
    if h.mem_pressure_level != Level::Ok {
        let hunter: u64 = sv.services.iter().filter_map(|s| s.mem_bytes).sum();
        let mut facts = vec![format!(
            "内存压力：{}",
            h.mem_pressure.clone().unwrap_or_default()
        )];
        if let (Some(t), Some(u)) = (h.mem_total_bytes, h.mem_used_bytes) {
            facts.push(format!(
                "这台电脑内存 {} / {}",
                crate::flow::human_bytes(u),
                crate::flow::human_bytes(t)
            ));
        }
        // **先把「Hunter 占了多少」摆出来**，免得用户以为电脑卡都是它造成的
        facts.push(format!(
            "Hunter 全部服务加起来占 {}",
            crate::flow::human_bytes(hunter)
        ));
        let share = match h.mem_used_bytes {
            Some(u) if u > 0 => Some(hunter * 100 / u),
            _ => None,
        };
        let detail = match share {
            Some(p) => format!(
                "系统报内存压力{}。Hunter 占了其中约 {p}% —— {}",
                if h.mem_pressure_level == Level::Crit {
                    "已经到「危急」"
                } else {
                    "「吃紧」"
                },
                if p < 20 {
                    "也就是说，卡的主要原因多半不在它。"
                } else {
                    "停掉 Hunter 能明显缓解。"
                }
            ),
            None => "系统报内存压力偏高。".to_string(),
        };
        out.alerts.push(Alert {
            id: "host-mem".into(),
            level: h.mem_pressure_level,
            title: "这台电脑的内存吃紧".into(),
            detail,
            facts,
            actions: Vec::new(),
            advice: vec![if rt.applicable {
                "在运行面板点「停止」并勾上「顺便停运行环境」，能把 Hunter 占的内存整块还给系统。"
                    .to_string()
            } else {
                "在运行面板点「停止」能把 Hunter 占的内存还给系统。".to_string()
            }],
            needs_ai: false,
        });
    }

    // ⑥ 服务反复重启 / 被 OOM 杀掉
    let window = i64::from(cfg.monitor.restart_window_min) * 60;
    push_restart_samples(&mut st, &sv.services, now, window);
    for s in &sv.services {
        if s.oom_killed == Some(true) {
            out.alerts.push(Alert {
                id: format!("oom-{}", s.service),
                level: Level::Crit,
                title: format!("{} 被系统当成内存大户杀掉过", s.service),
                detail: format!(
                    "docker 报 {} 这个容器上一次退出是被 OOM 杀的 —— 内存不够用了。",
                    s.service
                ),
                facts: vec![
                    format!("docker inspect {}：OOMKilled = true", s.service),
                    format!(
                        "它现在占 {}",
                        s.mem_bytes
                            .map(crate::flow::human_bytes)
                            .unwrap_or_else(|| "—".into())
                    ),
                ],
                actions: Vec::new(),
                advice: vec![
                    "这一档启动器不替你改内存分配：给虚拟机调内存要重启运行环境，是个会中断服务的决定。点「让 AI 帮我看看」会把这个服务的日志（脱敏）交给诊断助手分析。".into(),
                ],
                // 为什么被 OOM 要看日志才知道，规则层只能确认「确实被杀了」
                needs_ai: true,
            });
        }
        let n = restarts_in_window(
            st.restarts
                .get(&s.service)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            now,
            window,
        );
        if n >= cfg.monitor.restart_warn_count {
            out.alerts.push(Alert {
                id: format!("restart-{}", s.service),
                level: Level::Warn,
                title: format!("{} 在反复重启", s.service),
                detail: format!(
                    "{} 这个服务在最近 {} 分钟里重启了 {n} 次。反复重启通常说明它每次起来都撞到同一个问题。",
                    s.service, cfg.monitor.restart_window_min
                ),
                facts: vec![
                    format!(
                        "docker inspect 的 RestartCount 现在是 {}",
                        s.restart_count.unwrap_or(0)
                    ),
                    format!(
                        "窗口 {} 分钟内的增量：{n} 次（阈值 {} 次）",
                        cfg.monitor.restart_window_min, cfg.monitor.restart_warn_count
                    ),
                ],
                actions: Vec::new(),
                advice: vec!["点「让 AI 帮我看看」会把这个服务最近的日志（脱敏）交给诊断助手。".into()],
                needs_ai: true,
            });
        }
    }

    // ⑦ 数据库体积单日增长
    if let Some(db) = store.db_bytes {
        let today = crate::timefmt::file_stamp_unix(now)[..8].to_string();
        let prev: Option<(String, u64)> = st
            .db_size
            .iter()
            .rfind(|(d, _)| **d < today)
            .map(|(d, v)| (d.clone(), *v));
        st.db_size.insert(today.clone(), db);
        // 只留最近 14 天
        while st.db_size.len() > 14 {
            let Some(k) = st.db_size.keys().next().cloned() else {
                break;
            };
            st.db_size.remove(&k);
        }
        if let Some((d, old)) = prev {
            let grow = db.saturating_sub(old);
            let limit = cfg.monitor.db_growth_warn_gb * 1_000_000_000;
            if grow > limit {
                out.alerts.push(Alert {
                    id: "db-growth".into(),
                    level: Level::Warn,
                    title: "数据库一天里长得有点快".into(),
                    detail: format!(
                        "数据库从 {} 的 {} 涨到现在的 {}，一天多了 {}。",
                        d,
                        crate::flow::human_bytes(old),
                        crate::flow::human_bytes(db),
                        crate::flow::human_bytes(grow)
                    ),
                    facts: vec![
                        format!("docker system df -v 里那个数据卷：{db} 字节"),
                        format!("阈值：单日增长超过 {} GB", cfg.monitor.db_growth_warn_gb),
                    ],
                    actions: Vec::new(),
                    advice: vec!["占得最大的那几张表可以在「备份与恢复」里看到（每份备份的 meta.json 都记了表数与行数）。".into()],
                    needs_ai: true,
                });
            }
        }
    }

    // ⑧ 备份失败（不在 R7 那张表里，是 R6 6.3 要求的那一条）
    if !cfg.backup.last_error.is_empty() {
        let streak = cfg.backup.fail_streak;
        out.alerts.push(Alert {
            id: "backup-failed".into(),
            level: if streak >= 2 {
                Level::Crit
            } else {
                Level::Warn
            },
            title: if streak >= 2 {
                format!("自动备份连着失败了 {streak} 次")
            } else {
                "上一次自动备份失败了".into()
            },
            detail: format!("原因：{}", cfg.backup.last_error),
            facts: vec![
                format!("最近一次尝试：{}", cfg.backup.last_run_at),
                format!(
                    "最近一次成功：{}",
                    if cfg.backup.last_ok_at.is_empty() {
                        "从来没成功过".to_string()
                    } else {
                        cfg.backup.last_ok_at.clone()
                    }
                ),
            ],
            actions: Vec::new(),
            advice: vec![
                "在设置页的「数据备份」里点「立即备份一次」能当场看到是哪一步出的问题。".into(),
            ],
            // 连着两次失败就交给诊断助手（方案 R6 6.3）
            needs_ai: streak >= 2,
        });
    }

    // 「同一个问题 24 小时内不重复」
    let quiet = i64::from(cfg.monitor.quiet_hours) * 3600;
    for a in &out.alerts {
        let key = format!("{}:{:?}", a.id, a.level);
        let last = st.notified.get(&key).copied().unwrap_or(0);
        if now - last >= quiet {
            out.notify.push(a.id.clone());
            st.notified.insert(key, now);
        }
    }
    // 已经不存在的提醒，把记录清掉 —— 下次再出现时该立刻说一声
    let live: BTreeMap<String, ()> = out.alerts.iter().map(|a| (a.id.clone(), ())).collect();
    st.notified
        .retain(|k, _| live.contains_key(k.split(':').next().unwrap_or("")));
    save_state(&st);

    out.alerts.sort_by_key(|a| match a.level {
        Level::Crit => 0,
        Level::Warn => 1,
        Level::Ok => 2,
    });
    out
}

/// 交给诊断助手的**脱敏指标快照**（方案 R7：「判不了的把脱敏指标快照交给诊断助手」）。
///
/// 里面**只有数字与服务名**：没有路径、没有主机名、没有 key。
/// 这一条是红线 2 在 R7 这条路上的落点 —— AI 那一侧看得到的东西必须逐项可数。
pub fn snapshot_for_ai(a: &Alerts) -> Vec<String> {
    let mut v = vec![format!("时间 {}", a.at)];
    let h = host();
    if let (Some(u), Some(t)) = (h.mem_used_bytes, h.mem_total_bytes) {
        v.push(format!("这台电脑内存 {u}/{t} 字节"));
    }
    if let Some(p) = &h.mem_pressure {
        v.push(format!("内存压力 {p}"));
    }
    if let (Some(f), Some(t)) = (h.disk_free_bytes, h.disk_total_bytes) {
        v.push(format!("系统盘 剩 {f}/{t} 字节"));
    }
    let rt = runtime();
    if rt.applicable {
        if let (Some(u), Some(t)) = (rt.mem_used_bytes, rt.mem_total_bytes) {
            v.push(format!("运行环境内存 {u}/{t} 字节"));
        }
        if let (Some(u), Some(t)) = (rt.disk_used_bytes, rt.disk_total_bytes) {
            v.push(format!("运行环境磁盘 {u}/{t} 字节"));
        }
    }
    for s in services().services {
        v.push(format!(
            "服务 {} · CPU {} · 内存 {} · 重启 {} · OOM {}",
            s.service,
            s.cpu_pct
                .map(|x| format!("{x:.1}%"))
                .unwrap_or_else(|| "—".into()),
            s.mem_bytes
                .map(|x| x.to_string())
                .unwrap_or_else(|| "—".into()),
            s.restart_count
                .map(|x| x.to_string())
                .unwrap_or_else(|| "—".into()),
            s.oom_killed
                .map(|x| x.to_string())
                .unwrap_or_else(|| "—".into()),
        ));
    }
    for x in &a.alerts {
        v.push(format!("提醒 {} · {:?} · {}", x.id, x.level, x.title));
    }
    // 最后统一过一遍脱敏（万一以后有人往上面加了带路径的行）
    v.into_iter().map(|l| crate::redact::redact(&l)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_b_解析出总量与已用() {
        let out =
            "               total        used        free      shared  buff/cache   available\n\
                   Mem:     4102369280  2214592512  1073741824      12288   814034944  1789569024\n\
                   Swap:             0           0           0\n";
        assert_eq!(parse_free(out), Some((4_102_369_280, 2_214_592_512)));
    }

    #[test]
    fn free_认不出来就说认不出来_不猜() {
        assert_eq!(parse_free("bash: free: command not found"), None);
        assert_eq!(parse_free("Mem: 只有两列"), None);
    }

    #[test]
    fn df_b1_解析出总量与已用() {
        let out = "Filesystem      1B-blocks       Used   Available Use% Mounted on\n\
                   /dev/vda1     64377495552 8482488320 52603711488  14% /\n";
        assert_eq!(parse_df(out), Some((64_377_495_552, 8_482_488_320)));
    }

    #[test]
    fn docker_stats_是_1024_进制() {
        let out = "hunter-web-1\t1.23%\t135.2MiB / 3.822GiB\n\
                   hunter-redis-1\t0.15%\t9.5MiB / 3.822GiB\n";
        let m = parse_stats(out);
        let web = m.get("hunter-web-1").expect("web");
        assert!((web.0 - 1.23).abs() < 0.001);
        assert_eq!(web.1, (135.2 * 1024.0 * 1024.0) as u64);
        assert_eq!(web.2, Some((3.822 * 1024.0 * 1024.0 * 1024.0) as u64));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn docker_stats_一行坏了不拖累别的行() {
        let out = "hunter-web-1\t--\t--\nhunter-redis-1\t0.15%\t9.5MiB / 3.822GiB\n";
        let m = parse_stats(out);
        assert!(!m.contains_key("hunter-web-1"), "看不懂的那一行就别进来");
        assert!(m.contains_key("hunter-redis-1"));
    }

    #[test]
    fn 卷大小按十进制_容器内存按二进制() {
        // docker 自己就是两套：system df 用 go-units 的 kB=1000，stats 用 MiB=1024。
        // 我们照它的来，界面上的数字才和用户敲命令看到的一致
        assert_eq!(
            crate::compose::parse_human_size("312.4MB"),
            Some(312_400_000)
        );
        assert_eq!(
            crate::compose::parse_human_size("4.003GB"),
            Some(4_003_000_000)
        );
        assert_eq!(crate::compose::parse_human_size("0B"), Some(0));
        assert_eq!(crate::compose::parse_human_size("看不懂"), None);
        assert_eq!(parse_binary_size("1MiB"), Some(1_048_576));
    }

    #[test]
    fn 用户自己的_docker_下运行环境那一层不适用() {
        // 测试机上没有内置运行时 → applicable 必须是 false，而且有一句说得清的原因。
        // **不能悄悄给一组 0** —— 那就是编数字
        let m = runtime();
        if !m.applicable {
            assert!(!m.reason.is_empty(), "不适用就必须说清为什么");
            assert!(m.mem_used_bytes.is_none());
            assert!(m.disk_total_bytes.is_none());
        }
    }

    #[test]
    fn 这台电脑那一层至少给得出核数与内存() {
        let m = host();
        assert!(m.cpu_cores.unwrap_or(0) >= 1, "{m:?}");
        assert!(m.mem_total_bytes.unwrap_or(0) > 0, "{m:?}");
        // 拿不到的每一项都必须配一句原因
        if m.disk_total_bytes.is_none() {
            assert!(m.reasons.contains_key("disk"));
        }
        if m.mem_pressure.is_none() {
            assert!(m.reasons.contains_key("memPressure"));
        }
    }

    // ── I13 · R7 ────────────────────────────────────────────────────────

    #[test]
    fn 重启次数看的是窗口内的增量不是累计值() {
        let now = 1_800_000_000i64;
        // 一台跑了半年的机器：累计 200 次，但这一小时里一次都没重启
        let quiet = [(now - 3000, 200u32), (now - 100, 200)];
        assert_eq!(restarts_in_window(&quiet, now, 3600), 0);
        // 这一小时里从 200 涨到 204 → 4 次
        let busy = [(now - 3000, 200u32), (now - 600, 202), (now - 60, 204)];
        assert_eq!(restarts_in_window(&busy, now, 3600), 4);
        // 窗口外的那一条不算
        assert_eq!(restarts_in_window(&busy, now, 300), 0);
        // 一条采样都没有时是 0，不是 panic
        assert_eq!(restarts_in_window(&[], now, 3600), 0);
    }

    #[test]
    fn 采样只留窗口内的不会无限长() {
        let mut st = State::default();
        let mut now = 1_800_000_000i64;
        for i in 0..50u32 {
            let sv = vec![ServiceUsage {
                service: "api".into(),
                restart_count: Some(i),
                ..Default::default()
            }];
            push_restart_samples(&mut st, &sv, now, 3600);
            now += 600;
        }
        let v = st.restarts.get("api").expect("有采样");
        assert!(v.len() <= 13, "窗口两倍之外的要丢掉，实际 {}", v.len());
        // 计数没变的时候只更新时间戳，不新增一条。
        // 先空推一次让 retain 先把窗口外的丢掉，再看后面几次会不会把表撑长
        let same = |st: &mut State, now: i64| {
            push_restart_samples(
                st,
                &[ServiceUsage {
                    service: "api".into(),
                    restart_count: Some(49),
                    ..Default::default()
                }],
                now,
                3600,
            )
        };
        same(&mut st, now);
        let before = st.restarts["api"].len();
        for _ in 0..5 {
            same(&mut st, now);
        }
        assert_eq!(st.restarts["api"].len(), before, "计数没变不该新增采样");
        assert_eq!(
            st.restarts["api"].last().map(|x| x.1),
            Some(49),
            "最后那一条的计数应该还是 49"
        );
    }

    #[test]
    fn 状态文件落在工作目录里且读坏了不崩() {
        let _h = crate::paths::test_home("monitor-state");
        assert!(state_path().starts_with(crate::paths::root()));
        std::fs::write(state_path(), "{ 这不是 JSON").unwrap();
        let st = load_state();
        assert!(st.restarts.is_empty(), "读坏了就用默认值，不该崩");
        let mut st2 = State::default();
        st2.notified.insert("host-disk:Warn".into(), 42);
        save_state(&st2);
        assert_eq!(load_state().notified.get("host-disk:Warn"), Some(&42));
    }

    #[test]
    fn 交给_ai_的快照里没有路径没有_key() {
        let a = Alerts {
            at: "2026-09-23 10:00:00".into(),
            ..Default::default()
        };
        let lines = snapshot_for_ai(&a);
        assert!(!lines.is_empty());
        let joined = lines.join("\n");
        assert!(!joined.contains("hunt_tools_"), "{joined}");
        assert!(
            !joined.contains("/home/") && !joined.contains("/Users/"),
            "快照里不该出现家目录：{joined}"
        );
    }

    #[test]
    fn 跑一遍体检不会崩而且每条提醒都有实测数字() {
        let _h = crate::paths::test_home("monitor-alerts");
        let a = alerts();
        assert!(!a.at.is_empty());
        for x in &a.alerts {
            assert!(!x.id.is_empty());
            assert!(!x.title.is_empty());
            assert!(!x.detail.is_empty());
            assert!(
                !x.facts.is_empty(),
                "每一条提醒都要摆出它凭什么这么说：{x:?}"
            );
            assert_ne!(x.level, Level::Ok, "Ok 不该出现在提醒列表里");
        }
        // 采不到的那几层要有原因，不能默默当成「没事」
        for (k, v) in &a.reasons {
            assert!(!v.is_empty(), "{k} 没给原因");
        }
    }

    #[test]
    fn 备份目录所在盘查得出来() {
        let _h = crate::paths::test_home("monitor-disk-for");
        // 目录还不存在时也要能问出它将来落在哪块盘上
        let d = crate::paths::root().join("还没建出来的备份目录");
        assert!(!d.exists());
        let r = disk_for(&d);
        assert!(r.is_some(), "往上找到第一个存在的祖先就行");
    }
}
