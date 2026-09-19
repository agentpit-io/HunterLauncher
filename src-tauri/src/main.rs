// Windows 的发布构建不要弹控制台窗口。
// 注意：headless 模式下我们要往终端打字，所以在 Windows 上会先 AttachConsole（见下）。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use hunter_launcher_lib::headless;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = headless::parse_args(args.clone());

    if parsed.headless || parsed.help || parsed.version {
        // Windows 上 windows_subsystem="windows" 的程序默认没有控制台，
        // 从 cmd/PowerShell 里带 --headless 跑时要先贴回父进程的控制台，否则什么都看不见。
        #[cfg(windows)]
        unsafe {
            windows_attach_console();
        }
        std::process::exit(headless::run(&parsed));
    }

    // `--tray-menu <id>`：交给**已经在跑的那个实例**去执行（single-instance 插件转发）。
    // 没有实例在跑的话这一路就是正常启动一次界面，参数被忽略 —— 下面那句提示不该出现。
    let tray_cmd = hunter_launcher_lib::tray_menu_arg(&args);
    let minimized = hunter_launcher_lib::minimized_arg(&args);

    // 没有图形环境就别去 build 一个 GUI：tao 会在 GTK 初始化那一步 panic，
    // 用户看到的是一段 Rust backtrace 加一个 core dump（I1 在测试机上实测，退出码 134）。
    // 这条路真实存在 —— `--tray-menu` 在模块注释里就是写给 SSH 用户的遥控口，
    // 而 SSH 会话默认没有 DISPLAY。
    #[cfg(all(unix, not(target_os = "macos")))]
    if !has_display() {
        eprintln!("这台机器上没有图形环境（DISPLAY 与 WAYLAND_DISPLAY 都是空的），开不了界面。");
        if tray_cmd.is_some() {
            eprintln!(
                "`--tray-menu` 要有一个正在跑的界面实例才有意义。没有界面的机器请直接用命令行："
            );
        } else {
            eprintln!("命令行模式可以完成同样的事：");
        }
        eprintln!("  hunter-launcher --headless    安装 / 继续安装");
        eprintln!("  hunter-launcher --status      看状态");
        eprintln!("  hunter-launcher --start | --stop | --restart | --down");
        eprintln!("  hunter-launcher --logs [服务名] | --diagnose | --help");
        std::process::exit(2);
    }

    if !args.is_empty() && tray_cmd.is_none() && !minimized {
        // 有参数但不是我们认得的：提示一下再照常开界面，别让用户以为程序坏了
        eprintln!("没认出这些参数：{}。用 --help 看用法。", args.join(" "));
    }
    hunter_launcher_lib::run()
}

/// 这台机器有没有图形环境。X11 看 `DISPLAY`，Wayland 看 `WAYLAND_DISPLAY`，
/// 两个都空就是没有。macOS 不看（它的窗口系统不靠环境变量），Windows 同理。
#[cfg(all(unix, not(target_os = "macos")))]
fn has_display() -> bool {
    ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty()))
}

#[cfg(windows)]
unsafe fn windows_attach_console() {
    // 只用一个 extern 声明，不引 windows-sys 这一整套依赖
    extern "system" {
        fn AttachConsole(dwProcessId: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    AttachConsole(ATTACH_PARENT_PROCESS);
}
