//! 删除应用（I13 · R4）。
//!
//! 用户的原话：
//!
//! > 可以执行删除应用，但需要输入「删除应用」文字，避免用户错误点击按钮。
//! > 删除前，还需要提醒，数据库是否需要保留，还是只删除应用。
//!
//! ## 三步，缺一不可
//!
//! 1. **选范围**（[`Scope`]）。默认是「只删应用、保留数据」——
//!    默认项必须是**后果最轻**的那一个。
//! 2. **逐字输入**（[`confirm_phrase`]）。范围 ① 输「删除应用」，
//!    范围 ② 输「删除应用和数据」。差一个字按钮就点不了（[`check_confirm`]）。
//! 3. **执行并报告**：删了什么、留了什么、腾出多少空间，逐条实测。
//!
//! ## 只动本项目，这是代码不是承诺
//!
//! | 要删的东西 | 凭什么确定它是我们的 |
//! |---|---|
//! | 容器 | `docker compose --project-name hunter down`（compose 自己只动这个项目） |
//! | 数据卷 | 每一个都先过 [`crate::assist::guard::own_volume`]：`com.docker.compose.project` 标签必须是 `hunter`，**查不到也不删** |
//! | 镜像 | 只删 [`crate::config::images`] 算出来的那几个引用，而且走 [`crate::assist::guard::argv_image_rm`]（`prune` 一律拒绝） |
//! | 文件 | 只删 `~/.hunter`（[`crate::paths::root`]），而且删之前核一遍它不是家目录、不是根目录 |
//! | 运行环境 | 走 [`crate::runtime::builtin::uninstall`]（I7 就有），只动 `~/.hunter/runtime` |
//!
//! **不碰**：用户别的 compose 项目、`~/.colima`、OrbStack / Docker Desktop 本身、
//! 以及**备份目录**（那是删除之后唯一的退路，见 [`crate::backup`]）。
//!
//! ## 内置运行时的那个陷阱
//!
//! macOS 上数据卷在 Colima 虚拟机的磁盘镜像里 —— **删虚拟机 = 删数据**。
//! 所以「保留数据」的同时勾「删运行环境」是一个自相矛盾的选择，
//! [`plan`] 会把它标成 `runtime_blocked`，界面禁用那个勾选并说明原因，
//! [`run`] 再拒一次（界面有 bug 也不该把用户的数据删掉）。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::err::{AppError, AppResult, Code};
use crate::{backup, compose, config, paths};

/// 删到什么程度。**默认 [`Scope::AppOnly`]**。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// ① 只删应用，保留数据（推荐，默认）
    #[default]
    AppOnly,
    /// ② 删除应用和全部数据
    AppAndData,
}

impl Scope {
    pub fn parse(s: &str) -> Scope {
        match s.trim().to_ascii_lowercase().as_str() {
            "app-and-data" | "all" | "data" => Scope::AppAndData,
            _ => Scope::AppOnly,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::AppOnly => "app-only",
            Scope::AppAndData => "app-and-data",
        }
    }
    pub fn cn(self) -> &'static str {
        match self {
            Scope::AppOnly => "只删除应用，保留数据",
            Scope::AppAndData => "删除应用和全部数据",
        }
    }
}

/// 这一档要用户**逐字输入**的那句话。
///
/// 两句刻意不一样：范围 ② 的后果重得多，让手指多打四个字是值得的。
pub fn confirm_phrase(scope: Scope) -> &'static str {
    match scope {
        Scope::AppOnly => "删除应用",
        Scope::AppAndData => "删除应用和数据",
    }
}

/// 输入的字对不对。**一个字都不能差**（前后空白不算）。
///
/// 和 I7 的接管二次确认是同一条原则：危险操作要用户**自己把那句话打出来**，
/// 而不是点一个可能被误触的按钮。
pub fn check_confirm(scope: Scope, typed: &str) -> bool {
    typed.trim() == confirm_phrase(scope)
}

/// 这次删除要做的事（**只读**，给界面摆出来让用户看清楚）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub scope: Scope,
    pub confirm_phrase: String,
    /// 要删的容器（compose 项目 `hunter` 名下的）
    pub containers: Vec<String>,
    /// 本项目的数据卷。范围 ① 保留、范围 ② 删除
    pub volumes: Vec<compose::VolumeInfo>,
    pub volumes_bytes: Option<u64>,
    /// 本项目的镜像与各自大小（查不到的不计入）
    pub images: Vec<ImageItem>,
    pub images_bytes: Option<u64>,
    pub images_found: usize,
    /// `~/.hunter` 现在多大
    pub home_bytes: u64,
    /// 内置运行时（`~/.hunter/runtime` + 虚拟机磁盘）多大。没装就是 `None`
    pub runtime_bytes: Option<u64>,
    /// 这台机器用的是内置运行时吗（决定「删运行环境」这个勾选的后果）
    pub builtin_runtime: bool,
    /// 「保留数据的同时删运行环境」为什么不行。为 `None` 才允许勾
    pub runtime_blocked: Option<String>,
    /// 数据概况（表数、最近写入）。范围 ② 的界面上必须摆出来
    pub table_count: Option<u32>,
    pub last_write: Option<String>,
    /// 现在有几份备份、最近一份是什么时候
    pub backups: usize,
    pub last_backup_at: Option<String>,
    /// 备份目录在哪（**永远不删**，界面要说清楚）
    pub backup_dir: String,
    /// 定时备份任务装着吗（删除时一并移除）
    pub schedule_installed: bool,
    /// 大约能腾出多少（按上面那几项实测值相加）
    pub est_freed_bytes: u64,
    /// 要特别说清楚的话
    pub warnings: Vec<String>,
    /// 逐条实测原话
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageItem {
    pub reference: String,
    pub bytes: Option<u64>,
}

/// 删除时的选项。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    pub scope: Scope,
    /// 顺带把本项目的镜像也删了（约几个 GB）
    #[serde(default)]
    pub remove_images: bool,
    /// 顺带删掉内置运行时（虚拟机）
    #[serde(default)]
    pub remove_runtime: bool,
    /// 删之前先备份一次（范围 ② 默认勾上）
    #[serde(default)]
    pub backup_first: bool,
    /// 用户逐字打出来的那句话
    #[serde(default)]
    pub confirm: String,
}

/// 一次删除的报告。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub scope: String,
    pub steps: Vec<String>,
    pub removed_containers: usize,
    pub removed_volumes: Vec<String>,
    pub removed_images: Vec<String>,
    pub removed_runtime: bool,
    pub removed_schedule: bool,
    pub kept: Vec<String>,
    pub backup_id: Option<String>,
    pub freed_bytes: u64,
    pub elapsed_ms: u64,
    pub headline: String,
    /// 没删成的东西（如实列出来，不当作没发生）
    pub failures: Vec<String>,
}

// ── 体检 ──────────────────────────────────────────────────────────────────

/// 算一遍这次删除会动到什么。**一个字节都不改**。
pub fn plan(scope: Scope, deep: bool) -> Plan {
    let cfg = config::LauncherConfig::load();
    let mut lines = Vec::new();
    let mut warnings = Vec::new();

    let containers: Vec<String> = compose::container_names().into_values().collect();
    lines.push(format!("本项目名下有 {} 个容器", containers.len()));

    let mut volumes = compose::volumes_of_project().unwrap_or_default();
    let sizes = compose::volume_sizes().unwrap_or_default();
    let mut vb = 0u64;
    let mut any_size = false;
    for v in &mut volumes {
        if let Some(b) = sizes.get(&v.name) {
            v.size_bytes = Some(*b);
            vb += b;
            any_size = true;
        }
    }
    lines.push(format!(
        "本项目名下有 {} 个数据卷（判据是 docker 的 com.docker.compose.project 标签，不是名字前缀）",
        volumes.len()
    ));

    let refs: Vec<String> = config::images(
        &cfg.hunter.registry_prefix,
        &cfg.hunter.base_prefix,
        &cfg.hunter.tag,
    )
    .iter()
    .map(|i| i.reference.clone())
    .collect();
    let (images_bytes, images_found) = compose::image_disk_usage(&refs);
    let per = compose::image_sizes(&refs);
    let images: Vec<ImageItem> = refs
        .iter()
        .map(|r| ImageItem {
            reference: r.clone(),
            bytes: per.get(r).copied(),
        })
        .collect();

    let home_bytes = backup::dir_size(&paths::root());
    let builtin = crate::runtime::builtin::installed().is_some();
    let runtime_bytes = builtin.then(|| backup::dir_size(&paths::runtime_dir()));

    // 内置运行时 + 保留数据 + 删虚拟机 = 数据跟着虚拟机一起没（方案第六节第 1 条）
    let runtime_blocked = if scope == Scope::AppOnly && builtin && !volumes.is_empty() {
        Some(format!(
            "这台电脑上 Hunter 跑在它自己的一台虚拟机里，你的数据（{} 个数据卷）就存在那台虚拟机的磁盘里。\
             删掉运行环境等于把数据一起删掉 —— 而你选的是「保留数据」。\
             要腾出这部分空间，请改选「删除应用和全部数据」，或者先做一次备份。",
            volumes.len()
        ))
    } else {
        None
    };
    if let Some(w) = &runtime_blocked {
        warnings.push(w.clone());
    }

    // 数据概况：范围 ② 的界面上必须摆出来（方案 R4 第 1 步）
    let (table_count, last_write) = if deep && !volumes.is_empty() {
        let d = crate::datacheck::probe_deep();
        (d.table_count, d.last_write)
    } else {
        (None, None)
    };

    let all = backup::list();
    let last_backup_at = all.first().map(|m| m.at.clone());
    if scope == Scope::AppAndData {
        if all.is_empty() {
            warnings.push(
                "这台机器上还没有任何备份。删掉之后这些数据找不回来 —— 建议勾上「删除前先备份一次」。"
                    .into(),
            );
        }
        warnings.push(format!(
            "备份目录 {} 不会被删除。",
            crate::redact::mask_home(&backup::dir().to_string_lossy())
        ));
    }

    let est = {
        let mut n = home_bytes;
        if scope == Scope::AppAndData && any_size {
            n += vb;
        }
        n
    };

    Plan {
        scope,
        confirm_phrase: confirm_phrase(scope).to_string(),
        containers,
        volumes,
        volumes_bytes: any_size.then_some(vb),
        images,
        images_bytes,
        images_found,
        home_bytes,
        runtime_bytes,
        builtin_runtime: builtin,
        runtime_blocked,
        table_count,
        last_write,
        backups: all.len(),
        last_backup_at,
        backup_dir: crate::redact::mask_home(&backup::dir().to_string_lossy()),
        schedule_installed: crate::schedule::status().installed,
        est_freed_bytes: est,
        warnings,
        lines,
    }
}

// ── 执行 ──────────────────────────────────────────────────────────────────

/// 真的删。**每一条禁令都在这里再核一遍**，不指望界面。
pub fn run(opts: &Options, mut note: impl FnMut(&str)) -> AppResult<Report> {
    let t0 = std::time::Instant::now();
    if !check_confirm(opts.scope, &opts.confirm) {
        return Err(AppError::new(
            Code::NotImplemented,
            format!(
                "要删除得逐字输入「{}」。这一步是故意做成这样的 —— 删除应用不该被一次误点触发。",
                confirm_phrase(opts.scope)
            ),
        ));
    }
    let p = plan(opts.scope, false);
    if opts.remove_runtime {
        if let Some(why) = &p.runtime_blocked {
            return Err(AppError::new(Code::NotImplemented, why.clone()));
        }
    }
    let mut rep = Report {
        scope: opts.scope.cn().to_string(),
        ..Default::default()
    };
    let mut step = |rep: &mut Report, s: String| {
        note(&s);
        crate::linfo!("删除应用：{s}");
        rep.steps.push(s);
    };

    // ① 删之前先备份（范围 ② 默认勾上）
    if opts.backup_first {
        let cfg = config::LauncherConfig::load();
        match backup::create(backup::Kind::Manual, &cfg.hunter.tag, |t| {
            crate::linfo!("删除前备份：{t}")
        }) {
            Ok(m) => {
                rep.backup_id = Some(m.id.clone());
                step(
                    &mut rep,
                    format!(
                        "已先备份到 {}（{}）",
                        crate::redact::mask_home(&m.path().to_string_lossy()),
                        crate::flow::human_bytes(m.total_bytes)
                    ),
                );
            }
            Err(e) => {
                // 范围 ② 下备份做不成就**停下来** —— 数据删掉就没有了
                if opts.scope == Scope::AppAndData {
                    return Err(AppError::new(
                        Code::UpdateFailed,
                        format!(
                            "你勾了「删除前先备份一次」，但这一次备份没做成：{}。\
                             数据删掉就找不回来了，所以停在这里，什么都没删。",
                            e.msg
                        ),
                    ));
                }
                step(
                    &mut rep,
                    format!("备份没做成（{}），但范围是「保留数据」，继续", e.msg),
                );
            }
        }
    }

    // ② 移除定时备份任务（方案第六节第 3 条：它是启动器自己装的，要一并收回）
    //
    // 这里刻意不把 `note` 传进去：它已经被上面那个 `step` 闭包可变借走了。
    // 定时任务那一层自己写日志，过程流上只要下面那一句结论就够。
    match crate::schedule::remove(|_| {}) {
        Ok(()) => {
            rep.removed_schedule = true;
            step(&mut rep, "已移除自动备份的定时任务".into());
        }
        Err(e) => rep.failures.push(format!("移除定时任务没成：{}", e.msg)),
    }

    // ③ 停掉并删掉容器与网络。**不带 `-v`** —— 卷的去留由范围决定，不由这条命令决定
    compose::guard_project_owner()?;
    let before = p.containers.len();
    match compose::down() {
        Ok(()) => {
            rep.removed_containers = before;
            step(&mut rep, format!("已删掉 {before} 个容器与本项目的网络"));
        }
        Err(e) => {
            rep.failures.push(format!("删容器没成：{}", e.msg));
            step(&mut rep, format!("删容器没成：{}", e.msg));
        }
    }

    // ④ 数据卷（**只有范围 ② 才删**，而且逐个核标签）
    if opts.scope == Scope::AppAndData {
        let freed: u64 = p.volumes.iter().filter_map(|v| v.size_bytes).sum();
        match remove_own_volumes(&p.volumes) {
            Ok(names) => {
                rep.freed_bytes += freed;
                step(&mut rep, format!("已删掉 {} 个数据卷", names.len()));
                rep.removed_volumes = names;
            }
            Err(e) => rep.failures.push(format!("删数据卷没成：{}", e.msg)),
        }
    } else {
        rep.kept.push(format!(
            "{} 个数据卷（数据库、密钥卷、会话、自建技能全都原样留着）",
            p.volumes.len()
        ));
    }

    // ⑤ 镜像（可选）
    if opts.remove_images {
        let freed = p.images_bytes.unwrap_or(0);
        match remove_own_images(&p.images) {
            Ok(v) => {
                rep.freed_bytes += freed;
                step(
                    &mut rep,
                    format!(
                        "已删掉 {} 个镜像（{}）",
                        v.len(),
                        crate::flow::human_bytes(freed)
                    ),
                );
                rep.removed_images = v;
            }
            Err(e) => rep.failures.push(format!("删镜像没成：{}", e.msg)),
        }
    } else {
        rep.kept.push(format!(
            "{} 个镜像（{}）—— 下次重装不用再下载一遍",
            p.images_found,
            p.images_bytes
                .map(crate::flow::human_bytes)
                .unwrap_or_else(|| "大小没查到".into())
        ));
    }

    // ⑥ 运行环境（可选，前面已经拦过一次）
    if opts.remove_runtime {
        let freed = p.runtime_bytes.unwrap_or(0);
        match crate::runtime::builtin::uninstall() {
            Ok(_) => {
                rep.removed_runtime = true;
                rep.freed_bytes += freed;
                step(
                    &mut rep,
                    format!(
                        "已删掉 Hunter 自己那套运行环境（{}）",
                        crate::flow::human_bytes(freed)
                    ),
                );
            }
            Err(e) => rep.failures.push(format!("删运行环境没成：{}", e.msg)),
        }
    }

    // ⑦ `~/.hunter` 里的文件
    let home_bytes = backup::dir_size(&paths::root());
    match opts.scope {
        Scope::AppOnly => {
            let kept_env = wipe_app_only()?;
            rep.freed_bytes += home_bytes.saturating_sub(backup::dir_size(&paths::root()));
            step(
                &mut rep,
                "已清掉启动器的配置与日志；**`.env` 留着**（里面的数据库口令与 JWT_SECRET 一丢，\
                 留下来的数据库就再也连不上了）"
                    .into(),
            );
            if kept_env {
                rep.kept.push(format!(
                    "{}（含数据库口令与 JWT_SECRET，重装会直接沿用，登录不会失效）",
                    crate::redact::mask_home(&paths::env_file().to_string_lossy())
                ));
            }
        }
        Scope::AppAndData => match wipe_all() {
            Ok(()) => {
                rep.freed_bytes += home_bytes;
                step(
                    &mut rep,
                    format!(
                        "已删掉工作目录 {}",
                        crate::redact::mask_home(&paths::root().to_string_lossy())
                    ),
                );
            }
            Err(e) => rep.failures.push(format!("删工作目录没成：{}", e.msg)),
        },
    }
    rep.kept.push(format!(
        "备份目录 {}（{} 份备份，一份都没动）",
        crate::redact::mask_home(&backup::dir().to_string_lossy()),
        backup::list().len()
    ));

    rep.elapsed_ms = t0.elapsed().as_millis() as u64;
    rep.headline = format!(
        "{} · 腾出约 {} · 用时 {} 秒",
        opts.scope.cn(),
        crate::flow::human_bytes(rep.freed_bytes),
        rep.elapsed_ms / 1000
    );
    crate::linfo!("删除应用完成：{}", rep.headline);
    Ok(rep)
}

/// 逐个核过标签之后删数据卷。**核不过的那一个跳过，不是整批放弃。**
fn remove_own_volumes(volumes: &[compose::VolumeInfo]) -> AppResult<Vec<String>> {
    let mut ok: Vec<String> = Vec::new();
    for v in volumes {
        if let Err(e) = crate::assist::guard::own_volume(&v.name) {
            crate::lwarn!("数据卷 {} 没过归属检查，跳过：{}", v.name, e.msg);
            continue;
        }
        ok.push(v.name.clone());
    }
    if ok.is_empty() {
        return Ok(ok);
    }
    let bin = crate::runtime::which::docker_bin();
    let mut argv = vec![bin.clone(), "volume".into(), "rm".into()];
    argv.extend(ok.iter().cloned());
    // 这里的 `true` 不是一个从外面传进来的标志位 —— 上面那个循环就是「核过了」这件事本身
    crate::assist::guard::argv_volume_rm(&argv, true)?;
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    let r = crate::proc::run_timeout(&bin, &args, Duration::from_secs(120))?;
    if !r.ok() {
        return Err(AppError::new(
            Code::Unknown,
            format!("docker volume rm 失败：{}", r.err_line()),
        ));
    }
    crate::assist::guard::audit(
        "uninstall_remove_volumes",
        &std::collections::BTreeMap::new(),
        crate::assist::guard::Proposer::User,
        Some(crate::assist::guard::Level::Sensitive),
        &format!("删掉本项目的数据卷：{}", ok.join("、")),
    );
    Ok(ok)
}

/// 删本项目的镜像。**只删 [`config::images`] 算出来的那几个引用**，
/// 一个 `prune` 都不跑。
fn remove_own_images(images: &[ImageItem]) -> AppResult<Vec<String>> {
    let refs: Vec<String> = images.iter().map(|i| i.reference.clone()).collect();
    if refs.is_empty() {
        return Ok(Vec::new());
    }
    let bin = crate::runtime::which::docker_bin();
    let mut argv = vec![bin.clone(), "image".into(), "rm".into()];
    argv.extend(refs.iter().cloned());
    crate::assist::guard::argv_image_rm(&argv, true)?;
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    let r = crate::proc::run_timeout(&bin, &args, Duration::from_secs(300))?;
    // 有的镜像可能本来就不在（用户自己删过），那不算失败
    if !r.ok() && !r.stderr.contains("No such image") {
        return Err(AppError::new(
            Code::Unknown,
            format!("docker image rm 失败：{}", r.err_line()),
        ));
    }
    Ok(refs)
}

/// 范围 ①：清掉启动器的配置与日志，**留下 `.env` 与备份**。
///
/// `launcher.toml` 不是整个删掉，而是**重写成只剩 `[backup]` 那一段** ——
/// 用户改过的备份目录要是跟着没了，重装之后就找不到自己那些备份了。
fn wipe_app_only() -> AppResult<bool> {
    let keep_env = paths::env_file().exists();
    for d in [
        paths::logs_dir(),
        paths::telemetry_dir(),
        paths::diagnostics_dir(),
        paths::updates_dir(),
    ] {
        if under_root(&d) && d.exists() {
            let _ = std::fs::remove_dir_all(&d);
        }
    }
    for f in [
        paths::compose_file(),
        paths::override_file(),
        paths::version_file(),
    ] {
        if under_root(&f) && f.exists() {
            let _ = std::fs::remove_file(&f);
        }
    }
    // launcher.toml：只留备份设置
    let old = config::LauncherConfig::load();
    let mut fresh = config::LauncherConfig {
        backup: old.backup,
        ..Default::default()
    };
    fresh.backup.schedule_installed = false;
    fresh.save()?;
    Ok(keep_env)
}

/// 范围 ②：把 `~/.hunter` 整个删掉。**备份目录要是落在里面就绕开它。**
fn wipe_all() -> AppResult<()> {
    let root = paths::root();
    guard_root(&root)?;
    let bdir = backup::dir();
    // 用户可能把备份目录配在了 `~/.hunter` 里面（不推荐，但配置就是让人改的）。
    // 那种情况下**不能**整棵删 —— 删掉的正是他唯一的退路
    if bdir.starts_with(&root) {
        crate::lwarn!(
            "备份目录 {} 落在工作目录里，改成逐项删、绕开它",
            bdir.display()
        );
        let Ok(rd) = std::fs::read_dir(&root) else {
            return Ok(());
        };
        for e in rd.flatten() {
            let p = e.path();
            if bdir.starts_with(&p) {
                continue;
            }
            let _ = if p.is_dir() {
                std::fs::remove_dir_all(&p)
            } else {
                std::fs::remove_file(&p)
            };
        }
        return Ok(());
    }
    std::fs::remove_dir_all(&root).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!(
                "删 {} 失败：{e}",
                crate::redact::mask_home(&root.to_string_lossy())
            ),
        )
    })
}

/// 一个路径在 `~/.hunter` 里面吗。删任何文件之前都要问这一句。
fn under_root(p: &Path) -> bool {
    let root = paths::root();
    match (p.canonicalize(), root.canonicalize()) {
        (Ok(a), Ok(b)) => a.starts_with(&b),
        _ => p.starts_with(&root),
    }
}

/// 「要删的这个目录真的是工作目录吗」。
///
/// `HUNTER_HOME` 是个环境变量，被设成 `/` 或者家目录不是不可能的事
/// （脚本里一个空变量就够了）。这道检查挡的就是那一下。
pub fn guard_root(root: &Path) -> AppResult<()> {
    let s = root.to_string_lossy().to_string();
    if s.trim().is_empty() || s == "/" || s == "\\" {
        return Err(AppError::new(
            Code::NotImplemented,
            "工作目录解析成了根目录，拒绝删除。".to_string(),
        ));
    }
    let home = paths::home();
    if root == home {
        return Err(AppError::new(
            Code::NotImplemented,
            "工作目录解析成了你的家目录，拒绝删除。".to_string(),
        ));
    }
    if root.parent().is_none() {
        return Err(AppError::new(
            Code::NotImplemented,
            "工作目录没有上级目录，拒绝删除。".to_string(),
        ));
    }
    // 至少要有一层名字 —— `~/.hunter` 这种
    if root.file_name().is_none() {
        return Err(AppError::new(
            Code::NotImplemented,
            "工作目录的名字是空的，拒绝删除。".to_string(),
        ));
    }
    Ok(())
}

/// 备份目录**永远不删**。这一条单独写成函数是为了能单测它。
pub fn never_delete() -> Vec<PathBuf> {
    vec![
        backup::dir(),
        backup::legacy_dir(),
        paths::home().join(".colima"),
        paths::home().join(".docker"),
    ]
}

/// 要删的这个路径，是不是撞上了「永远不删」的清单。
pub fn is_protected(p: &Path) -> bool {
    never_delete().iter().any(|q| {
        p == q
            || match (p.canonicalize(), q.canonicalize()) {
                (Ok(a), Ok(b)) => a == b || b.starts_with(&a),
                _ => q.starts_with(p),
            }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 两种范围各自要打的那句话不一样() {
        assert_eq!(confirm_phrase(Scope::AppOnly), "删除应用");
        assert_eq!(confirm_phrase(Scope::AppAndData), "删除应用和数据");
        assert_ne!(
            confirm_phrase(Scope::AppOnly),
            confirm_phrase(Scope::AppAndData),
            "后果不一样的两件事不该打同一句话"
        );
    }

    #[test]
    fn 输入错一个字就不放行() {
        // 方案 R4 第 2 步、第七节用例 6
        assert!(check_confirm(Scope::AppOnly, "删除应用"));
        assert!(
            check_confirm(Scope::AppOnly, "  删除应用  "),
            "前后空白不算"
        );
        for bad in [
            "删除应",
            " 删除应用和数据",
            "删除引用",
            "删除 应用",
            "删除应用。",
            "删除应用!",
            "shanchuyingyong",
            "",
            "删除应用和数据",
        ] {
            assert!(
                !check_confirm(Scope::AppOnly, bad),
                "「{bad}」不该被当成「删除应用」"
            );
        }
        assert!(check_confirm(Scope::AppAndData, "删除应用和数据"));
        assert!(
            !check_confirm(Scope::AppAndData, "删除应用"),
            "范围 ② 要打的是更长的那一句，打短的不算"
        );
    }

    #[test]
    fn 默认范围是后果最轻的那一个() {
        assert_eq!(Scope::default(), Scope::AppOnly);
        assert_eq!(Scope::parse(""), Scope::AppOnly);
        assert_eq!(Scope::parse("什么都不认识"), Scope::AppOnly);
        assert_eq!(Scope::parse("app-and-data"), Scope::AppAndData);
    }

    #[test]
    fn 工作目录守卫_根目录与家目录一律拒() {
        assert!(guard_root(Path::new("/")).is_err());
        assert!(guard_root(Path::new("")).is_err());
        assert!(guard_root(&paths::home()).is_err());
        // 正常的那一个要放行
        assert!(guard_root(&paths::home().join(".hunter")).is_ok());
    }

    #[test]
    fn 备份目录在永远不删的清单里() {
        let _h = paths::test_home("uninstall-protect");
        assert!(is_protected(&backup::dir()), "备份目录永远不删");
        assert!(is_protected(&backup::legacy_dir()));
        assert!(
            is_protected(&paths::home().join(".colima")),
            "~/.colima 是用户自己的东西（I9 的教训）"
        );
        assert!(!is_protected(&paths::app_dir()));
    }

    #[test]
    fn 只删应用那一档不碰_env() {
        let _h = paths::test_home("uninstall-app-only");
        std::fs::write(paths::env_file(), "JWT_SECRET=abc\nPOSTGRES_PASSWORD=xyz\n").unwrap();
        std::fs::write(paths::compose_file(), "services: {}\n").unwrap();
        std::fs::write(paths::logs_dir().join("launcher.log"), "x").unwrap();
        let kept = wipe_app_only().unwrap();
        assert!(kept);
        assert!(
            paths::env_file().exists(),
            "删掉 .env 会让留下来的数据库再也连不上（待办池 P1-29）"
        );
        assert_eq!(
            std::fs::read_to_string(paths::env_file()).unwrap(),
            "JWT_SECRET=abc\nPOSTGRES_PASSWORD=xyz\n",
            ".env 一个字节都不该动"
        );
        assert!(!paths::compose_file().exists(), "compose 文件该清掉");
        assert!(
            !paths::logs_dir().join("launcher.log").exists(),
            "日志该清掉"
        );
    }

    #[test]
    fn 只删应用那一档把备份设置留下来() {
        let _h = paths::test_home("uninstall-keep-backup-cfg");
        let mut cfg = config::LauncherConfig::default();
        cfg.backup.dir = "/mnt/外接盘/Hunter 备份".into();
        cfg.backup.keep_days = 7;
        cfg.install.done = true;
        cfg.save().unwrap();
        wipe_app_only().unwrap();
        let after = config::LauncherConfig::load();
        assert_eq!(
            after.backup.dir, "/mnt/外接盘/Hunter 备份",
            "他改过的备份目录要是跟着没了，重装之后就找不到自己那些备份了"
        );
        assert_eq!(after.backup.keep_days, 7);
        assert!(!after.install.done, "安装标记要清掉，下次打开回到欢迎页");
    }

    #[test]
    fn 整棵删时绕开落在里面的备份目录() {
        let _h = paths::test_home("uninstall-inner-backup");
        // 用户把备份目录配在了 ~/.hunter 里面（不推荐，但配置就是让人改的）
        let mut cfg = config::LauncherConfig::default();
        cfg.backup.dir = paths::root()
            .join("my-backups")
            .to_string_lossy()
            .into_owned();
        cfg.save().unwrap();
        let b = paths::root().join("my-backups");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("keep-me.txt"), "x").unwrap();
        std::fs::write(paths::app_dir().join(".env"), "k=v").unwrap();
        wipe_all().unwrap();
        assert!(b.join("keep-me.txt").exists(), "备份永远不删");
        assert!(!paths::app_dir().exists(), "别的都该没了");
    }
}
