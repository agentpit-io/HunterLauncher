<div align="center">

# Hunter 启动器 · HunterLauncher

**把 [HunterCode 开源版](https://github.com/agentpit-io/hunter-community) 的部署，从「clone → 改 .env → 敲命令行」变成「下载 → 填一把 key → 等几分钟」。**

[![License: MIT](https://img.shields.io/badge/License-MIT-amber.svg)](LICENSE)
[![状态](https://img.shields.io/badge/状态-开发中%20·%20尚无可用版本-orange.svg)](docs/开发文档/总进度表.md)
[![平台](https://img.shields.io/badge/平台-Windows%20·%20macOS%20·%20Linux-1e293b.svg)](#三平台支持)

</div>

---

## ⚠️ 当前状态：开发中，**还没有可下载的安装包**

本仓库处于 **M0（预研与骨架）** 阶段，尚未编写业务代码，也**没有发布任何 Release**。
在 [`docs/开发文档/总进度表.md`](docs/开发文档/总进度表.md) 可以看到里程碑进度；
[`docs/开发文档/M0-预研结论.md`](docs/开发文档/M0-预研结论.md) 里是全部实测数据。

看到这里想现在就用 HunterCode 的话，请直接走
[hunter-community 的 `docker compose up -d`](https://github.com/agentpit-io/hunter-community#快速开始)——
启动器做的事和它完全一样，只是替你点鼠标。

---

## 这是什么

HunterCode 开源版是一套跑在你自己机器上的多智能体投研终端，用 `docker compose` 起 6 个容器。
对愿意敲命令行的人，这没什么难度；但它的目标用户里有相当一部分**正是怕命令行的那批人**。

Hunter 启动器是一个轻量的跨平台桌面程序，替这批用户做完全部的部署动作（Linux `.deb` 实测 2.3 MB，AppImage 约 74 MB —— 后者要自带整套 WebKitGTK）：

| 启动器替你做的事 | 原来要手动做的 |
|---|---|
| 检测 Docker，没装就按平台给安装引导 | 自己查文档装 Docker Desktop / Engine |
| 填一把 `hunt_tools_` key，当场校验并显示今日额度 | 申请 key、手改 `.env` 的 4~6 个变量 |
| 拉 6 个镜像，每个镜像一条实时进度条 | `docker compose pull` 看满屏滚动 |
| 端口被占用时自动改端口并写进覆盖文件 | 自己查谁占了 3100，手改 compose |
| 起容器、轮询 6 个服务健康、自动开浏览器 | `up -d` 之后自己 `ps` 看健康 |
| 托盘常驻：启动 / 停止 / 重启 / 日志 / 升级 / 反馈 | 每次都回到终端 |

**一把 key 走通全部链路**：同一把 `hunt_tools_` key 同时用于大模型网关、行情数据网关和工具网关，
不需要再去申请任何厂商的 API key。（这一点 M0 已实测验证，见预研结论第一节。）

## 三平台支持

| 平台 | 目标版本 | 安装包 | 真机测试 |
|---|---|---|---|
| Windows | 10 21H2+ / 11 · x64 | NSIS `.exe` + MSI | CI 编译 + 代码审阅（暂无真机） |
| macOS | 12+ · Intel / Apple Silicon | `.dmg`（通用二进制） | CI 编译 + 代码审阅（暂无真机） |
| Linux | Ubuntu 22.04+ / Debian 12+ · x64 | `.deb`（约 2.3 MB，推荐）+ `.AppImage`（约 74 MB，免安装） | ✅ 真机测试（Ubuntu 24.04） |

> 只有 Linux 有真实测试环境。Windows / macOS 以「CI 能编译通过 + 代码路径审阅」为准，
> 每个里程碑的报告里会写明哪些是未真机验证的。

## 架构

```
┌──────────────────── Hunter 启动器（Tauri 2） ─────────────────────┐
│  前端 React 18 + TypeScript + Vite + Tailwind                     │
│   ├─ 向导  欢迎 → Docker → 输入 key → 选择模型 → 拉镜像 → 启动     │
│   ├─ 运行面板  额度 / 服务健康 / 日志 / 环境                        │
│   └─ 托盘菜单                                                      │
│                                                                    │
│  Rust 核心                                                         │
│   ├─ runtime::docker  检测运行时 / daemon / 版本 / 安装引导         │
│   ├─ compose          pull · up · ps · logs（子进程 + 流式解析）    │
│   ├─ config           .env 与覆盖文件生成、端口冲突改写             │
│   ├─ gateway          key 校验、额度查询、模型列表                   │
│   ├─ updater          启动器自更新 + Hunter 版本升级与回滚           │
│   ├─ feedback         诊断包收集 → 本地脱敏 → 导出 / 提 issue        │
│   └─ tray             托盘菜单与通知                                │
└────────────┬───────────────────────────────┬──────────────────────┘
             │ docker / docker compose CLI   │ HTTPS
             ▼                               ▼
   Docker Desktop / Engine / OrbStack   hunter.agentpit.io（网关）
   └─ hunter 6 个容器                    api.github.com（版本检查）
      web · api · opencode                ghcr.io（镜像）
      llm-shim · postgres · redis
```

启动器**不替代 Docker**，也不把 Hunter 打包成原生程序 —— 它调的是和你手工部署
一模一样的 `docker compose` 命令，所以出了问题你可以照着日志自己复现。

## 与 hunter-community 的关系

| | hunter-community | HunterLauncher（本仓库） |
|---|---|---|
| 是什么 | HunterCode 开源版**本体**：6 个服务的业务代码与镜像 | 部署层的**薄壳**，帮你把上面那套跑起来 |
| 许可证 | Apache-2.0 | MIT |
| 关系 | 被启动 | 启动别人 |

**本仓库对 hunter-community 零改动。** 启动器消费的是它已经对外提供的东西：
`docker-compose.yml`、`.env.example`、镜像里自带的健康检查、以及 `hunter.agentpit.io` 的网关接口。
如果确实需要上游配合（例如缺某个接口），会写进当期里程碑报告的「需上游配合」一节，由上游自己决定改不改。

版本对应关系：启动器默认部署 hunter-community 的最新 Release tag（当前 `v1.2.0`），
并在设置里允许固定到指定 tag。

## 开发方式

```bash
# 前置：Node 22+ · pnpm · Rust stable · 平台对应的 Tauri 2 系统依赖
#   Ubuntu 24.04:
#     sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
#                      librsvg2-dev libgtk-3-dev libsoup-3.0-dev \
#                      libjavascriptcoregtk-4.1-dev build-essential curl file pkg-config

pnpm install
pnpm tauri dev      # 开发（热重载）
pnpm tauri build    # 出当前平台的安装包
```

Windows / macOS 的安装包只在 GitHub Actions 上构建。代码签名证书需要付费，
当前 CI **不签名**，产出的包在 Windows 会触发 SmartScreen、在 macOS 需要手动放行，
Release 说明里会写明。

**所有文档、提交信息、注释、界面文案一律中文**（代码标识符照常用英文）。
提交信息用 [Conventional Commits](https://www.conventionalcommits.org/zh-hans/)。

### 隐私与安全底线

- `hunt_tools_` key 只存在 `~/.hunter/app/.env`（权限 600）与内存里 —— 不写日志、不进诊断包、不进仓库
- 除 web 端口外，api / opencode / postgres / redis 一律只绑 `127.0.0.1`
- 子进程调用一律用参数数组，不拼 shell 字符串
- **不做假数据**：额度、镜像大小、进度、服务状态全部来自真实调用；拿不到就显示 `—` 并给原因
- 遥测默认关闭；当前**没有**上报服务端，反馈走「本地诊断包导出 + 去 GitHub 提 issue」

## 许可证

[MIT](LICENSE) © 2026 agentpit.io

被启动的 [hunter-community](https://github.com/agentpit-io/hunter-community) 是 Apache-2.0，两者相互独立。

---

## English

### Hunter Launcher

A small cross-platform desktop app (Tauri 2 + React) that turns deploying
[HunterCode Community Edition](https://github.com/agentpit-io/hunter-community)
from *"clone the repo, edit `.env`, run docker compose"* into
*"download, paste one key, wait a few minutes."*

**Status: under active development — no installable release yet.** See
[`docs/开发文档/总进度表.md`](docs/开发文档/总进度表.md) for milestone progress.
If you want to run HunterCode today, use
[its own `docker compose up -d`](https://github.com/agentpit-io/hunter-community#quick-start) instead.

**What it does.** Detects (or guides you through installing) Docker; validates a single
`hunt_tools_` key against the Hunter gateway and shows your daily model quota; pulls the six
container images with real per-image progress; rewrites ports when they collide; starts the stack
and waits for all six services to report healthy; then lives in your system tray for
start / stop / restart / logs / upgrade / feedback.

**One key for everything.** The same `hunt_tools_` key authenticates the LLM gateway, the market-data
gateway and the tools gateway — no third-party provider keys required. (Verified by measurement in M0.)

**Platforms.** Windows 10 21H2+/11 x64, macOS 12+ (Intel & Apple Silicon), Ubuntu 22.04+/Debian 12+ x64.
Only Linux has a real test machine; Windows and macOS are validated by CI compilation and code review,
and each milestone report says explicitly what was not tested on real hardware.

**Relationship to hunter-community.** This repo makes *zero changes* to hunter-community. The launcher is
a deployment-layer shell that consumes what upstream already publishes: `docker-compose.yml`,
`.env.example`, the healthchecks baked into the images, and the `hunter.agentpit.io` gateway API.
It shells out to the very same `docker compose` commands you would run by hand, so any failure is
reproducible without the launcher.

**Privacy.** Your key is stored only in `~/.hunter/app/.env` (mode 600) and in memory — never logged,
never bundled into a diagnostic export, never committed. Telemetry is off and currently has no server
endpoint at all; feedback works by exporting a locally-redacted diagnostic bundle and opening a
pre-filled GitHub issue. Nothing in this app fabricates a number it could not measure.

**Development.** `pnpm install && pnpm tauri dev`. Requires Node 22+, Rust stable, and your platform's
Tauri 2 system dependencies. Documentation, commit messages, comments and UI copy are in Chinese
(code identifiers stay in English).

**License.** MIT © 2026 agentpit.io. hunter-community is Apache-2.0 and independently licensed.
