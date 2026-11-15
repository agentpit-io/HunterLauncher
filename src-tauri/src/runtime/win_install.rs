//! Windows 环境自动安装 Docker Desktop 全流程闭环（I23）。
//!
//! ## 为什么要最大化消除界面外手工操作
//!
//! 用户要求在 Windows 环境下最大化减少用户在系统向导和界面外需要自己手工操作安装 Docker 的过程：
//!
//! 1. **自动预写 `%USERPROFILE%\.wslconfig`（[`ensure_wslconfig_in`]）**：
//!    按本机物理内存合理限制 WSL2 最大内存占用（8 GB → `3500MB`、12~16 GB → `6GB`、24 GB+ → `8GB`），
//!    防止 WSL2 吃光整机内存导致卡死；**若用户已有 `.wslconfig` 且包含 `[wsl2]` 与 `memory=`，绝不覆盖**。
//! 2. **自动预写 `%APPDATA%\Docker\settings-store.json`（[`preconfigure_docker_desktop_in`]）**：
//!    写入 `"DisplayedOnboarding": true`、`"licenseTermsVersion": 2`、`"AutoStart": true`，
//!    跳过首次打开 Docker Desktop 时的全英文欢迎教程与协议弹窗；若选中了非系统盘（如 `D:\Hunter`），
//!    一并写入 `"dataFolder": "D:\\Hunter\\docker-data"`。
//! 3. **双源高速下载 + 单次 UAC 静默安装（[`installer_args`] / [`install`]）**：
//!    优先从国内高速源（`<CN_DOWNLOAD_BASE>/runtime/docker-desktop/Docker%20Desktop%20Installer.exe`）下载，
//!    官方源（`https://desktop.docker.com/win/main/amd64/Docker%20Desktop%20Installer.exe`）兜底；
//!    传入 `install --quiet --accept-license --backend=wsl-2 --always-run-service`，并在选中非系统盘时
//!    追加 `--installation-dir=<选中盘>\Hunter\DockerApp` 与 `--wsl-default-data-root=<选中盘>\Hunter\docker-data`。
//! 4. **需要重启时自动写 `RunOnce` 注册表开机续装（[`register_run_once`]）**：
//!    若首次启用 WSL2 特性或安装器返回 `3010`（需要重启生效），自动写入
//!    `HKCU\Software\Microsoft\Windows\CurrentVersion\RunOnce` 的 `HunterLauncherResume` 键，
//!    重启登录后自动拉起启动器接着装完 Hunter。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::err::{AppError, AppResult, Code};

/// 国内高速桶上的 Docker Desktop 安装包地址。
pub fn cn_installer_url() -> String {
    format!(
        "{}/runtime/docker-desktop/Docker%20Desktop%20Installer.exe",
        crate::config::CN_DOWNLOAD_BASE
    )
}

/// Docker 官方 Windows amd64 安装包直链。
pub const OFFICIAL_INSTALLER_URL: &str =
    "https://desktop.docker.com/win/main/amd64/Docker%20Desktop%20Installer.exe";

/// Windows `RunOnce` 注册表路径（用户级，无需管理员权限）。
pub const RUN_ONCE_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\RunOnce";
/// Windows `RunOnce` 里的值名。
pub const RUN_ONCE_VALUE: &str = "HunterLauncherResume";

/// 根据本机物理内存（GiB）计算 `.wslconfig` 里的 `memory=` 上限字符串。
///
/// * `<= 9`（8 GB 电脑）→ `"3500MB"`
/// * `10 ..= 18`（12~16 GB 电脑）→ `"6GB"`
/// * `> 18`（24 GB 及以上电脑）→ `"8GB"`
pub fn wsl_memory_limit_for(total_mem_gib: u32) -> &'static str {
    if total_mem_gib <= 9 {
        "3500MB"
    } else if total_mem_gib <= 18 {
        "6GB"
    } else {
        "8GB"
    }
}

/// 生成 `.wslconfig` 的完整文本（纯函数）。
pub fn render_wslconfig(total_mem_gib: u32, cpu_cores: u32) -> String {
    let mem = wsl_memory_limit_for(total_mem_gib);
    let procs = (cpu_cores / 2).clamp(2, 4);
    format!(
        "# 由 Hunter 启动器自动生成，防止 WSL2 占用过多内存导致电脑卡顿\n\
         [wsl2]\n\
         memory={mem}\n\
         processors={procs}\n\
         localhostForwarding=true\n"
    )
}

/// 判断已有的 `.wslconfig` 内容是否已经由用户配置过 `[wsl2]` 和 `memory=`。
pub fn has_existing_wsl_memory_config(content: &str) -> bool {
    let mut in_wsl2 = false;
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_wsl2 = line.eq_ignore_ascii_case("[wsl2]");
            continue;
        }
        if in_wsl2 {
            if let Some((k, _)) = line.split_once('=') {
                if k.trim().eq_ignore_ascii_case("memory") {
                    return true;
                }
            }
        }
    }
    false
}

/// 在指定家目录下确保 `.wslconfig` 存在且配置了合理的内存上限。
///
/// **安全守则**：如果文件已存在且里面已经包含 `[wsl2]` 和 `memory=`，**绝不覆盖用户已有的配置**！
/// 返回 `Ok(true)` 表示本次新建或补写了配置，`Ok(false)` 表示保留了用户已有配置。
pub fn ensure_wslconfig_in(home_dir: &Path, total_mem_gib: u32, cpu_cores: u32) -> AppResult<bool> {
    let path = home_dir.join(".wslconfig");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if has_existing_wsl_memory_config(&existing) {
            return Ok(false);
        }
        if !existing.trim().is_empty() {
            // 文件已存在且非空，为避免破坏用户自定义段落，如果已有 [wsl2] 段但没写 memory=，也不强行重写整文件
            if existing
                .lines()
                .any(|l| l.trim().eq_ignore_ascii_case("[wsl2]"))
            {
                return Ok(false);
            }
        }
    }
    std::fs::create_dir_all(home_dir).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("创建目录 {} 失败：{e}", home_dir.display()),
        )
    })?;
    let body = render_wslconfig(total_mem_gib, cpu_cores);
    std::fs::write(&path, body).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("写入 {} 失败：{e}", path.display()),
        )
    })?;
    Ok(true)
}

/// 在真实用户家目录下预写 `.wslconfig`。
pub fn ensure_wslconfig(total_mem_gib: u32) -> AppResult<bool> {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4);
    ensure_wslconfig_in(&crate::paths::home(), total_mem_gib, cores)
}

/// 在指定的 `APPDATA` 目录下预写 `Docker\settings-store.json`，免除首次打开的全英文欢迎向导。
///
/// 若文件已存在，只合并缺失键，绝不清空用户已有键。
pub fn preconfigure_docker_desktop_in(
    appdata_dir: &Path,
    data_root: Option<&Path>,
) -> AppResult<PathBuf> {
    let docker_dir = appdata_dir.join("Docker");
    std::fs::create_dir_all(&docker_dir).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("创建目录 {} 失败：{e}", docker_dir.display()),
        )
    })?;
    let settings_path = docker_dir.join("settings-store.json");
    let mut obj = if let Ok(raw) = std::fs::read_to_string(&settings_path) {
        serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    } else {
        serde_json::Map::new()
    };

    obj.entry("DisplayedOnboarding".to_string())
        .or_insert(serde_json::Value::Bool(true));
    obj.entry("licenseTermsVersion".to_string())
        .or_insert(serde_json::Value::Number(2.into()));
    obj.entry("AutoStart".to_string())
        .or_insert(serde_json::Value::Bool(true));

    if let Some(root) = data_root {
        let s = root.to_string_lossy().into_owned();
        if !s.trim().is_empty() {
            obj.entry("dataFolder".to_string())
                .or_insert(serde_json::Value::String(s));
        }
    }

    let text = serde_json::to_string_pretty(&serde_json::Value::Object(obj)).unwrap_or_default();
    std::fs::write(&settings_path, text).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("写入 {} 失败：{e}", settings_path.display()),
        )
    })?;
    Ok(settings_path)
}

/// 在本机 `%APPDATA%` 下预写 Docker Desktop 免向导与数据盘配置。
pub fn preconfigure_docker_desktop(data_root: Option<&Path>) -> AppResult<PathBuf> {
    let appdata = std::env::var("APPDATA")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::paths::home().join("AppData").join("Roaming"));
    preconfigure_docker_desktop_in(&appdata, data_root)
}

/// 判断用户选中的目录是否位于非系统盘（非 `C:` 盘且非空）。
fn is_non_system_target(target_root: &str) -> bool {
    let t = target_root.trim();
    if t.is_empty() {
        return false;
    }
    let first = t.chars().next().unwrap_or('C');
    !first.eq_ignore_ascii_case(&'c')
}

/// 构造 Docker Desktop 静默安装命令参数（纯函数，单测钉死）。
///
/// * 基础参数：`[installer_path, "install", "--quiet", "--accept-license", "--backend=wsl-2", "--always-run-service"]`
/// * 当 `target_root` 为非系统盘路径（如 `D:\Hunter`）时，自动追加：
///   `--installation-dir=D:\Hunter\DockerApp` 与 `--wsl-default-data-root=D:\Hunter\docker-data`。
pub fn installer_args(installer_path: &str, target_root: Option<&str>) -> Vec<String> {
    let mut args = vec![
        installer_path.to_string(),
        "install".into(),
        "--quiet".into(),
        "--accept-license".into(),
        "--backend=wsl-2".into(),
        "--always-run-service".into(),
    ];
    if let Some(raw) = target_root {
        let trimmed = raw.trim().trim_end_matches(['\\', '/']);
        if is_non_system_target(trimmed) {
            let sep =
                if trimmed.contains('\\') || trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':' {
                    '\\'
                } else {
                    '/'
                };
            args.push(format!("--installation-dir={trimmed}{sep}DockerApp"));
            args.push(format!("--wsl-default-data-root={trimmed}{sep}docker-data"));
        }
    }
    args
}

/// 构造写入 Windows `RunOnce` 注册表的 `reg.exe` 参数数组（纯函数）。
pub fn run_once_reg_args(exe_path: &str) -> Vec<String> {
    vec![
        "add".into(),
        RUN_ONCE_KEY.into(),
        "/v".into(),
        RUN_ONCE_VALUE.into(),
        "/t".into(),
        "REG_SZ".into(),
        "/d".into(),
        format!("\"{exe_path}\""),
        "/f".into(),
    ]
}

/// 在 Windows 上写入 `HKCU\Software\Microsoft\Windows\CurrentVersion\RunOnce` 以便重启后自动续装。
pub fn register_run_once() -> AppResult<()> {
    if !cfg!(windows) {
        return Ok(());
    }
    let exe = std::env::current_exe()
        .map_err(|e| AppError::new(Code::ConfigWrite, format!("读取启动器自身路径失败：{e}")))?;
    let args = run_once_reg_args(&exe.to_string_lossy());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let r = crate::proc::run_timeout("reg", &refs, Duration::from_secs(15))?;
    if !r.ok() {
        return Err(AppError::new(
            Code::ConfigWrite,
            format!("写入开机自动续装注册表项失败：{}", r.err_line()),
        ));
    }
    Ok(())
}

/// 读取当前配置的非系统盘目标目录（比如用户在选盘页选了 `D:\Hunter`）。
pub fn configured_target_root() -> Option<String> {
    let cfg = crate::config::LauncherConfig::load();
    let d = cfg.runtime.data_dir.trim().to_string();
    if is_non_system_target(&d) {
        return Some(d);
    }
    // 若用户还没手选过，自动看 disk::volumes() 是否有推荐的非系统盘（如 D:\）
    let vols = crate::runtime::disk::volumes();
    crate::runtime::disk::pick(&vols).map(|v| {
        crate::runtime::disk::target_dir(&v.mount)
            .to_string_lossy()
            .into_owned()
    })
}

/// 下载 Docker Desktop 安装包（国内高速源优先，官方源兜底）。
pub fn download_installer(
    say: &mut dyn FnMut(&str),
    bytes_cb: &mut dyn FnMut(u64, u64),
    cancel: &dyn Fn() -> bool,
) -> AppResult<PathBuf> {
    let cache = crate::paths::runtime_cache();
    std::fs::create_dir_all(&cache).map_err(|e| {
        AppError::new(
            Code::ConfigWrite,
            format!("创建下载缓存目录 {} 失败：{e}", cache.display()),
        )
    })?;
    let dest = cache.join("DockerDesktopInstaller.exe");
    crate::assist::guard::writable_path(&dest)?;

    // 若本地已有下载好的完整安装包（> 350 MB），直接复用免重下
    if let Ok(meta) = std::fs::metadata(&dest) {
        if meta.len() > 350 * 1024 * 1024 {
            say("检测到本机已下载好 Docker Desktop 安装包，直接复用");
            return Ok(dest);
        }
    }

    let cn_url = cn_installer_url();
    let sources = [
        ("国内高速源", cn_url.as_str()),
        ("Docker 官方源", OFFICIAL_INSTALLER_URL),
    ];

    let mut last_err = String::new();
    for (label, url) in sources {
        if cancel() {
            return Err(AppError::new(Code::Unknown, "安装已取消。".to_string()));
        }
        say(&format!(
            "正在从{label}下载 Docker 运行环境安装包（约 500 MB）…"
        ));
        let mut last_report = 0u64;
        let res = crate::http::download_to_file(
            url,
            &dest,
            Duration::from_secs(3600),
            1200 * 1024 * 1024,
            cancel,
            &mut |got, total| {
                let tot = total.unwrap_or(0);
                bytes_cb(got, tot);
                if got.saturating_sub(last_report) >= 16 * 1024 * 1024 {
                    last_report = got;
                    if tot > 0 {
                        say(&format!(
                            "已下载 {} / {}",
                            crate::assist::probe::human_bytes(got),
                            crate::assist::probe::human_bytes(tot)
                        ));
                    } else {
                        say(&format!(
                            "已下载 {}",
                            crate::assist::probe::human_bytes(got)
                        ));
                    }
                }
            },
        );
        match res {
            Ok(n) if n > 100 * 1024 * 1024 => {
                say(&format!(
                    "Docker 安装包下载完成（{}）",
                    crate::assist::probe::human_bytes(n)
                ));
                return Ok(dest);
            }
            Ok(n) => {
                let _ = std::fs::remove_file(&dest);
                last_err = format!("{label}返回的文件过小（{n} 字节）");
                say(&format!("{last_err}，自动切换备用源"));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&dest);
                last_err = e.msg;
                say(&format!(
                    "从{label}下载未成功（{last_err}），自动切换备用源"
                ));
            }
        }
    }

    Err(AppError::new(
        Code::DockerMissing,
        format!("下载 Docker Desktop 安装包失败：{last_err}"),
    ))
}

/// Windows 全自动安装 Docker Desktop 主入口。
pub fn install(p: &mut super::builtin::Progress) -> AppResult<String> {
    if !cfg!(windows) {
        return Err(AppError::new(
            Code::NotImplemented,
            "Windows 自动安装 Docker 仅在 Windows 系统上可用。".to_string(),
        ));
    }

    // 1. 预写 .wslconfig 限制 WSL2 内存占用
    let mem_gib = crate::hwcheck::check()
        .total_mem_bytes
        .map(|b| (b / (1024 * 1024 * 1024)) as u32)
        .unwrap_or(8);
    match ensure_wslconfig(mem_gib) {
        Ok(true) => (p.say)(&format!(
            "已为您自动配置虚拟化内存上限（{}），防止后台吃光电脑内存",
            wsl_memory_limit_for(mem_gib)
        )),
        Ok(false) => (p.say)("检测到您已有自定义虚拟化内存设置，保持原样不改动"),
        Err(e) => crate::lwarn!("预写 .wslconfig 失败（不阻断安装）：{}", e.msg),
    }

    // 2. 确定目标安装盘并预写 Docker Desktop 免首次欢迎向导配置
    let target_root = configured_target_root();
    let data_root_path = target_root
        .as_deref()
        .map(|r| Path::new(r).join("docker-data"));
    match preconfigure_docker_desktop(data_root_path.as_deref()) {
        Ok(_) => {
            if let Some(ref root) = target_root {
                (p.say)(&format!(
                    "已预设免英文向导配置，并将容器与镜像数据目录指向 {root}"
                ));
            } else {
                (p.say)("已预设免英文向导与自动启动配置");
            }
        }
        Err(e) => crate::lwarn!("预写 Docker Desktop 配置失败（不阻断安装）：{}", e.msg),
    }

    // 3. 下载安装包
    let installer = download_installer(p.say, p.bytes, p.cancel)?;

    // 4. 发起单次 UAC 提权静默安装（若 WSL2 未就绪则在同一提权会话内开启 WSL2 特性）
    (p.say)(
        "正在为您开启 Windows 虚拟化子系统并静默安装 Docker，系统会弹出一次管理员确认框，点「是」即可",
    );
    let inner = installer_args(&installer.to_string_lossy(), target_root.as_deref());
    let code = super::elevate::run_windows_installer(
        super::elevate::Op::WinInstallDocker,
        &inner,
        Duration::from_secs(1800),
    )?;

    // 退出码 3010 (ERROR_SUCCESS_REBOOT_REQUIRED) 或首次启用 WSL2 特性需要重启电脑
    let wsl_ready = super::docker::detect().wsl.unwrap_or(true);
    if code == 3010 || !wsl_ready {
        let _ = register_run_once();
        return Err(AppError::new(
            Code::WslMissing,
            "运行环境已安装完成，Windows 需要重启一次让虚拟化生效；重启后启动器会自动打开并接着为您装完 Hunter，不用您再从头操作。"
                .to_string(),
        ));
    }

    // 5. 无需重启：自动拉起 Docker Desktop 并等待就绪
    (p.say)("Docker 安装完成，正在为您启动后台服务并等待就绪…");
    let _ = super::docker::try_start_daemon(p.say);
    if !super::chain::wait_daemon(Duration::from_secs(180), p.say) {
        let _ = register_run_once();
        return Err(AppError::new(
            Code::DaemonDown,
            "Docker 已经安装完成，但后台服务尚未就绪。如果这是首次开启虚拟化，请重启一次电脑，重启后启动器会自动接着完成安装。"
                .to_string(),
        ));
    }

    Ok("Docker Desktop 已自动安装并启动就绪".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 按内存档位生成合理的_wslconfig_限制() {
        assert_eq!(wsl_memory_limit_for(8), "3500MB");
        assert_eq!(wsl_memory_limit_for(12), "6GB");
        assert_eq!(wsl_memory_limit_for(16), "6GB");
        assert_eq!(wsl_memory_limit_for(32), "8GB");

        let cfg8 = render_wslconfig(8, 8);
        assert!(cfg8.contains("[wsl2]"));
        assert!(cfg8.contains("memory=3500MB"));
        assert!(cfg8.contains("processors=4"));
    }

    #[test]
    fn 已有_wslconfig_包含_memory_时绝不覆盖() {
        let dir = std::env::temp_dir().join(format!("hunter-wslcfg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 首次不存在 → 自动创建并返回 true
        let created = ensure_wslconfig_in(&dir, 8, 8).unwrap();
        assert!(created);
        let first_text = std::fs::read_to_string(dir.join(".wslconfig")).unwrap();
        assert!(first_text.contains("memory=3500MB"));

        // 用户自己改成了 memory=10GB → 再次调用绝不覆盖，返回 false
        let custom = "[wsl2]\nmemory=10GB\nswap=0\n";
        std::fs::write(dir.join(".wslconfig"), custom).unwrap();
        let overwritten = ensure_wslconfig_in(&dir, 8, 8).unwrap();
        assert!(!overwritten, "绝不覆盖用户已有的 memory 配置");
        let kept = std::fs::read_to_string(dir.join(".wslconfig")).unwrap();
        assert_eq!(kept, custom);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 预写_docker_desktop_配置跳过欢迎页且保留已有键() {
        let dir = std::env::temp_dir().join(format!("hunter-ddcfg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let docker_dir = dir.join("Docker");
        std::fs::create_dir_all(&docker_dir).unwrap();
        std::fs::write(
            docker_dir.join("settings-store.json"),
            r#"{"CustomUserKey": "keep-me"}"#,
        )
        .unwrap();

        let p = preconfigure_docker_desktop_in(&dir, Some(Path::new(r"D:\Hunter\docker-data")))
            .unwrap();
        let raw = std::fs::read_to_string(p).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["DisplayedOnboarding"], true);
        assert_eq!(v["AutoStart"], true);
        assert_eq!(v["licenseTermsVersion"], 2);
        assert_eq!(v["dataFolder"], r"D:\Hunter\docker-data");
        assert_eq!(v["CustomUserKey"], "keep-me", "用户原有的键必须保留");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 静默安装参数在有无非系统盘时拼装正确() {
        let base = installer_args(r"C:\cache\DockerDesktopInstaller.exe", None);
        assert_eq!(
            base,
            vec![
                r"C:\cache\DockerDesktopInstaller.exe",
                "install",
                "--quiet",
                "--accept-license",
                "--backend=wsl-2",
                "--always-run-service",
            ]
        );

        // C 盘路径不追加非系统盘目录重定向
        let c_drive = installer_args(r"C:\cache\DockerDesktopInstaller.exe", Some(r"C:\Hunter"));
        assert_eq!(c_drive.len(), 6);

        // D 盘路径自动追加 --installation-dir 与 --wsl-default-data-root
        let d_drive = installer_args(r"C:\cache\DockerDesktopInstaller.exe", Some(r"D:\Hunter"));
        assert_eq!(
            d_drive,
            vec![
                r"C:\cache\DockerDesktopInstaller.exe",
                "install",
                "--quiet",
                "--accept-license",
                "--backend=wsl-2",
                "--always-run-service",
                r"--installation-dir=D:\Hunter\DockerApp",
                r"--wsl-default-data-root=D:\Hunter\docker-data",
            ]
        );
    }

    #[test]
    fn run_once_注册表参数形状固定() {
        let args = run_once_reg_args(r"C:\Program Files\Hunter\hunter-launcher.exe");
        assert_eq!(args[0], "add");
        assert_eq!(args[1], RUN_ONCE_KEY);
        assert_eq!(args[3], RUN_ONCE_VALUE);
        assert_eq!(args[7], r#""C:\Program Files\Hunter\hunter-launcher.exe""#);
    }
}
