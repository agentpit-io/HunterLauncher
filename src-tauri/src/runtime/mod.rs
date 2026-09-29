//! 容器运行时相关（技术方案 §5.1 / §6；内置运行时是 I7 加的，
//! 装软件的兜底链与系统授权框是 I8 加的）。
//!
//! ## [`Os`] 为什么是一个值，而不是到处写 `cfg!`（I18 · P0-1）
//!
//! 「哪个平台该跑哪条命令」本来满仓库都是 `cfg!(target_os = …)`。那种写法的毛病是
//! **写不出单测**：`cfg!` 在编译期就定死了，而开发机与 CI 的 Linux job 只有一种取值，
//! 于是「Windows 上该启动 Docker Desktop」这条断言永远跑不到 ——
//! 而它恰恰是 I18 要修的那个现场（客户那台 Windows 上，那个「启动运行时」按钮什么也做不了）。
//!
//! 把它做成显式参数之后，三平台的分支都能在**任意一台机器**上被钉住。
//! 真实行为仍然由 [`OS`] 决定，两者不会分叉。
pub mod brew;
pub mod builtin;
pub mod chain;
pub mod disk;
pub mod docker;
pub mod effective;
pub mod elevate;
pub mod env;
pub mod manifest;
pub mod netcheck;
pub mod orbstack;
pub mod vmdns;
pub mod which;

/// 这台机器是什么系统。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Windows,
    Linux,
}

/// 编译这台机器时定下来的系统。`cfg!` 会展开成字面量，所以能进 `const`。
pub const OS: Os = if cfg!(target_os = "macos") {
    Os::Mac
} else if cfg!(target_os = "windows") {
    Os::Windows
} else {
    Os::Linux
};
