# 第三方组件与各自的许可证

Hunter 轻启动器本身按 **Apache License 2.0** 发布（见 [LICENSE](LICENSE) 与 [NOTICE](NOTICE)）。
**本项目换协议不影响下面这些组件** —— 它们各自保留各自的许可证，条款以各自项目为准。

最后核实：2026-09-21（I8）。SPDX 标识符逐个查自各项目仓库的 `GET /repos/{owner}/{repo}/license`，
不是凭印象写的。

## 一、启动器**运行时下载**的程序（不随安装包分发）

这一组是「内置运行时」（`~/.hunter/runtime/`）按需下载的。它们是**独立的可执行文件**，
由启动器以子进程方式调用，既不静态链接进启动器，也不打包进 dmg / msi / AppImage ——
所以这里是「使用」而不是「再分发」。版本与校验和写死在 `src-tauri/src/runtime/manifest.rs`。

| 组件 | 版本 | 许可证 | 来源 |
|---|---|---|---|
| Colima | v0.10.3 | MIT | https://github.com/abiosoft/colima |
| colima-core 虚拟机系统镜像 | v0.10.4 | MIT（镜像内的 Ubuntu 用户态是各自独立的协议，见下） | https://github.com/abiosoft/colima-core |
| Lima | v2.2.0 | Apache-2.0 | https://github.com/lima-vm/lima |
| Docker CLI（静态包） | 29.8.1 | Apache-2.0 | https://download.docker.com/mac/static/stable/ · https://github.com/docker/cli |
| Docker Compose v2 插件 | v5.5.1 | Apache-2.0 | https://github.com/docker/compose |

**关于虚拟机系统镜像**：`ubuntu-24.04-minimal-cloudimg-*-docker.raw.gz` 是 colima-core
打包的 Ubuntu 24.04 minimal cloud image。打包脚本本身是 MIT，**镜像里跑的是完整的
Ubuntu 用户态**，其中每个软件包按 Ubuntu 自己的协议分发（GPL / LGPL / BSD / MIT 等）。
启动器只是把这个文件原样下载到本机再交给 Colima，不修改、不再分发。
Ubuntu 的许可条款见 https://ubuntu.com/legal/intellectual-property-policy。

## 二、兜底链里可能替用户安装的第三方软件（I8）

这两条路只有在「内置运行时装不起来」时才会走到，而且都在一次授权页上写明了。

| 软件 | 许可证 | 说明 |
|---|---|---|
| OrbStack | **专有软件**，按其自己的条款授权（https://orbstack.dev/terms） | 启动器只做「下载官方 dmg → 验苹果签名与公证 → 安装 → 打开」，不修改它、不再分发它。个人非商业使用免费，商业使用要按它的条款付费 —— 这一条在界面上写明了 |
| Homebrew | BSD-2-Clause | https://github.com/Homebrew/brew。启动器跑的是它官方的安装脚本（`NONINTERACTIVE=1`），不做任何改动 |

## 三、打包进安装包里的东西

| 组件 | 许可证 | 说明 |
|---|---|---|
| Noto Sans SC（子集） | SIL Open Font License 1.1 | 见 `src/assets/fonts/LICENSE-Noto-Sans-SC.txt` |
| JetBrains Mono（子集） | SIL Open Font License 1.1 | 见 `src/assets/fonts/LICENSE-JetBrains-Mono.txt` |
| Rust / npm 依赖 | 各自协议 | 完整清单见 `src-tauri/Cargo.lock` 与 `pnpm-lock.yaml` |

## 四、启动器要部署的那套应用

| 项目 | 许可证 | 说明 |
|---|---|---|
| hunter-community | Apache-2.0 | https://github.com/agentpit-io/hunter-community。启动器只是**拉它的容器镜像并起起来**，两个项目独立授权 |
