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

    if !args.is_empty() && tray_cmd.is_none() && !minimized {
        // 有参数但不是我们认得的：提示一下再照常开界面，别让用户以为程序坏了
        eprintln!("没认出这些参数：{}。用 --help 看用法。", args.join(" "));
    }
    hunter_launcher_lib::run()
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
