//! 数据备份与恢复（I13 · R6；I1–I12 期间它只是「升级前备份」）。
//!
//! ## 一份备份是什么
//!
//! `<备份目录>/hunter-<YYYYMMDD-HHmm>-v<tag>/` 下的一组文件：
//!
//! | 文件 | 怎么来的 | 丢了会怎样 |
//! |---|---|---|
//! | `hunter.dump` | `pg_dump -Fc`（在 postgres 容器里跑，输出**直接落盘**） | 账号、配置、自选股、历史分析全没了 |
//! | `secrets.tar.gz` | 一次性容器挂 `hunter_secrets` 打包 | 数据库里加密过的配置解不开 |
//! | `user_skills.tar.gz` / `opencode_data.tar.gz` | 同上（可选，默认都打） | 自建技能 / 历史对话 |
//! | `.env` | 复制 | **里面有 key**，所以整个目录 700、文件 600 |
//! | `docker-compose.yml` · `docker-compose.launcher.yml` · `launcher.toml` | 复制 | 换机器恢复时照着它把栈还原回去 |
//! | `meta.json` | 本模块生成 | 界面上那张列表、校验结论、表数行数概况 |
//!
//! ## 三条硬规矩
//!
//! **一、转储不进内存。** 用户的库可能有几百 MB，先攒成 `String` 再写文件等于
//! 把它翻倍塞进内存。所以这里不走 [`crate::compose::run`]，而是拿
//! [`crate::compose::argv`] 自己起子进程，把 stdout 直接接到文件上（见 [`pipe`]）。
//! 卷的 tar 包同理。
//!
//! **二、没校验过的不算备份。** 打完包立刻 `pg_restore --list` 读一遍目录，
//! 读不出来就判这次失败（方案 R6 6.2）。一份「看起来在那儿、其实恢复不了」的
//! 备份比没有备份更糟 —— 它会让人在真出事的那一天才发现。
//!
//! **三、备份目录在 `~/.hunter` 之外。** 删除应用（R4）会把 `~/.hunter` 整个删掉。
//! 默认位置见 [`crate::config::suggested_backup_dir`]，用户可改。
//! `~/.hunter/backups/` 里 0.1.12 及之前留下的那些**照样列得出来、恢复得了**
//! （[`legacy_dir`]），只是新备份不再往那儿写。
//!
//! ## 为什么 `-Fc` 而不是纯 SQL
//!
//! 0.1.12 及之前用的是 `pg_dump --clean --if-exists`（纯 SQL 文本）。换成
//! 自定义格式（`-Fc`）有三个实在的理由：**它自带压缩**（测试机实测见 I13 报告）、
//! **`pg_restore --list` 能在不恢复的前提下把目录读出来**（校验靠它）、
//! 以及恢复时可以只挑一部分对象。代价是必须用 `pg_restore` 而不是 `psql` ——
//! 两者都在 postgres 容器里，用户什么都不用装。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::err::{AppError, AppResult, Code};
use crate::{compose, config, paths};

/// `pg_dump` 的上限。大库慢盘也够了；卡死在这一步比失败更难解释。
const DUMP_TIMEOUT: Duration = Duration::from_secs(900);
/// `pg_restore` 灌回的上限。
const RESTORE_TIMEOUT: Duration = Duration::from_secs(1800);
/// 打一个卷的上限。
const TAR_TIMEOUT: Duration = Duration::from_secs(600);
/// `pg_restore --list` 的上限。只读目录，很快。
const VERIFY_TIMEOUT: Duration = Duration::from_secs(120);
/// 逐表数行数时，表多到这个数以上就只报表数不逐表数（避免在大库上跑几分钟）。
const MAX_TABLES_TO_COUNT: usize = 40;

/// 转储文件名。**这一行是恢复那一侧的唯一依据**，改它等于让老备份恢复不了。
pub const DUMP_NAME: &str = "hunter.dump";
/// 0.1.12 及之前那种纯 SQL 转储的文件名。恢复时认它，新备份不再产生它。
pub const LEGACY_SQL_NAME: &str = "hunter.sql";
pub const META_NAME: &str = "meta.json";

/// 这次备份是谁发起的。**保留策略按它分两套**（方案 R6 6.2 末尾）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// 用户在界面上点的
    Manual,
    /// 定时任务跑的（`--backup --scheduled`）
    Scheduled,
    /// 升级前自动做的。**不参与「保留近 N 天」的轮换，另外单独留最近 2 份**
    ///
    /// 这也是默认值：0.1.12 及之前的备份全都是升级前备份，
    /// 它们的 `meta.json` 里没有 `kind` 这一项，读出来就该是这一档。
    #[default]
    PreUpgrade,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Manual => "manual",
            Kind::Scheduled => "scheduled",
            Kind::PreUpgrade => "pre-upgrade",
        }
    }
    pub fn cn(self) -> &'static str {
        match self {
            Kind::Manual => "你手动做的",
            Kind::Scheduled => "定时备份",
            Kind::PreUpgrade => "升级前自动备份",
        }
    }
    pub fn parse(s: &str) -> Kind {
        match s.trim().to_ascii_lowercase().as_str() {
            "manual" => Kind::Manual,
            "scheduled" => Kind::Scheduled,
            _ => Kind::PreUpgrade,
        }
    }
    /// 参不参加「保留最近 N 天」那套轮换。
    pub fn rotates(self) -> bool {
        !matches!(self, Kind::PreUpgrade)
    }
}

/// 备份目录里的一个文件。`sha256` 是**落盘之后**算的，不是边写边算 ——
/// 我们要校验的是「盘上这一份」，不是「我们以为写进去的那一份」。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

/// 一张表有多少行。**`count(*)` 数出来的真值**，不是 `n_live_tup` 那种统计估算。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TableRows {
    pub table: String,
    pub rows: u64,
}

/// 备份里包含的一个数据卷。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeEntry {
    /// compose 里声明的短名，例如 `hunter_secrets`
    pub short: String,
    /// 打出来的文件名，例如 `secrets.tar.gz`
    pub file: String,
    pub bytes: Option<u64>,
    /// 没打成的原因。**有值就说明这个卷这次没备上**
    pub error: Option<String>,
}

/// 一次备份的元信息。写进 `meta.json`，界面也用它列清单。
///
/// **所有 I13 新增的字段都带 `#[serde(default)]`**：0.1.12 及之前写下的
/// `meta.json` 只有 `id / tag / at / sqlBytes / sqlError / files` 六项，
/// 那些备份必须照样列得出来、恢复得了。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackupMeta {
    /// 备份目录名，形如 `hunter-20260923-0000-v1.2.0`
    pub id: String,
    /// 备份时 Hunter 的 tag
    pub tag: String,
    /// 上海时间 `YYYY-MM-DD HH:MM:SS`
    pub at: String,
    #[serde(default)]
    pub kind: Kind,
    /// 哪一版启动器做的
    #[serde(default)]
    pub launcher_version: String,
    #[serde(default)]
    pub project: String,

    /// 逐个文件的大小与 sha256
    #[serde(default)]
    pub entries: Vec<FileEntry>,
    /// `hunter.dump` 的字节数；没转成就是 `None`
    #[serde(default)]
    pub dump_bytes: Option<u64>,
    /// 转储没成功时的原因（如实写，不遮掩）
    #[serde(default)]
    pub dump_error: Option<String>,
    /// `pg_restore --list` 读得出目录吗。**读不出就不算成功的备份**
    #[serde(default)]
    pub verified: bool,
    /// 校验的结论原话（读出多少条目 / 为什么没读成）
    #[serde(default)]
    pub verify_note: String,

    /// public schema 下有几张表
    #[serde(default)]
    pub table_count: Option<u32>,
    /// 行数最多的那几张表（真 `count(*)`）
    #[serde(default)]
    pub tables: Vec<TableRows>,
    /// 全部表的行数合计
    #[serde(default)]
    pub rows_total: Option<u64>,
    /// 表太多没逐表数时说明一句
    #[serde(default)]
    pub tables_note: String,

    /// 打进来的数据卷
    #[serde(default)]
    pub volumes: Vec<VolumeEntry>,
    /// 这一份备份一共多少字节
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub elapsed_ms: u64,

    /// 备份所在的完整目录（**列清单时现填**，不信文件里存的那一份 ——
    /// 用户完全可能把整个备份目录搬到别处）
    #[serde(default)]
    pub dir: String,

    // ── 0.1.12 及之前的字段，只读不写 ────────────────────────────────
    /// 配置文件备份了哪几个
    #[serde(default)]
    pub files: Vec<String>,
    /// 老格式（纯 SQL）转储的字节数
    #[serde(default)]
    pub sql_bytes: Option<u64>,
    #[serde(default)]
    pub sql_error: Option<String>,
}

impl BackupMeta {
    /// 这次备份到底有没有把数据库存下来。
    pub fn has_dump(&self) -> bool {
        self.dump_bytes.unwrap_or(0) > 0 || self.sql_bytes.unwrap_or(0) > 0
    }
    /// 这份备份是老格式（纯 SQL）还是新格式（`-Fc`）。恢复时分两条路走。
    pub fn legacy_sql(&self) -> bool {
        self.dump_bytes.unwrap_or(0) == 0 && self.sql_bytes.unwrap_or(0) > 0
    }
    /// 能不能拿它恢复。老备份没有 `verified` 这一项，所以不能拿它当门槛。
    pub fn restorable(&self) -> bool {
        self.has_dump()
    }
    pub fn path(&self) -> PathBuf {
        if self.dir.is_empty() {
            dir_of(&self.id)
        } else {
            PathBuf::from(&self.dir)
        }
    }
    /// 目录名里的日期（`hunter-20260923-0000-v1.2.0` → `20260923`）。
    /// 轮换按它分组；认不出来的返回 `None`（那种目录**永远不自动删**）。
    pub fn day(&self) -> Option<String> {
        day_of_id(&self.id)
    }
}

/// 目录名里的那一段日期。新旧两种命名都认：
/// `hunter-20260923-0000-v1.2.0` 与 `2026-09-20_031854-v1.1.0`。
pub fn day_of_id(id: &str) -> Option<String> {
    if let Some(rest) = id.strip_prefix("hunter-") {
        let d = rest.split('-').next()?;
        if d.len() == 8 && d.chars().all(|c| c.is_ascii_digit()) {
            return Some(d.to_string());
        }
        return None;
    }
    // 老命名：`2026-09-20_031854-v1.1.0`
    let d = id.split('_').next()?;
    let digits: String = d.chars().filter(char::is_ascii_digit).collect();
    (digits.len() == 8).then_some(digits)
}

// ── 目录 ──────────────────────────────────────────────────────────────────

/// 现在生效的备份目录（配置里填的，或者按平台建议的那一个）。
pub fn dir() -> PathBuf {
    config::LauncherConfig::load().backup.effective_dir()
}

/// 0.1.12 及之前那个固定在 `~/.hunter/backups` 的目录。
/// 新备份不往这儿写，但**里面的东西照样列、照样能恢复**。
pub fn legacy_dir() -> PathBuf {
    paths::backups_dir()
}

pub fn dir_of(id: &str) -> PathBuf {
    let d = dir().join(id);
    if d.exists() {
        return d;
    }
    let l = legacy_dir().join(id);
    if l.exists() {
        return l;
    }
    d
}

pub fn dump_path(id: &str) -> PathBuf {
    dir_of(id).join(DUMP_NAME)
}

/// 备份目录名（方案 R6 6.2：`hunter-<YYYYMMDD-HHmm>-v<tag>`）。
///
/// 同一分钟内做第二次备份时后面加 `-2`、`-3`…… —— 方案给的格式只到分钟，
/// 而「升级前备份 + 紧接着的手动备份」完全可能落在同一分钟里。
pub fn make_id(at_shanghai: &str, tag: &str) -> String {
    let stamp = compact_stamp(at_shanghai);
    format!("hunter-{stamp}-v{tag}")
}

/// `2026-09-23 00:00:12` → `20260923-0000`。
fn compact_stamp(at: &str) -> String {
    let (d, t) = at.split_once(' ').unwrap_or((at, "0000"));
    let day: String = d.chars().filter(char::is_ascii_digit).collect();
    let hm: String = t
        .split(':')
        .take(2)
        .flat_map(|x| x.chars())
        .filter(char::is_ascii_digit)
        .collect();
    format!("{day}-{hm}")
}

/// 同名时加后缀，直到找到一个还不存在的目录。
fn unique_dir(base: &Path, id: &str) -> (String, PathBuf) {
    let mut n = 1;
    loop {
        let name = if n == 1 {
            id.to_string()
        } else {
            format!("{id}-{n}")
        };
        let p = base.join(&name);
        if !p.exists() {
            return (name, p);
        }
        n += 1;
        if n > 50 {
            return (name, p);
        }
    }
}

// ── 做一次备份 ────────────────────────────────────────────────────────────

/// 做一次完整备份。**升级流程、定时任务、界面按钮走的都是这一个函数。**
///
/// 失败的定义（任意一条命中就是失败，`Err` 里说清是哪一条）：
/// 转储没做成、或者做成了但 `pg_restore --list` 读不出目录。
/// 卷没打成**不算整次失败**，但会如实记进 `meta.json` 与返回的报告里 ——
/// 数据库在就还有救，会话记录没打上不该让整次备份作废。
pub fn create(kind: Kind, tag: &str, mut note: impl FnMut(&str)) -> AppResult<BackupMeta> {
    let t0 = Instant::now();
    let cfg = config::LauncherConfig::load();
    let base = cfg.backup.effective_dir();
    std::fs::create_dir_all(&base).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!(
                "建备份目录 {} 失败：{e}。换一个目录或者腾出空间之后再试。",
                crate::redact::mask_home(&base.to_string_lossy())
            ),
        )
    })?;
    let at = crate::timefmt::now_shanghai();
    let (id, dir) = unique_dir(&base, &make_id(&at, tag));
    std::fs::create_dir_all(&dir).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("建备份目录 {} 失败：{e}", dir.display()),
        )
    })?;
    // 备份目录里有 .env（带 key），整个目录收成 700
    let _ = paths::chmod_700(&dir);
    note(&format!(
        "备份放到 {}",
        crate::redact::mask_home(&dir.to_string_lossy())
    ));

    let mut meta = BackupMeta {
        id: id.clone(),
        tag: tag.to_string(),
        at,
        kind,
        launcher_version: env!("CARGO_PKG_VERSION").to_string(),
        project: config::PROJECT.to_string(),
        dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };

    // ① 配置四件套
    for (src, name) in [
        (paths::env_file(), ".env"),
        (paths::compose_file(), "docker-compose.yml"),
        (paths::override_file(), "docker-compose.launcher.yml"),
        (paths::launcher_toml(), "launcher.toml"),
    ] {
        if !src.exists() {
            continue;
        }
        let dst = dir.join(name);
        std::fs::copy(&src, &dst)
            .map_err(|e| AppError::new(Code::ConfigWrite, format!("备份 {name} 失败：{e}")))?;
        if name == ".env" {
            paths::chmod_600(&dst)?;
        }
        meta.files.push(name.to_string());
    }
    note(&format!("已备份配置：{}", meta.files.join("、")));

    // ② 数据库。**这一步失败就是整次失败**
    let dump = dir.join(DUMP_NAME);
    match dump_db(&dump, &mut note) {
        Ok(n) => meta.dump_bytes = Some(n),
        Err(e) => {
            meta.dump_error = Some(e.msg.clone());
            meta.elapsed_ms = t0.elapsed().as_millis() as u64;
            let _ = write_meta(&dir, &meta);
            return Err(e);
        }
    }
    note(&format!(
        "数据库已转储（{}）",
        crate::flow::human_bytes(meta.dump_bytes.unwrap_or(0))
    ));

    // ③ 校验。**读不出目录就判这次失败**（方案 R6 6.2）
    let (ok, why) = verify_dump(&dump);
    meta.verified = ok;
    meta.verify_note = why.clone();
    if !ok {
        meta.elapsed_ms = t0.elapsed().as_millis() as u64;
        let _ = write_meta(&dir, &meta);
        return Err(AppError::new(
            Code::UpdateFailed,
            format!("备份做完了但没通过校验，这一份不能算数：{why}"),
        ));
    }
    note(&format!("校验通过 · {why}"));

    // ④ 表数与行数概况。**问不出来不算失败** —— 它是给人看的概况，不是备份本身
    match table_summary() {
        Ok(s) => {
            meta.table_count = Some(s.count);
            meta.tables = s.tables;
            meta.rows_total = s.rows_total;
            meta.tables_note = s.note;
        }
        Err(e) => meta.tables_note = format!("表数行数没问出来：{e}"),
    }

    // ⑤ 数据卷。**密钥卷必打**（和数据库成对），另两个按设置
    let mut wanted: Vec<&str> = vec![compose::VOL_SECRETS];
    if cfg.backup.include_skills {
        wanted.push(compose::VOL_SKILLS);
    }
    if cfg.backup.include_sessions {
        wanted.push(compose::VOL_SESSIONS);
    }
    let vols = compose::volumes_of_project().unwrap_or_default();
    for short in wanted {
        let file = format!("{}.tar.gz", short.trim_start_matches("hunter_"));
        let full = vols
            .iter()
            .find(|v| v.short == short)
            .map(|v| v.name.clone());
        let mut e = VolumeEntry {
            short: short.to_string(),
            file: file.clone(),
            bytes: None,
            error: None,
        };
        match full {
            None => {
                e.error = Some("这台机器上没有这个数据卷".into());
            }
            Some(name) => match tar_volume(&name, &dir.join(&file)) {
                Ok(n) => {
                    e.bytes = Some(n);
                    note(&format!(
                        "已打包 {short}（{}）",
                        crate::flow::human_bytes(n)
                    ));
                }
                Err(err) => {
                    e.error = Some(err.msg);
                    crate::lwarn!("打包数据卷 {short} 没成：{:?}", e.error);
                }
            },
        }
        meta.volumes.push(e);
    }

    // ⑥ 逐个文件算 sha256 与大小
    meta.entries = scan_entries(&dir);
    meta.total_bytes = meta.entries.iter().map(|e| e.bytes).sum();
    meta.elapsed_ms = t0.elapsed().as_millis() as u64;
    write_meta(&dir, &meta)?;
    let _ = write_readme_at(&dir, &meta);

    crate::linfo!(
        "备份完成 {} · {} 字节 · 校验 {} · 卷 {} 个",
        id,
        meta.total_bytes,
        meta.verified,
        meta.volumes.iter().filter(|v| v.bytes.is_some()).count()
    );
    Ok(meta)
}

fn write_meta(dir: &Path, meta: &BackupMeta) -> AppResult<()> {
    let s = serde_json::to_string_pretty(meta)
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("meta.json 序列化失败：{e}")))?;
    std::fs::write(dir.join(META_NAME), s)
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("写 meta.json 失败：{e}")))
}

/// 目录里每个文件的大小与 sha256（`meta.json` 自己不算进去 —— 它还没写完）。
fn scan_entries(dir: &Path) -> Vec<FileEntry> {
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return v;
    };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if name == META_NAME || name == "README.txt" {
            continue;
        }
        let bytes = p.metadata().map(|m| m.len()).unwrap_or(0);
        let sha = crate::runtime::builtin::sha256_file(&p).unwrap_or_default();
        v.push(FileEntry {
            name,
            bytes,
            sha256: sha,
        });
    }
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

// ── 子进程：输出直接落盘 / 输入直接来自文件 ──────────────────────────────

/// 起一个子进程，stdin / stdout 直接接文件，**中间不过内存**。
/// 返回退出状态与 stderr（stderr 很短，攒在内存里没问题）。
fn pipe(
    program: &str,
    args: &[String],
    stdin: Option<std::fs::File>,
    stdout: Option<std::fs::File>,
    timeout: Duration,
    what: &str,
) -> AppResult<(ExitStatus, String)> {
    let mut cmd = crate::proc::base_command(program);
    cmd.args(args);
    cmd.stdin(match stdin {
        Some(f) => Stdio::from(f),
        None => Stdio::null(),
    });
    cmd.stdout(match stdout {
        Some(f) => Stdio::from(f),
        None => Stdio::piped(),
    });
    cmd.stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::new(Code::UpdateFailed, format!("起不了 {what}：{e}")))?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AppError::new(
                        Code::UpdateFailed,
                        format!("{what} 超过 {} 秒没有结束", timeout.as_secs()),
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(AppError::new(Code::UpdateFailed, format!("等 {what}：{e}"))),
        }
    };
    let mut err = String::new();
    if let Some(mut s) = child.stderr.take() {
        use std::io::Read;
        let _ = s.read_to_string(&mut err);
    }
    Ok((status, err))
}

/// 和 [`pipe`] 一样起子进程，但 **stdout 收进内存**（只给输出很短的那几条命令用，
/// 目前只有 `pg_restore --list` —— 它的目录只有几 KB）。
fn pipe_capture(
    program: &str,
    args: &[String],
    stdin: Option<std::fs::File>,
    timeout: Duration,
    what: &str,
) -> AppResult<(ExitStatus, String, String)> {
    let mut cmd = crate::proc::base_command(program);
    cmd.args(args);
    cmd.stdin(match stdin {
        Some(f) => Stdio::from(f),
        None => Stdio::null(),
    });
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::new(Code::UpdateFailed, format!("起不了 {what}：{e}")))?;
    let mut op = child.stdout.take();
    let mut ep = child.stderr.take();
    // 一边跑一边读：管道缓冲区只有 64 KB，先等退出再读会互等（I7 的教训）
    let ho = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(p) = op.as_mut() {
            let _ = std::io::Read::read_to_end(p, &mut b);
        }
        b
    });
    let he = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(p) = ep.as_mut() {
            let _ = std::io::Read::read_to_end(p, &mut b);
        }
        b
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AppError::new(
                        Code::UpdateFailed,
                        format!("{what} 超过 {} 秒没有结束", timeout.as_secs()),
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(AppError::new(Code::UpdateFailed, format!("等 {what}：{e}"))),
        }
    };
    let out = String::from_utf8_lossy(&ho.join().unwrap_or_default()).into_owned();
    let err = String::from_utf8_lossy(&he.join().unwrap_or_default()).into_owned();
    Ok((status, out, err))
}

fn last_line(s: &str) -> String {
    s.trim()
        .lines()
        .next_back()
        .unwrap_or("（没有输出）")
        .to_string()
}

/// `.env` 里的库名与用户名。读不到就用 compose 默认值（和 compose 文件里的 `:-hunter` 一致）。
fn db_user_and_name() -> (String, String) {
    let env = config::parse_env_file(&paths::env_file());
    (
        env.get("POSTGRES_USER")
            .cloned()
            .unwrap_or_else(|| "hunter".into()),
        env.get("POSTGRES_DB")
            .cloned()
            .unwrap_or_else(|| "hunter".into()),
    )
}

/// `docker compose exec -T postgres pg_dump -Fc …` → 直接写进 `out`。返回字节数。
fn dump_db(out: &Path, note: &mut impl FnMut(&str)) -> AppResult<u64> {
    ensure_postgres_up(note)?;
    let (user, db) = db_user_and_name();
    // `-Fc`：自定义格式，自带压缩，`pg_restore --list` 能读出目录（校验靠它）。
    // `--no-owner`：容器里的属主名不一定和恢复目标一致，带上会平白报一堆 role 不存在。
    let (program, args) = compose::argv(&[
        "exec",
        "-T",
        "postgres",
        "pg_dump",
        "-U",
        &user,
        "-d",
        &db,
        "-Fc",
        "--no-owner",
    ]);
    note(&format!("正在转储数据库 {db}（用户 {user}）…"));
    let file = std::fs::File::create(out).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!(
                "建备份文件 {} 失败：{e}",
                crate::redact::mask_home(&out.to_string_lossy())
            ),
        )
    })?;
    let (status, err) = pipe(&program, &args, None, Some(file), DUMP_TIMEOUT, "pg_dump")?;
    if !status.success() {
        let _ = std::fs::remove_file(out);
        return Err(AppError::new(
            Code::UpdateFailed,
            format!("pg_dump 退出码 {:?}：{}", status.code(), last_line(&err)),
        ));
    }
    let n = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    if n == 0 {
        let _ = std::fs::remove_file(out);
        return Err(AppError::new(
            Code::UpdateFailed,
            "pg_dump 退出码是 0，但转储文件是空的".to_string(),
        ));
    }
    Ok(n)
}

/// `pg_restore --list` 读一遍转储的目录（方案 R6 6.2 的校验）。
///
/// **不写任何东西**：`--list` 只把 TOC 打出来。读得出来就说明这份文件
/// 至少是一份完整的、格式正确的自定义格式转储。
pub fn verify_dump(dump: &Path) -> (bool, String) {
    let Ok(file) = std::fs::File::open(dump) else {
        return (false, format!("打不开 {}", dump.display()));
    };
    let (program, args) = compose::argv(&["exec", "-T", "postgres", "pg_restore", "--list"]);
    // `--list` 的输出走管道（TOC 只有几 KB），不落盘
    let (status, stdout, stderr) = match pipe_capture(
        &program,
        &args,
        Some(file),
        VERIFY_TIMEOUT,
        "pg_restore --list",
    ) {
        Ok(v) => v,
        Err(e) => return (false, e.msg),
    };
    if !status.success() {
        return (
            false,
            format!(
                "pg_restore --list 退出码 {:?}：{}",
                status.code(),
                last_line(&stderr)
            ),
        );
    }
    let n = count_toc(&stdout);
    if n == 0 {
        return (
            false,
            "pg_restore --list 跑通了，但一条目录项都没读出来".to_string(),
        );
    }
    (true, format!("pg_restore --list 读出 {n} 条目录项"))
}

/// `pg_restore --list` 的输出里有几条真正的目录项（以 `;` 开头的是注释）。
pub fn count_toc(stdout: &str) -> usize {
    stdout
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with(';')
        })
        .count()
}

/// 一次性容器把数据卷打成 `tar.gz`，**输出直接落盘**。返回字节数。
///
/// 卷挂成 `:ro` —— 备份这件事没有任何理由写卷。
fn tar_volume(volume: &str, out: &Path) -> AppResult<u64> {
    let bin = crate::runtime::which::docker_bin();
    let mount = format!("{volume}:/vol:ro");
    let image = helper_image();
    let args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-v".into(),
        mount,
        "--entrypoint".into(),
        "tar".into(),
        image,
        "-C".into(),
        "/vol".into(),
        "-czf".into(),
        "-".into(),
        ".".into(),
    ];
    let file = std::fs::File::create(out)
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("建 {} 失败：{e}", out.display())))?;
    let (status, err) = pipe(&bin, &args, None, Some(file), TAR_TIMEOUT, "tar")?;
    if !status.success() {
        let _ = std::fs::remove_file(out);
        return Err(AppError::new(
            Code::UpdateFailed,
            format!(
                "打包 {volume} 退出码 {:?}：{}",
                status.code(),
                last_line(&err)
            ),
        ));
    }
    let n = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    if n == 0 {
        let _ = std::fs::remove_file(out);
        return Err(AppError::new(
            Code::UpdateFailed,
            format!("打包 {volume} 退出码是 0，但包是空的"),
        ));
    }
    Ok(n)
}

/// 一次性容器用哪个镜像。和 [`crate::datacheck`] 同一个选择：
/// **这次装的那个 postgres 镜像**，本机一定有，不引入第四个镜像。
fn helper_image() -> String {
    let cfg = config::LauncherConfig::load();
    if cfg.hunter.base_prefix == "docker.io/library" || cfg.hunter.base_prefix.is_empty() {
        "postgres:16-alpine".to_string()
    } else {
        format!("{}/postgres:16-alpine", cfg.hunter.base_prefix)
    }
}

/// postgres 没在跑就单独把它拉起来并等到能接受连接。
///
/// 返回「这次是不是我们起的」—— 定时备份在 Hunter 已停止时要**做完再停回去**
/// （方案 R6 6.3），靠的就是这个返回值。
pub fn ensure_postgres_up(note: &mut impl FnMut(&str)) -> AppResult<bool> {
    let up = compose::ps()
        .unwrap_or_default()
        .iter()
        .any(|s| s.service == "postgres" && s.state == "running");
    if up {
        return Ok(false);
    }
    note("postgres 没在运行，先把它单独拉起来再备份");
    // 这一句也是 `up -d`，同样要先确认 `hunter` 这个项目名是我们的（待办池 P0-5）
    compose::guard_project_owner()?;
    let r = compose::run(&["up", "-d", "postgres"], Duration::from_secs(180))?;
    if !r.ok() {
        return Err(AppError::new(
            Code::UpdateFailed,
            format!("拉起 postgres 失败：{}", r.err_line()),
        ));
    }
    // `pg_isready` 是 postgres 镜像自带的，比盲等靠谱
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let r = compose::run(
            &["exec", "-T", "postgres", "pg_isready"],
            Duration::from_secs(20),
        );
        if matches!(&r, Ok(x) if x.ok()) {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Err(AppError::new(
                Code::UpdateFailed,
                "等了 90 秒 postgres 还没准备好接受连接".to_string(),
            ));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

// ── 表数与行数概况 ────────────────────────────────────────────────────────

pub struct TableSummary {
    pub count: u32,
    pub tables: Vec<TableRows>,
    pub rows_total: Option<u64>,
    pub note: String,
}

/// public schema 下有几张表、各有多少行。
///
/// **行数是 `count(*)` 数出来的真值**，不是 `pg_stat_user_tables.n_live_tup`
/// 那种统计估算 —— 备份里写「1,234 行」而实际是 1,180 行，那就是编数字（红线 1）。
/// 代价是表很多时要跑一会儿，所以超过 [`MAX_TABLES_TO_COUNT`] 张就只报表数。
pub fn table_summary() -> Result<TableSummary, String> {
    let names = psql_lines(
        "select table_name from information_schema.tables \
         where table_schema='public' and table_type='BASE TABLE' order by table_name",
    )?;
    let count = names.len() as u32;
    if names.is_empty() {
        return Ok(TableSummary {
            count: 0,
            tables: Vec::new(),
            rows_total: Some(0),
            note: "public schema 下一张表都没有".into(),
        });
    }
    if names.len() > MAX_TABLES_TO_COUNT {
        return Ok(TableSummary {
            count,
            tables: Vec::new(),
            rows_total: None,
            note: format!(
                "这个库有 {count} 张表，超过 {MAX_TABLES_TO_COUNT} 张就不逐表数行了（数一遍要很久）"
            ),
        });
    }
    let union = names
        .iter()
        .map(|t| {
            format!(
                "select '{}' as t, count(*) as n from \"{}\"",
                esc(t),
                esc(t)
            )
        })
        .collect::<Vec<_>>()
        .join(" union all ");
    let rows = psql_lines(&format!("{union} order by n desc"))?;
    let mut tables = Vec::new();
    let mut total = 0u64;
    for l in rows {
        let Some((t, n)) = l.split_once('|') else {
            continue;
        };
        let n: u64 = n.trim().parse().unwrap_or(0);
        total += n;
        tables.push(TableRows {
            table: t.trim().to_string(),
            rows: n,
        });
    }
    Ok(TableSummary {
        count,
        tables,
        rows_total: Some(total),
        note: String::new(),
    })
}

/// 单引号在 SQL 字面量里要成对写。表名来自 `information_schema`，
/// 本来就不会有引号；这一层是纵深防御，不是在信任输入。
fn esc(s: &str) -> String {
    s.replace('\'', "''").replace('"', "")
}

/// 在 postgres 容器里跑一条只读 SQL，按行返回（`-tA` 无表头、无对齐）。
fn psql_lines(sql: &str) -> Result<Vec<String>, String> {
    let (user, db) = db_user_and_name();
    let r = compose::run(
        &[
            "exec", "-T", "postgres", "psql", "-U", &user, "-d", &db, "-tA", "-F", "|", "-c", sql,
        ],
        Duration::from_secs(120),
    )
    .map_err(|e| e.msg)?;
    if !r.ok() {
        return Err(r.err_line());
    }
    Ok(r.stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

// ── 列清单 ────────────────────────────────────────────────────────────────

/// 列出**当前备份目录 + 老的 `~/.hunter/backups`** 里的全部备份，新的在前。
pub fn list() -> Vec<BackupMeta> {
    let mut seen: BTreeMap<String, BackupMeta> = BTreeMap::new();
    for d in [dir(), legacy_dir()] {
        for m in list_in(&d) {
            seen.entry(m.path().to_string_lossy().into_owned())
                .or_insert(m);
        }
    }
    let mut out: Vec<BackupMeta> = seen.into_values().collect();
    out.sort_by(|a, b| b.at.cmp(&a.at).then(b.id.cmp(&a.id)));
    out
}

/// 列出某一个目录下的备份（换电脑迁移时用户会指一个任意目录）。
pub fn list_in(base: &Path) -> Vec<BackupMeta> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(base) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        if let Some(m) = read_meta(&p) {
            out.push(m);
        }
    }
    out.sort_by(|a, b| b.at.cmp(&a.at));
    out
}

/// 读一个备份目录的 `meta.json`。**`dir` 一律按实际位置现填**。
pub fn read_meta(p: &Path) -> Option<BackupMeta> {
    let s = std::fs::read_to_string(p.join(META_NAME)).ok()?;
    let mut m: BackupMeta = serde_json::from_str(&s).ok()?;
    m.dir = p.to_string_lossy().into_owned();
    if m.id.is_empty() {
        m.id = p.file_name()?.to_string_lossy().into_owned();
    }
    Some(m)
}

/// 按 id 找一份备份（两个目录都找）。
pub fn find(id: &str) -> Option<BackupMeta> {
    list().into_iter().find(|m| m.id == id)
}

/// 用户给的可能是 id，也可能是一个目录（换电脑迁移）。两种都认。
pub fn resolve(id_or_path: &str) -> AppResult<BackupMeta> {
    let s = id_or_path.trim();
    if s.is_empty() {
        return Err(AppError::new(
            Code::NotImplemented,
            "没说要恢复哪一份备份。".to_string(),
        ));
    }
    if let Some(m) = find(s) {
        return Ok(m);
    }
    let p = config::expand_home(s);
    if p.join(META_NAME).exists() {
        if let Some(m) = read_meta(&p) {
            return Ok(m);
        }
    }
    Err(AppError::new(
        Code::NotImplemented,
        format!(
            "找不到备份「{}」。它既不在备份目录里，也不是一个含 meta.json 的目录。",
            crate::redact::mask_home(s)
        ),
    ))
}

/// 备份目录占了多少字节（设置页显示，让用户知道该不该清理）。
pub fn total_bytes() -> u64 {
    dir_size(&dir()) + dir_size(&legacy_dir())
}

pub fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            _ => e.metadata().map(|m| m.len()).unwrap_or(0),
        })
        .sum()
}

/// 一份备份大概多大（用来判「这块盘还放得下几份」）。
/// 没有任何备份时返回 `None` —— **不拿一个拍脑袋的数去吓唬用户**。
pub fn typical_bytes() -> Option<u64> {
    let all = list();
    let v: Vec<u64> = all
        .iter()
        .filter(|m| m.total_bytes > 0)
        .map(|m| m.total_bytes)
        .collect();
    if v.is_empty() {
        return None;
    }
    Some(v.iter().copied().max().unwrap_or(0))
}

/// 把备份目录的清单写成一行行文本（诊断包里用；**不含任何文件内容**，
/// 因为 `.env` 在里面，红线 2）。
pub fn summary_lines() -> Vec<String> {
    let all = list();
    if all.is_empty() {
        return vec!["（还没有任何备份）".to_string()];
    }
    let mut v: Vec<String> = all
        .iter()
        .map(|m| {
            format!(
                "{} · v{} · {} · {} · 数据库 {} · 校验 {}",
                m.at,
                m.tag,
                m.id,
                m.kind.cn(),
                match (m.dump_bytes.or(m.sql_bytes), &m.dump_error) {
                    (Some(n), _) => crate::flow::human_bytes(n),
                    (None, Some(e)) => format!("没有（{e}）"),
                    (None, None) => "没有".to_string(),
                },
                if m.verified { "通过" } else { "没有记录" }
            )
        })
        .collect();
    v.push(format!(
        "合计占用 {}",
        crate::flow::human_bytes(total_bytes())
    ));
    v
}

// ── 轮换 ──────────────────────────────────────────────────────────────────

/// 一次轮换的结果。**先算清楚删哪些，再删** —— 这样同一套判定可以单测。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PruneResult {
    pub removed: Vec<String>,
    pub kept: Vec<String>,
    /// 认不出来、因此**不敢删**的目录（没有 meta.json，或者名字里读不出日期）
    pub skipped: Vec<String>,
    pub freed_bytes: u64,
}

/// 算出「按保留策略，哪些该删」。**纯函数，不碰磁盘**。
///
/// 两套策略，互不干扰（方案 R6 6.2 末尾）：
///
/// * 手动 / 定时备份：**按天保留最近 `keep_days` 天，同一天只留最后一份**；
/// * 升级前备份：**单独留最近 `keep_upgrade` 份**，不参与上面那套轮换。
///
/// 认不出日期的一律留着 —— 删一份认不出来的备份，和删用户的文件没有区别。
pub fn plan_prune(
    all: &[BackupMeta],
    keep_days: u32,
    keep_upgrade: usize,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut keep: Vec<String> = Vec::new();
    let mut drop: Vec<String> = Vec::new();
    let mut skip: Vec<String> = Vec::new();

    // 升级前备份：按时间倒序留最近 keep_upgrade 份
    let mut ups: Vec<&BackupMeta> = all.iter().filter(|m| !m.kind.rotates()).collect();
    ups.sort_by(|a, b| b.at.cmp(&a.at).then(b.id.cmp(&a.id)));
    for (i, m) in ups.iter().enumerate() {
        if i < keep_upgrade {
            keep.push(m.id.clone());
        } else {
            drop.push(m.id.clone());
        }
    }

    // 轮换那一类：先按天分组，每天留最后一份；再留最近 keep_days 天
    let mut by_day: BTreeMap<String, Vec<&BackupMeta>> = BTreeMap::new();
    for m in all.iter().filter(|m| m.kind.rotates()) {
        match m.day() {
            Some(d) => by_day.entry(d).or_default().push(m),
            None => skip.push(m.id.clone()),
        }
    }
    // BTreeMap 按天升序，倒过来就是最近的在前
    let days: Vec<String> = by_day.keys().rev().cloned().collect();
    for (i, d) in days.iter().enumerate() {
        let mut same = by_day.remove(d).unwrap_or_default();
        same.sort_by(|a, b| b.at.cmp(&a.at).then(b.id.cmp(&a.id)));
        let keep_this_day = i < keep_days.max(1) as usize;
        for (j, m) in same.iter().enumerate() {
            if keep_this_day && j == 0 {
                keep.push(m.id.clone());
            } else {
                drop.push(m.id.clone());
            }
        }
    }
    keep.sort();
    drop.sort();
    skip.sort();
    (keep, drop, skip)
}

/// 真的按保留策略删掉过期的备份。**只删备份目录下、meta.json 认得出来的那些目录。**
pub fn prune() -> PruneResult {
    let cfg = config::LauncherConfig::load();
    let all = list();
    let (keep, drop, skipped) = plan_prune(&all, cfg.backup.keep_days_clamped(), 2);
    let mut r = PruneResult {
        kept: keep,
        skipped,
        ..Default::default()
    };
    for id in drop {
        let Some(m) = all.iter().find(|x| x.id == id) else {
            continue;
        };
        let p = m.path();
        // 最后一道：要删的目录必须**真的在**我们两个备份目录之一下面
        if !under_backup_root(&p) {
            r.skipped.push(id);
            continue;
        }
        let n = dir_size(&p);
        match std::fs::remove_dir_all(&p) {
            Ok(()) => {
                r.freed_bytes += n;
                r.removed.push(id);
            }
            Err(e) => {
                crate::lwarn!("删过期备份 {} 失败：{e}", p.display());
                r.skipped.push(id);
            }
        }
    }
    if !r.removed.is_empty() {
        crate::linfo!(
            "备份轮换：删掉 {} 份，腾出 {}",
            r.removed.len(),
            crate::flow::human_bytes(r.freed_bytes)
        );
    }
    r
}

/// 这个路径是不是**我们自己的备份目录**下面的一层子目录。
///
/// 这是删除那一侧的守卫：配置里的 `dir` 是用户填的，填成 `/` 也不是不可能。
/// 判据是「父目录正是两个备份根之一」，不是前缀匹配 —— 前缀匹配对
/// `/home/u/Hunter-backups-old` 这种名字会误判。
pub fn under_backup_root(p: &Path) -> bool {
    let roots = [dir(), legacy_dir()];
    let Some(parent) = p.parent() else {
        return false;
    };
    roots.iter().any(|r| {
        // canonicalize 解开软链与 `..`；解不开（目录不存在）就退回原样比
        match (r.canonicalize(), parent.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => r == parent,
        }
    })
}

// ── 恢复 ──────────────────────────────────────────────────────────────────

/// 一次恢复的完整报告。界面与命令行显示的是同一份。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub from: String,
    /// 恢复前自动做的那一份当前数据的备份
    pub safety_backup: Option<String>,
    pub steps: Vec<String>,
    pub restored_volumes: Vec<String>,
    pub jwt_restored: bool,
    pub ready: usize,
    pub total: usize,
    pub elapsed_ms: u64,
    pub headline: String,
}

/// 恢复前先看看这一份能不能恢复到这台机器上（**只读，不改任何东西**）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestorePreflight {
    pub id: String,
    pub dir: String,
    pub tag: String,
    pub current_tag: String,
    /// 备份来自比现在更新的 Hunter → 先升级再恢复（方案 R6 6.4）
    pub needs_upgrade: bool,
    pub has_dump: bool,
    pub legacy_sql: bool,
    pub verified: bool,
    /// sha256 与 `meta.json` 对不上的文件（有一条就别恢复）
    pub checksum_mismatch: Vec<String>,
    pub volumes: Vec<String>,
    pub blocked: Option<String>,
    pub lines: Vec<String>,
}

/// 版本号比较（`1.2.0` vs `1.10.0`）。按数字逐段比，认不出来的段按 0。
pub fn version_newer(a: &str, b: &str) -> bool {
    let seg = |s: &str| -> Vec<u64> {
        s.trim()
            .trim_start_matches('v')
            .split(['.', '-'])
            .map(|x| x.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (x, y) = (seg(a), seg(b));
    for i in 0..x.len().max(y.len()) {
        let (p, q) = (
            x.get(i).copied().unwrap_or(0),
            y.get(i).copied().unwrap_or(0),
        );
        if p != q {
            return p > q;
        }
    }
    false
}

/// 恢复前的只读体检。
pub fn preflight(id_or_path: &str) -> AppResult<RestorePreflight> {
    let m = resolve(id_or_path)?;
    let cfg = config::LauncherConfig::load();
    let dir = m.path();
    let mut lines = Vec::new();
    let mut mismatch = Vec::new();

    // sha256 逐个核。老备份没有 entries，那就没什么可核的（如实说）
    if m.entries.is_empty() {
        lines.push(
            "这份备份的 meta.json 里没有 sha256（0.1.12 及之前的格式），没法逐个校验文件".into(),
        );
    } else {
        for e in &m.entries {
            let p = dir.join(&e.name);
            if !p.exists() {
                mismatch.push(format!("{}（文件不见了）", e.name));
                continue;
            }
            if e.sha256.is_empty() {
                continue;
            }
            match crate::runtime::builtin::sha256_file(&p) {
                Some(s) if s == e.sha256 => {}
                Some(s) => mismatch.push(format!(
                    "{}（算出来是 {}，记的是 {}）",
                    e.name,
                    &s[..8.min(s.len())],
                    &e.sha256[..8.min(e.sha256.len())]
                )),
                None => mismatch.push(format!("{}（读不动）", e.name)),
            }
        }
        lines.push(format!("逐个核过 {} 个文件的 sha256", m.entries.len()));
    }

    let has_dump = dir.join(DUMP_NAME).exists() || dir.join(LEGACY_SQL_NAME).exists();
    let legacy = !dir.join(DUMP_NAME).exists() && dir.join(LEGACY_SQL_NAME).exists();
    if legacy {
        lines.push("这是 0.1.12 及之前的纯 SQL 备份，恢复时走 psql 那条路".into());
    }
    let needs_upgrade = version_newer(&m.tag, &cfg.hunter.tag);
    if needs_upgrade {
        lines.push(format!(
            "这份备份来自 Hunter v{}，这台机器现在装的是 v{} —— 先升级再恢复",
            m.tag, cfg.hunter.tag
        ));
    }
    let volumes: Vec<String> = m
        .volumes
        .iter()
        .filter(|v| v.bytes.is_some())
        .map(|v| v.short.clone())
        .collect();
    if volumes.is_empty() {
        lines.push("这份备份里没有任何数据卷（加密过的配置恢复之后可能解不开）".into());
    } else {
        lines.push(format!("备份里有这些数据卷：{}", volumes.join("、")));
    }

    let blocked = if !has_dump {
        Some("这份备份里没有数据库转储，恢复不了。".to_string())
    } else if !mismatch.is_empty() {
        Some(format!(
            "有 {} 个文件和记录对不上（{}），这一份不能用来恢复。",
            mismatch.len(),
            mismatch.join("；")
        ))
    } else if needs_upgrade {
        Some(format!(
            "备份来自 Hunter v{}，比这台机器上的 v{} 新。先把 Hunter 升到 v{} 再恢复，\
             否则新版本写下的数据灌进老版本的库会出事。",
            m.tag, cfg.hunter.tag, m.tag
        ))
    } else {
        None
    };

    Ok(RestorePreflight {
        id: m.id.clone(),
        dir: crate::redact::mask_home(&dir.to_string_lossy()),
        tag: m.tag.clone(),
        current_tag: cfg.hunter.tag.clone(),
        needs_upgrade,
        has_dump,
        legacy_sql: legacy,
        verified: m.verified,
        checksum_mismatch: mismatch,
        volumes,
        blocked,
        lines,
    })
}

/// 恢复要用户**逐字输入**的那句话。和删除应用是同一条原则：
/// 覆盖现有数据这件事不该被一次误点触发。
pub const RESTORE_PHRASE: &str = "恢复数据";

/// 检测到的外接盘 / 云盘上的备份位置建议（方案 R6 6.1：**只建议，不自动选**）。
///
/// 判据是「这块盘挂在常见的外置挂载点下、而且不是系统盘」——
/// macOS 的 `/Volumes/*`、Linux 的 `/media/*` 与 `/mnt/*`、Windows 上非 C 盘的盘符。
/// 一个都没检测到就返回空表，界面上那一行**不出现**（不编一个不存在的盘）。
pub fn external_suggestions() -> Vec<String> {
    let home_mount = crate::monitor::disk_for(&paths::home()).map(|(m, _, _)| m);
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut v = Vec::new();
    for d in disks.list() {
        let mp = d.mount_point();
        let s = mp.to_string_lossy().to_string();
        if Some(&s) == home_mount.as_ref() {
            continue;
        }
        // 太小的（U 盘里的分区、只读的系统分区）不建议
        if d.total_space() < 8_000_000_000 {
            continue;
        }
        let looks_external = s.starts_with("/Volumes/")
            || s.starts_with("/media/")
            || s.starts_with("/mnt/")
            || (cfg!(windows) && s.len() <= 3 && !s.starts_with('C'));
        if !looks_external {
            continue;
        }
        v.push(mp.join("Hunter 备份").to_string_lossy().into_owned());
    }
    v.sort();
    v.dedup();
    v.truncate(3);
    v
}

/// 恢复时先停哪几个服务。**postgres 与 redis 留着**：转储要灌进正在跑的
/// postgres，停了它就没地方灌了。
pub const RESTORE_STOP: &[&str] = &["api", "opencode", "web", "llm-shim"];

/// 真的恢复（方案 R6 6.4 的六步）。
///
/// ```text
/// 先给当前数据做一份备份 → 停 api/opencode/web/llm-shim
///   → pg_restore --clean --if-exists → 恢复密钥卷与 JWT_SECRET
///   → 启动 → 等健康
/// ```
///
/// 第一步不是客套：**恢复是会把现在的数据抹掉的操作**，而用户挑错一份备份
/// 是完全可能的。有了这一步，挑错了还能再恢复回来。
pub fn restore(id_or_path: &str, mut note: impl FnMut(&str)) -> AppResult<RestoreReport> {
    let t0 = Instant::now();
    let pre = preflight(id_or_path)?;
    if let Some(b) = &pre.blocked {
        return Err(AppError::new(Code::UpdateFailed, b.clone()));
    }
    let m = resolve(id_or_path)?;
    let dir = m.path();
    let mut rep = RestoreReport {
        from: crate::redact::mask_home(&dir.to_string_lossy()),
        ..Default::default()
    };
    let mut step = |rep: &mut RestoreReport, s: String| {
        note(&s);
        rep.steps.push(s);
    };

    // ① 先给现在的数据做一份备份
    let cfg = config::LauncherConfig::load();
    match create(Kind::Manual, &cfg.hunter.tag, |t| {
        crate::linfo!("恢复前的保命备份：{t}")
    }) {
        Ok(b) => {
            rep.safety_backup = Some(b.id.clone());
            step(&mut rep, format!("先把现在的数据备份成 {}", b.id));
        }
        Err(e) => {
            // 保命备份做不成时**不硬来**：恢复是不可逆的
            return Err(AppError::new(
                Code::UpdateFailed,
                format!(
                    "恢复之前要先把现在的数据备份一份，可是这一步没做成：{}。\
                     没有这份保底，恢复一旦挑错就找不回来了，所以停在这里。",
                    e.msg
                ),
            ));
        }
    }

    // ② 停掉会写库的服务
    let names: Vec<&str> = RESTORE_STOP.to_vec();
    let mut args = vec!["stop"];
    args.extend_from_slice(&names);
    let _ = compose::run(&args, Duration::from_secs(180));
    step(&mut rep, format!("已停下 {}", names.join("、")));

    // ③ 灌回数据库
    let mut nb = |_: &str| {};
    ensure_postgres_up(&mut nb)?;
    let n = if pre.legacy_sql {
        restore_sql(&dir.join(LEGACY_SQL_NAME))?
    } else {
        restore_dump(&dir.join(DUMP_NAME))?
    };
    step(
        &mut rep,
        format!("数据库已灌回（{}）", crate::flow::human_bytes(n)),
    );

    // ④ 密钥卷与另外两个卷
    for v in &m.volumes {
        if v.bytes.is_none() {
            continue;
        }
        let f = dir.join(&v.file);
        if !f.exists() {
            step(&mut rep, format!("{} 的包不在，跳过", v.short));
            continue;
        }
        match untar_volume(&v.short, &f) {
            Ok(()) => {
                rep.restored_volumes.push(v.short.clone());
                step(&mut rep, format!("已恢复数据卷 {}", v.short));
            }
            Err(e) => step(&mut rep, format!("数据卷 {} 没恢复成：{}", v.short, e.msg)),
        }
    }

    // ⑤ JWT_SECRET —— 它变了所有人的登录都会失效
    match restore_jwt(&dir) {
        Ok(true) => {
            rep.jwt_restored = true;
            step(
                &mut rep,
                "已把备份里的 JWT_SECRET 写回（登录不会失效）".into(),
            );
        }
        Ok(false) => step(&mut rep, "备份里没有 JWT_SECRET，这一项保持现状".into()),
        Err(e) => step(&mut rep, format!("JWT_SECRET 没写回：{}", e.msg)),
    }

    // ⑥ 起回来 + 等健康
    compose::up()?;
    step(&mut rep, "已重新启动全部服务".into());
    let st = compose::wait_healthy(compose::START_TIMEOUT, |_| {});
    let services = compose::ps().unwrap_or_default();
    rep.total = crate::selfcheck::EXPECTED.len();
    rep.ready = services
        .iter()
        .filter(|s| compose::service_ready(s))
        .count();
    if let Err(e) = st {
        crate::lwarn!("恢复之后等健康没等到：{}", e.msg);
    }
    rep.elapsed_ms = t0.elapsed().as_millis() as u64;
    rep.headline = format!(
        "已从 {} 恢复 · {}/{} 个服务就绪 · 用时 {} 秒",
        m.id,
        rep.ready,
        rep.total,
        rep.elapsed_ms / 1000
    );
    crate::linfo!("{}", rep.headline);
    Ok(rep)
}

/// `pg_restore --clean --if-exists` 把 `-Fc` 转储灌回去。
fn restore_dump(dump: &Path) -> AppResult<u64> {
    let (user, db) = db_user_and_name();
    let file = std::fs::File::open(dump)
        .map_err(|e| AppError::new(Code::UpdateFailed, format!("读不了转储文件：{e}")))?;
    let (program, args) = compose::argv(&[
        "exec",
        "-T",
        "postgres",
        "pg_restore",
        "-U",
        &user,
        "-d",
        &db,
        "--clean",
        "--if-exists",
        "--no-owner",
    ]);
    let (status, err) = pipe(
        &program,
        &args,
        Some(file),
        None,
        RESTORE_TIMEOUT,
        "pg_restore",
    )?;
    // `pg_restore` 在 `--clean` 下对「本来就不存在的对象」会报 warning 并以非 0 退出。
    // 判据因此不能只看退出码：真正的失败会在 stderr 里带 `error:`。
    let fatal = err
        .lines()
        .any(|l| l.contains("error:") && !l.contains("does not exist"));
    if !status.success() && fatal {
        return Err(AppError::new(
            Code::UpdateFailed,
            format!("pg_restore 退出码 {:?}：{}", status.code(), last_line(&err)),
        ));
    }
    if !status.success() {
        crate::lwarn!(
            "pg_restore 退出码 {:?}，但 stderr 里只有「对象本来就不存在」这类告警，按成功算：{}",
            status.code(),
            last_line(&err)
        );
    }
    Ok(std::fs::metadata(dump).map(|m| m.len()).unwrap_or(0))
}

/// 老格式（纯 SQL）走 `psql`。0.1.12 及之前的备份靠它才恢复得了。
fn restore_sql(sql: &Path) -> AppResult<u64> {
    let (user, db) = db_user_and_name();
    let file = std::fs::File::open(sql)
        .map_err(|e| AppError::new(Code::UpdateFailed, format!("读不了转储文件：{e}")))?;
    let (program, args) = compose::argv(&[
        "exec",
        "-T",
        "postgres",
        "psql",
        "-U",
        &user,
        "-d",
        &db,
        "-v",
        "ON_ERROR_STOP=0",
    ]);
    let (status, err) = pipe(&program, &args, Some(file), None, RESTORE_TIMEOUT, "psql")?;
    if !status.success() {
        return Err(AppError::new(
            Code::UpdateFailed,
            format!("psql 退出码 {:?}：{}", status.code(), last_line(&err)),
        ));
    }
    Ok(std::fs::metadata(sql).map(|m| m.len()).unwrap_or(0))
}

/// 一次性容器把 `tar.gz` 解回数据卷。卷不在就**带着 compose 的标签**建一个。
fn untar_volume(short: &str, tgz: &Path) -> AppResult<()> {
    let bin = crate::runtime::which::docker_bin();
    let full = ensure_volume(short)?;
    let mount = format!("{full}:/vol");
    let image = helper_image();
    let args: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "-i".into(),
        "-v".into(),
        mount,
        "--entrypoint".into(),
        "tar".into(),
        image,
        "-C".into(),
        "/vol".into(),
        "-xzf".into(),
        "-".into(),
    ];
    let file = std::fs::File::open(tgz)
        .map_err(|e| AppError::new(Code::UpdateFailed, format!("读不了 {}：{e}", tgz.display())))?;
    let (status, err) = pipe(&bin, &args, Some(file), None, TAR_TIMEOUT, "tar")?;
    if !status.success() {
        return Err(AppError::new(
            Code::UpdateFailed,
            format!(
                "解包 {short} 退出码 {:?}：{}",
                status.code(),
                last_line(&err)
            ),
        ));
    }
    Ok(())
}

/// 数据卷在就返回它的完整名；不在就建一个，**带上 compose 自己会打的那两个标签**
/// —— 不带标签的话 [`compose::volumes_of_project`] 认不出它，
/// 下一次删除应用就会漏掉它。
fn ensure_volume(short: &str) -> AppResult<String> {
    if let Ok(v) = compose::volumes_of_project() {
        if let Some(x) = v.iter().find(|x| x.short == short) {
            return Ok(x.name.clone());
        }
    }
    let full = format!("{}_{short}", config::PROJECT);
    let bin = crate::runtime::which::docker_bin();
    let l1 = format!("com.docker.compose.project={}", config::PROJECT);
    let l2 = format!("com.docker.compose.volume={short}");
    let r = crate::proc::run_timeout(
        &bin,
        &["volume", "create", "--label", &l1, "--label", &l2, &full],
        Duration::from_secs(30),
    )?;
    if !r.ok() {
        return Err(AppError::new(
            Code::UpdateFailed,
            format!("建数据卷 {full} 失败：{}", r.err_line()),
        ));
    }
    crate::linfo!("恢复时建了数据卷 {full}（带 compose 标签）");
    Ok(full)
}

/// 把备份里的 `JWT_SECRET` 写回当前 `.env`。返回「有没有真的写」。
///
/// **只动这一行**。备份里的 `.env` 还有 hunter key、数据库口令 ——
/// 那些都该保持这台机器的现状（数据库口令换了 api 就连不上库了，待办池 P1-29）。
fn restore_jwt(dir: &Path) -> AppResult<bool> {
    let src = dir.join(".env");
    if !src.exists() {
        return Ok(false);
    }
    let old = config::parse_env_file(&src);
    let Some(secret) = old.get("JWT_SECRET").filter(|s| !s.trim().is_empty()) else {
        return Ok(false);
    };
    let cur_path = paths::env_file();
    let cur = std::fs::read_to_string(&cur_path)
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("读不了当前 .env：{e}")))?;
    let mut out = String::with_capacity(cur.len() + 64);
    let mut replaced = false;
    for l in cur.lines() {
        if l.trim_start().starts_with("JWT_SECRET=") {
            out.push_str(&format!("JWT_SECRET={secret}\n"));
            replaced = true;
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    if !replaced {
        out.push_str(&format!("JWT_SECRET={secret}\n"));
    }
    std::fs::write(&cur_path, out)
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("写 .env 失败：{e}")))?;
    paths::chmod_600(&cur_path)?;
    Ok(true)
}

/// 把配置三件套从备份目录写回去（**升级失败回滚时用**）。**不碰数据库**。
///
/// 这是 0.1.12 就有的那条路，I13 一个字没改：升级失败绝大多数发生在
/// 「镜像拉不下来」或「服务起不来」，那两种情况下数据库根本没被动过 ——
/// 这时候灌回一份转储反而会把升级中途写进去的东西抹掉
/// （[`crate::upgrade`] 的回滚就是只调它，不调 [`restore`]）。
pub fn restore_config(id: &str) -> AppResult<Vec<String>> {
    let dir = dir_of(id);
    let mut done = Vec::new();
    for (name, dst) in [
        (".env", paths::env_file()),
        ("docker-compose.yml", paths::compose_file()),
        ("docker-compose.launcher.yml", paths::override_file()),
    ] {
        let src = dir.join(name);
        if !src.exists() {
            continue;
        }
        std::fs::copy(&src, &dst).map_err(|e| {
            AppError::new(
                Code::UpdateFailed,
                format!("回滚时写回 {name} 失败：{e}（备份仍在 {}）", dir.display()),
            )
        })?;
        if name == ".env" {
            paths::chmod_600(&dst)?;
        }
        done.push(name.to_string());
    }
    if done.is_empty() {
        return Err(AppError::new(
            Code::UpdateFailed,
            format!("备份目录 {} 里没有可写回的配置文件", dir.display()),
        ));
    }
    crate::linfo!("已从备份 {id} 写回配置：{}", done.join("、"));
    Ok(done)
}

/// 写一个可读的提示文件到备份目录（用户翻到这里时知道这些文件是什么）。
pub fn write_readme(id: &str) -> AppResult<()> {
    let dir = dir_of(id);
    let m = read_meta(&dir).unwrap_or_default();
    write_readme_at(&dir, &m)
}

fn write_readme_at(dir: &Path, m: &BackupMeta) -> AppResult<()> {
    let text = format!(
        "\
这是 Hunter 启动器做的一份备份（{}）。

  {DUMP_NAME}                       数据库（pg_dump -Fc，自定义压缩格式）
  secrets.tar.gz                    密钥卷。**必须和数据库成对**，缺了它加密过的配置解不开
  user_skills.tar.gz                你自己建的技能
  opencode_data.tar.gz              历史对话
  .env                              配置。**里面有你的 key，不要分享给别人**
  docker-compose.yml                备份时那个版本的 compose
  docker-compose.launcher.yml       启动器生成的端口覆盖文件
  launcher.toml                     启动器自己的设置
  {META_NAME}                         这份备份的元信息（大小、sha256、表数行数、校验结论）

怎么恢复：打开 Hunter 启动器 → 运行面板 →「备份与恢复」→ 选中这一份 →「恢复到这一份」。
换了电脑也一样：把整个目录拷过去，在新机器的启动器里选「从别的目录选一份备份」。

恢复会先自动把当前数据备份一份，所以挑错了还找得回来。
",
        m.kind.cn()
    );
    let mut f = std::fs::File::create(dir.join("README.txt"))
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("写备份说明失败：{e}")))?;
    f.write_all(text.as_bytes())
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("写备份说明失败：{e}")))
}

// ── 结果落进配置（定时备份跑在没有界面的进程里，靠这几项让下一次打开时看得见）──

/// 一次备份的结果记进 `[backup]`。**成功清零失败计数，失败累加。**
pub fn record_result(ok: bool, err: Option<&str>) -> u32 {
    let mut cfg = config::LauncherConfig::load();
    cfg.backup.last_run_at = crate::timefmt::now_shanghai();
    if ok {
        cfg.backup.last_ok_at = cfg.backup.last_run_at.clone();
        cfg.backup.last_error = String::new();
        cfg.backup.fail_streak = 0;
    } else {
        cfg.backup.last_error = err.unwrap_or("（没有原因）").to_string();
        cfg.backup.fail_streak = cfg.backup.fail_streak.saturating_add(1);
    }
    let streak = cfg.backup.fail_streak;
    if let Err(e) = cfg.save() {
        crate::lwarn!("备份结果没记进配置：{}", e.msg);
    }
    streak
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, at: &str, kind: Kind) -> BackupMeta {
        BackupMeta {
            id: id.into(),
            at: at.into(),
            kind,
            tag: "1.2.0".into(),
            dump_bytes: Some(1024),
            ..Default::default()
        }
    }

    #[test]
    fn 备份目录名按方案的格式() {
        assert_eq!(
            make_id("2026-09-23 00:00:12", "1.2.0"),
            "hunter-20260923-0000-v1.2.0"
        );
    }

    #[test]
    fn 目录名能排序且新的在后() {
        let a = make_id("2026-09-23 00:00:00", "1.2.0");
        let b = make_id("2026-09-23 04:00:00", "1.2.0");
        assert!(a < b, "{a} 应该排在 {b} 前面");
    }

    #[test]
    fn 新旧两种目录名都读得出日期() {
        assert_eq!(
            day_of_id("hunter-20260923-0000-v1.2.0").as_deref(),
            Some("20260923")
        );
        // 0.1.12 及之前的命名
        assert_eq!(
            day_of_id("2026-09-20_031854-v1.1.0").as_deref(),
            Some("20260920")
        );
        // 认不出来的一律 None —— 那种目录永远不自动删
        assert_eq!(day_of_id("我自己建的文件夹"), None);
        assert_eq!(day_of_id("hunter-abc-v1.2.0"), None);
    }

    #[test]
    fn 有没有转储分得清楚() {
        let mut m = meta("x", "2026-09-23 00:00:00", Kind::Manual);
        assert!(m.has_dump());
        m.dump_bytes = None;
        m.dump_error = Some("postgres 没起来".into());
        assert!(!m.has_dump());
        // 老备份只有 sqlBytes，照样要算「有转储」
        m.sql_bytes = Some(999);
        assert!(m.has_dump());
        assert!(m.legacy_sql(), "只有 sqlBytes 的是老格式，恢复走 psql");
    }

    #[test]
    fn 转储字节数为零时不算有备份() {
        let mut m = meta("x", "2026-09-23 00:00:00", Kind::Manual);
        m.dump_bytes = Some(0);
        assert!(!m.has_dump(), "空文件不能算备份成功");
    }

    #[test]
    fn 老的_meta_json_读得出来且算成升级前备份() {
        // 0.1.12 写下的那种：只有六个字段，没有 kind
        let s = r#"{"id":"2026-09-20_031854-v1.1.0","tag":"1.1.0",
                    "at":"2026-09-20 03:18:54","sqlBytes":1234,"sqlError":null,
                    "files":[".env"]}"#;
        let m: BackupMeta = serde_json::from_str(s).expect("老格式必须读得出来");
        assert_eq!(m.id, "2026-09-20_031854-v1.1.0");
        assert!(m.has_dump());
        assert_eq!(m.kind, Kind::PreUpgrade, "没有 kind 的老备份算升级前备份");
        assert!(!m.kind.rotates(), "升级前备份不参与按天轮换");
    }

    #[test]
    fn 轮换_按天保留最后一份且只留最近三天() {
        let all = vec![
            meta(
                "hunter-20260923-2300-v1.2.0",
                "2026-09-23 23:00:00",
                Kind::Scheduled,
            ),
            meta(
                "hunter-20260923-0000-v1.2.0",
                "2026-09-23 00:00:00",
                Kind::Scheduled,
            ),
            meta(
                "hunter-20260922-0000-v1.2.0",
                "2026-09-22 00:00:00",
                Kind::Scheduled,
            ),
            meta(
                "hunter-20260921-0000-v1.2.0",
                "2026-09-21 00:00:00",
                Kind::Scheduled,
            ),
            meta(
                "hunter-20260920-0000-v1.2.0",
                "2026-09-20 00:00:00",
                Kind::Scheduled,
            ),
            meta(
                "hunter-20260919-0000-v1.2.0",
                "2026-09-19 00:00:00",
                Kind::Manual,
            ),
        ];
        let (keep, drop, skip) = plan_prune(&all, 3, 2);
        assert!(skip.is_empty(), "{skip:?}");
        // 最近三天各留一份：23 日留 23:00 那一份（同一天的最后一份）
        assert_eq!(
            keep,
            vec![
                "hunter-20260921-0000-v1.2.0".to_string(),
                "hunter-20260922-0000-v1.2.0".to_string(),
                "hunter-20260923-2300-v1.2.0".to_string(),
            ]
        );
        assert_eq!(
            drop,
            vec![
                "hunter-20260919-0000-v1.2.0".to_string(),
                "hunter-20260920-0000-v1.2.0".to_string(),
                "hunter-20260923-0000-v1.2.0".to_string(),
            ]
        );
    }

    #[test]
    fn 轮换_升级前备份单独留最近两份不参与按天() {
        let all = vec![
            meta(
                "hunter-20260923-1000-v1.2.0",
                "2026-09-23 10:00:00",
                Kind::PreUpgrade,
            ),
            meta(
                "hunter-20260922-1000-v1.2.0",
                "2026-09-22 10:00:00",
                Kind::PreUpgrade,
            ),
            meta(
                "hunter-20260901-1000-v1.1.0",
                "2026-09-01 10:00:00",
                Kind::PreUpgrade,
            ),
            meta(
                "hunter-20260923-0000-v1.2.0",
                "2026-09-23 00:00:00",
                Kind::Scheduled,
            ),
        ];
        let (keep, drop, _) = plan_prune(&all, 3, 2);
        assert!(keep.contains(&"hunter-20260923-1000-v1.2.0".to_string()));
        assert!(keep.contains(&"hunter-20260922-1000-v1.2.0".to_string()));
        assert!(
            keep.contains(&"hunter-20260923-0000-v1.2.0".to_string()),
            "同一天的定时备份不该被升级前备份挤掉：{keep:?}"
        );
        assert_eq!(drop, vec!["hunter-20260901-1000-v1.1.0".to_string()]);
    }

    #[test]
    fn 轮换_认不出日期的一份都不删() {
        let all = vec![
            meta("我自己建的文件夹", "2026-01-01 00:00:00", Kind::Manual),
            meta(
                "hunter-20260923-0000-v1.2.0",
                "2026-09-23 00:00:00",
                Kind::Manual,
            ),
        ];
        let (_, drop, skip) = plan_prune(&all, 1, 2);
        assert!(drop.is_empty(), "认不出日期的不能删：{drop:?}");
        assert_eq!(skip, vec!["我自己建的文件夹".to_string()]);
    }

    #[test]
    fn 轮换_保留天数被手改成零时也至少留一天() {
        let all = vec![
            meta(
                "hunter-20260923-0000-v1.2.0",
                "2026-09-23 00:00:00",
                Kind::Scheduled,
            ),
            meta(
                "hunter-20260922-0000-v1.2.0",
                "2026-09-22 00:00:00",
                Kind::Scheduled,
            ),
        ];
        let (keep, _, _) = plan_prune(&all, 0, 2);
        assert_eq!(keep, vec!["hunter-20260923-0000-v1.2.0".to_string()]);
    }

    #[test]
    fn 版本比较认得出补丁号与两位数() {
        assert!(version_newer("1.3.0", "1.2.0"));
        assert!(version_newer("1.10.0", "1.9.0"), "两位数不能按字符串比");
        assert!(!version_newer("1.2.0", "1.2.0"));
        assert!(!version_newer("1.2.0", "1.3.0"));
        assert!(version_newer("v1.2.1", "1.2.0"), "前面那个 v 不该影响判定");
    }

    #[test]
    fn 校验只数真正的目录项不数注释() {
        let out = "\
;
; Archive created at 2026-09-23 00:00:00 CST
;     dbname: hunter
;
215; 1259 16400 TABLE public users hunter
216; 1259 16410 TABLE public stocks hunter
";
        assert_eq!(count_toc(out), 2);
        assert_eq!(count_toc(";\n; 全是注释\n"), 0);
    }

    #[test]
    fn 备份命令走的是参数数组且带_exec_t_与项目名() {
        // 红线 3：不拼 shell 字符串。顺带盯住 `-T`（没有它在非 TTY 下会直接失败）
        let (program, args) = compose::argv(&["exec", "-T", "postgres", "pg_dump", "-Fc"]);
        assert!(
            program.ends_with("docker")
                || program.ends_with("docker.exe")
                || program.ends_with("docker-compose"),
            "{program}"
        );
        assert_eq!(args[0], "compose");
        assert!(args.iter().any(|a| a == "-T"), "{args:?}");
        assert!(args.iter().any(|a| a == "-Fc"), "{args:?}");
        assert!(
            args.iter().any(|a| a == "--project-name"),
            "备份也要带项目名，否则会去动别人的栈：{args:?}"
        );
    }

    #[test]
    fn 没有备份时摘要也有一行话() {
        assert!(!summary_lines().is_empty());
    }

    #[test]
    fn 删除守卫_只认备份根下面那一层() {
        let _h = paths::test_home("backup-guard");
        let root = legacy_dir();
        std::fs::create_dir_all(root.join("hunter-20260923-0000-v1.2.0")).unwrap();
        assert!(under_backup_root(&root.join("hunter-20260923-0000-v1.2.0")));
        // 再深一层不认（免得有人构造 `<备份根>/x/../../../etc`）
        assert!(!under_backup_root(&root.join("a").join("b")));
        // 家目录、根目录一律不认
        assert!(!under_backup_root(&paths::home()));
        assert!(!under_backup_root(std::path::Path::new("/")));
    }

    #[test]
    fn 同一分钟的第二次备份不会覆盖第一次() {
        let _h = paths::test_home("backup-unique");
        let base = legacy_dir();
        let (a, pa) = unique_dir(&base, "hunter-20260923-0000-v1.2.0");
        std::fs::create_dir_all(&pa).unwrap();
        let (b, _) = unique_dir(&base, "hunter-20260923-0000-v1.2.0");
        assert_eq!(a, "hunter-20260923-0000-v1.2.0");
        assert_eq!(b, "hunter-20260923-0000-v1.2.0-2");
    }

    #[test]
    fn 建议的备份目录一定在工作目录之外() {
        // 方案 R6 6.1：备份目录放在 ~/.hunter 之外，删除应用才波及不到
        let d = config::suggested_backup_dir();
        assert!(
            !d.starts_with(paths::root()),
            "备份目录 {} 不能落在工作目录 {} 里",
            d.display(),
            paths::root().display()
        );
    }
}
