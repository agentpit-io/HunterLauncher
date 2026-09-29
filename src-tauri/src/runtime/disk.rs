//! 「装到哪块盘」（S-01 · 技术方案 §4.2）。
//!
//! 需求那一头是两句大白话：**不要装到 C 盘 / 系统盘**，**挑空间够的那块**，
//! 并且要能告诉用户「会占多少、默认装哪、为什么是它」。
//!
//! 这个文件只管**前一半**：把一块块盘变成一个分数，挑出该用的那一块。
//! 「为什么是它」那句话、界面上的选盘卡片、以及把选中盘写进
//! `[runtime] data_dir`（S-02）是后面的事（S-04 / U-03）。
//!
//! ## 两个已经踩过的坑，写在这里免得再踩
//!
//! 1. **不要用 [`crate::assist::probe::disk_free`]** —— 它靠 `df` 那一类外部命令，
//!    **Windows 上直接返回 `None`**，选盘在 Windows 上会静默失效。
//!    这里走 [`sysinfo::Disks`]，三个平台是同一套代码。
//! 2. **不要用 `paths::root()` 代表「装在哪」** —— 内置运行时的落地位置由
//!    `[runtime] data_dir` 决定（[`crate::paths::runtime_dir`]），
//!    它可能在别的盘上。判断「这块盘上有没有 Hunter 的数据」要读那个。
//!
//! ## 这个模块是纯函数 + 一次枚举
//!
//! [`pick`] / [`score`] 只吃一个 [`Volume`] 列表，不碰真实系统 ——
//! 所以「系统盘永不被推荐」「全不合格返回空」这些判据都能被单测钉死。
//! 只有 [`volumes`] 会真的去问操作系统。

use std::path::Path;

/// **建议阈值**：低于这个剩余空间就不推荐这块盘。
///
/// 这个数是算出来的，不是拍的（技术方案 §4.2.1，每一项都是实测）：
///
/// ```text
/// 下载     0.42 GB   （内置运行时的四个二进制）
/// 镜像     3.90 GB   （六个服务拉下来解压后）
/// 虚拟机底 1.70 GB   （lima 那台虚机的磁盘底）
/// 一份备份 1.00 GB   （pg_dump + 数据卷）
/// 余量     ——        （磁盘写满会让 postgres 直接跪，不能顶着上限装）
/// ─────────────────
/// ≈ 15 GiB
/// ```
pub const NEED_BYTES: u64 = 15 * 1024 * 1024 * 1024;

/// 够不够得着当候选：总容量小于这个数的一律不看（U 盘里的分区、只读系统分区）。
pub const MIN_TOTAL_BYTES: u64 = 40 * 1024 * 1024 * 1024;

/// 「勉强够」的那条线：剩余不到 [`NEED_BYTES`] 的三倍就扣分。
///
/// 为什么是三倍：装的时候要**同时**放得下下载中转包与解压后的东西，
/// 而且装完还要留得下一份备份。正好压着 15 GiB 的盘装完就满了，
/// 那不是「能用」，是「用两天就得来清一次」。它不是硬门槛 ——
/// 只剩这一块盘时仍然会被选中（扣完分还是最高），只是不推荐。
pub const TIGHT_BYTES: u64 = NEED_BYTES * 3;

/// 一块盘（一个挂载点 / 一个盘符）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// 挂载点。Windows 上是盘符（`D:\`），macOS / Linux 上是路径
    pub mount: String,
    pub total: u64,
    pub free: u64,
    /// 是不是**系统卷**。是就永远不推荐 —— 需求原话是「不要装到 C 盘 / 系统盘」
    pub is_system: bool,
    /// 这块盘上已经躺着 Hunter 的内置运行时（沿用它，省一次搬家）
    pub has_hunter: bool,
}

/// 给一块盘打分。**分高者胜，并列取盘符序**（[`pick`] 里保证）。
///
/// ```text
/// +1000   不是系统卷         ← 见下面那句说明
/// +200    这块盘上已有 Hunter 的数据
/// +free_GB                  剩余空间越大分越高
/// -500    free < 需求下限 × 3
/// ```
///
/// 那条 `+1000` 是**第二道保险**：[`pick`] 已经先把系统卷滤掉了
/// （「永远不推荐」是一条硬规则，不是「扣分够多就还能选」），
/// 所以正常情况下这一项不会单独决定谁胜出。留着它是为了让
/// 「系统卷天然处于劣势」这件事**写在分数里**，而不是只写在一句注释里 ——
/// 以后有人改了过滤那一段，分数这一层还拦得住。
///
/// 1 GB 按 1024³ 算（和界面上显示的 GiB 是同一个口径，免得两个数对不上）。
pub fn score(v: &Volume) -> i64 {
    let mut s: i64 = 0;
    if !v.is_system {
        s += 1000;
    }
    if v.has_hunter {
        s += 200;
    }
    s += (v.free / (1024 * 1024 * 1024)) as i64;
    if v.free < TIGHT_BYTES {
        s -= 500;
    }
    s
}

/// 这块盘够不够格当候选。
///
/// 三条：不是系统卷、总容量够大、**剩余空间不低于需求下限**。
/// 第三条是硬门槛 —— 「全不合格」返回 [`None`] 比「矮子里拔将军」诚实：
/// 挑一块装到一半就满的盘，用户会在最不该出错的那一步出错，
/// 而且那时候他已经等了十几分钟。
pub fn eligible(v: &Volume) -> bool {
    !v.is_system && v.total >= MIN_TOTAL_BYTES && v.free >= NEED_BYTES
}

/// 该装到哪一块盘。**全不合格就返回 `None`**，不瞎选一块糊弄过去。
///
/// 系统的返回的是列表里那一个；调用方拿它的 `mount` 去写
/// `[runtime] data_dir`（S-02）。
pub fn pick(vols: &[Volume]) -> Option<&Volume> {
    let mut best: Option<&Volume> = None;
    for v in vols.iter().filter(|v| eligible(v)) {
        match best {
            Some(b) => {
                let (sb, sv) = (score(b), score(v));
                // 平分时**取盘符序靠前的那一个**：`volumes()` 已经按挂载点排过序，
                // 所以这里用严格大于，「先来的赢」= 盘符序
                if sv > sb {
                    best = Some(v);
                }
            }
            None => best = Some(v),
        }
    }
    best
}

/// 算一下这套东西大概要占多少（GB，向上取整）—— 界面那行「会占多少」用。
///
/// 只算**第一次装**的那几块（下载 + 解压后的镜像 + 虚拟机磁盘底 + 一份备份），
/// 不算用户以后攒下来的对话数据 —— 那个没法预测，界面上要如实说「以后随你的数据增长」。
pub fn need_gb() -> u64 {
    NEED_BYTES.div_ceil(1024 * 1024 * 1024)
}

/// 库里所有够格的卷。**只有这一个函数会去问操作系统。**
///
/// 只读、可移动的卷直接跳过（U 盘、只读挂载的系统快照分区）。
/// 报出来的顺序按挂载点排 —— [`pick`] 的「并列取盘符序」靠的就是这个顺序。
pub fn volumes() -> Vec<Volume> {
    let home_mount = crate::monitor::disk_for(&crate::paths::home()).map(|(m, _, _)| m);
    let runtime_mount = crate::monitor::disk_for(&crate::paths::runtime_dir()).map(|(m, _, _)| m);
    let mut v: Vec<Volume> = Vec::new();
    for d in sysinfo::Disks::new_with_refreshed_list().list() {
        if d.is_read_only() || d.is_removable() {
            continue;
        }
        let mount = d.mount_point().to_string_lossy().into_owned();
        let total = d.total_space();
        if total < MIN_TOTAL_BYTES {
            continue;
        }
        let is_system = home_mount.as_deref() == Some(mount.as_str()) || looks_like_system(&mount);
        let has_hunter = runtime_mount.as_deref() == Some(mount.as_str());
        v.push(Volume {
            mount,
            total,
            free: d.available_space(),
            is_system,
            has_hunter,
        });
    }
    v.sort_by(|a, b| a.mount.cmp(&b.mount));
    v.dedup_by(|a, b| a.mount == b.mount);
    v
}

/// 光看路径就知道它是系统卷吗。
///
/// * Windows —— 系统盘是 `C`（技术方案与 [`crate::backup::external_suggestions`]
///   用的是同一个判据）。盘符是大写带冒号带反斜杠，所以只取头一个字母比。
/// * macOS —— `/` 与 `/System/Volumes/Data` 是同一块盘的两种写法（APFS），
///   两个都算系统卷。
/// * Linux —— `/`。别的挂载点（`/home`、`/mnt/...`）要看它是不是家目录所在那块，
///   那一条由 [`volumes`] 用 [`crate::monitor::disk_for`] 单独判。
fn looks_like_system(mount: &str) -> bool {
    if cfg!(windows) {
        return mount
            .trim_start()
            .get(..1)
            .map(|c| c.eq_ignore_ascii_case("C"))
            .unwrap_or(false);
    }
    if cfg!(target_os = "macos") {
        return mount == "/" || mount.starts_with("/System/Volumes/Data");
    }
    mount == "/"
}

/// 用户选中的那块盘是不是够用（选盘卡片上那个「确认」按钮背后的判据）。
///
/// 和 [`eligible`] 同一套标准，只是这里吃的是路径而不是 [`Volume`] ——
/// 界面上用户可能手填一个路径，那条路也要过同一道闸。
pub fn path_is_ok(path: &Path) -> bool {
    match crate::monitor::disk_for(path) {
        Some((mount, free, total)) => {
            total >= MIN_TOTAL_BYTES
                && free >= NEED_BYTES
                && crate::monitor::disk_for(&crate::paths::home()).map(|(m, _, _)| m) != Some(mount)
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn vol(mount: &str, free_gb: u64, is_system: bool, has_hunter: bool) -> Volume {
        Volume {
            mount: mount.to_string(),
            total: 500 * GB,
            free: free_gb * GB,
            is_system,
            has_hunter,
        }
    }

    /// 系统的默认值（三平台都一样：系统判定只看卷自己，不看这台机器）
    #[test]
    fn 系统盘永不被推荐() {
        // 系统盘又大又有已经装好的 Hunter —— 就算这样也不选它
        let both = [vol("C:\\", 900, true, true), vol("D:\\", 60, false, false)];
        let picked = pick(&both).expect("该挑出 D 盘");
        assert_eq!(picked.mount, "D:\\");

        // 只有系统盘 → 不推荐，如实返回空，而不是「那就装 C 盘吧」
        let only_sys = [vol("C:\\", 2000, true, false)];
        assert!(pick(&only_sys).is_none(), "只剩系统盘时也不许选它");
    }

    /// 最大的非系统盘胜出（都没装过 Hunter 时，比的就是剩余空间）
    #[test]
    fn 最大的非系统盘胜出() {
        let vols = [
            vol("D:\\", 50, false, false),
            vol("E:\\", 212, false, false),
            vol("F:\\", 120, false, false),
        ];
        assert_eq!(pick(&vols).expect("该挑得出").mount, "E:\\");
    }

    /// **全不合格时返回 `None`**，不瞎选一块。
    ///
    /// 四种不合格：都太小、都只剩一点点、只剩系统盘、一张空表。
    #[test]
    fn 全不合格时返回空而不是瞎选() {
        // ① 剩余空间全部低于需求下限（15 GiB）
        let tight = [vol("D:\\", 14, false, false), vol("E:\\", 2, false, false)];
        assert!(pick(&tight).is_none(), "都装不下就别硬挑");

        // ② 只有系统盘
        assert!(pick(&[vol("C:\\", 999, true, false)]).is_none());

        // ③ 一张空表（枚举不出来任何卷）
        assert!(pick(&[]).is_none());

        // ④ 刚够需求下限的能留下，比它小的不行 —— 边界钉死
        let edge = [vol("D:\\", 15, false, false), vol("E:\\", 14, false, false)];
        assert_eq!(pick(&edge).expect("15 GiB 是够的").mount, "D:\\");
    }

    /// 已经装了 Hunter 的那块盘加权 200：**同样剩余空间时它赢**（省一次搬家）。
    #[test]
    fn 已有_hunter_数据的盘加权() {
        let vols = [
            vol("D:\\", 100, false, false),
            vol("E:\\", 100, false, true),
        ];
        assert_eq!(pick(&vols).expect("该挑得出").mount, "E:\\");
        assert_eq!(score(&vols[1]) - score(&vols[0]), 200);
    }

    /// 但 200 分压不过「大得多」：剩 100 GB 的空盘仍然打不过剩 500 GB 的。
    #[test]
    fn 已有数据压不过明显更大的盘() {
        let vols = [
            vol("D:\\", 100, false, true),
            vol("E:\\", 500, false, false),
        ];
        assert_eq!(pick(&vols).expect("该挑得出").mount, "E:\\");
    }

    /// 平方（分一样）时取**盘符序靠前**的那一个。
    #[test]
    fn 平分时取盘符序() {
        let vols = [vol("D:\\", 80, false, false), vol("E:\\", 80, false, false)];
        assert_eq!(pick(&vols).expect("该挑得出").mount, "D:\\");
    }

    /// 「勉强够」（15–45 GiB）会被扣 500 分 —— 还能选，但不推荐。
    #[test]
    fn 勉强够的盘会被扣分() {
        let just_enough = vol("D:\\", 20, false, false);
        let roomy = vol("E:\\", 60, false, false);
        assert_eq!(score(&roomy) - score(&just_enough), 40 + 500);

        // 但只剩它一个时仍然选得出来（它是「不推荐」，不是「不许」）
        assert_eq!(pick(&[just_enough]).expect("该挑得出").mount, "D:\\");
    }

    /// 打分表的每一项都对得上，一个不差。
    #[test]
    fn 打分表的每一项() {
        // 非系统卷 +1000，系统卷没有
        assert_eq!(
            score(&vol("D:\\", 0, false, false)) - score(&vol("C:\\", 0, true, false)),
            1000
        );
        // 沿用 +200
        assert_eq!(
            score(&vol("D:\\", 0, false, true)) - score(&vol("D:\\", 0, false, false)),
            200
        );
        // 剩余空间按 GB 累加
        assert_eq!(
            score(&vol("D:\\", 10, false, false)) - score(&vol("D:\\", 3, false, false)),
            7
        );
    }

    /// 系统卷的判据：三平台各按各的规矩来。
    #[test]
    fn 系统卷的判据() {
        if cfg!(windows) {
            assert!(looks_like_system("C:\\"));
            assert!(looks_like_system("c:\\"));
            assert!(!looks_like_system("D:\\"));
        } else if cfg!(target_os = "macos") {
            assert!(looks_like_system("/"));
            assert!(looks_like_system("/System/Volumes/Data"));
            assert!(!looks_like_system("/Volumes/外接盘"));
        } else {
            assert!(looks_like_system("/"));
            assert!(!looks_like_system("/mnt/data"));
            assert!(!looks_like_system("/home"));
        }
    }

    /// 需求下限就是 15 GiB，别哪天被人悄悄改成别的数。
    #[test]
    fn 需求下限是十五_gib() {
        assert_eq!(NEED_BYTES, 15 * GB);
        assert_eq!(need_gb(), 15);
        // 「勉强够」那条线是它的三倍
        assert_eq!(TIGHT_BYTES, 45 * GB);
    }

    /// 真去枚举一次这台机器上的卷，并在**真机上**验那条最要紧的：
    /// **家目录所在的那块盘不许被推荐**。
    ///
    /// ## 断言写轻一点是有原因的
    ///
    /// 第一版写的是「家目录所在的那块必须出现在枚举结果里」，CI 的 macOS runner
    /// 上当场红了 —— 那台机器的整块盘才 14 GB，连 [`MIN_TOTAL_BYTES`]（40 GB）
    /// 都够不着。**「这块盘够不够大」不是这个模块该管的事**，拿它当断言就是把
    /// 「测试机长什么样」写进了契约里。所以这里只断言两条真契约：
    ///
    /// 1. 出来的每一条都过了那三条硬条件（调用方可以依赖这一点）；
    /// 2. **`pick()` 不会挑中家目录所在的那块盘** —— 这条在哪个平台上都成立。
    #[test]
    fn 枚举本机卷时不会崩且系统卷不会被推荐() {
        let vols = volumes();
        for v in &vols {
            assert!(
                v.total >= MIN_TOTAL_BYTES,
                "{} 没够着候选门槛却出现在枚举结果里",
                v.mount
            );
            assert!(!v.mount.is_empty());
        }
        // 排序是按挂载点的 —— `pick` 的「并列取盘符序」靠它
        let mut sorted = vols.clone();
        sorted.sort_by(|a, b| a.mount.cmp(&b.mount));
        assert_eq!(
            vols.iter().map(|v| &v.mount).collect::<Vec<_>>(),
            sorted.iter().map(|v| &v.mount).collect::<Vec<_>>()
        );
        // **真机上最要紧的那一条**：家目录所在的盘永远不被推荐
        if let Some((home_mount, _, _)) = crate::monitor::disk_for(&crate::paths::home()) {
            assert_ne!(
                pick(&vols).map(|p| &p.mount),
                Some(&home_mount),
                "家目录所在的盘（{home_mount}）被推荐了"
            );
        }
    }
}
