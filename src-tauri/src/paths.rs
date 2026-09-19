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
    chmod_dir_700(&root())?;
    Ok(())
}

/// 把目录权限收成 700。
fn chmod_dir_700(path: &std::path::Path) -> AppResult<()> {
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
#[cfg(test)]
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
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
