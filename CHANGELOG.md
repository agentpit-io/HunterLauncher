# 更新日志

本文件记录**用户看得见的**变化。格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。时间一律上海时间。

`scripts/release-notes.sh` 会把对应版本的小节原样抄进 GitHub Release 说明，
所以这里只写变化，下载地址、未签名放行步骤那些每版都一样的内容由脚本补。

---

## [0.1.0] - 2026-09-20

第一个公开版本。**标记为预发布（prerelease）**：安装包没有代码签名，
Windows 会触发 SmartScreen、macOS 需要手动放行（步骤见 README 与 `docs/常见问题.md`）。
要不要买证书、转正式版由用户决定。

### 新增

**部署向导** —— 检测 Docker（Engine / Desktop / OrbStack / Colima，Podman 标为实验支持）、
校验 `hunt_tools_` key 并显示真实额度、选模型（Hunter 网关或自带 OpenAI 兼容 key）、
测速选镜像源、拉 6 个镜像（每镜像一条真实进度条）、写配置、起容器、等 6 个服务健康、开浏览器。

**端口冲突自动处理** —— 3100 / 8100 / 3921 / 5442 / 6479 被占用时自动换到空闲端口并写进配置。
除 web 外的端口一律只绑 `127.0.0.1`。

**运行面板** —— 服务状态、今日额度、数据源、容器日志、停止 / 重启 / 日志 / 反馈。

**托盘常驻** —— 11 项菜单，状态行每 15 秒刷新；退出时若容器还在跑会问"保持后台运行"还是"一起停止"。
三平台开机自启（默认关，显示的是系统里的真实状态）。

**Hunter 升级与回滚** —— 每 6 小时对比上游最新 Release tag；升级前自动 `pg_dump` 到
`~/.hunter/backups/` 并备份 `.env` 与两个 compose 文件（**数据库没备份成功就不往下走**）；
拉镜像或起容器失败时自动把配置写回旧版本并用旧镜像重启，报 `E_UPDATE_FAILED`。
界面上能看到 Release Notes 摘要，跨大版本会额外提醒。

**启动器自更新** —— 启动 20 秒后 + 每 24 小时查一次，更新清单用 minisign 签名，
公钥编译在程序里，验不过不装。AppImage / Windows / macOS 就地安装并重启；
`.deb` 因为换 `/usr` 下的文件要 root，改成"下载到 `~/.hunter/updates/` + 给一条
`sudo apt install` 命令"。**任何情况下都不自动安装。**

**离线包** —— `--export-images` 打包、界面或 `--import-images` 导入；
导入后逐个 `docker image inspect` 复核，六个齐了才跳过拉取。给内网机器用。

**命令行模式** —— `--headless` 与界面版跑的是同一套代码。另有 `--status` / `--start` /
`--stop` / `--restart` / `--down` / `--logs` / `--diagnose` / `--check-update` / `--upgrade` /
`--backups` / `--pull-only` / `--self-update`。
`--check-update` 与 `--self-update` 在没有桌面的服务器上也能用（GUI 走 Tauri 的 updater
插件，它要 AppHandle；headless 自己读清单并用同一把公钥验签）。

**反馈与诊断** —— 分节预览、逐节删减、整包脱敏自查、导出 zip、预填 GitHub issue。
**不上传任何东西**，发不发由你决定。

**遥测** —— 默认关闭，而且当前没有上报服务端。打开也只是写本机 `queue.jsonl`，
可以在设置里看原文、随时清空；关掉的那一刻队列会被删掉。

**国内源** —— 镜像与下载都走腾讯云香港，两个源真实测速后选快的，拉失败自动换源重试 3 次。

### 已知问题

* **AppImage 78.01 MiB**，远超设计目标里的 40 MB —— 要自带整套 WebKitGTK，压不下去。
  其余格式都很小：`.deb` 3.82 MiB、Windows NSIS 2.64 MiB、MSI 3.46 MiB、macOS dmg 6.51 MiB。
  介意体积就别用 AppImage。（都是 v0.1.0 实际发布产物的值。）
* **`.deb` 装的启动器不能就地自更新**（见上）。
* **运行面板「今日对话」「晨报」两张卡显示 `—`**：hunter-community 的 api 没有对应的
  免登录接口（逐条实测过）。缺哪几条在面板上可以点开看。
* **Windows / macOS 只在 CI 里编译与打包通过，没有在真机上跑过。**
* **一台机器上只能跑一套**：用 `HUNTER_HOME` 开第二个工作目录时，两套会抢同一个
  compose 项目名，后启动的会顶掉先装好的（数据卷不会丢，重装一次就回来）。
* **key 无效的四种原因分不开**：网关对"格式错 / 不存在 / 已吊销 / 没带"返回的响应
  字节级完全相同，只能统一显示"网关不认这把 key"。
* **额度用尽（402）与限流（429）没有实测过**，处理逻辑是照着网关代码写的。
* **窗口是直角的**，视觉稿里是圆角 —— 要做圆角得开窗口透明并依赖桌面合成器，
  Linux 上不是所有环境都支持，做不好会出现黑色直角。
* **Windows 上三个窗口按钮在左上角**（视觉稿是 mac 风格），与 Windows 习惯相反。
* **Podman 完全没有验证过**，虽然会被识别出来。

### 安全

* `hunt_tools_` key 只存 `~/.hunter/app/.env`（权限 600）与进程内存；
  不进日志、不进遥测、不进诊断包，界面与日志里一律打码成 `hunt_tools_****`。
* 子进程调用一律用参数数组，不拼 shell 字符串。
* 写完 compose 配置后用 `docker compose config --format json` 复核一遍
  "除 web 外的端口是否都绑在 127.0.0.1"，没绑住就拒绝启动。
* 启动器**从不**执行 `docker compose down -v`。

---

<!--
## [未发布]

### 新增
### 变更
### 修复
-->

[0.1.0]: https://github.com/agentpit-io/HunterLauncher/releases/tag/launcher-v0.1.0
