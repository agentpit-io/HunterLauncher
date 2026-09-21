//! 内置运行时的下载清单（I7 · 设计文档 §五 `install_runtime`）。
//!
//! ## 为什么版本与 sha256 写死在代码里
//!
//! 「跟着 latest 走」在安装器里是一个会咬人的设计：上游哪天改了打包方式、
//! 换了资产命名、或者发了一个坏版本，用户这边就是**装到一半炸掉**，
//! 而且每台机器装到的东西还不一样，出了事根本复现不了。
//!
//! 所以这里的每一条都是**钉死的**：版本号、文件名、sha256、字节数。
//! 下载回来先按 sha256 对一遍，对不上就**拒绝**（不是警告、不是重试），
//! 并把「期望 / 实际」两个值原样报出来。要升级运行时，就改这个文件、
//! 重新跑一遍同步、重新发一版启动器 —— 这是一次有记录的改动，不是一次漂移。
//!
//! ## 两个下载源
//!
//! | 源 | 地址 |
//! |---|---|
//! | GitHub / docker.com（海外） | 各家官方地址 |
//! | 腾讯云香港桶（国内） | `<CN_DOWNLOAD_BASE>/runtime/<组件>/<版本>/<文件名>` |
//!
//! 两边**并列测速择优**（沿用 [`crate::registry`] 那一套的思路），两边都拿不到
//! 才交给诊断员。国内那一份由 `.github/workflows/runtime-mirror.yml` 同步，
//! 同步前同样按本文件的 sha256 校验 —— 桶里放的必须和官方的**逐字节相同**。
//!
//! ## 校验和是哪儿来的（要能查证）
//!
//! | 组件 | 上游有没有发布校验和 | 本文件里的值 |
//! |---|---|---|
//! | colima | 有（`<资产名>.sha256sum`） | 与上游发布值**一致**，2026-09-21 核对过 |
//! | lima | 有（`SHA256SUMS`） | 与上游发布值**一致** |
//! | docker compose | 有（`<资产名>.sha256`） | 与上游发布值**一致** |
//! | docker CLI 静态包 | **没有**（`download.docker.com` 不发 `.sha256`，实测 404） | 我们从官方 HTTPS 地址下载后**自算并钉住**，报告里写明这一点 |

use serde::Serialize;

/// 一个文件在本地解包之后要变成什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Unpack {
    /// 本身就是可执行文件，落到 `bin/<name>` 并 chmod +x
    Binary,
    /// tar.gz，整包解到 `dist/<name>/`（保留包内的相对布局）
    TarGz,
    /// tar.gz，但只取包内某一个文件，落到 `bin/<name>`（docker 静态包是 `docker/docker`）
    TarGzPick(&'static str),
}

/// 清单里的一条。
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// 组件名（也是桶里的一级目录）
    pub component: &'static str,
    pub version: &'static str,
    /// `macos` / `linux` / `windows`
    pub os: &'static str,
    /// Rust 的 `std::env::consts::ARCH`：`x86_64` / `aarch64`
    pub arch: &'static str,
    /// 上游文件名（同时也是桶里的文件名）
    pub file: &'static str,
    /// 官方下载地址
    pub url: &'static str,
    /// 小写十六进制的 sha256
    pub sha256: &'static str,
    /// 字节数。下载前就知道总量，进度条才有分母（红线 1：分母也得是真的）
    pub size: u64,
    pub unpack: Unpack,
    /// 落地后的名字（`Binary` / `TarGzPick` 是 `bin/` 下的文件名；`TarGz` 是 `dist/` 下的目录名）
    pub dest: &'static str,
}

impl Item {
    /// 国内桶里的键。
    pub fn cos_key(&self) -> String {
        format!("runtime/{}/{}/{}", self.component, self.version, self.file)
    }
    /// 国内桶的完整地址。
    pub fn cn_url(&self) -> String {
        format!("{}/{}", crate::config::CN_DOWNLOAD_BASE, self.cos_key())
    }
    /// 人话的一行（事件流里显示）。
    pub fn label(&self) -> String {
        format!("{} {}", self.component, self.version)
    }
}

/// macOS 内置运行时的四个组件。**全部用户态，不需要管理员密码。**
///
/// * `colima` —— 一条命令起一台跑着 docker 的虚拟机，socket 在用户目录下
/// * `lima` —— colima 的底座（`limactl`）。它要求 `share/lima` 与 `bin/limactl`
///   保持包内的相对位置，所以整包解压、不拆
/// * `docker` —— 官方静态 CLI（只有客户端，不含 daemon；daemon 在虚拟机里）
/// * `docker-compose` —— 作为 CLI 插件放进我们自己的 `DOCKER_CONFIG/cli-plugins`
pub const MACOS: &[Item] = &[
    Item {
        component: "colima",
        version: "v0.10.3",
        os: "macos",
        arch: "x86_64",
        file: "colima-Darwin-x86_64",
        url: "https://github.com/abiosoft/colima/releases/download/v0.10.3/colima-Darwin-x86_64",
        sha256: "3082737fe8a98afda11cba7d9a20b6e56fe80c6153464beda04bec630758770b",
        size: 16_954_240,
        unpack: Unpack::Binary,
        dest: "colima",
    },
    Item {
        component: "colima",
        version: "v0.10.3",
        os: "macos",
        arch: "aarch64",
        file: "colima-Darwin-arm64",
        url: "https://github.com/abiosoft/colima/releases/download/v0.10.3/colima-Darwin-arm64",
        sha256: "980ad8bf61a4ca370243f4cb41401a61276dcd2c2502bee7b9b86f9250169f34",
        size: 15_656_320,
        unpack: Unpack::Binary,
        dest: "colima",
    },
    Item {
        component: "lima",
        version: "v2.2.0",
        os: "macos",
        arch: "x86_64",
        file: "lima-2.2.0-Darwin-x86_64.tar.gz",
        url: "https://github.com/lima-vm/lima/releases/download/v2.2.0/lima-2.2.0-Darwin-x86_64.tar.gz",
        sha256: "0d6f99c19f6e4bc3c92730c4c29d929e6927f0cb0a0ba1a84383367135a8ff31",
        size: 24_415_554,
        unpack: Unpack::TarGz,
        dest: "lima",
    },
    Item {
        component: "lima",
        version: "v2.2.0",
        os: "macos",
        arch: "aarch64",
        file: "lima-2.2.0-Darwin-arm64.tar.gz",
        url: "https://github.com/lima-vm/lima/releases/download/v2.2.0/lima-2.2.0-Darwin-arm64.tar.gz",
        sha256: "bbdef91774885a0d05f7b048c4eb89ae2bcf3a0c252ae7ca7934e63df76d93c3",
        size: 37_586_365,
        unpack: Unpack::TarGz,
        dest: "lima",
    },
    Item {
        component: "docker-cli",
        version: "29.8.1",
        os: "macos",
        arch: "x86_64",
        file: "docker-29.8.1-x86_64.tgz",
        url: "https://download.docker.com/mac/static/stable/x86_64/docker-29.8.1.tgz",
        // download.docker.com 不发布校验和（`.sha256` / `.sha256sum` 实测都是 404），
        // 这一条是我们从官方 HTTPS 地址下载后自算、钉在这里的
        sha256: "de42b6bb38d0ea08333cdddc18b054d61d4c9f003b3616ae55d85ccea72c47c9",
        size: 20_888_599,
        unpack: Unpack::TarGzPick("docker/docker"),
        dest: "docker",
    },
    Item {
        component: "docker-cli",
        version: "29.8.1",
        os: "macos",
        arch: "aarch64",
        file: "docker-29.8.1-aarch64.tgz",
        url: "https://download.docker.com/mac/static/stable/aarch64/docker-29.8.1.tgz",
        sha256: "5a8f5604d7673202b2af925229d15eb4bbb86f7f542e4ac8cd7aa3f14cfa0f8b",
        size: 19_613_004,
        unpack: Unpack::TarGzPick("docker/docker"),
        dest: "docker",
    },
    Item {
        component: "docker-compose",
        version: "v5.5.1",
        os: "macos",
        arch: "x86_64",
        file: "docker-compose-darwin-x86_64",
        url: "https://github.com/docker/compose/releases/download/v5.5.1/docker-compose-darwin-x86_64",
        sha256: "a264d61e824bf08a78867e59cdf32eb09f0aee9ecdf9f6ebfa43f76dc52880f1",
        size: 32_686_480,
        unpack: Unpack::Binary,
        dest: "docker-compose",
    },
    Item {
        component: "docker-compose",
        version: "v5.5.1",
        os: "macos",
        arch: "aarch64",
        file: "docker-compose-darwin-aarch64",
        url: "https://github.com/docker/compose/releases/download/v5.5.1/docker-compose-darwin-aarch64",
        sha256: "998735c9b6fe68a4f05895e6ea73d71ad06f9fc7046383ad89e47346781b6af5",
        size: 30_532_210,
        unpack: Unpack::Binary,
        dest: "docker-compose",
    },
];

/// 这台机器要下的那几个。认不出平台 / 架构就返回空 —— **不猜**。
pub fn for_host() -> Vec<&'static Item> {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        return Vec::new();
    };
    for_target(os, std::env::consts::ARCH)
}

/// 按平台 + 架构挑（拆出来是为了能在任何 CI 上测清单本身）。
pub fn for_target(os: &str, arch: &str) -> Vec<&'static Item> {
    MACOS
        .iter()
        .filter(|i| i.os == os && i.arch == arch)
        .collect()
}

/// 清单里全部条目（同步流水线与单测用）。
pub fn all() -> &'static [Item] {
    MACOS
}

/// 这份清单一共要下多少字节（进度条的分母）。
pub fn total_bytes(items: &[&Item]) -> u64 {
    items.iter().map(|i| i.size).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 每一条的_sha256_都是_64_位小写十六进制() {
        for i in all() {
            assert_eq!(i.sha256.len(), 64, "{} 的 sha256 长度不对", i.file);
            assert!(
                i.sha256
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{} 的 sha256 里有非小写十六进制字符",
                i.file
            );
            assert!(i.size > 0, "{} 没写字节数，进度条就没有分母", i.file);
        }
    }

    /// 清单**不许跟着 latest 漂**：地址里出现 `latest` 就是错的。
    #[test]
    fn 地址里不许出现_latest() {
        for i in all() {
            assert!(
                !i.url.contains("latest"),
                "{} 的地址跟着 latest 走了：{}",
                i.file,
                i.url
            );
            assert!(i.url.starts_with("https://"), "{} 不是 https", i.file);
        }
    }

    /// 每个平台 + 架构组合都要**四件套齐全**，少一件装出来的运行时就是坏的。
    #[test]
    fn mac_两个架构都有四个组件() {
        for arch in ["x86_64", "aarch64"] {
            let v = for_target("macos", arch);
            assert_eq!(v.len(), 4, "macos/{arch} 应该有四个组件，实际 {}", v.len());
            let mut names: Vec<&str> = v.iter().map(|i| i.component).collect();
            names.sort_unstable();
            assert_eq!(
                names,
                vec!["colima", "docker-cli", "docker-compose", "lima"]
            );
            assert!(total_bytes(&v) > 80 * 1024 * 1024, "总字节数看起来不对");
        }
    }

    /// 同一个组件的两个架构必须是**同一个版本**（否则 Intel 与 M 芯片装到的东西不一样）。
    #[test]
    fn 同组件两架构版本一致() {
        for c in ["colima", "lima", "docker-cli", "docker-compose"] {
            let vs: Vec<&str> = all()
                .iter()
                .filter(|i| i.component == c)
                .map(|i| i.version)
                .collect();
            assert!(
                vs.windows(2).all(|w| w[0] == w[1]),
                "{c} 的版本不一致：{vs:?}"
            );
        }
    }

    /// 两个架构的 sha256 不能相同 —— 一样就说明清单是复制粘贴出来的。
    #[test]
    fn 不同文件的_sha256_互不相同() {
        let mut seen = std::collections::HashSet::new();
        for i in all() {
            assert!(seen.insert(i.sha256), "{} 的 sha256 和别的条目重了", i.file);
        }
    }

    #[test]
    fn 国内地址落在香港桶的_runtime_目录下() {
        let i = &MACOS[0];
        assert_eq!(i.cos_key(), "runtime/colima/v0.10.3/colima-Darwin-x86_64");
        assert!(
            i.cn_url().starts_with("https://hunter-dl-hk-"),
            "{}",
            i.cn_url()
        );
        assert!(i.cn_url().ends_with(&i.cos_key()));
    }

    /// 本机能不能装，取决于清单里有没有这台机器的条目。
    /// 在 Linux 的 CI 上这一条必须返回空（Linux 分支不走内置运行时）。
    #[test]
    fn 认不出平台就返回空而不是硬凑() {
        assert!(for_target("plan9", "x86_64").is_empty());
        assert!(for_target("macos", "riscv64").is_empty());
        assert!(for_target("linux", "x86_64").is_empty());
    }
}
