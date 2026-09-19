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
    for d in [root(), app_dir(), logs_dir(), backups_dir()] {
        std::fs::create_dir_all(&d).map_err(|e| {
            AppError::new(
                Code::ConfigWrite,
                format!("建目录 {} 失败：{e}", d.display()),
            )
        })?;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunter_home_能覆盖根目录() {
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
