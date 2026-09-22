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
///
/// 按**挂载点最长匹配**选盘：`/` 和 `/System/Volumes/Data` 同时列出来时，
/// 家目录真正落在后者上（macOS 的 APFS 布局），选错了给出来的剩余空间是另一个数。
fn home_disk() -> Option<(String, u64, u64)> {
    let home = crate::paths::home();
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
    out.services
        .sort_by_key(|s| crate::selfcheck::EXPECTED.iter().position(|e| *e == s.service));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_b_解析出总量与已用() {
        let out = "               total        used        free      shared  buff/cache   available\n\
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
        assert_eq!(crate::compose::parse_human_size("312.4MB"), Some(312_400_000));
        assert_eq!(crate::compose::parse_human_size("4.003GB"), Some(4_003_000_000));
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
}
