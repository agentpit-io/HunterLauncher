//! 容器运行时相关（技术方案 §5.1 / §6；内置运行时是 I7 加的，
//! 装软件的兜底链与系统授权框是 I8 加的）。
pub mod brew;
pub mod builtin;
pub mod chain;
pub mod docker;
pub mod effective;
pub mod elevate;
pub mod env;
pub mod manifest;
pub mod netcheck;
pub mod orbstack;
pub mod vmdns;
pub mod which;
