#!/usr/bin/env bash
# 生成 GitHub Release 的说明。用法：release-notes.sh <版本>
#
# 内容 = CHANGELOG.md 里该版本那一节 + 三段固定文字：
#   · 下载（三平台 + 国内地址）
#   · 未签名说明（SmartScreen / Gatekeeper 怎么放行）
#   · 社区反馈回应（方案 §11.4 的第四项反馈机制；首发时是占位）
#
# 固定文字写在这里而不是 CHANGELOG 里：它们每个版本都一样，抄进 CHANGELOG 只会让
# 真正的变更被淹掉。
set -euo pipefail

V="${1:?用法：release-notes.sh <版本>}"
REPO="agentpit-io/HunterLauncher"
CN_BASE="${CN_DOWNLOAD_BASE:-https://hunter-dl-hk-1253756459.cos.ap-hongkong.myqcloud.com}"
TAG="launcher-v${V}"

# ── CHANGELOG 里这个版本那一节 ────────────────────────────────────────────
# 预发布（0.1.0-rc.1）在 CHANGELOG 里通常没有自己的小节 —— 它和要发的正式版是同一批变更，
# 为每个 rc 抄一遍只会让 CHANGELOG 变成流水账。所以：先找精确匹配，找不到就退回基础版本
# （去掉 `-rc.N` 后缀）那一节，并在前面说明这是预发布。
section() {
  awk -v v="$1" '
    $0 ~ "^## \\[?" v "\\]?" { on = 1; print; next }
    on && /^## / { exit }
    on { print }
  ' CHANGELOG.md
}

if [ -f CHANGELOG.md ]; then
  BODY=$(section "$V")
  if [ -z "$BODY" ]; then
    BASE="${V%%-*}"
    BODY=$(section "$BASE")
    if [ -n "$BODY" ]; then
      echo "> 这是 **${V}**（${BASE} 的预发布）。下面是 ${BASE} 的变更清单。"
      echo
    fi
  fi
  printf '%s\n' "$BODY"
fi

cat <<EOF

## 下载

| 平台 | 文件 | 说明 |
|---|---|---|
| Windows 10/11 x64 | \`hunter-launcher_${V}_x64-setup.exe\`（推荐）/ \`hunter-launcher_${V}_x64_en-US.msi\` | NSIS 安装包 / MSI |
| macOS 12+ | \`hunter-launcher_${V}_universal.dmg\` | 通用二进制（Intel + Apple Silicon） |
| Ubuntu 22.04+ / Debian 12+ x64 | \`hunter-launcher_${V}_amd64.deb\`（推荐）/ \`hunter-launcher_${V}_amd64.AppImage\` | deb 约 3.8 MiB；AppImage 约 78 MiB（自带整套 WebKitGTK，这是格式的固有代价） |

**国内下载**（腾讯云香港，不用翻墙）：
\`${CN_BASE}/launcher/${V}/\`

**GitHub 下载**：https://github.com/${REPO}/releases/tag/${TAG}

Linux 上装 deb：

\`\`\`bash
sudo apt install -y ./hunter-launcher_${V}_amd64.deb
hunter-launcher            # 有桌面就开界面
hunter-launcher --headless # 纯 SSH 的机器走这条
\`\`\`

## ⚠ 这些包没有代码签名

Windows 代码签名证书（100–500 美元/年）与 Apple 开发者账号（99 美元/年）都要花钱，
本项目目前**没有购买**，所以：

* **Windows**：SmartScreen 会拦一下。点「更多信息」→「仍要运行」。
* **macOS**：会说「无法打开，因为它来自身份不明的开发者」或「已损坏」。
  右键点 .app →「打开」→ 再点一次「打开」；仍然不行就在终端里跑
  \`xattr -dr com.apple.quarantine "/Applications/Hunter Launcher.app"\`（路径里有空格，引号别漏）。
* **Linux**：不受影响，deb / AppImage 本来就不强制签名。

因此这个 Release 标为 **prerelease（预发布）**。要不要买证书、转正式版由用户决定。
详细的图文步骤在 [docs/常见问题.md](https://github.com/${REPO}/blob/main/docs/常见问题.md)。

**自更新的清单是签名的**：updater 用的 minisign 密钥是本项目自己生成的，公钥编译在程序里，
签名验不过的更新包不会被安装。这一条与上面的代码签名是两件事。

## 已知问题

* AppImage 有 78 MiB，远超设计目标里写的 40 MB —— 它要自带整套 WebKitGTK，压不下去。
  介意体积就用 deb。
* \`.deb\` 装的启动器**不能就地自更新**（换 /usr 下的文件要 root）。点「更新」时启动器会把
  新包下到 \`~/.hunter/updates/\` 并给出一条 \`sudo apt install\` 命令，最后一步由你来。
  AppImage / Windows / macOS 是全自动的。
* 运行面板上「今日对话」「晨报」两张卡显示 \`—\`：hunter-community 的 api 没有对应的
  免登录接口（逐条实测过），按「拿不到就不编」的原则空着。上游接口清单在面板上可以点开看。
* Windows / macOS 只在 CI 里编译与打包通过，**没有在真机上跑过**。

## 社区反馈回应

<!-- 每个版本在这里逐条回应上一版收到的 issue / 讨论：谁提的、提了什么、这一版做了什么。
     方案 §11.4 的第四项反馈机制。首发版本还没有收到反馈，这一节先空着。 -->

这是启动器的第一个公开版本，还没有收到社区反馈。用得不顺手请开 issue：
https://github.com/${REPO}/issues/new —— 启动器里「反馈」按钮能一键生成预填好的 issue，
并附一份**脱敏过**的诊断包（不含 key）。
EOF
