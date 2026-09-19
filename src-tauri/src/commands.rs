//! Tauri command 层。
//!
//! 约定：失败一律返回 `"<错误码>: <说明>"` 形式的字符串，前端 `src/lib/ipc.ts` 按这个格式解析出
//! `IpcError { code, message }`，再由页面显示「—」和原因。
//!
//! M1 里只有窗口控制、版本信息和「用浏览器打开链接」是真的，其余全部返回 `E_NOT_IMPLEMENTED`。
//! **不返回任何假数据** —— 总控规则红线 1。

use tauri::Manager;
use tauri_plugin_opener::OpenerExt;

use crate::AppInfo;

type Result<T> = std::result::Result<T, String>;

fn todo_in(milestone: &str, what: &str) -> String {
    format!("E_NOT_IMPLEMENTED: {what} 要到 {milestone} 才实现（M1 只做界面骨架）")
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        launcher_version: env!("CARGO_PKG_VERSION").to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        window_chrome: if cfg!(target_os = "macos") {
            "native"
        } else {
            "custom"
        }
        .to_string(),
    }
}

#[tauri::command]
pub fn window_minimize(app: tauri::AppHandle) -> Result<()> {
    main_window(&app)?
        .minimize()
        .map_err(|e| format!("E_UNKNOWN: {e}"))
}

#[tauri::command]
pub fn window_toggle_maximize(app: tauri::AppHandle) -> Result<()> {
    let w = main_window(&app)?;
    let maximized = w.is_maximized().map_err(|e| format!("E_UNKNOWN: {e}"))?;
    if maximized {
        w.unmaximize()
    } else {
        w.maximize()
    }
    .map_err(|e| format!("E_UNKNOWN: {e}"))
}

#[tauri::command]
pub fn window_close(app: tauri::AppHandle) -> Result<()> {
    main_window(&app)?
        .close()
        .map_err(|e| format!("E_UNKNOWN: {e}"))
}

/// 用系统默认浏览器打开链接。只放行 https 与本机 http，防止被当成任意程序启动器。
#[tauri::command]
pub fn open_external(app: tauri::AppHandle, url: String) -> Result<()> {
    let allowed = url.starts_with("https://")
        || url.starts_with("http://localhost")
        || url.starts_with("http://127.0.0.1");
    if !allowed {
        return Err(format!("E_UNKNOWN: 拒绝打开非 https / 非本机地址：{url}"));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("E_UNKNOWN: {e}"))
}

// ── 以下是 M2 / M3 的活。签名先定下来，实现留空，绝不返回假数据。 ──────────────

#[tauri::command]
pub fn detect_docker() -> Result<serde_json::Value> {
    Err(todo_in("M2", "Docker 运行时检测"))
}

#[tauri::command]
pub fn validate_key(key: String) -> Result<serde_json::Value> {
    // key 不进日志、不回显（红线 2）：这里只用到它的长度做一次本地格式判断的占位
    let _ = key.len();
    Err(todo_in("M2", "hunter key 校验与额度查询"))
}

#[tauri::command]
pub fn pull_progress() -> Result<serde_json::Value> {
    Err(todo_in("M2", "镜像拉取进度"))
}

#[tauri::command]
pub fn runtime_status() -> Result<serde_json::Value> {
    Err(todo_in("M3", "运行状态与服务健康"))
}

#[tauri::command]
pub fn starting_status() -> Result<serde_json::Value> {
    Err(todo_in("M3", "启动过程中的健康轮询"))
}

#[tauri::command]
pub fn read_settings() -> Result<serde_json::Value> {
    Err(todo_in("M3", "启动器设置读写"))
}

#[tauri::command]
pub fn launcher_log(tail: usize) -> Result<Vec<String>> {
    let _ = tail;
    Err(todo_in("M3", "本地日志读取"))
}

fn main_window(app: &tauri::AppHandle) -> Result<tauri::WebviewWindow> {
    app.get_webview_window("main")
        .ok_or_else(|| "E_UNKNOWN: 找不到主窗口".to_string())
}
