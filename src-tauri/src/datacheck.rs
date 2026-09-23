//! 「重装之前先看看有没有旧数据」（I12 · R5）。
//!
//! ## 这件事要解决的
//!
//! 用户原话：「重新安装过程，也要分析，目前数据库是否存在，如果数据库和数据都存在，
//! 不要重新安装数据库和数据。」
//!
//! 0.1.9 起 `.env` 里的 `JWT_SECRET` / `POSTGRES_PASSWORD` / `OPENCODE_PASS` 已经会沿用
//! （`config::read_sticky`），所以「重装不掉登录」这一半是做到了的。缺的是另一半：
//! **数据卷本身在不在、里面那份库是不是还能用、要不要拦下这次安装。**
//!
//! ## 只读到什么程度
//!
//! 方案 R5 写的是「用一次性容器**只读**挂载查询」。实测做不到字面意义的只读 ——
//! PostgreSQL 打开一个数据目录时必然要写（WAL 回放、`postmaster.pid`、控制文件），
//! 数据目录挂成 `:ro` 的话它连启动都启不来。所以这里按**两段**来，
//! 把「真只读」和「不得不写一点」分得清清楚楚：
//!
//! | 段 | 做什么 | 会不会写 |
//! |---|---|---|
//! | 浅查（[`probe`]） | `docker volume ls` 看卷在不在；一个 `--rm` 的 busybox **`:ro`** 挂上去读 `PG_VERSION`；读 `.env` 里有没有 `JWT_SECRET` | **完全不写** |
//! | 深查（[`probe_deep`]） | 起一个 `--rm` 的 postgres 查表数、迁移版本、最近写入时间 | 会写（WAL 回放），所以**先用浅查确认大版本一致才做** |
//!
//! 大版本不一致（卷里是 17、目标镜像是 16）时**绝不进深查** —— 那正是会把数据目录
//! 弄坏的那一种操作。这一档直接判成「降级」，拦下来给人看。
//!
//! ## 版本号是什么（方案说的是 Alembic，实测不是）
//!
//! 方案 R5 写「查 Alembic 版本号」。测试机上实查：这套库里**没有 `alembic_version` 表**，
//! 上游用的是自己的 `schema_migrations`（列：`filename` / `checksum` / `applied_at`），
//! 迁移文件形如 `0022_schema_migrations.sql`，在 api 镜像里放在 `/opt/hunter-migrations/`。
//! 所以「版本」在这里就是**已应用的最大迁移编号**，目标版本则是目标 api 镜像里
//! 那个目录下的最大编号。以实测为准（总控规则）。

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::compose::{VolumeInfo, VOL_DB, VOL_SECRETS};
use crate::err::{AppError, AppResult, Code};
use crate::proc;
use crate::runtime::which;

/// 一次性容器给多久。
const SHORT: Duration = Duration::from_secs(30);
/// 等一次性 postgres 起来给多久。
const PG_WAIT: Duration = Duration::from_secs(60);
/// 深查用的一次性容器名。固定名字是为了**失败之后下一次还清得掉**。
const PROBE_NAME: &str = "hunter-datacheck-probe";

/// 检测结果对应的做法（方案 R5 的决策表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Decision {
    /// 什么都没有 —— 全新安装
    Fresh,
    /// 数据库 + 密钥卷 + `JWT_SECRET` 都在，版本不高于目标 → **直接沿用，不重建**
    Reuse,
    /// 数据库在，但密钥卷不见了 → 停下来说清后果
    MissingSecrets,
    /// 卷里的库比目标镜像新 → 拒绝直接降级
    Downgrade,
    /// 没有数据卷，但备份目录里有备份 → 提供「从备份恢复」
    BackupOnly,
}

impl Decision {
    /// 这一档要不要拦住安装、先问用户一句。
    pub fn blocks_install(self) -> bool {
        matches!(self, Decision::MissingSecrets | Decision::Downgrade)
    }
    pub fn cn(self) -> &'static str {
        match self {
            Decision::Fresh => "这台机器上没有以前的数据，按全新安装来",
            Decision::Reuse => "检测到你以前的数据，将直接沿用，不会重建",
            Decision::MissingSecrets => "数据库还在，但密钥卷不见了",
            Decision::Downgrade => "你的数据比要装的这一版新，不能直接装回去",
            Decision::BackupOnly => "没有找到数据卷，但备份目录里有备份",
        }
    }
}

/// 一次检测的全部结果。**每一项都是实测值**，问不到就是 `None` + 一句原因。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataCheck {
    pub decision: Decision,
    /// 本项目名下的全部数据卷（docker 标签筛出来的）
    pub volumes: Vec<VolumeInfo>,
    pub has_db: bool,
    pub has_secrets: bool,
    /// `~/.hunter/app/.env` 里有没有非空的 `JWT_SECRET`（**不打印它本身**，红线 2）
    pub has_jwt_secret: bool,
    /// 数据卷里那份库是哪个大版本（`PG_VERSION` 文件）
    pub pg_version: Option<String>,
    /// 这次要装的 postgres 镜像是哪个大版本
    pub target_pg_version: Option<String>,
    /// 已应用的最大迁移（例如 `0022_schema_migrations.sql`）。只有深查才有
    pub migration_max: Option<String>,
    /// 目标 api 镜像里最大的那个迁移文件。只有深查、而且镜像已在本机时才有
    pub target_migration_max: Option<String>,
    /// public schema 下有几张表。只有深查才有
    pub table_count: Option<u32>,
    /// 最近一次写入（所有带 `updated_at` 的表里取最大值）。只有深查才有
    pub last_write: Option<String>,
    /// 备份目录里有几份备份
    pub backups: usize,
    /// 其中有几份**含密钥卷**（I13 · R6）。
    ///
    /// 这一项是给「缺密钥卷」那一档用的：0.1.12 的备份只有数据库与配置，
    /// 那时候说「从备份恢复密钥卷」是一句空话（待办池 P1-32）。
    /// I13 的备份格式里有 `secrets.tar.gz` 了，所以这里如实数一下**真的有几份**
    /// —— 有才提这条路，没有就不提
    #[serde(default)]
    pub backups_with_secrets: usize,
    /// 深查做没做、为什么没做
    pub deep: bool,
    pub deep_skipped: Option<String>,
    /// 一句人话的结论，界面直接显示
    pub headline: String,
    /// 逐条证据（实测原话），折叠在「详情」里
    pub lines: Vec<String>,
    pub elapsed_ms: u64,
}

impl DataCheck {
    /// 「这一次安装可以直接沿用已有的数据库」。
    pub fn reuse(&self) -> bool {
        self.decision == Decision::Reuse
    }
}

/// 浅查：**一个字节都不写**。
pub fn probe() -> DataCheck {
    run(false)
}

/// 深查：浅查之后，再起一个一次性 postgres 把表数、迁移版本、最近写入问出来。
///
/// 大版本对不上时**自动降级成浅查**并说明原因 —— 那种情况下起 postgres
/// 正是会把数据目录弄坏的操作。
pub fn probe_deep() -> DataCheck {
    run(true)
}

fn run(deep: bool) -> DataCheck {
    let t0 = Instant::now();
    let mut lines: Vec<String> = Vec::new();

    let volumes = match crate::compose::volumes_of_project() {
        Ok(v) => {
            if v.is_empty() {
                lines.push("docker 里没有 compose 项目 hunter 名下的数据卷".into());
            } else {
                for x in &v {
                    let note = crate::compose::VOLUME_NOTES
                        .iter()
                        .find(|(k, _)| *k == x.short)
                        .map(|(_, n)| *n)
                        .unwrap_or("（compose 里声明的卷，用途未登记）");
                    lines.push(format!("数据卷 {} · {} · {note}", x.name, x.short));
                }
            }
            v
        }
        Err(e) => {
            lines.push(format!("列数据卷没问出来：{}", e.msg));
            Vec::new()
        }
    };

    let has_db = volumes.iter().any(|v| v.short == VOL_DB);
    let has_secrets = volumes.iter().any(|v| v.short == VOL_SECRETS);
    let sticky = crate::config::read_sticky(&crate::paths::env_file());
    let has_jwt_secret = sticky.jwt_secret.is_some();
    lines.push(format!(
        "{} 里的 JWT_SECRET：{}",
        crate::redact::mask_home(&crate::paths::env_file().to_string_lossy()),
        if has_jwt_secret {
            "在（重装会原样沿用，登录不会失效）"
        } else {
            "不在"
        }
    ));

    let all_backups = crate::backup::list();
    let backups = all_backups.len();
    let backups_with_secrets = all_backups
        .iter()
        .filter(|m| {
            m.volumes
                .iter()
                .any(|v| v.short == crate::compose::VOL_SECRETS && v.bytes.is_some())
        })
        .count();
    if backups > 0 {
        lines.push(format!(
            "备份目录里有 {backups} 份备份，其中 {backups_with_secrets} 份带着密钥卷"
        ));
    }

    // 卷里那份库是哪个大版本。一个 `--rm` 的一次性容器，数据卷挂成 `:ro`
    let (pg_version, target_pg_version) = if has_db {
        let v = read_pg_version(&db_volume_name(&volumes));
        match &v {
            Ok(x) => lines.push(format!("数据卷里的 PostgreSQL 大版本：{x}")),
            Err(e) => lines.push(format!("读不到数据卷里的 PG_VERSION：{e}")),
        }
        let t = target_pg_major();
        match &t {
            Some(x) => lines.push(format!("这次要装的 postgres 镜像是 {x} 系")),
            None => lines.push("看不出这次要装的 postgres 是哪个大版本".into()),
        }
        (v.ok(), t)
    } else {
        (None, None)
    };

    let mut c = DataCheck {
        decision: Decision::Fresh,
        volumes,
        has_db,
        has_secrets,
        has_jwt_secret,
        pg_version,
        target_pg_version,
        migration_max: None,
        target_migration_max: None,
        table_count: None,
        last_write: None,
        backups,
        backups_with_secrets,
        deep: false,
        deep_skipped: None,
        headline: String::new(),
        lines,
        elapsed_ms: 0,
    };

    // ── 决策 ──────────────────────────────────────────────────────────
    //
    // 顺序有意义：先判「不能装」的两档，再判「可以沿用」。
    // 「密钥卷缺了」排在版本之前 —— 版本再对，配置解不开也是白装。
    if !c.has_db {
        c.decision = if c.backups > 0 {
            Decision::BackupOnly
        } else {
            Decision::Fresh
        };
        return finish(c, t0);
    }

    // PG 大版本：卷里的比目标镜像新 = 降级，直接拦下（**这一档不许进深查**）
    if let (Some(have), Some(want)) = (&c.pg_version, &c.target_pg_version) {
        if major(have) > major(want) {
            c.decision = Decision::Downgrade;
            c.deep_skipped = Some(format!(
                "数据卷里是 PostgreSQL {have}，这次要装的是 {want} —— \
                 用 {want} 去打开 {have} 的数据目录会把它弄坏，所以连查都不查"
            ));
            return finish(c, t0);
        }
    }

    if !c.has_secrets {
        c.decision = Decision::MissingSecrets;
    } else {
        c.decision = Decision::Reuse;
    }

    // ── 深查 ──────────────────────────────────────────────────────────
    if deep {
        match c.pg_version.as_deref().zip(c.target_pg_version.as_deref()) {
            Some((have, want)) if major(have) != major(want) => {
                c.deep_skipped = Some(format!(
                    "数据卷里是 PostgreSQL {have}、镜像是 {want}，大版本对不上，不去起它"
                ));
            }
            None => {
                c.deep_skipped =
                    Some("没能确认数据卷与镜像的 PostgreSQL 大版本一致，稳妥起见不去起它".into());
            }
            Some(_) => match deep_query(&c) {
                Ok(d) => {
                    c.deep = true;
                    c.table_count = d.tables;
                    c.migration_max = d.migration_max.clone();
                    c.last_write = d.last_write.clone();
                    c.lines.extend(d.lines);
                    // 迁移编号比目标镜像高 = 降级
                    c.target_migration_max = target_migration_max();
                    if let (Some(have), Some(want)) = (&c.migration_max, &c.target_migration_max) {
                        c.lines
                            .push(format!("已应用到 {have}；这次要装的镜像里最大的是 {want}"));
                        if mig_num(have) > mig_num(want) {
                            c.decision = Decision::Downgrade;
                        }
                    }
                }
                Err(e) => {
                    c.deep_skipped = Some(e.msg);
                }
            },
        }
    }
    finish(c, t0)
}

fn finish(mut c: DataCheck, t0: Instant) -> DataCheck {
    c.elapsed_ms = t0.elapsed().as_millis() as u64;
    c.headline = headline(&c);
    c
}

/// 结论那一句人话。**数字全部来自上面真问到的值**，问不到就不提那个数（红线 1）。
fn headline(c: &DataCheck) -> String {
    match c.decision {
        Decision::Fresh => "这台机器上没有以前的 Hunter 数据，这次是全新安装。".to_string(),
        Decision::BackupOnly => format!(
            "没有找到以前的数据卷，但备份目录里有 {} 份备份 —— 可以从最近的一份恢复，也可以全新开始。",
            c.backups
        ),
        Decision::Reuse => {
            let mut s = String::from("检测到你以前的数据");
            match (c.table_count, c.last_write.as_deref()) {
                (Some(n), Some(w)) => s.push_str(&format!("（{n} 张表，最近更新于 {w}）")),
                (Some(n), None) => s.push_str(&format!("（{n} 张表）")),
                _ => {}
            }
            s.push_str("，将直接沿用，不会重建数据库、也不会重新生成密钥。");
            s
        }
        Decision::MissingSecrets => {
            let base = "数据库还在，但密钥卷（hunter_secrets）不见了。\
                        数据库里用它加密过的配置解不开 —— 别的数据不受影响。";
            // I13：备份格式里有 `secrets.tar.gz` 之后，这一档才**真的**有第二条路
            if c.backups_with_secrets > 0 {
                format!(
                    "{base}备份目录里有 {} 份带着密钥卷的备份，\
                     装完之后可以在「备份与恢复」里把它恢复回来。",
                    c.backups_with_secrets
                )
            } else {
                base.to_string()
            }
        }
        Decision::Downgrade => {
            let a = c.pg_version.as_deref().unwrap_or("—");
            let b = c.target_pg_version.as_deref().unwrap_or("—");
            match (&c.migration_max, &c.target_migration_max) {
                (Some(h), Some(w)) => format!(
                    "你的数据已经跑到 {h}，而这次要装的这一版只到 {w} —— \
                     装回去会对不上，先升级镜像或者从备份恢复。"
                ),
                _ => format!(
                    "数据卷里是 PostgreSQL {a}，这次要装的是 {b} —— 不能直接降级。"
                ),
            }
        }
    }
}

// ── 卷里那份库的大版本（真·只读）────────────────────────────────────────

fn db_volume_name(volumes: &[VolumeInfo]) -> String {
    volumes
        .iter()
        .find(|v| v.short == VOL_DB)
        .map(|v| v.name.clone())
        .unwrap_or_else(|| format!("{}_{VOL_DB}", crate::config::PROJECT))
}

/// 读数据卷里的 `PG_VERSION`。
///
/// 一次性 busybox 容器，数据卷**挂成 `:ro`** —— 这一步是字面意义的只读。
/// 用的是 `base_prefix` 下的 busybox（国内源下也有），拉不到就如实报错。
fn read_pg_version(volume: &str) -> Result<String, String> {
    let bin = which::docker_bin();
    let mount = format!("{volume}:/data:ro");
    let image = helper_image();
    let r = proc::run_timeout(
        &bin,
        &[
            "run",
            "--rm",
            "-v",
            &mount,
            "--entrypoint",
            "cat",
            &image,
            "/data/PG_VERSION",
        ],
        SHORT,
    )
    .map_err(|e| e.msg)?;
    if !r.ok() {
        return Err(format!("一次性容器读不到 PG_VERSION：{}", r.err_line()));
    }
    let v = r.stdout.trim().to_string();
    if v.is_empty() {
        return Err("PG_VERSION 是空的".to_string());
    }
    Ok(v)
}

/// 一次性容器用哪个镜像。直接用**这次要装的那个 postgres 镜像** ——
/// 它本机一定有（或者马上要拉），不引入第四个镜像。
fn helper_image() -> String {
    let cfg = crate::config::LauncherConfig::load();
    if cfg.hunter.base_prefix == "docker.io/library" {
        "postgres:16-alpine".to_string()
    } else {
        format!("{}/postgres:16-alpine", cfg.hunter.base_prefix)
    }
}

/// 目标 postgres 镜像的大版本。从镜像引用里切（`postgres:16-alpine` → `16`）。
fn target_pg_major() -> Option<String> {
    let img = helper_image();
    let tag = img.rsplit(':').next()?;
    let major = tag.split('-').next()?;
    major
        .chars()
        .all(|c| c.is_ascii_digit())
        .then(|| major.to_string())
}

fn major(v: &str) -> u32 {
    v.trim()
        .split('.')
        .next()
        .and_then(|x| x.parse().ok())
        .unwrap_or(0)
}

/// `0022_schema_migrations.sql` → 22。认不出来就是 0。
pub fn mig_num(s: &str) -> u32 {
    let head: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    head.parse().unwrap_or(0)
}

// ── 深查：起一个一次性 postgres 问三件事 ────────────────────────────────

struct Deep {
    tables: Option<u32>,
    migration_max: Option<String>,
    last_write: Option<String>,
    lines: Vec<String>,
}

fn deep_query(c: &DataCheck) -> AppResult<Deep> {
    let bin = which::docker_bin();
    let env = crate::config::parse_env_file(&crate::paths::env_file());
    let user = env.get("POSTGRES_USER").cloned().unwrap_or_default();
    let db = env.get("POSTGRES_DB").cloned().unwrap_or_default();
    let pw = env.get("POSTGRES_PASSWORD").cloned().unwrap_or_default();
    if user.is_empty() || db.is_empty() || pw.is_empty() {
        return Err(AppError::new(
            Code::Unknown,
            format!(
                "{} 里没有 POSTGRES_USER / DB / PASSWORD，问不了数据库",
                crate::redact::mask_home(&crate::paths::env_file().to_string_lossy())
            ),
        ));
    }

    // 已经有一个在跑的 postgres 容器就直接用它 —— 那是最便宜、也最不会出事的一条路
    if let Some(name) = running_postgres() {
        let mut lines = vec![format!("直接问正在跑的 {name}，没有起新容器")];
        let d = query_via(&bin, &name, &user, &db, &mut lines)?;
        return Ok(Deep { lines, ..d });
    }

    let mut lines = Vec::new();
    // 起一个一次性的。名字固定 → 上一次没清干净的这次也清得掉
    let _ = proc::run_timeout(&bin, &["rm", "-f", PROBE_NAME], SHORT);
    let volume = db_volume_name(&c.volumes);
    let mount = format!("{volume}:/var/lib/postgresql/data");
    let pwenv = format!("POSTGRES_PASSWORD={pw}");
    let image = helper_image();
    let r = proc::run_timeout(
        &bin,
        &[
            "run", "-d", "--rm", "--name", PROBE_NAME, "-v", &mount, "-e", &pwenv, &image,
        ],
        Duration::from_secs(120),
    )?;
    if !r.ok() {
        return Err(AppError::new(
            Code::Unknown,
            format!("起一次性 postgres 失败：{}", r.err_line()),
        ));
    }
    lines.push(format!(
        "起了一个一次性容器（{image}，用完就删）把数据卷挂上去问了三件事"
    ));

    let out = (|| -> AppResult<Deep> {
        wait_ready(&bin, &user, &db)?;
        query_via(&bin, PROBE_NAME, &user, &db, &mut lines)
    })();

    // 不管问没问到，一次性容器都要收掉
    match proc::run_timeout(&bin, &["rm", "-f", PROBE_NAME], SHORT) {
        Ok(r) if r.ok() => lines.push("一次性容器已经删掉".into()),
        Ok(r) => crate::lwarn!("一次性容器没删掉：{}", r.err_line()),
        Err(e) => crate::lwarn!("一次性容器没删掉：{}", e.msg),
    }
    let d = out?;
    Ok(Deep { lines, ..d })
}

/// 本项目**正在跑**的 postgres 容器。没有就是 `None`。
fn running_postgres() -> Option<String> {
    let names = crate::compose::container_names();
    let name = names.get("postgres")?.clone();
    let bin = which::docker_bin();
    let r = proc::run_timeout(
        &bin,
        &["inspect", &name, "--format", "{{.State.Running}}"],
        SHORT,
    )
    .ok()?;
    (r.ok() && r.stdout.trim() == "true").then_some(name)
}

fn wait_ready(bin: &str, user: &str, db: &str) -> AppResult<()> {
    let t0 = Instant::now();
    let mut last = String::new();
    while t0.elapsed() < PG_WAIT {
        if let Ok(r) = proc::run_timeout(
            bin,
            &["exec", PROBE_NAME, "pg_isready", "-U", user, "-d", db],
            Duration::from_secs(10),
        ) {
            if r.ok() {
                return Ok(());
            }
            last = r.err_line();
        }
        std::thread::sleep(Duration::from_millis(800));
    }
    Err(AppError::new(
        Code::StartTimeout,
        format!(
            "一次性 postgres 在 {} 秒内没起来：{last}",
            PG_WAIT.as_secs()
        ),
    ))
}

fn query_via(
    bin: &str,
    container: &str,
    user: &str,
    db: &str,
    lines: &mut Vec<String>,
) -> AppResult<Deep> {
    let mut d = Deep {
        tables: None,
        migration_max: None,
        last_write: None,
        lines: Vec::new(),
    };

    let q = |sql: &str| -> Option<String> {
        let r = proc::run_timeout(
            bin,
            &["exec", container, "psql", "-U", user, "-d", db, "-tAc", sql],
            Duration::from_secs(30),
        )
        .ok()?;
        r.ok().then(|| r.stdout.trim().to_string())
    };

    match q("select count(*) from information_schema.tables where table_schema='public'")
        .and_then(|s| s.parse::<u32>().ok())
    {
        Some(n) => {
            d.tables = Some(n);
            lines.push(format!("public schema 下有 {n} 张表"));
        }
        None => lines.push("数了一下表，没数出来".into()),
    }

    // 上游用的是自己的 `schema_migrations`（不是 Alembic，测试机实查确认）
    match q("select filename from schema_migrations order by filename desc limit 1")
        .filter(|s| !s.is_empty())
    {
        Some(f) => {
            lines.push(format!("已应用的最大迁移：{f}"));
            d.migration_max = Some(f);
        }
        None => lines.push(
            "没有 schema_migrations 表，或者它是空的（上游用的不是 Alembic，是这张表）".into(),
        ),
    }

    // 最近一次写入：所有带 updated_at 的表里取最大值。
    // 一条 SQL 拼出来跑 —— 表名来自 information_schema，都是标识符，不是用户输入
    match q(LAST_WRITE_SQL).filter(|s| !s.is_empty()) {
        Some(w) => {
            lines.push(format!("最近一次写入：{w}"));
            d.last_write = Some(w);
        }
        None => lines.push("带 updated_at 的表里没问出最近写入时间".into()),
    }
    Ok(d)
}

/// 「最近一次写入」的查询。
///
/// 动态拼出 `select max(updated_at) from <表>` 的并集再取最大值。
/// 表名来自 `information_schema.columns`，用 `quote_ident` 转义之后才拼进去 ——
/// 这些都是数据库自己报的标识符，不是用户输入，但转义一分钱不要，就转。
const LAST_WRITE_SQL: &str = "\
select to_char(max(m), 'YYYY-MM-DD HH24:MI') from ( \
  select (xpath('/row/max/text()', query_to_xml( \
    format('select max(updated_at) as max from %I.%I', table_schema, table_name), \
    false, true, '')))[1]::text::timestamptz as m \
  from information_schema.columns \
  where table_schema='public' and column_name='updated_at' \
) t";

/// 目标 api 镜像里最大的那个迁移文件名。
///
/// 起一个 `--rm` 的一次性容器 `ls /opt/hunter-migrations`（镜像里那个目录，
/// 测试机实查确认）。镜像还没拉下来的话这一步拿不到 → `None`，**不猜**。
fn target_migration_max() -> Option<String> {
    let cfg = crate::config::LauncherConfig::load();
    let api = crate::config::images(
        &cfg.hunter.registry_prefix,
        &cfg.hunter.base_prefix,
        &cfg.hunter.tag,
    )
    .into_iter()
    .find(|i| i.service == "api")?
    .reference;
    let bin = which::docker_bin();
    let r = proc::run_timeout(
        &bin,
        &[
            "run",
            "--rm",
            "--entrypoint",
            "sh",
            &api,
            "-c",
            "ls /opt/hunter-migrations 2>/dev/null | sort | tail -1",
        ],
        SHORT,
    )
    .ok()?;
    let s = r.stdout.trim().to_string();
    (r.ok() && !s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> DataCheck {
        DataCheck {
            decision: Decision::Fresh,
            volumes: Vec::new(),
            has_db: false,
            has_secrets: false,
            has_jwt_secret: false,
            pg_version: None,
            target_pg_version: None,
            migration_max: None,
            target_migration_max: None,
            table_count: None,
            last_write: None,
            backups: 0,
            backups_with_secrets: 0,
            deep: false,
            deep_skipped: None,
            headline: String::new(),
            lines: Vec::new(),
            elapsed_ms: 0,
        }
    }

    #[test]
    fn 迁移编号从文件名切出来() {
        assert_eq!(mig_num("0022_schema_migrations.sql"), 22);
        assert_eq!(mig_num("0009_compliance_violation.sql"), 9);
        assert_eq!(mig_num("看不懂"), 0);
    }

    #[test]
    fn 决策表_什么都没有就是全新安装() {
        let mut c = base();
        c.decision = Decision::Fresh;
        c.headline = headline(&c);
        assert!(c.headline.contains("全新安装"), "{}", c.headline);
        assert!(!c.decision.blocks_install());
    }

    #[test]
    fn 决策表_没有卷但有备份() {
        let mut c = base();
        c.backups = 3;
        c.decision = Decision::BackupOnly;
        c.headline = headline(&c);
        assert!(c.headline.contains("3 份备份"), "{}", c.headline);
        assert!(!c.decision.blocks_install());
    }

    #[test]
    fn 决策表_都在就直接沿用_而且把实测数字说出来() {
        let mut c = base();
        c.has_db = true;
        c.has_secrets = true;
        c.has_jwt_secret = true;
        c.table_count = Some(61);
        c.last_write = Some("2026-09-22 20:22".into());
        c.decision = Decision::Reuse;
        c.headline = headline(&c);
        assert!(c.headline.contains("61 张表"), "{}", c.headline);
        assert!(c.headline.contains("2026-09-22 20:22"), "{}", c.headline);
        assert!(c.headline.contains("不会重建"), "{}", c.headline);
        assert!(!c.decision.blocks_install());
        assert!(c.reuse());
    }

    #[test]
    fn 决策表_沿用但还没深查时不提表数() {
        // 数字没问到就一个字都不提 —— 不拿 0 冒充（红线 1）
        let mut c = base();
        c.has_db = true;
        c.has_secrets = true;
        c.decision = Decision::Reuse;
        c.headline = headline(&c);
        assert!(!c.headline.contains("张表"), "{}", c.headline);
        assert!(c.headline.contains("直接沿用"), "{}", c.headline);
    }

    #[test]
    fn 决策表_缺密钥卷要拦下来() {
        let mut c = base();
        c.has_db = true;
        c.has_secrets = false;
        c.decision = Decision::MissingSecrets;
        c.headline = headline(&c);
        assert!(c.decision.blocks_install(), "缺密钥卷必须先问用户");
        assert!(c.headline.contains("解不开"), "{}", c.headline);
        assert!(
            c.headline.contains("别的数据不受影响"),
            "要说清后果的边界：{}",
            c.headline
        );
    }

    #[test]
    fn 决策表_降级要拦下来_pg大版本() {
        let mut c = base();
        c.has_db = true;
        c.pg_version = Some("17".into());
        c.target_pg_version = Some("16".into());
        c.decision = Decision::Downgrade;
        c.headline = headline(&c);
        assert!(c.decision.blocks_install());
        assert!(c.headline.contains("PostgreSQL 17"), "{}", c.headline);
        assert!(c.headline.contains("16"), "{}", c.headline);
    }

    #[test]
    fn 决策表_降级要拦下来_迁移编号() {
        let mut c = base();
        c.has_db = true;
        c.migration_max = Some("0030_new.sql".into());
        c.target_migration_max = Some("0022_schema_migrations.sql".into());
        c.decision = Decision::Downgrade;
        c.headline = headline(&c);
        assert!(c.headline.contains("0030_new.sql"), "{}", c.headline);
        assert!(
            c.headline.contains("0022_schema_migrations.sql"),
            "{}",
            c.headline
        );
        assert!(mig_num("0030_new.sql") > mig_num("0022_schema_migrations.sql"));
    }

    #[test]
    fn 大版本比较() {
        assert_eq!(major("16"), 16);
        assert_eq!(major("17\n"), 17);
        assert_eq!(major("看不懂"), 0);
    }

    #[test]
    fn 目标_pg_大版本从镜像引用里切() {
        // 默认是 docker.io/library/postgres:16-alpine
        assert_eq!(target_pg_major().as_deref(), Some("16"));
    }

    /// I13 · R6：备份里真的有密钥卷了，「缺密钥卷」那一档才提得起第二条路。
    ///
    /// 0.1.12 的备份只有数据库与配置 —— 那时候说「从备份恢复密钥卷」是一句空话
    /// （待办池 P1-32 记的就是这件事）。所以这里钉的是「有才说，没有不说」。
    #[test]
    fn 缺密钥卷_有带密钥卷的备份时才提那条路() {
        let mut c = base();
        c.has_db = true;
        c.has_jwt_secret = true;
        c.decision = Decision::MissingSecrets;

        c.backups = 3;
        c.backups_with_secrets = 0;
        c.headline = headline(&c);
        assert!(c.headline.contains("解不开"), "{}", c.headline);
        assert!(
            !c.headline.contains("备份与恢复"),
            "一份带密钥卷的备份都没有时，不该提一条走不通的路：{}",
            c.headline
        );

        c.backups_with_secrets = 2;
        c.headline = headline(&c);
        assert!(c.headline.contains("2 份"), "{}", c.headline);
        assert!(c.headline.contains("备份与恢复"), "{}", c.headline);
    }
}