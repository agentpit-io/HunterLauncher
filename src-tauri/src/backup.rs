//! 升级前的备份与回滚素材（技术方案 §10「升级前自动 `pg_dump` 到 `~/.hunter/backups/`」）。
//!
//! 一次备份是 `~/.hunter/backups/<上海时间戳>-v<tag>/` 下的四个文件：
//!
//! | 文件 | 内容 | 回滚时怎么用 |
//! |---|---|---|
//! | `hunter.sql` | `pg_dump --clean --if-exists` 的全库转储 | 数据真被新版本迁坏了才用，见下面「为什么默认不自动恢复数据」 |
//! | `.env` | 升级前那份（含 key，所以整个备份目录是 700 / 文件 600） | 回滚时原样写回去 |
//! | `docker-compose.yml` | 旧 tag 的 compose | 同上 |
//! | `docker-compose.launcher.yml` | 旧的覆盖文件（端口） | 同上 |
//! | `meta.json` | tag、时间、各文件字节数、pg_dump 的退出情况 | 界面上列备份用 |
//!
//! ## 两个刻意的决定
//!
//! **一、`pg_dump` 的输出直接落盘，不进内存。** 用户的库可能有几百 MB，
//! 先攒成 `String` 再写文件等于把它翻倍塞进内存。所以这里不用 [`crate::compose::run`]，
//! 而是拿 [`crate::compose::argv`] 自己起子进程、把 stdout 接到文件上。
//!
//! **二、回滚默认只还原配置，不自动灌回数据。** Hunter 的迁移是 Alembic 往前做的，
//! 升级失败通常发生在「镜像拉不下来」或「服务起不来」，这两种情况下**数据库根本没被动过**，
//! 这时候灌回一份转储反而会把升级中途写进去的东西抹掉。所以：
//! 回滚 = 配置回到旧 tag + 用旧镜像重新 `up -d`；数据转储留在盘上，
//! 真出现「数据被迁坏了」时由用户显式点「从备份恢复数据」（[`restore_db`]）。
//! 这条决定写进了成果文档。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::err::{AppError, AppResult, Code};
use crate::{compose, config, paths};

/// `pg_dump` 的上限。大库慢盘也够了；卡死在这一步比升级失败更难解释。
const DUMP_TIMEOUT: Duration = Duration::from_secs(600);
/// `psql` 灌回的上限。
const RESTORE_TIMEOUT: Duration = Duration::from_secs(900);

/// 一次备份的元信息。写进备份目录的 `meta.json`，界面也用它列清单。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupMeta {
    /// 备份目录名，形如 `2026-09-20_031854-v1.1.0`
    pub id: String,
    /// 备份时 Hunter 的 tag
    pub tag: String,
    /// 上海时间 `YYYY-MM-DD HH:MM:SS`
    pub at: String,
    /// 数据库转储的字节数；没转成就是 `None`
    pub sql_bytes: Option<u64>,
    /// 转储没成功时的原因（如实写，不遮掩）
    pub sql_error: Option<String>,
    /// 配置文件备份了哪几个
    pub files: Vec<String>,
}

impl BackupMeta {
    /// 这次备份到底有没有把数据库存下来。界面上「备份文件存在」指的就是它。
    pub fn has_dump(&self) -> bool {
        self.sql_bytes.unwrap_or(0) > 0
    }
}

pub fn dir_of(id: &str) -> PathBuf {
    paths::backups_dir().join(id)
}

pub fn sql_path(id: &str) -> PathBuf {
    dir_of(id).join("hunter.sql")
}

/// 备份目录名。用上海时间，和日志、成果文档里的时间对得上。
pub fn make_id(at_shanghai: &str, tag: &str) -> String {
    // `2026-09-20 03:18:54` → `2026-09-20_031854`
    let stamp = match at_shanghai.split_once(' ') {
        Some((d, t)) => format!("{d}_{}", t.replace(':', "")),
        None => at_shanghai.replace([' ', ':'], ""),
    };
    format!("{stamp}-v{tag}")
}

/// 做一次完整备份。**升级流程的第一步，失败就不往下走**。
///
/// `postgres` 容器没在跑时会先把它单独拉起来（`up -d postgres`）——
/// 用户完全可能在栈停着的时候点升级，为这个让他先去启动一遍不合理。
pub fn create(tag: &str, mut note: impl FnMut(&str)) -> AppResult<BackupMeta> {
    paths::ensure_dirs()?;
    let at = crate::timefmt::now_shanghai();
    let id = make_id(&at, tag);
    let dir = dir_of(&id);
    std::fs::create_dir_all(&dir).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("建备份目录 {} 失败：{e}", dir.display()),
        )
    })?;
    // 备份目录里有 .env（带 key），整个目录收成 700
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }

    // ① 配置三件套
    let mut files = Vec::new();
    for (src, name) in [
        (paths::env_file(), ".env"),
        (paths::compose_file(), "docker-compose.yml"),
        (paths::override_file(), "docker-compose.launcher.yml"),
        (paths::launcher_toml(), "launcher.toml"),
    ] {
        if src.exists() {
            let dst = dir.join(name);
            std::fs::copy(&src, &dst)
                .map_err(|e| AppError::new(Code::ConfigWrite, format!("备份 {name} 失败：{e}")))?;
            if name == ".env" {
                paths::chmod_600(&dst)?;
            }
            files.push(name.to_string());
        }
    }
    note(&format!("已备份配置：{}", files.join("、")));

    // ② 数据库
    let (sql_bytes, sql_error) = match dump_db(&sql_path(&id), &mut note) {
        Ok(n) => (Some(n), None),
        Err(e) => (None, Some(e.msg)),
    };

    let meta = BackupMeta {
        id: id.clone(),
        tag: tag.to_string(),
        at,
        sql_bytes,
        sql_error: sql_error.clone(),
        files,
    };
    write_meta(&dir, &meta)?;

    match (sql_bytes, &sql_error) {
        (Some(n), _) => note(&format!(
            "已备份数据库：{} （{}）",
            sql_path(&id).display(),
            crate::flow::human_bytes(n)
        )),
        (None, Some(e)) => note(&format!("数据库没备份成功：{e}")),
        (None, None) => {}
    }
    crate::linfo!(
        "备份完成 {} · sql={:?} 字节 · 错误={:?}",
        id,
        sql_bytes,
        sql_error
    );
    Ok(meta)
}

fn write_meta(dir: &Path, meta: &BackupMeta) -> AppResult<()> {
    let s = serde_json::to_string_pretty(meta)
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("meta.json 序列化失败：{e}")))?;
    std::fs::write(dir.join("meta.json"), s)
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("写 meta.json 失败：{e}")))
}

/// `docker compose exec -T postgres pg_dump …` → 直接写进 `out`。返回字节数。
fn dump_db(out: &Path, note: &mut impl FnMut(&str)) -> AppResult<u64> {
    ensure_postgres_up(note)?;
    let env = config::parse_env_file(&paths::env_file());
    let user = env
        .get("POSTGRES_USER")
        .cloned()
        .unwrap_or_else(|| "hunter".into());
    let db = env
        .get("POSTGRES_DB")
        .cloned()
        .unwrap_or_else(|| "hunter".into());

    // `--clean --if-exists`：恢复时先把同名对象删掉，能灌进一个非空的库。
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
        "--clean",
        "--if-exists",
        "--no-owner",
    ]);
    note(&format!("正在 pg_dump 数据库 {db}（用户 {user}）…"));

    let file = std::fs::File::create(out).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("建备份文件 {} 失败：{e}", out.display()),
        )
    })?;
    let mut child = crate::proc::base_command(&program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::new(Code::UpdateFailed, format!("起不了 pg_dump：{e}")))?;

    let deadline = std::time::Instant::now() + DUMP_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AppError::new(
                        Code::UpdateFailed,
                        format!("pg_dump 超过 {} 秒没有结束", DUMP_TIMEOUT.as_secs()),
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return Err(AppError::new(
                    Code::UpdateFailed,
                    format!("等 pg_dump：{e}"),
                ))
            }
        }
    };
    let mut err = String::new();
    if let Some(mut s) = child.stderr.take() {
        use std::io::Read;
        let _ = s.read_to_string(&mut err);
    }
    if !status.success() {
        let _ = std::fs::remove_file(out);
        return Err(AppError::new(
            Code::UpdateFailed,
            format!(
                "pg_dump 退出码 {:?}：{}",
                status.code(),
                err.trim().lines().next_back().unwrap_or("（没有输出）")
            ),
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

/// postgres 没在跑就单独把它拉起来并等到能接受连接。
fn ensure_postgres_up(note: &mut impl FnMut(&str)) -> AppResult<()> {
    let up = compose::ps()
        .unwrap_or_default()
        .iter()
        .any(|s| s.service == "postgres" && s.state == "running");
    if up {
        return Ok(());
    }
    note("postgres 没在运行，先把它单独拉起来再备份");
    // 这一句也是 `up -d`，同样要先确认 `hunter` 这个项目名是我们的（待办池 P0-5）——
    // 备份跑在升级之前，比 `compose::up` 那道闸门更早
    compose::guard_project_owner()?;
    let r = compose::run(&["up", "-d", "postgres"], Duration::from_secs(180))?;
    if !r.ok() {
        return Err(AppError::new(
            Code::UpdateFailed,
            format!("拉起 postgres 失败：{}", r.err_line()),
        ));
    }
    // `pg_isready` 是 postgres 镜像自带的，比盲等靠谱
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    loop {
        let r = compose::run(
            &["exec", "-T", "postgres", "pg_isready"],
            Duration::from_secs(20),
        );
        if matches!(&r, Ok(x) if x.ok()) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(AppError::new(
                Code::UpdateFailed,
                "等了 90 秒 postgres 还没准备好接受连接".to_string(),
            ));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// 把配置三件套从备份目录写回去（回滚用）。**不碰数据库**。
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

/// 把某次备份的数据库转储灌回去。**用户显式要求时才做**（见模块头的决定二）。
pub fn restore_db(id: &str) -> AppResult<String> {
    let sql = sql_path(id);
    if !sql.exists() {
        return Err(AppError::new(
            Code::UpdateFailed,
            format!("这次备份里没有数据库转储：{}", sql.display()),
        ));
    }
    let env = config::parse_env_file(&paths::env_file());
    let user = env
        .get("POSTGRES_USER")
        .cloned()
        .unwrap_or_else(|| "hunter".into());
    let db = env
        .get("POSTGRES_DB")
        .cloned()
        .unwrap_or_else(|| "hunter".into());
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
    let file = std::fs::File::open(&sql)
        .map_err(|e| AppError::new(Code::UpdateFailed, format!("读不了转储文件：{e}")))?;
    let mut child = crate::proc::base_command(&program)
        .args(&args)
        .stdin(Stdio::from(file))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::new(Code::UpdateFailed, format!("起不了 psql：{e}")))?;

    let deadline = std::time::Instant::now() + RESTORE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AppError::new(
                        Code::UpdateFailed,
                        format!("psql 超过 {} 秒没有结束", RESTORE_TIMEOUT.as_secs()),
                    ));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(AppError::new(Code::UpdateFailed, format!("等 psql：{e}"))),
        }
    };
    let mut err = String::new();
    if let Some(mut s) = child.stderr.take() {
        use std::io::Read;
        let _ = s.read_to_string(&mut err);
    }
    if !status.success() {
        return Err(AppError::new(
            Code::UpdateFailed,
            format!(
                "psql 退出码 {:?}：{}",
                status.code(),
                err.trim().lines().next_back().unwrap_or("（没有输出）")
            ),
        ));
    }
    let n = std::fs::metadata(&sql).map(|m| m.len()).unwrap_or(0);
    crate::linfo!("已从备份 {id} 恢复数据库（{n} 字节的转储）");
    Ok(format!(
        "已把备份 {id} 的数据库转储（{}）灌回去。",
        crate::flow::human_bytes(n)
    ))
}

/// 列出 `~/.hunter/backups/` 下的全部备份，**新的在前**。
pub fn list() -> Vec<BackupMeta> {
    let mut out: Vec<BackupMeta> = Vec::new();
    let Ok(rd) = std::fs::read_dir(paths::backups_dir()) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path().join("meta.json");
        if let Ok(s) = std::fs::read_to_string(&p) {
            if let Ok(m) = serde_json::from_str::<BackupMeta>(&s) {
                out.push(m);
            }
        }
    }
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

/// 备份目录占了多少字节（设置页显示，让用户知道该不该清理）。
pub fn total_bytes() -> u64 {
    fn walk(p: &Path) -> u64 {
        let Ok(rd) = std::fs::read_dir(p) else {
            return 0;
        };
        rd.flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path()),
                _ => e.metadata().map(|m| m.len()).unwrap_or(0),
            })
            .sum()
    }
    walk(&paths::backups_dir())
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
                "{} · v{} · {} · 数据库转储 {}",
                m.at,
                m.tag,
                m.id,
                match (m.sql_bytes, &m.sql_error) {
                    (Some(n), _) => crate::flow::human_bytes(n),
                    (None, Some(e)) => format!("没有（{e}）"),
                    (None, None) => "没有".to_string(),
                }
            )
        })
        .collect();
    v.push(format!(
        "合计占用 {}",
        crate::flow::human_bytes(total_bytes())
    ));
    v
}

/// 写一个可读的提示文件到备份目录（用户翻到这里时知道这些文件是什么）。
pub fn write_readme(id: &str) -> AppResult<()> {
    let dir = dir_of(id);
    let text = "\
这是 Hunter 启动器在升级前自动做的备份。

  hunter.sql                        数据库全量转储（pg_dump --clean --if-exists --no-owner）
  .env                              升级前的配置。**里面有 key，不要外发**
  docker-compose.yml                升级前那个版本的 compose
  docker-compose.launcher.yml       启动器生成的端口覆盖文件
  meta.json                         这次备份的元信息

手工恢复数据库：
  docker compose -p hunter exec -T postgres psql -U hunter -d hunter < hunter.sql

启动器里的入口：设置 → 备份 → 「从备份恢复数据」。
升级成功后这个目录不会被自动删除，确认没问题了可以自己删。
";
    let mut f = std::fs::File::create(dir.join("README.txt"))
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("写备份说明失败：{e}")))?;
    f.write_all(text.as_bytes())
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("写备份说明失败：{e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 备份目录名按上海时间加版本() {
        assert_eq!(
            make_id("2026-09-20 03:18:54", "1.1.0"),
            "2026-09-20_031854-v1.1.0"
        );
    }

    #[test]
    fn 目录名能排序且新的在后() {
        let a = make_id("2026-09-20 03:18:54", "1.1.0");
        let b = make_id("2026-09-20 04:00:00", "1.1.0");
        assert!(a < b, "{a} 应该排在 {b} 前面");
    }

    #[test]
    fn 有没有转储分得清楚() {
        let mut m = BackupMeta {
            id: "x".into(),
            tag: "1.1.0".into(),
            at: "2026-09-20 03:18:54".into(),
            sql_bytes: Some(1234),
            sql_error: None,
            files: vec![".env".into()],
        };
        assert!(m.has_dump());
        m.sql_bytes = None;
        m.sql_error = Some("postgres 没起来".into());
        assert!(!m.has_dump());
    }

    #[test]
    fn 转储字节数为零时不算有备份() {
        let m = BackupMeta {
            id: "x".into(),
            tag: "1.1.0".into(),
            at: "2026-09-20 03:18:54".into(),
            sql_bytes: Some(0),
            sql_error: None,
            files: vec![],
        };
        assert!(!m.has_dump(), "空文件不能算备份成功");
    }

    #[test]
    fn 备份命令走的是参数数组且带_exec_t() {
        // 红线 3：不拼 shell 字符串。这里顺带盯住 `-T`（没有它在非 TTY 下会直接失败）
        let (program, args) = compose::argv(&["exec", "-T", "postgres", "pg_dump"]);
        // I4 起 program 是**解析出来的 docker 绝对路径**（定位不到时退回裸名字），
        // 参数第一个才是 compose 子命令
        assert!(
            program.ends_with("docker")
                || program.ends_with("docker.exe")
                || program.ends_with("docker-compose"),
            "{program}"
        );
        assert_eq!(args[0], "compose");
        assert!(args.iter().any(|a| a == "-T"), "{args:?}");
        assert!(args.iter().any(|a| a == "pg_dump"), "{args:?}");
        assert!(
            args.iter().any(|a| a == "--project-name"),
            "备份也要带项目名，否则会去动别人的栈：{args:?}"
        );
    }

    #[test]
    fn 没有备份时摘要也有一行话() {
        // 这个函数会读真实目录；无论有没有内容，都不能返回空 Vec
        assert!(!summary_lines().is_empty());
    }
}
