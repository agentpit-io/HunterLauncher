//! `~/.hunter/` 下的目录结构（技术方案 §7）。
//!
//! 测试与「同一台机器上跑两套」靠环境变量 `HUNTER_HOME` 覆盖根目录 ——
//! 单元测试要往临时目录里写文件，不能真去碰用户的 `~/.hunter`。

use std::path::PathBuf;

use crate::err::{AppError, AppResult, Code};

/// 工作目录根。默认 `~/.hunter`；`HUNTER_HOME` 存在时用它。
pub fn root() -> PathBuf {
    if let Ok(p) = std::env::var("HUNTER_HOME") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    home().join(".hunter")
}

/// 用户主目录。Windows 用 `USERPROFILE`，其余用 `HOME`。
pub fn home() -> PathBuf {
    #[cfg(windows)]
    let key = "USERPROFILE";
    #[cfg(not(windows))]
    let key = "HOME";
    std::env::var(key)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn app_dir() -> PathBuf {
    root().join("app")
}
pub fn logs_dir() -> PathBuf {
    root().join("logs")
}
pub fn backups_dir() -> PathBuf {
    root().join("backups")
}
/// 遥测本地队列所在目录（里程碑 M3 第 6 项、方案 §12）。
pub fn telemetry_dir() -> PathBuf {
    root().join("telemetry")
}
/// 遥测本地队列文件。**只写本机，默认没有任何上报地址**。
pub fn telemetry_queue() -> PathBuf {
    telemetry_dir().join("queue.jsonl")
}
/// 诊断包的默认落地目录（反馈页导出 zip 用）。
pub fn diagnostics_dir() -> PathBuf {
    root().join("diagnostics")
}
/// 自更新下载下来的安装包放这里（`.deb` 这类装不了的格式只能下到本地让用户自己装，
/// 见 [`crate::selfupdate`]）。
pub fn updates_dir() -> PathBuf {
    root().join("updates")
}
/// 内置容器运行时装在这里（I7）。**整棵树都是启动器自己生成的**，
/// 所以「卸载内置运行时」允许整个删掉 —— 它在 `~/.hunter` 里，守卫放行。
pub fn runtime_dir() -> PathBuf {
    root().join("runtime")
}
/// 内置运行时的可执行文件（docker / colima）。
pub fn runtime_bin() -> PathBuf {
    runtime_dir().join("bin")
}
/// 解压出来的整包（lima 要求 `bin/limactl` 与 `share/lima` 保持相对位置）。
pub fn runtime_dist() -> PathBuf {
    runtime_dir().join("dist")
}
/// 下载中转（校验通过才往外搬）。
pub fn runtime_cache() -> PathBuf {
    runtime_dir().join("cache")
}
/// `LIMA_HOME`：虚拟机的磁盘镜像就在这里面。
pub fn lima_home() -> PathBuf {
    runtime_dir().join("lima")
}
/// `COLIMA_HOME`：colima 的 profile 与 docker.sock。
pub fn colima_home() -> PathBuf {
    runtime_dir().join("colima")
}
/// 内置运行时自己的 `DOCKER_CONFIG`（compose 作为 CLI 插件放在它的 `cli-plugins` 下）。
/// **和用户的 `~/.docker` 没有任何关系。**
pub fn runtime_docker_config() -> PathBuf {
    runtime_dir().join("docker-config")
}
/// 装了什么版本（卸载与「设置页显示」都读它）。
pub fn runtime_manifest() -> PathBuf {
    runtime_dir().join("installed.json")
}

pub fn launcher_toml() -> PathBuf {
    root().join("launcher.toml")
}
pub fn env_file() -> PathBuf {
    app_dir().join(".env")
}
pub fn compose_file() -> PathBuf {
    app_dir().join("docker-compose.yml")
}
pub fn override_file() -> PathBuf {
    app_dir().join("docker-compose.launcher.yml")
}
pub fn version_file() -> PathBuf {
    app_dir().join("VERSION")
}
pub fn launcher_log() -> PathBuf {
    logs_dir().join("launcher.log")
}

/// 建好全部目录。失败一律报 `E_CONFIG_WRITE` 并带上真实路径，
/// 「写不进去」这种事只有说清是哪个目录才有得查。
pub fn ensure_dirs() -> AppResult<()> {
    for d in [
        root(),
        app_dir(),
        logs_dir(),
        backups_dir(),
        telemetry_dir(),
        diagnostics_dir(),
        updates_dir(),
    ] {
        std::fs::create_dir_all(&d).map_err(|e| {
            AppError::new(
                Code::ConfigWrite,
                format!("建目录 {} 失败：{e}", d.display()),
            )
        })?;
    }
    // 工作目录整体收成 700。`.env` 本身已经是 600（红线 2），
    // 这一层是纵深防御：同一台机器上别的用户连目录都列不出来。
    //
    // **子目录也要收**（I2 自审发现）：原来只收了 root，子目录跟着 umask 走 ——
    // 测试机上的 umask 是 002，于是 `~/.hunter/app` 这几个实际是 775。
    // 根目录 700 已经挡住了遍历，所以这只是纵深防御的一层；但「工作目录里的东西
    // 只有属主能碰」要么是真的，要么就别写在文档里。
    chmod_700(&root())?;
    for d in [
        app_dir(),
        logs_dir(),
        backups_dir(),
        telemetry_dir(),
        diagnostics_dir(),
        updates_dir(),
    ] {
        chmod_700(&d)?;
    }
    Ok(())
}

/// 把目录权限收成 700。
pub fn chmod_700(path: &std::path::Path) -> AppResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
            AppError::new(
                Code::ConfigWrite,
                format!("设置 {} 权限失败：{e}", path.display()),
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// 把文件权限收成 600（只有属主能读写）。红线 2 要求 `.env` 必须是这个权限。
/// Windows 上没有 POSIX 权限位，靠的是用户目录本身的 ACL，这里直接跳过并如实返回 Ok。
pub fn chmod_600(path: &std::path::Path) -> AppResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
            AppError::new(
                Code::ConfigWrite,
                format!("设置 {} 权限失败：{e}", path.display()),
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// `HUNTER_HOME` 是**进程级**环境变量，两条测试同时改它必然打架。
/// 凡是动这个变量的测试都要先拿这把锁。
///
/// ## 为什么这把锁必须只有一把
///
/// 原来 `paths` / `config` / `telemetry` / `dockercfg` / `log` **各有一把自己的锁**。
/// 每个模块内部是串行的，模块之间照样并行 —— 而它们抢的是同一个进程级变量。
/// 症状是间歇性的、而且长得完全不像根因：`config` 那边写覆盖文件时
/// `std::fs::write` 成功、紧接着 `chmod` 报 `No such file or directory`，
/// 因为中间 `dockercfg` 那边把它自己的临时目录（也就是当时的 `HUNTER_HOME`）删掉了。
///
/// 它在 I7 之前一直没炸过，只是因为时序没撞上；I7 给 `write_override` 加了一次
/// `LauncherConfig::load()`，时序变了一点点，macOS runner 上当场翻车。
/// **这种测试不是"偶尔失败"，是一直坏着、只是还没被看见。**
///
/// 所以这把锁挪到 `paths` 里，`pub(crate)`，全进程**只有这一把**。
#[cfg(test)]
pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// 测试专用：把 `HUNTER_HOME` 指到一个**空的临时目录**，并在 guard 析构时还原。
///
/// I9 加的。在这之前每个模块各自抄一遍「存旧值 → set_var → 断言 → 还原」，
/// 抄漏一次（忘了还原、或者忘了拿 [`env_lock`]）就会让**别的文件里的测试**
/// 莫名其妙地红 —— 而且红的位置和肇事者毫无关系，最难查的那一类。
/// 现在只有这一份实现，锁与还原都在 `Drop` 里。
#[cfg(test)]
pub(crate) struct TestHome {
    _guard: std::sync::MutexGuard<'static, ()>,
    old: Option<String>,
    dir: PathBuf,
}

#[cfg(test)]
impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        match &self.old {
            Some(v) => std::env::set_var("HUNTER_HOME", v),
            None => std::env::remove_var("HUNTER_HOME"),
        }
    }
}

/// 拿一个干净的临时 `HUNTER_HOME`。`tag` 只用来区分目录名，随便起。
#[cfg(test)]
pub(crate) fn test_home(tag: &str) -> TestHome {
    let guard = env_lock();
    let old = std::env::var("HUNTER_HOME").ok();
    let dir = std::env::temp_dir().join(format!("hunter-t-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时工作目录");
    std::env::set_var("HUNTER_HOME", &dir);
    TestHome {
        _guard: guard,
        old,
        dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn 工作目录建出来是_700() {
        let _g = env_lock();
        use std::os::unix::fs::PermissionsExt;
        let old = std::env::var("HUNTER_HOME").ok();
        let dir = std::env::temp_dir().join(format!("hunter-perm-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("HUNTER_HOME", &dir);
        ensure_dirs().expect("建目录应当成功");
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "工作目录必须是 700，实际是 {mode:o}");
        // 子目录也要 700：umask 是 002 的机器上它们原本会是 775（I2 自审）
        for sub in [
            "app",
            "logs",
            "backups",
            "telemetry",
            "diagnostics",
            "updates",
        ] {
            let m = std::fs::metadata(dir.join(sub))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(m, 0o700, "{sub} 必须是 700，实际是 {m:o}");
        }
        let _ = std::fs::remove_dir_all(&dir);
        match old {
            Some(v) => std::env::set_var("HUNTER_HOME", v),
            None => std::env::remove_var("HUNTER_HOME"),
        }
    }

    #[test]
    fn hunter_home_能覆盖根目录() {
        let _g = env_lock();
        // 注：单测里改进程级环境变量，同文件内的测试要串行跑（cargo 默认并行），
        // 所以这里用一个别处不会用到的值，并在断言后立刻恢复。
        let old = std::env::var("HUNTER_HOME").ok();
        std::env::set_var("HUNTER_HOME", "/tmp/hunter-paths-test");
        assert_eq!(root(), PathBuf::from("/tmp/hunter-paths-test"));
        assert_eq!(env_file(), PathBuf::from("/tmp/hunter-paths-test/app/.env"));
        match old {
            Some(v) => std::env::set_var("HUNTER_HOME", v),
            None => std::env::remove_var("HUNTER_HOME"),
        }
    }
}
