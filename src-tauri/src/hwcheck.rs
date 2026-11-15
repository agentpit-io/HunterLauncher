//! 安装前的电脑硬件环境体检与三档分级提醒（I23）。
//!
//! ## 为什么要前置硬件体检
//!
//! 用户的真机反馈：在 8 GB 小内存笔记本上安装 Hunter 时，同时拉起 6 个后台服务
//! （`web` / `api` / `atomcode` / `llm-shim` / `postgres` / `redis`）加上宿主机系统与浏览器占用，
//! 容易因为可用内存不足而拉不动，事先却没有任何配置检查与分级提醒。
//!
//! 本模块在欢迎页与诊断包中对**电脑总内存、当前空闲内存、处理器核心数、可用磁盘空间、
//! 系统版本与容器引擎分配内存**做一次实测体检，给出三档直观结论：
//!
//! * [`HwLevel::Good`]（绿灯）：配置充裕，可以流畅运行；
//! * [`HwLevel::Tight`]（黄灯）：内存或配置偏紧（如 8 GB / 12 GB 笔记本），启动器会自动开启
//!   小内存减负，同时提醒关闭占内存的大型软件或推荐换用 16 GB 以上的台式机；
//! * [`HwLevel::Insufficient`]（红灯）：当前可用资源明显不足，直接安装容易卡死。
//!
//! ## 数字诚实性红线
//!
//! 所有数值全部来自真实探测（`sysinfo` 与 `docker info`），读不到就是 `None` + 原因说明，
//! **绝不编造任何假数字**。分级判据抽成纯函数 [`evaluate`]，由单元测试逐档钉死。

use std::time::Duration;

use serde::Serialize;

use crate::runtime::disk;

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

/// 8 GB 机器上报的物理总内存常见在 7.7–7.9 GiB 之间，因此「满 8 GB 档」的下限取 7.5 GiB。
pub const MEM_MIN_BYTES: u64 = 7 * GIB + 512 * MIB;
/// 16 GB 机器上报的物理总内存常见在 15.4–15.9 GiB 之间，因此「满 16 GB 充裕档」的下限取 15.0 GiB。
pub const MEM_GOOD_BYTES: u64 = 15 * GIB;

/// 未运行 Hunter 时，当前空闲内存的最低红线（2.2 GiB）。
pub const AVAIL_MIN_BYTES: u64 = 2 * GIB + 205 * MIB;
/// 未运行 Hunter 时，当前空闲内存的充裕线（4.0 GiB）。
pub const AVAIL_GOOD_BYTES: u64 = 4 * GIB;
/// 若本机 Hunter 6 个容器已经在运行，它们本身已占约 1.5–2 GiB，空闲内存阈值相应放宽 1.5 GiB。
pub const AVAIL_RUNNING_RELIEF_BYTES: u64 = GIB + 512 * MIB;

/// 磁盘空间的充裕线（25 GiB；最低线沿用 `runtime::disk::NEED_BYTES` = 15 GiB）。
pub const DISK_GOOD_BYTES: u64 = 25 * GIB;

/// Docker 守护进程分配给容器的最低合理内存（2.6 GiB）。
pub const DOCKER_MEM_TIGHT_BYTES: u64 = 2 * GIB + 615 * MIB;

/// 三档硬件评级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HwLevel {
    /// 绿灯：配置充裕，可流畅运行
    Good,
    /// 黄灯：配置偏紧/小内存笔记本，能跑但需提醒关闭大型软件或推荐用台式机
    Tight,
    /// 红灯：硬件明显不足，强行安装容易卡死或启动失败
    Insufficient,
}

impl HwLevel {
    pub fn cn(self) -> &'static str {
        match self {
            HwLevel::Good => "充裕",
            HwLevel::Tight => "偏紧",
            HwLevel::Insufficient => "不足",
        }
    }
}

/// 单项检查结果。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HwItem {
    /// `"memory"` / `"avail_memory"` / `"cpu"` / `"disk"` / `"os_virt"`
    pub id: String,
    /// 显示名称（如 `"电脑总内存"`）
    pub label: String,
    /// 实测人类可读值；读不到为 `None`
    pub value: Option<String>,
    pub level: HwLevel,
    /// 一句话大白话说明（无任何终端命令）
    pub note: String,
}

/// 完整硬件体检报告。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HwReport {
    /// 所有单项中最严的一档
    pub level: HwLevel,
    /// 总结标题
    pub headline: String,
    /// 大白话建议
    pub summary: String,
    pub total_mem_bytes: Option<u64>,
    pub avail_mem_bytes: Option<u64>,
    pub cpu_cores: Option<usize>,
    pub disk_free_bytes: Option<u64>,
    pub disk_mount: Option<String>,
    pub docker_mem_bytes: Option<u64>,
    pub items: Vec<HwItem>,
}

/// 喂给纯函数 [`evaluate`] 的实测原始输入。
#[derive(Debug, Clone)]
pub struct RawHwInput {
    pub total_mem_bytes: Option<u64>,
    pub avail_mem_bytes: Option<u64>,
    pub cpu_cores: Option<usize>,
    pub arch: String,
    pub disk_free_bytes: Option<u64>,
    pub disk_mount: Option<String>,
    pub os_name: String,
    pub macos_major: Option<u32>,
    pub docker_mem_bytes: Option<u64>,
    pub hunter_running: bool,
}

/// 把字节数格式化为保留 1 位小数的 GB（按 GiB 1024³ 折算，与系统内存条标称一致）。
pub fn fmt_gib_1(bytes: u64) -> String {
    let g = (bytes as f64) / (GIB as f64);
    format!("{g:.1} GB")
}

/// 把字节数格式化为整数 GB（向下取整，给磁盘余量显示用）。
pub fn fmt_gib_int(bytes: u64) -> String {
    format!("{} GB", bytes / GIB)
}

/// 纯函数分级判据：输入实测硬件指标，输出 [`HwReport`]。
pub fn evaluate(inp: &RawHwInput) -> HwReport {
    let mut items = Vec::with_capacity(5);

    // 1. 电脑总内存
    let mem_item = match inp.total_mem_bytes {
        Some(b) if b < MEM_MIN_BYTES => HwItem {
            id: "memory".into(),
            label: "电脑总内存".into(),
            value: Some(fmt_gib_1(b)),
            level: HwLevel::Insufficient,
            note: "总内存不足 8 GB，同时运行 6 个服务会严重卡顿，建议换用 16 GB 以上的台式机或主力电脑".into(),
        },
        Some(b) if b < MEM_GOOD_BYTES => HwItem {
            id: "memory".into(),
            label: "电脑总内存".into(),
            value: Some(fmt_gib_1(b)),
            level: HwLevel::Tight,
            note: "总内存偏小（推荐 16 GB 及以上），启动器会自动为您调低运行环境内存占用，建议安装时关掉其他大型软件".into(),
        },
        Some(b) => HwItem {
            id: "memory".into(),
            label: "电脑总内存".into(),
            value: Some(fmt_gib_1(b)),
            level: HwLevel::Good,
            note: "内存充足（推荐 16 GB 及以上）".into(),
        },
        None => HwItem {
            id: "memory".into(),
            label: "电脑总内存".into(),
            value: None,
            level: HwLevel::Tight,
            note: "未能读取到系统总内存数值".into(),
        },
    };
    items.push(mem_item);

    // 2. 当前空闲内存
    let relief = if inp.hunter_running {
        AVAIL_RUNNING_RELIEF_BYTES
    } else {
        0
    };
    let avail_min = AVAIL_MIN_BYTES.saturating_sub(relief);
    let avail_good = AVAIL_GOOD_BYTES.saturating_sub(relief);
    let avail_item = match inp.avail_mem_bytes {
        Some(b) if b < avail_min => HwItem {
            id: "avail_memory".into(),
            label: "当前空闲内存".into(),
            value: Some(fmt_gib_1(b)),
            level: HwLevel::Insufficient,
            note: "当前剩余可用内存不到 2.2 GB，请先关掉浏览器多余标签或大型软件后再试".into(),
        },
        Some(b) if b < avail_good => HwItem {
            id: "avail_memory".into(),
            label: "当前空闲内存".into(),
            value: Some(fmt_gib_1(b)),
            level: HwLevel::Tight,
            note: "当前空闲内存偏少，建议先关掉几个不用的软件，避免下载解压和启动时卡顿".into(),
        },
        Some(b) => HwItem {
            id: "avail_memory".into(),
            label: "当前空闲内存".into(),
            value: Some(fmt_gib_1(b)),
            level: HwLevel::Good,
            note: "当前空闲内存充裕".into(),
        },
        None => HwItem {
            id: "avail_memory".into(),
            label: "当前空闲内存".into(),
            value: None,
            level: HwLevel::Tight,
            note: "未能读取到当前空闲内存数值".into(),
        },
    };
    items.push(avail_item);

    // 3. 处理器核心
    let cpu_item = match inp.cpu_cores {
        Some(c) if c < 2 => HwItem {
            id: "cpu".into(),
            label: "处理器核心".into(),
            value: Some(format!("{c} 核 ({})", inp.arch)),
            level: HwLevel::Insufficient,
            note: "处理器核心数不足 2 核".into(),
        },
        Some(c) if c < 4 => HwItem {
            id: "cpu".into(),
            label: "处理器核心".into(),
            value: Some(format!("{c} 核 ({})", inp.arch)),
            level: HwLevel::Tight,
            note: "处理器核心较少，首次解压与启动可能需要多等几分钟".into(),
        },
        Some(c) => HwItem {
            id: "cpu".into(),
            label: "处理器核心".into(),
            value: Some(format!("{c} 核 ({})", inp.arch)),
            level: HwLevel::Good,
            note: "处理器核心充足".into(),
        },
        None => HwItem {
            id: "cpu".into(),
            label: "处理器核心".into(),
            value: None,
            level: HwLevel::Tight,
            note: "未能读取到处理器核心数".into(),
        },
    };
    items.push(cpu_item);

    // 4. 可用磁盘空间
    let disk_val = inp
        .disk_free_bytes
        .map(|free| match inp.disk_mount.as_deref() {
            Some(m) if !m.is_empty() => format!("{m} 剩余 {}", fmt_gib_int(free)),
            _ => format!("剩余 {}", fmt_gib_int(free)),
        });
    let disk_item = match inp.disk_free_bytes {
        Some(free) if free < disk::NEED_BYTES => HwItem {
            id: "disk".into(),
            label: "可用磁盘空间".into(),
            value: disk_val,
            level: HwLevel::Insufficient,
            note: "可用磁盘空间不足 15 GB，装完 6 个服务后空间会见底".into(),
        },
        Some(free) if free < DISK_GOOD_BYTES => HwItem {
            id: "disk".into(),
            label: "可用磁盘空间".into(),
            value: disk_val,
            level: HwLevel::Tight,
            note: "磁盘剩余空间刚好够用，建议预留 25 GB 以上".into(),
        },
        Some(_) => HwItem {
            id: "disk".into(),
            label: "可用磁盘空间".into(),
            value: disk_val,
            level: HwLevel::Good,
            note: "磁盘空间充裕".into(),
        },
        None => HwItem {
            id: "disk".into(),
            label: "可用磁盘空间".into(),
            value: None,
            level: HwLevel::Tight,
            note: "未能读取到目标磁盘的剩余空间".into(),
        },
    };
    items.push(disk_item);

    // 5. 系统与容器内存分配
    let os_item = if inp.os_name == "macos" && inp.macos_major.is_some_and(|v| v < 13) {
        let ver = inp.macos_major.unwrap_or(0);
        HwItem {
            id: "os_virt".into(),
            label: "系统与虚拟化".into(),
            value: Some(format!("macOS {ver}")),
            level: HwLevel::Insufficient,
            note: "系统版本低于 macOS 13，无法使用自带的轻量虚拟化框架".into(),
        }
    } else if let Some(dm) = inp.docker_mem_bytes {
        let val = format!(
            "{} · 容器可用 {}",
            os_display(&inp.os_name, inp.macos_major),
            fmt_gib_1(dm)
        );
        if dm < DOCKER_MEM_TIGHT_BYTES {
            HwItem {
                id: "os_virt".into(),
                label: "系统与虚拟化".into(),
                value: Some(val),
                level: HwLevel::Tight,
                note: "当前 Docker 分配给容器的内存不足 3 GB，6 个服务同时运行可能会因为内存不够而退出".into(),
            }
        } else {
            HwItem {
                id: "os_virt".into(),
                label: "系统与虚拟化".into(),
                value: Some(val),
                level: HwLevel::Good,
                note: "系统与运行环境内存分配正常".into(),
            }
        }
    } else {
        HwItem {
            id: "os_virt".into(),
            label: "系统与虚拟化".into(),
            value: Some(os_display(&inp.os_name, inp.macos_major)),
            level: HwLevel::Good,
            note: "系统环境支持自动配置运行环境".into(),
        }
    };
    items.push(os_item);

    let level = items.iter().map(|i| i.level).max().unwrap_or(HwLevel::Good);

    let headline = match level {
        HwLevel::Good => "这台电脑配置充裕，可以流畅运行 Hunter".to_string(),
        HwLevel::Tight => "这台电脑内存或配置偏紧，运行时可能会变慢".to_string(),
        HwLevel::Insufficient => "这台电脑当前可用资源不足，直接安装容易卡住".to_string(),
    };

    let summary = match level {
        HwLevel::Good => "各项硬件指标均满足推荐要求，可以直接开始安装。".to_string(),
        HwLevel::Tight => "Hunter 需要同时运行 6 个后台服务（网页、数据库、对话引擎等），推荐 16 GB 及以上内存。当前电脑若继续安装，启动器会自动为您开启小内存减负模式；建议先关掉浏览器多余标签和占内存的大型软件，如果您有内存更大的台式机，更推荐在台式机上安装。".to_string(),
        HwLevel::Insufficient => {
            let bad_labels: Vec<&str> = items
                .iter()
                .filter(|i| i.level == HwLevel::Insufficient)
                .map(|i| i.label.as_str())
                .collect();
            format!(
                "检测到「{}」未达到流畅运行最低要求。建议先关闭其他占内存或占空间的软件后点「重新检测」，或换用 16 GB 及以上内存的台式机安装。",
                bad_labels.join("、")
            )
        }
    };

    HwReport {
        level,
        headline,
        summary,
        total_mem_bytes: inp.total_mem_bytes,
        avail_mem_bytes: inp.avail_mem_bytes,
        cpu_cores: inp.cpu_cores,
        disk_free_bytes: inp.disk_free_bytes,
        disk_mount: inp.disk_mount.clone(),
        docker_mem_bytes: inp.docker_mem_bytes,
        items,
    }
}

fn os_display(os_name: &str, macos_major: Option<u32>) -> String {
    match (os_name, macos_major) {
        ("macos", Some(v)) => format!("macOS {v}"),
        ("macos", None) => "macOS".into(),
        ("windows", _) => "Windows".into(),
        ("linux", _) => "Linux".into(),
        (other, _) => other.to_string(),
    }
}

/// 采集这台电脑的真实硬件指标并生成 [`HwReport`]。
pub fn check() -> HwReport {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());

    let total_raw = sys.total_memory();
    let total_mem_bytes = (total_raw > 0).then_some(total_raw);
    let avail_raw = sys.available_memory();
    let avail_mem_bytes = if avail_raw > 0 {
        Some(avail_raw)
    } else if total_raw > 0 {
        Some(total_raw.saturating_sub(sys.used_memory()))
    } else {
        None
    };

    let cores_raw = sys.cpus().len();
    let cpu_cores = if cores_raw > 0 {
        Some(cores_raw)
    } else {
        std::thread::available_parallelism().ok().map(|n| n.get())
    };

    let (disk_free_bytes, disk_mount) = probe_target_disk();
    let macos_major = probe_macos_major();
    let docker_mem_bytes = probe_docker_mem_bytes();
    let hunter_running = crate::compose::is_up();

    let inp = RawHwInput {
        total_mem_bytes,
        avail_mem_bytes,
        cpu_cores,
        arch: std::env::consts::ARCH.to_string(),
        disk_free_bytes,
        disk_mount,
        os_name: std::env::consts::OS.to_string(),
        macos_major,
        docker_mem_bytes,
        hunter_running,
    };
    evaluate(&inp)
}

/// 诊断包 `summary` 节用的一行硬件摘要。
pub fn one_line_summary() -> String {
    let r = check();
    let cores = r
        .cpu_cores
        .map(|c| c.to_string())
        .unwrap_or_else(|| "—".into());
    let total = r
        .total_mem_bytes
        .map(fmt_gib_1)
        .unwrap_or_else(|| "—".into());
    let avail = r
        .avail_mem_bytes
        .map(fmt_gib_1)
        .unwrap_or_else(|| "—".into());
    format!(
        "硬件: CPU {} 核 · 总内存 {} · 当前可用 {} · 评级 {}",
        cores,
        total,
        avail,
        r.level.cn()
    )
}

/// 优先看 `runtime::disk::volumes()` 里的推荐非系统盘；若没有则看 `runtime_dir` 所在盘或家目录所在盘。
fn probe_target_disk() -> (Option<u64>, Option<String>) {
    let vols = disk::volumes();
    if let Some(picked) = disk::pick(&vols) {
        return (Some(picked.free), Some(picked.mount.clone()));
    }
    if let Some((mount, free, _)) = crate::monitor::disk_for(&crate::paths::runtime_dir()) {
        return (Some(free), Some(mount));
    }
    if let Some((mount, free, _)) = crate::monitor::disk_for(&crate::paths::home()) {
        return (Some(free), Some(mount));
    }
    if let Some(first) = vols.first() {
        return (Some(first.free), Some(first.mount.clone()));
    }
    (None, None)
}

fn probe_macos_major() -> Option<u32> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let r = crate::proc::run_timeout(
        "/usr/bin/sw_vers",
        &["-productVersion"],
        Duration::from_secs(5),
    )
    .ok()?;
    if !r.ok() {
        return None;
    }
    r.stdout
        .trim()
        .split('.')
        .next()?
        .trim()
        .parse::<u32>()
        .ok()
}

/// 如果 Docker 已经在运行，顺手问一下它分到了多少总内存（`docker info --format '{{.MemTotal}}'`）。
/// 没装或没起时快速返回 `None`，不阻塞欢迎页。
fn probe_docker_mem_bytes() -> Option<u64> {
    let bin = crate::runtime::which::docker_probe().resolved?;
    let r = crate::proc::run_timeout(
        &bin,
        &["info", "--format", "{{.MemTotal}}"],
        Duration::from_secs(3),
    )
    .ok()?;
    if !r.ok() {
        return None;
    }
    let b = r.stdout.trim().parse::<u64>().ok()?;
    (b > 0).then_some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_good() -> RawHwInput {
        RawHwInput {
            total_mem_bytes: Some(16 * GIB),
            avail_mem_bytes: Some(9 * GIB),
            cpu_cores: Some(8),
            arch: "aarch64".into(),
            disk_free_bytes: Some(120 * GIB),
            disk_mount: Some("/".into()),
            os_name: "macos".into(),
            macos_major: Some(14),
            docker_mem_bytes: None,
            hunter_running: false,
        }
    }

    #[test]
    fn 十六_gb_充裕机器返回绿灯() {
        let r = evaluate(&sample_good());
        assert_eq!(r.level, HwLevel::Good);
        assert!(r.headline.contains("充裕"), "{}", r.headline);
        assert_eq!(r.items.len(), 5);
        assert!(r.items.iter().all(|i| i.level == HwLevel::Good));
    }

    #[test]
    fn 八_gb_笔记本返回黄灯并给出台式机与减负提醒() {
        let mut inp = sample_good();
        // 8GB 笔记本物理内存通常上报为 7.8 GiB 左右
        inp.total_mem_bytes = Some(7 * GIB + 800 * MIB);
        inp.avail_mem_bytes = Some(3 * GIB);
        let r = evaluate(&inp);
        assert_eq!(r.level, HwLevel::Tight);
        assert!(r.headline.contains("偏紧"), "{}", r.headline);
        assert!(r.summary.contains("台式机"), "{}", r.summary);
        let mem = r.items.iter().find(|i| i.id == "memory").unwrap();
        assert_eq!(mem.level, HwLevel::Tight);
        assert!(mem.note.contains("自动为您调低"), "{}", mem.note);
    }

    #[test]
    fn 四_gb_小内存或空闲内存不足_2_2_gb_返回红灯() {
        let mut inp = sample_good();
        inp.total_mem_bytes = Some(4 * GIB);
        let r = evaluate(&inp);
        assert_eq!(r.level, HwLevel::Insufficient);
        assert!(r.summary.contains("电脑总内存"), "{}", r.summary);

        // 总内存够（16GB），但当前被别的软件吃光，只剩 1.8 GiB 空闲内存
        let mut inp2 = sample_good();
        inp2.avail_mem_bytes = Some(GIB + 800 * MIB);
        let r2 = evaluate(&inp2);
        assert_eq!(r2.level, HwLevel::Insufficient);
        let avail = r2.items.iter().find(|i| i.id == "avail_memory").unwrap();
        assert_eq!(avail.level, HwLevel::Insufficient);
    }

    #[test]
    fn 磁盘空间不足_15_gb_返回红灯_15_到_25_gb_返回黄灯() {
        let mut inp = sample_good();
        inp.disk_free_bytes = Some(12 * GIB);
        let r = evaluate(&inp);
        assert_eq!(r.level, HwLevel::Insufficient);

        inp.disk_free_bytes = Some(20 * GIB);
        let r2 = evaluate(&inp);
        assert_eq!(r2.level, HwLevel::Tight);
    }

    #[test]
    fn macos_低于_13_或_docker_内存过小会被识别() {
        let mut inp = sample_good();
        inp.macos_major = Some(12);
        let r = evaluate(&inp);
        assert_eq!(r.level, HwLevel::Insufficient);

        let mut inp2 = sample_good();
        inp2.docker_mem_bytes = Some(2 * GIB);
        let r2 = evaluate(&inp2);
        assert_eq!(r2.level, HwLevel::Tight);
        let os_item = r2.items.iter().find(|i| i.id == "os_virt").unwrap();
        assert_eq!(os_item.level, HwLevel::Tight);
    }

    #[test]
    fn 本机_check_真实探测不崩溃且不编假数据() {
        let r = check();
        assert_eq!(r.items.len(), 5);
        assert!(!r.headline.is_empty());
        let line = one_line_summary();
        assert!(line.starts_with("硬件: CPU "), "{line}");
    }
}
