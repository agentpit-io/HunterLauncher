//! B 层兜底：**不依赖系统任务库的每日备份**（R2，方案 §4.3）。
//!
//! ## 为什么要这一层
//!
//! 客户那台 Windows 上，`schtasks` 三条路**全都失败了** ——
//! `/Create /XML` 报「未指定的错误」两次，连**一个 XML 都不碰**的
//! `/SC DAILY` 也报「拒绝访问」。三条路共享同一个前提（这个进程有资格
//! 往 Windows 任务库里写），前提不成立就一起死，代码里没有第四条路。
//!
//! 于是用户以为自己有每日备份，**实际一次都不会跑**。
//!
//! 「任务计划」这条路要的是权限；而 `HKCU\...\Run` 登录自启
//! （[`crate::autostart`]，写的是 `"<exe>" --minimized`）**一个权限都不要**。
//! 启动器本来就靠在它登录时起来，那就把备份这份活交给启动器自己。
//!
//! ## 三个触发点（方案 §4.3）
//!
//! | 触发点 | 什么时候 | 做什么 |
//! |---|---|---|
//! | ① 启动后延迟补跑（**本模块**） | 启动器启动**满 120 分钟**（可配）之后，且「距最近一次成功备份 ≥ 24 小时」 | 静默跑一次 `--backup --missed` |
//! | ② 运行中定时（**本模块**） | 进程活着时每 15 分钟查一次，用的就是 ① 那套判据 | 同上 |
//! | ③ 任务计划（增强） | 装上了就用（`crate::schedule`，本轮没动） | 照旧，它多了「错过会补跑」 |
//!
//! ## 为什么是「启动满 2 小时」而不是「登录就备」
//!
//! 用户 2026-09-28 定的原话是：*「避免在登录时候进行备份，会导致影响正常使用，
//! 在启动 2 个小时后，再启动备份」*。
//!
//! * 开机 / 登录那几分钟是用户最需要机器响应的时候（系统在起服务、用户在开浏览器），
//!   而备份要连 docker、跑 `pg_dump`、大量写盘 —— 那会儿插进去就是明抢；
//! * 2 小时之后机器已进入稳态、用户也多半进入正常使用节奏，此时备份的影响面最小；
//! * 这个数是**可配**的（`[backup] fallback_delay_mins`，范围 5–720）：
//!   **如果这台电脑经常开不到 2 小时，等待时间必须能调小**，否则
//!   等待期永远走不完、补跑永远不发生，等于没修（方案 §8 风险 5b）。
//!
//! ## 计时口径（三条，不能改，否则会出现「明明开着却永远不备份」）
//!
//! 1. 计时基准是**启动器进程的启动时刻**（不是登录时刻；被 `Run` 键拉起、
//!    缩在托盘里也一样算），用**墙钟时间戳**记录 —— **不用单调时钟**；
//! 2. **睡眠 / 休眠的时间算在里面**：唤醒后第一次检查若发现「启动至今 ≥ 等待时长」，
//!    就照常补跑（睡前开着、早上唤醒 → 补跑，这是期望行为）。
//!    这也是为什么判据每次都用 [`crate::timefmt::now_unix`] 重新算，
//!    而不是靠 `Instant` 累加；
//! 3. 检查沿用运行中定时器每 15 分钟一拍，所以**最坏会晚 15 分钟** ——
//!    界面上不承诺「准点」。
//!
//! ## 平台范围：**只 Windows**
//!
//! macOS 用用户级 LaunchAgent、Linux 用 `systemd --user`，两个都不需要管理员，
//! 现状是好的。那两个平台**不引入 2 小时延迟** —— 它们的定时机制本来就是准时的，
//! 不该跟着一起降级。

use std::sync::OnceLock;
use std::time::Duration;

use crate::config::BackupSection;
use crate::err::AppResult;

/// 默认等待：启动器起来之后先按兵不动 120 分钟。
pub const DEFAULT_WAIT_MINS: i64 = 120;
/// 等待时长的下限。**不允许 0** —— `0` 等于「启动就允许补跑」，正是要避免的事。
pub const WAIT_MINS_MIN: i64 = 5;
/// 等待时长的上限（12 小时）。
pub const WAIT_MINS_MAX: i64 = 720;
/// 两次成功备份之间最长的间隔（小时）。
pub const DEFAULT_INTERVAL_HOURS: u32 = 24;

/// 运行中多久查一次。**最坏会晚这么多**，界面文案里不承诺「准点」。
pub const TICK: Duration = Duration::from_secs(15 * 60);

/// 补跑那个子进程最多允许跑多久。
///
/// 比 [`TICK`] 长得多：一次完整备份（`pg_dump -Fc` + 三个卷的 tar + 校验）
/// 在慢机器上十几分钟是常事。子进程自己还有更细的超时（`backup::create` 里）。
const RUN_TIMEOUT: Duration = Duration::from_secs(60 * 60);

// ── 该不该补跑：判据 ───────────────────────────────────────────────────────

/// 把等待时长收进 `5–720`。
///
/// **不静默夹取**：设置页上明写「填 0 会按 5 算，因为 0 等于启动就允许补跑」，
/// 存下来之后输入框里显示的就是 5，用户看得见发生了什么事。
pub fn clamp_wait_mins(v: i64) -> i64 {
    v.clamp(WAIT_MINS_MIN, WAIT_MINS_MAX)
}

/// **该不该补跑**（方案 §4.3 的那个判据，纯函数）。
///
/// ```text
/// should_catch_up(启动时刻, 现在, 最近一次成功备份时刻, 等待分钟, 间隔小时)
///   = 现在 - 启动时刻 ≥ 等待分钟
///     且（从未成功过 或 现在 - 最近一次成功备份时刻 ≥ 间隔小时）
/// ```
///
/// 所有时刻都是 **Unix 秒（墙钟）**。`last_ok` 传 `None` = 从来没成功过
/// （刚装上的机器就是这一档）。
///
/// **时钟异常时的行为写死在这里**（不是靠运气）：
/// * `now < started_at`（时钟被往回拨）⇒ `now - started_at` 为负
///   ⇒ 不到等待时长 ⇒ **不补跑**；
/// * `last_ok` 在未来（同一类异常）⇒ 差值仍小于间隔 ⇒ **不补跑**。
///
/// 两个方向都偏向「这一次先别动」：多等一拍不会丢什么，而误判成「已经超过
/// 24 小时没备份」会平白多跑一次 `pg_dump` 去抢用户刚启动的机器的磁盘。
/// **不会 panic，也不会回绕。**
pub fn should_catch_up(
    started_at: i64,
    now: i64,
    last_ok: Option<i64>,
    wait_mins: i64,
    interval_hours: u32,
) -> bool {
    let wait_secs = clamp_wait_mins(wait_mins).saturating_mul(60);
    let ran_for = now.saturating_sub(started_at);
    if ran_for < wait_secs {
        return false;
    }
    match last_ok {
        // 从来没成功过 → 只要等满了就补
        None => true,
        Some(t) => now.saturating_sub(t) >= (interval_hours as i64).saturating_mul(3600),
    }
}

/// **这一拍该不该动手** —— 把开关也算进来的那一版，`tick` 与单测都用它。
///
/// 两道开关缺一不可：
/// * `enabled`：自动备份总开关（关着的时候连定时任务都会被卸掉）；
/// * `windows_fallback`：B 层自己的开关（关掉就回到 R2 之前的行为）。
pub fn due(cfg: &BackupSection, started_at: i64, now: i64, last_ok: Option<i64>) -> bool {
    cfg.enabled
        && cfg.windows_fallback
        && should_catch_up(
            started_at,
            now,
            last_ok,
            cfg.fallback_wait_mins(),
            cfg.fallback_interval_hours_clamped(),
        )
}

/// 这台机器上，自动备份现在**靠启动器自己补跑**吗（界面 U3 那行常驻状态用它）。
///
/// 只判「机制在不在这一档」，不去现查 `schtasks`（那是 `schedule::status()` 的事，
/// 每调一次都要起一个子进程）。`to_backup_settings` 会被频繁调用，不能带这个开销。
pub fn active(cfg: &BackupSection) -> bool {
    cfg!(windows) && cfg.enabled && cfg.windows_fallback
}

// ── 启动时刻（墙钟） ──────────────────────────────────────────────────────

/// 启动器进程是什么时候起来的。**墙钟 Unix 秒**，不是 `Instant` ——
/// 睡眠 / 休眠的时间必须算在里面（方案 §4.3 计时口径第 1、2 条）。
static STARTED_AT: OnceLock<i64> = OnceLock::new();

/// 记下启动时刻。**在 [`crate::run`] 的最开头调一次**，越早越准。
pub fn mark_started() {
    let _ = STARTED_AT.set(crate::timefmt::now_unix());
}

/// 启动器进程的启动时刻。没记过就按「现在」算 —— **宁可从头等一遍**，
/// 也不要在没把握的时候判成「已经等满了」。
pub fn started_at() -> i64 {
    *STARTED_AT.get_or_init(crate::timefmt::now_unix)
}

// ── 最近一次成功备份 ──────────────────────────────────────────────────────

/// 「最近一次成功备份」是哪一刻（Unix 秒）。从来没有过就是 `None`。
///
/// **两份来源取更近的那个**，不新加状态存储（方案 §4.3）：
/// * 备份目录里最新那一份的 `at` —— 这是**盘上真有的东西**（红线 1 的那一侧）；
/// * `[backup] last_ok_at` —— 定时备份跑在没有界面的进程里，它成功时写的这一项。
///
/// 之所以两份都要：用户可能把备份目录整个换到外接盘上，
/// 也可能手动删过目录里的几份 —— 任一来源单独看都可能偏老或偏新。
pub fn last_success_unix() -> Option<i64> {
    let mut best: Option<i64> = None;
    let mut keep = |t: Option<i64>| {
        if let Some(t) = t {
            best = Some(best.map_or(t, |b| b.max(t)));
        }
    };
    keep(crate::timefmt::unix_from_shanghai(
        &crate::config::LauncherConfig::load().backup.last_ok_at,
    ));
    keep(
        crate::backup::list()
            .iter()
            .filter_map(|m| crate::timefmt::unix_from_shanghai(&m.at))
            .max(),
    );
    best
}

// ── 运行中定时器 ──────────────────────────────────────────────────────────

/// 起一条后台线程，每 [`TICK`] 查一次。**平台范围：只 Windows**（见模块头注释）。
///
/// 与 lib.rs 里既有那几条「开机自查」线程并列，**不改动
/// [`crate::schedule::sync`] 的任何现有调用点** —— 任务计划那条路一字未动。
pub fn spawn_watch() {
    if !cfg!(windows) {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(TICK);
        tick_once();
    });
}

/// 查一拍。**这是全流程唯一会真的触发补跑的地方。**
fn tick_once() {
    let cfg = crate::config::LauncherConfig::load();
    let now = crate::timefmt::now_unix();
    let last = last_success_unix();
    if !due(&cfg.backup, started_at(), now, last) {
        return;
    }

    // 系统里的定时任务真的能用时**不补跑** —— 两种机制不要同一天跑两遍
    // （方案 §8 风险 4）。任务装不上正是这一层存在的理由，所以这里
    // 只有「装着且 Ready」才让路。
    let st = crate::schedule::status();
    if st.installed && st.enabled {
        crate::linfo!("兜底补跑：系统里已经有能用的定时任务，这一次不补跑（避免重复备份）");
        return;
    }

    match run_catch_up(&cfg.backup, now, last) {
        Ok(()) => {}
        Err(e) => crate::lwarn!("兜底补跑没成：{}", e.msg),
    }
}

/// 真的补跑一次：起一个 `--backup --missed` 的子进程（无界面）。
///
/// **为什么起子进程而不是在本进程里直接做**：这条路就是方案 §4.3 说的
/// 「静默跑一次 `--backup`（无界面，Hunter 容器停着也能做）」——
/// 它跑的是 `headless` 里那条已经存在的无界面入口，失败记录、跳过语义、
/// 备份结果落进配置，全都与定时任务那一次**完全同一条路**。
/// 而且跨进程锁也在子进程那一侧抢，两个备份不会撞上（`backup::try_lock`）。
fn run_catch_up(cfg: &BackupSection, now: i64, last: Option<i64>) -> AppResult<()> {
    let exe = crate::schedule::exe()?;
    let exe_s = exe.to_string_lossy().into_owned();
    let waited = crate::timefmt::human_secs(now.saturating_sub(started_at()).max(0) as u64);
    let gap = match last {
        Some(t) => crate::timefmt::human_secs(now.saturating_sub(t).max(0) as u64),
        None => "从来没有成功过".to_string(),
    };
    crate::linfo!(
        "兜底补跑：启动器已经开了 {waited}（等待 {} 分钟），距上次成功备份 {gap} —— 补跑一次",
        cfg.fallback_wait_mins()
    );
    let r = crate::proc::run_timeout(&exe_s, &["--backup", "--missed"], RUN_TIMEOUT)?;
    if r.ok() {
        crate::linfo!("兜底补跑：这一次做完了");
    } else {
        crate::lwarn!("兜底补跑：子进程退出码非零 —— {}", r.err_line());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个「绝对够用的」基准时刻：2026-09-23 00:00:00（上海）= 该时刻的 Unix 秒。
    const BASE: i64 = 1_790_092_800;

    fn mins(n: i64) -> i64 {
        n * 60
    }
    fn hours(n: i64) -> i64 {
        n * 3600
    }

    #[test]
    fn 不足等待时长时不补跑哪怕当天从来没备过() {
        // 用户 2026-09-28 的硬要求：**开机那阵子一次都不许动手**
        assert!(!should_catch_up(BASE, BASE + mins(119), None, 120, 24));
        assert!(
            !should_catch_up(BASE, BASE + mins(119) + 59, None, 120, 24),
            "差一秒也是没到"
        );
        assert!(!should_catch_up(BASE, BASE, None, 120, 24), "刚启动");
        assert!(!should_catch_up(BASE, BASE + mins(1), None, 120, 24));
    }

    #[test]
    fn 满等待时长且从未成功过就补跑() {
        assert!(should_catch_up(BASE, BASE + mins(120), None, 120, 24));
        assert!(should_catch_up(BASE, BASE + mins(121), None, 120, 24));
        assert!(
            should_catch_up(BASE, BASE + hours(9), None, 120, 24),
            "睡了一觉起来也是补"
        );
    }

    #[test]
    fn 满等待时长但当天已经备过就不补() {
        // 期间定时任务恰好跑了：3 小时前刚成功过，离 24 小时还远
        assert!(!should_catch_up(
            BASE,
            BASE + mins(180),
            Some(BASE + mins(60)),
            120,
            24
        ));
        // 刚启动就在 5 分钟前备过（用户自己点过「立即备份」）
        assert!(!should_catch_up(
            BASE,
            BASE + mins(120),
            Some(BASE + mins(5)),
            120,
            24
        ));
    }

    #[test]
    fn 间隔的两个边界_23小时59分不跑24小时01分跑() {
        let now = BASE + hours(48);
        // 23 小时 59 分：还差一分钟
        assert!(!should_catch_up(
            BASE,
            now,
            Some(now - (hours(24) - mins(1))),
            120,
            24
        ));
        // 24 小时 01 分：过了
        assert!(should_catch_up(
            BASE,
            now,
            Some(now - (hours(24) + mins(1))),
            120,
            24
        ));
    }

    #[test]
    fn 等待时长改了判据跟着变() {
        let now = BASE + mins(8);
        // 默认 120 分钟：没到
        assert!(!should_catch_up(BASE, now, None, 120, 24));
        // 用户改成 5 分钟：到了 —— 这个数真的生效，不是写死的
        assert!(should_catch_up(BASE, now, None, 5, 24));
        // 用户改成 720 分钟：更没到
        assert!(!should_catch_up(BASE, now, None, 720, 24));
        // 间隔也跟着变：110 分钟前备过的那次，24 小时的档期里不算旧、
        // 但把间隔改成 1 小时之后就算旧了
        assert!(!should_catch_up(
            BASE,
            BASE + mins(120),
            Some(BASE + mins(10)),
            120,
            24
        ));
        assert!(should_catch_up(
            BASE,
            BASE + mins(120),
            Some(BASE + mins(10)),
            120,
            1
        ));
    }

    #[test]
    fn 跨天边界() {
        // 昨天 00:05 备过、今天中午看：36 小时没备了 → 补
        let now = BASE + hours(36);
        assert!(should_catch_up(BASE, now, Some(BASE + mins(5)), 120, 24));
        // 今天 09:00 备过、今天中午看：3 小时 → 不补
        let now2 = BASE + hours(12);
        assert!(!should_catch_up(BASE, now2, Some(BASE + hours(9)), 120, 24));
        // 今天 00:00 整备过、今天 23:00 看：23 小时 → 还不补（差一小时）
        assert!(!should_catch_up(
            BASE,
            BASE + hours(23),
            Some(BASE),
            120,
            24
        ));
    }

    #[test]
    fn 时钟回拨与未来时间不误判也不崩() {
        let now = BASE + hours(10);
        // 时钟被往回拨：现在比启动时刻还早 → 不补跑
        assert!(!should_catch_up(now, now - hours(5), None, 120, 24));
        assert!(!should_catch_up(BASE, BASE - 1, None, 120, 24));
        // 最近一次成功备份在未来（同一类异常）→ 不补跑
        assert!(!should_catch_up(BASE, now, Some(now + hours(3)), 120, 24));
        // 极端值：i64 的上下限也不能 panic、不能回绕
        assert!(!should_catch_up(i64::MAX, i64::MIN, None, 120, 24));
        assert!(should_catch_up(i64::MIN, i64::MAX, None, 120, 24));
        assert!(!should_catch_up(0, i64::MIN, Some(i64::MAX), 120, 24));
    }

    #[test]
    fn 等待时长被收进_5_到_720() {
        assert_eq!(clamp_wait_mins(0), 5, "0 等于启动就补跑，按 5 算");
        assert_eq!(clamp_wait_mins(-30), 5);
        assert_eq!(clamp_wait_mins(4), 5);
        assert_eq!(clamp_wait_mins(5), 5);
        assert_eq!(clamp_wait_mins(120), 120);
        assert_eq!(clamp_wait_mins(720), 720);
        assert_eq!(clamp_wait_mins(721), 720);
        assert_eq!(clamp_wait_mins(100_000), 720);
        // 判据也照收窄后的值算：填 0 的机器不会变成「启动就补」
        assert!(!should_catch_up(BASE, BASE + mins(4), None, 0, 24));
        assert!(should_catch_up(BASE, BASE + mins(5), None, 0, 24));
    }

    #[test]
    fn 开关关掉时以上全不触发() {
        let now = BASE + hours(30);
        let mut cfg = BackupSection::default();
        assert!(due(&cfg, BASE, now, None), "默认是开着的");

        cfg.windows_fallback = false;
        assert!(!due(&cfg, BASE, now, None), "B 层开关关掉 → 什么都不触发");

        cfg.windows_fallback = true;
        cfg.enabled = false;
        assert!(
            !due(&cfg, BASE, now, None),
            "自动备份总开关关掉 → 什么都不触发"
        );

        // 就算当天从来没备过、等待时长调到最小，关着就是关着
        cfg.enabled = false;
        cfg.fallback_delay_mins = 5;
        assert!(!due(&cfg, BASE, BASE + hours(100), None));
    }

    #[test]
    fn 判据用的是配置里那个等待时长() {
        let mut cfg = BackupSection::default();
        let now = BASE + mins(8);
        assert!(!due(&cfg, BASE, now, None), "默认 120 分钟，8 分钟时不动");
        cfg.fallback_delay_mins = 5;
        assert!(due(&cfg, BASE, now, None), "改成 5 分钟之后同一刻就动了");
        // 越界值按收窄后的算
        cfg.fallback_delay_mins = 0;
        assert!(due(&cfg, BASE, BASE + mins(5), None));
        assert!(!due(&cfg, BASE, BASE + mins(4), None));
    }

    #[test]
    fn 间隔被手改成零也不至于刚备过就再备() {
        let cfg = BackupSection {
            fallback_interval_hours: 0,
            ..Default::default()
        };
        assert_eq!(cfg.fallback_interval_hours_clamped(), 1);
        let now = BASE + hours(3);
        // 半小时前刚备过 → 不补（不是「只要满等待时长就补」）
        assert!(!due(&cfg, BASE, now, Some(now - mins(30))));
        // 两小时前备过 → 补
        assert!(due(&cfg, BASE, now, Some(now - hours(2))));
    }

    #[test]
    fn 等待时长的默认值就是用户定的那个数() {
        let cfg = BackupSection::default();
        assert_eq!(cfg.fallback_delay_mins, 120);
        assert_eq!(cfg.fallback_wait_mins(), 120);
        assert!(
            cfg.windows_fallback,
            "B 层默认开，这是用户 2026-09-28 拍的板"
        );
        assert!(cfg.enabled);
        assert_eq!(cfg.fallback_interval_hours, 24);
    }

    /// 老配置文件里没有这三项，读出来必须是「默认开 + 120 分钟」。
    #[test]
    fn 老配置里没有这三项时按默认值来() {
        let c: crate::config::LauncherConfig = toml::from_str(
            "[backup]\nenabled = true\ntime = \"00:00\"\nkeep_days = 3\n\
             include_sessions = true\ninclude_skills = true\n",
        )
        .expect("老配置要读得回来");
        assert!(c.backup.windows_fallback);
        assert_eq!(c.backup.fallback_delay_mins, 120);
        assert_eq!(c.backup.fallback_interval_hours, 24);
    }

    /// 配置文件被手写成一个负数，**整份配置不能因此退回默认值**（字段是 i64 的理由）。
    ///
    /// 字段是 `u32` 的话，这里读到 `-5` 会让 `toml::from_str` 整个失败 ——
    /// 而 `LauncherConfig::load()` 对解析失败的处理是「这次用默认设置」，
    /// 用户会莫名其妙地丢掉**所有**设置。
    #[test]
    fn 等待时长被手写成负数也读得回来并收进_5() {
        let c: crate::config::LauncherConfig = toml::from_str(
            "[backup]\nenabled = true\ntime = \"00:00\"\nkeep_days = 3\n\
             include_sessions = true\ninclude_skills = true\nfallback_delay_mins = -5\n",
        )
        .expect("负数是读得回来的");
        assert_eq!(c.backup.fallback_delay_mins, -5, "原样读进来");
        assert_eq!(c.backup.fallback_wait_mins(), 5, "用的时候收进 5");
    }

    #[test]
    fn 启动时记住的是墙钟秒() {
        let t = started_at();
        assert!(t > 1_600_000_000, "Unix 秒，不是 Instant 那种小数字：{t}");
        let d = crate::timefmt::now_unix() - t;
        assert!((0..3600).contains(&d), "刚记下不久：差 {d} 秒");
    }
}
