// Windows 的发布构建不要弹控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    hunter_launcher_lib::run()
}
