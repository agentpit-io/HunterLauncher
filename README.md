<div align="center">

# Hunter 启动器 · HunterLauncher

**把 [HunterCode 开源版](https://github.com/agentpit-io/hunter-community) 的部署，从「clone → 改 .env → 敲命令行」变成「下载 → 填一把 key → 等几分钟」。**

[![License: MIT](https://img.shields.io/badge/License-MIT-amber.svg)](LICENSE)
[![状态](https://img.shields.io/badge/状态-v0.1.0%20预发布%20·%20未签名-orange.svg)](https://github.com/agentpit-io/HunterLauncher/releases)
[![平台](https://img.shields.io/badge/平台-Windows%20·%20macOS%20·%20Linux-1e293b.svg)](#三平台支持)

</div>

---

## 下载与安装

> **0.1.0 是预发布版（prerelease）**，因为安装包**没有代码签名**（见下面的「未签名包怎么放行」）。
> 功能是完整的：Linux 上从零安装、升级、回滚、离线导入都真机跑通过；
> Windows / macOS **只在 CI 里编译与打包通过，没有在真机上跑过**。

| 平台 | 下载 | 大小 | 说明 |
|---|---|---|---|
| **Windows** 10 21H2+ / 11 · x64 | `hunter-launcher_<版本>_x64-setup.exe` | 见 Release 页 | NSIS 安装包，推荐 |
| | `hunter-launcher_<版本>_x64_en-US.msi` | 见 Release 页 | 要走组策略分发就用这个 |
| **macOS** 12+ · Intel / Apple Silicon | `hunter-launcher_<版本>_universal.dmg` | 见 Release 页 | 通用二进制，一个包通吃两种芯片 |
| **Linux** Ubuntu 22.04+ / Debian 12+ · x64 | `hunter-launcher_<版本>_amd64.deb` | 3.81 MiB | 推荐 |
| | `hunter-launcher_<版本>_amd64.AppImage` | 75.55 MiB | 免安装；大是因为要自带整套 WebKitGTK，压不下去 |

**GitHub 下载**：<https://github.com/agentpit-io/HunterLauncher/releases>
**国内下载**（腾讯云香港，不用翻墙）：`https://hunter-dl-hk-1253756459.cos.ap-hongkong.myqcloud.com/launcher/<版本>/`

装完打开，按向导走：欢迎 → Docker 检测 → 填 key → 选模型 → 拉镜像 → 启动 → 打开浏览器。
没有桌面环境的服务器用 `hunter-launcher --headless`，跑的是同一套逻辑。

完整的使用说明在 [`docs/使用说明.md`](docs/使用说明.md)，出问题先看 [`docs/常见问题.md`](docs/常见问题.md)。

### Linux

```bash
# deb（推荐）——— apt 会自己补依赖，比 dpkg -i 省事
sudo apt install -y ./hunter-launcher_<版本>_amd64.deb
hunter-launcher

# AppImage ——— 不用装，chmod 一下直接跑
chmod +x hunter-launcher_<版本>_amd64.AppImage
./hunter-launcher_<版本>_amd64.AppImage
```

桌面环境里托盘图标不出现的话，装一下 `libayatana-appindicator3-1`（deb 已经声明了这个依赖）。
容器日志里的中文显示成方块，是系统缺中文字体：`sudo apt install fonts-noto-cjk`。

### ⚠ 未签名包怎么放行

Windows 代码签名证书（OV 约 100–200 美元/年，EV 300–500）与 Apple 开发者账号（99 美元/年）
都要花钱，本项目**目前没有购买**。CI 里签名与公证的步骤已经写好，等有证书了配上 secrets 就会自动生效；
在那之前产出的是未签名包，系统会拦一下。这不是包有问题，是"没花钱买证书"的必然结果。

#### Windows · SmartScreen

双击 `.exe` 之后会弹一个蓝色的窗口：**"Windows 已保护你的电脑"**。

```
┌──────────────────────────────────────────────┐
│  Windows 已保护你的电脑                        │
│                                              │
│  Microsoft Defender SmartScreen 阻止了       │
│  无法识别的应用启动。运行此应用可能会使你的     │
│  电脑面临风险。                                │
│                                              │
│      更多信息   ←── ① 先点这里（是个链接）      │
│                                              │
│                          [ 不运行 ]           │
└──────────────────────────────────────────────┘
        ↓ 点完「更多信息」，窗口会多出一行和一个按钮

┌──────────────────────────────────────────────┐
│  Windows 已保护你的电脑                        │
│  ...                                          │
│  应用: hunter-launcher_0.1.0_x64-setup.exe    │
│  发布者: 未知发布者                             │
│                                              │
│              [ 仍要运行 ]  ←── ② 再点这里       │
│                          [ 不运行 ]           │
└──────────────────────────────────────────────┘
```

两步：**① 点左下角的「更多信息」→ ② 点出现的「仍要运行」**。
注意不要直接点右下角的「不运行」——那是关掉。

如果 `.exe` 是从浏览器下载的，右键 →「属性」底部可能还有一个「解除锁定」的勾选框，
勾上再点确定也能一并解决。

#### macOS · Gatekeeper

双击 `.dmg` 挂载、把 app 拖进「应用程序」之后，第一次打开会弹：

```
┌───────────────────────────────────────────────┐
│  ⚠  无法打开"hunter-launcher"，因为            │
│     Apple 无法检查其是否包含恶意软件。           │
│                                               │
│            [ 移到废纸篓 ]   [ 取消 ]            │
└───────────────────────────────────────────────┘
```

**不要点「移到废纸篓」。** 正确做法二选一：

1. **右键打开**（最常用）：在「应用程序」里找到 hunter-launcher，
   **按住 Control 点一下**（或右键）→ 选**「打开」**→ 弹窗这次会多一个「打开」按钮，点它。
   只需要做一次，以后双击就正常了。

   ```
   ┌───────────────────────────────────────────────┐
   │  ⚠  macOS 无法验证此 App 的开发者。             │
   │     确定要打开吗？                              │
   │                                               │
   │       [ 移到废纸篓 ]  [ 取消 ]  [ 打开 ]  ←点这个 │
   └───────────────────────────────────────────────┘
   ```

2. **系统设置放行**：打开「系统设置 → 隐私与安全性」，往下滚到「安全性」那一段，
   会看到一行 **"已阻止使用 hunter-launcher，因为来自身份不明的开发者"**，
   点它右边的 **「仍要打开」**。

macOS 15 (Sequoia) 起右键打开那条路被收紧了，只剩第 2 条。
如果提示的是**「已损坏，无法打开，你应该将它移到废纸篓」**（这是 quarantine 属性导致的，
不是文件真的坏了），在终端里跑一次：

```bash
# 注意路径里有空格：.app 的名字来自 productName（"Hunter Launcher"）
xattr -dr com.apple.quarantine "/Applications/Hunter Launcher.app"
```

#### 自更新是验签的

上面说的是**代码签名**（操作系统用来认开发者身份的那种，要花钱）。
启动器的**自更新通道是另一套、而且是签名的**：更新清单 `latest.json` 用 minisign 签名，
公钥编译在程序里，签名验不过的更新包不会被安装。这把密钥是本项目自己生成的，不花钱。

---

## 这是什么

HunterCode 开源版是一套跑在你自己机器上的多智能体投研终端，用 `docker compose` 起 6 个容器。
对愿意敲命令行的人，这没什么难度；但它的目标用户里有相当一部分**正是怕命令行的那批人**。

Hunter 启动器是一个轻量的跨平台桌面程序，替这批用户做完全部的部署动作（Linux `.deb` 实测 3.81 MiB，AppImage 75.55 MiB —— 后者要自带整套 WebKitGTK）：

| 启动器替你做的事 | 原来要手动做的 |
|---|---|
| 检测 Docker，没装就按平台给安装引导 | 自己查文档装 Docker Desktop / Engine |
| 填一把 `hunt_tools_` key，当场校验并显示今日额度 | 申请 key、手改 `.env` 的 4~6 个变量 |
| 拉 6 个镜像，每个镜像一条实时进度条 | `docker compose pull` 看满屏滚动 |
| 端口被占用时自动改端口并写进覆盖文件 | 自己查谁占了 3100，手改 compose |
| 起容器、轮询 6 个服务健康、自动开浏览器 | `up -d` 之后自己 `ps` 看健康 |
| 托盘常驻：启动 / 停止 / 重启 / 日志 / 升级 / 反馈 | 每次都回到终端 |
| 升级 Hunter：先 `pg_dump` 备份，失败自动回滚到旧版本 | 自己备份、自己改 tag、出事自己收拾 |
| 内网没有外网时「从文件导入」一个 `docker save` 的 tar | 自己 save / scp / load |
| 启动器自己也会检查更新（清单是签名的） | —— |

**一把 key 走通全部链路**：同一把 `hunt_tools_` key 同时用于大模型网关、行情数据网关和工具网关，
不需要再去申请任何厂商的 API key。（这一点 M0 已实测验证，见预研结论第一节。）

## 三平台支持

| 平台 | 目标版本 | 安装包 | 真机测试 |
|---|---|---|---|
| Windows | 10 21H2+ / 11 · x64 | NSIS `.exe` + MSI | CI 编译 + 代码审阅（暂无真机） |
| macOS | 12+ · Intel / Apple Silicon | `.dmg`（通用二进制） | CI 编译 + 代码审阅（暂无真机） |
| Linux | Ubuntu 22.04+ / Debian 12+ · x64 | `.deb`（3.81 MiB，推荐）+ `.AppImage`（75.55 MiB，免安装） | ✅ 真机测试（Ubuntu 24.04） |

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

# 只跑前端的检查（不需要 Rust）
pnpm lint && pnpm typecheck && pnpm test && pnpm build

# 演示数据模式：只在开发构建里生效，界面右上角会有「演示数据」角标，
# 用来在没有 Docker 的机器上预览界面、与视觉稿做截图比对
VITE_DEMO=1 pnpm dev
```

### 界面

设计令牌（颜色 / 字体 / 字号 / 间距 / 圆角）全部量自 `docs/design/` 下的三张视觉稿，
逐项列在 [`docs/design/设计令牌.md`](docs/design/设计令牌.md)，实现在 `src/styles/tokens.css`。
字体是 Noto Sans SC + JetBrains Mono 的**子集**，随程序打包（离线可用），均为 SIL OFL 1.1。

Windows / macOS 的安装包只在 GitHub Actions 上构建（`.github/workflows/release.yml`）。
代码签名证书需要付费，当前 CI **不签名**，产出的包在 Windows 会触发 SmartScreen、
在 macOS 需要手动放行（步骤见上面的「未签名包怎么放行」），因此 Release 一律标 prerelease。
签名与公证的步骤在流水线里已经写好，配上对应的 secrets 就会自动生效。

发版：`scripts/set-version.sh <版本>` 把三个文件里的版本号一起改掉 → 提交 →
打 `launcher-v<版本>` 的 tag 并推上去 → `release.yml` 出三平台产物、建 Release、
生成 updater 清单、同步到腾讯云香港。更多细节见 [`docs/开发指南.md`](docs/开发指南.md)。

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
