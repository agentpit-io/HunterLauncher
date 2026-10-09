# I20 Windows 验收手册

本手册用于在**没有桌面会话的 Windows Server**（例如 GCP `hunter-win-test`，Windows Server 2022，无桌面环境且无法安装 Docker Desktop）上验收 0.1.22 的升级闸门及诊断真话机制。

---

## 一、已知 Windows 无桌面环境避坑指南（必读）

在 Windows Server 2022 无桌面/远程终端环境下执行验收时，请务必注意以下 4 条已知陷阱：

1. **PowerShell 5.1 编码陷阱**：PowerShell 5.1 会把无 BOM 的 UTF-8 文件默认当 ANSI 处理。编写注入脚本或写入临时配置文件时，脚本文件必须使用纯 ASCII，或显式指定 UTF-8 编码。
2. **安装路径落点**：perUser 安装落点为 `%LOCALAPPDATA%\Hunter Launcher`（**没有** `\Programs\` 那一层，即 `C:\Users\<用户>\AppData\Local\Hunter Launcher`）。
3. **输出重定向截流**：`Start-Process -RedirectStandardOutput` 在部分 PowerShell 5.1 环境下抓不到 CLI 的标准输出。推荐直接在控制台执行命令，或通过 `.bat` / `cmd /c "hunter-launcher.exe ... > out.log 2>&1"` 进行重定向。
4. **`Get-Content` 读取乱码**：PowerShell 5.1 的 `Get-Content` 默认按系统 ANSI 读取文件，查看 UTF-8 日志文件时容易产生乱码。读取日志时请加 `-Encoding UTF8`（例如 `Get-Content -Encoding UTF8 .\launcher.log`），或使用 `type` 命令查看。

---

## 二、日志排查取法

CLI 运行及升级过程日志会写入以下两个位置：

1. **自定义临时环境目录**：
   如果设置了 `$env:HUNTER_HOME = "C:\temp\hunter_test"`，则日志直接写入：
   `$env:HUNTER_HOME\logs\launcher.log`
2. **默认用户根目录**：
   `$env:USERPROFILE\.hunter\logs\launcher.log`（即 `C:\Users\<用户>\.hunter\logs\launcher.log`）

查看日志最新输出命令（PowerShell）：
```powershell
Get-Content -Encoding UTF8 "$env:HUNTER_HOME\logs\launcher.log" -Tail 50
```

---

## 三、验收用例

### W1（本机真没有 Docker）：必须拦在 ⓪ 步，文案如实且不谎称「拉起经过」

#### 1. 前置准备与执行
通过设置环境变量 `$env:HUNTER_HOME` 指向独立的临时目录，隔离真实安装，并准备伪配置：

```powershell
# 1. 准备隔离测试目录与伪配置
$testDir = "C:\temp\hunter_w1"
if (Test-Path $testDir) { Remove-Item -Recurse -Force $testDir }
New-Item -ItemType Directory -Force -Path "$testDir\app" | Out-Null
New-Item -ItemType Directory -Force -Path "$testDir\logs" | Out-Null

# 写入最小化已安装配置（纯 ASCII 格式）
@"
[install]
done = true
runtime = "user"

[hunter]
tag = "1.2.3"
"@ | Set-Content -Path "$testDir\launcher.toml" -Encoding Ascii

# 2. 设置隔离环境变量并执行升级
$env:HUNTER_HOME = $testDir
# 确保没有残留的远程 DOCKER_HOST
Remove-Item Env:\DOCKER_HOST -ErrorAction SilentlyContinue

# 执行升级命令（进入安装目录或调用对应路径的 exe）
& "$env:LOCALAPPDATA\Hunter Launcher\hunter-launcher.exe" --upgrade 2.1.0
```

#### 2. 验收判据
1. **退出状态与错误码**：命令报错退出，错误码为 `E_DAEMON_DOWN`。
2. **拦截位置**：升级必须在第 ⓪ 步立即停止，**不得进入第 ① 步**（不能出现「正在取 v2.1.0 的 docker-compose.yml…」）。
3. **提示文案真实**：
   - 控制台或日志中出现：
     ```text
     运行环境（Docker）现在用不了（Windows: 未检测到已安装的 Docker），这次升级先停在这里 —— 配置一个字节都没改。
     ```
   - **严禁出现**旧版的假话：`上面那几行是启动器替你把它拉起来的经过。`（因为启动器根本未动手）。

---

### W2（能连到一台真 Docker）：必须成功越过 ⓪ 步进入后续步骤

#### 1. 前置准备与执行
提供一台可网络访问的真实 Docker 守护进程（开启 TCP 端口或由另一台 Linux 测试机暴露）：

```powershell
# 1. 准备隔离测试目录与伪配置
$testDir = "C:\temp\hunter_w2"
if (Test-Path $testDir) { Remove-Item -Recurse -Force $testDir }
New-Item -ItemType Directory -Force -Path "$testDir\app" | Out-Null
New-Item -ItemType Directory -Force -Path "$testDir\logs" | Out-Null

@"
[install]
done = true
runtime = "user"

[hunter]
tag = "1.2.3"
"@ | Set-Content -Path "$testDir\launcher.toml" -Encoding Ascii

# 2. 设置隔离环境与远程 Docker 地址（替换为实际可用 dockerd 地址）
$env:HUNTER_HOME = $testDir
$env:DOCKER_HOST = "tcp://10.0.0.2:2375"

# 执行升级命令
& "$env:LOCALAPPDATA\Hunter Launcher\hunter-launcher.exe" --upgrade 2.1.0
```

#### 2. 验收判据
1. **越过 ⓪ 步**：日志中**越过 ⓪ 步**，出现后续步骤日志：
   ```text
   正在取 v2.1.0 的 docker-compose.yml…
   ```
2. **放行证据**：由于环境变量 `DOCKER_HOST` 指向真实可用的 Docker Daemon，`runtime::engine::usable()` 返回 `true`，第 ⓪ 步检查无条件放行。（注：后续如果由于网络拉取镜像超时或无权限报错属于正常流程，关键判定点为**越过了第 ⓪ 步**）。

---

### W3（诊断报文 `--diagnose`）：两种环境下均为真话

#### 1. W1 状态下运行诊断（无 Docker）
```powershell
$env:HUNTER_HOME = "C:\temp\hunter_w1"
Remove-Item Env:\DOCKER_HOST -ErrorAction SilentlyContinue

& "$env:LOCALAPPDATA\Hunter Launcher\hunter-launcher.exe" --diagnose
```
**判据**：
输出中包含容器引擎行：
```text
容器引擎: Windows: 未检测到已安装的 Docker（可用: false）
```
或如果装了 Docker 但后台服务没在跑：
```text
容器引擎: Windows: Docker 已安装但后台未运行（可用: false）
```
绝不把「没装」误报为「装了没跑」，真话明确。

#### 2. W2 状态下运行诊断（可连接真 Docker）
```powershell
$env:HUNTER_HOME = "C:\temp\hunter_w2"
$env:DOCKER_HOST = "tcp://10.0.0.2:2375"

& "$env:LOCALAPPDATA\Hunter Launcher\hunter-launcher.exe" --diagnose
```
**判据**：
输出中容器引擎行显示后台正在运行且可用为 true：
```text
容器引擎: Windows: Docker 后台正在运行（客户端: ...，服务端: ...，模式: ...）（可用: true）
```
不再谎报「没有容器运行时」。


---

## 四、真机验收结果（2026-10-09 · GCP `hunter-win-test` · Windows Server 2022 无桌面）

- 被测版本：**0.1.22**，MSI 安装包 `hunter-launcher_0.1.22_x64_en-US.msi`
  （SHA256 `27c41ca20ffdf6be79aeb52cd28cbb32038e4750109956aa63185b899d356b50`）
- 执行方式：开发与编译在 `mixplode-hk-01`；注入与运行在 `hunter-win-test`（开机脚本 + 重置）；
  结果由测试机实时回传到中继机 `hl-docker-relay`，等待与判读全部在中继机上完成。

### 4.1 四条判据全部通过

| 用例 | 环境 | 关键证据 | 判定 |
|---|---|---|---|
| W1 | `--upgrade 2.1.0`，不设 `DOCKER_HOST` | 停在第 ⓪ 步，给出诚实话术；**没有**「正在取 v2.1.0 的 docker-compose.yml…」；**没有**「上面那几行是启动器替你把它拉起来的经过」 | 通过 |
| W2 | `--upgrade 2.1.0`，`DOCKER_HOST=tcp://10.128.0.14:2375`（对端是真容器后台 29.1.3） | 出现「正在取 v2.1.0 的 docker-compose.yml…」→「已取到（来自 hunter-dl-hk-…cos.ap-hongkong.myqcloud.com，36262 字节）」→进入「正在备份（数据库 + 配置）…」 | 通过 |
| W3a | `--diagnose`，不设 `DOCKER_HOST` | 第三行 `Docker DockerEngine · server None · compose None`；全文无「Docker 现在可用」 | 通过 |
| W3b | `--diagnose`，`DOCKER_HOST=tcp://10.128.0.14:2375` | 第三行 `Docker DockerEngine · server Some("29.1.3") · compose None`；出现「## Docker 现在可用，而这一套还没装完」 | 通过 |

W1 拦截时的真话（`launcher.log` 原文）：

```text
[error] headless 失败：运行环境（Docker）现在用不了（Windows: Docker 已安装但后台未运行），这次升级先停在这里 —— 配置一个字节都没改。
```

这正是 HL-D9MCCQ / HL-5FQNHX 那台客户机在 0.1.21 下永远看不到的结果：
0.1.21 无论后台在不在跑都只会说「用不了」；0.1.22 后台真在跑时放行（W2）、真没跑时才拦（W1）。

### 4.2 W2 之后后续步骤的附带观察（不影响本次判定）

W2 越过第 ⓪ 步后，停在第 ② 步「把 postgres 单独拉起来」：

```text
[error] headless 失败：拉起 postgres 失败：unknown flag: --project-name
```

原因是这台测试机装的是 **Docker CLI 29.x 的独立二进制**（`C:\Program Files\Docker\Docker\resources\bin\docker.exe`），
没有附带 `compose` 插件 —— 诊断首行的 `compose None` 已经如实点出这一点，
所以 `docker compose --project-name …` 走不通。按 §三 W2 的约定，后续步骤报错属正常，
**关键判定点只看有没有越过第 ⓪ 步**，已经越过。

### 4.3 本次实测新发现的三条坑（接 §一 的第 4 条往下编号）

5. **以系统账号（SYSTEM）静默装 NSIS 版会失败，而退出码仍是 0。**
   `setup.exe /S` 在 SYSTEM 下会写下注册表卸载项（指向
   `C:\Windows\system32\config\systemprofile\AppData\Local\Hunter Launcher`），**却一个文件都不落**；
   而机器上 `C:\Users\<某人>\AppData\Local\Hunter Launcher\` 里可能还躺着旧版本
   （本次实测是 0.1.17 与 0.1.18）。于是「按文件名找得到 exe」但装的是旧版本，后面全部判据都失真。
   **无桌面环境下请改走 MSI 机器范围安装**：

   ```powershell
   msiexec /i <包>.msi /qn /norestart ALLUSERS=1 /L*v C:\hl\msi.log
   ```

   装完 exe 落在 `%LOCALAPPDATA%\Hunter Launcher\hunter-launcher.exe`
   （SYSTEM 身份下即 `C:\Windows\system32\config\systemprofile\AppData\Local\…`）。
   **务必用 `(Get-Item $exe).VersionInfo.FileVersion` 核对版本**，不要只看文件是否存在。

6. **`Start-Process cmd.exe -ArgumentList '/c "<bat>"'` 在 SYSTEM 下拿不到任何东西**
   （实测退出码 255，既不生成重定向文件也不生成退出码文件）。
   可靠通道是直接起 exe、让 PowerShell 接管重定向：

   ```powershell
   Start-Process -FilePath $exe -ArgumentList @('--upgrade','2.1.0') `
     -RedirectStandardOutput out.txt -RedirectStandardError err.txt -PassThru
   ```

7. **PowerShell 里 `$home` 是只读自动变量**（等同 `$HOME`）。拿它当函数参数名会得到
   `SessionStateUnauthorizedAccessException: Cannot overwrite variable home because it is read-only or constant`；
   在 `$ErrorActionPreference = 'Continue'` 下这条错误**不会打印**，现场表现是「整段用例静默跳过、什么都不回传」。
   参数改名（例如 `$homeDir`），并给主流程套 `try/catch` 把异常写进回传。

### 4.4 测试用配置（§三 的样例不完整，会解析失败）

§三 给的最小 `launcher.toml` **缺少必填项**：`hunter.registry_id`、`hunter.registry_prefix`、
`hunter.base_prefix`、`[hunter.ports]` 五项、以及 `[install].at`。缺了会打

```text
[warn] launcher.toml 解析失败，这次用默认设置：TOML parse error ... missing field `registry_id`
```

于是 `install.done` 落回 false，`--upgrade` 会以「还没装过 Hunter，没有可升级的东西」提前退出，
**根本走不到第 ⓪ 步** —— 极易被误判成闸门有问题。下面这份已实测可解析：

```toml
[hunter]
tag = "1.2.3"
registry_id = "ghcr"
registry_prefix = "ghcr.io/agentpit-io"
base_prefix = "ghcr.io/agentpit-io"

[hunter.ports]
web = 3100
api = 8100
opencode = 3921
postgres = 5442
redis = 6479

[install]
done = true
at = "2026-10-09 12:00:00"
runtime = "user"
```

> 另注：§三 里写的预期输出「容器引擎: Windows: …（可用: false）」与 0.1.22 实际输出不一致，
> 实机输出的是诊断首行的 `Docker <来源> · server <None|Some("版本")> · compose <None|…>`，
> 以及按规则追加的「## Docker 现在可用，而这一套还没装完」小节。判据请以本节 4.1 表为准。
