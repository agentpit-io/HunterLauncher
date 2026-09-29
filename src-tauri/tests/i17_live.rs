//! I17 现场造景用的**在线**验收：把运行面板真正会拿到的那个 `RuntimeStatus`
//! 原样打印出来，再由 `scripts/i17-verify.sh` 逐条断言。
//!
//! ## 为什么要这么一个东西
//!
//! A1-1 / A1-3 要断的是面板上那三样东西 —— **要不要说「运行中」、
//! 给不给「打开 Hunter」的地址、给不给运行时长**。这三样都由
//! [`hunter_launcher_lib::flow::runtime_status`] 一个函数产出，
//! 而界面只是把它照抄上屏（`Dashboard.tsx` 的 `running` / `wwwUrl` / `uptimeSeconds`）。
//! 命令行里最接近的那个入口 `--status` 只印「运行 / 地址」两项，**不印时长**；
//! 拿不到时长那一项就断言不了「不给时长」，所以这里直接把整个结构打出来。
//!
//! ## 它默认不跑
//!
//! 它需要一台**真跑着 `hunter` 项目测试栈**的机器，CI 上没有。
//! 所以标了 `#[ignore]`，要跑得显式点名：
//!
//! ```bash
//! cargo test --manifest-path src-tauri/Cargo.toml --locked \
//!   --test i17_live -- --ignored --nocapture
//! ```
//!
//! 它**只读**：跑一遍现状复查 + 读一次配置，不碰容器、不碰卷、不改配置。

use hunter_launcher_lib::{config, flow, upgrade};

/// 把面板那一屏依赖的每一项打出来。字段名与 `RuntimeStatus` 的 serde 名一致
/// （`camelCase`），断言脚本按名字取值。
#[test]
#[ignore = "需要一台真跑着 hunter 测试栈的机器；由 scripts/i17-verify.sh 驱动"]
fn 面板状态原样打印() {
    let st = flow::AppState::new();
    let s = flow::runtime_status(&st);

    println!("=== RUNTIME_STATUS_BEGIN ===");
    println!(
        "{}",
        serde_json::to_string_pretty(&s).expect("RuntimeStatus 是可序列化的")
    );
    println!("=== RUNTIME_STATUS_END ===");

    // 三个「面板上说不说、给不给」的结论，单独拎出来一行，断言脚本只认这几行
    println!("PANEL running={}", s.running);
    println!("PANEL posture={:?}", s.posture);
    println!("PANEL web_ok={}", s.web_ok);
    println!("PANEL web_url={:?}", s.web_url);
    println!("PANEL uptime_seconds={:?}", s.uptime_seconds);
    println!("PANEL hunter_tag={:?}", s.hunter_tag);
    println!(
        "PANEL 服务={}",
        s.services
            .iter()
            .map(|x| format!("{}:{}", x.service, x.state))
            .collect::<Vec<_>>()
            .join(" ")
    );

    // 中断卡片（A3-x）：判据在后端，界面上那张卡片就是它
    let cfg = config::LauncherConfig::load();
    match upgrade::interrupted(&cfg) {
        Some(it) => {
            println!("INTERRUPTED yes");
            println!("INTERRUPTED config_tag={}", it.config_tag);
            println!("INTERRUPTED running_tag={:?}", it.running_tag);
            println!("INTERRUPTED last_good_tag={}", it.last_good_tag);
            println!("INTERRUPTED missing={}", it.missing_images.join(","));
            println!("INTERRUPTED headline={}", it.headline);
        }
        None => println!("INTERRUPTED no"),
    }

    // 开机判定（A1-x / A3-x 的另一半：面板顶上那句「点启动就能用」在不在）
    let rv = hunter_launcher_lib::selfcheck::review(false);
    println!("BOOT posture={:?}", rv.posture);
    println!("BOOT headline={}", rv.headline);
    for l in &rv.lines {
        println!("BOOT line={l}");
    }
}

/// P0-4 / A4-6 的判据层：**不升级就不说话**。
///
/// 这条在单测里已经钉过（`upgrade.rs` 的 `quit_guard`），这里再在真机上确认一次
/// 面板那条路拿到的确实是 `None` —— 脚本会把它和 `--status` 的退出码一起看。
#[test]
#[ignore = "由 scripts/i17-verify.sh 驱动"]
fn 退出拦截判据原样打印() {
    let st = flow::AppState::new();
    let armed = st.upgrade_in_flight();
    println!("QUIT_GUARD armed={armed:?}");
    println!(
        "QUIT_GUARD guard={:?}",
        upgrade::quit_guard(armed.as_deref()).map(|g| g.target_tag)
    );
}
