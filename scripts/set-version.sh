#!/usr/bin/env bash
# 一次把版本号改到四个地方。用法：scripts/set-version.sh 0.1.0-rc.1
#
# 为什么要一个脚本：版本号写在四个文件里，任何一个忘了改都会出真问题 ——
#
#   src-tauri/tauri.conf.json   打包用它命名产物，release.yml 会拿它跟 tag 对账
#   src-tauri/Cargo.toml        `env!("CARGO_PKG_VERSION")` 编译进程序，
#                               **updater 比的就是这个值**，落后了就会「装了新版还提示有新版」
#   package.json                前端的版本，标题栏右上角那行字读的是 app_info 给的值
#   src/lib/demo.ts             演示数据里的版本号。它只在 VITE_DEMO=1 的构建里生效，
#                               但**截图是用那个构建拍的** —— 忘了改的话每一轮的截图
#                               右上角都写着同一个旧版本号，看图的人分不清是哪一版
#                               （I8 截图时发现它从 M1 起就一直停在 0.1.0）
#
# 版本号必须是合法 semver：`0.1.0` / `0.1.0-rc.1`。预发布后缀带点号（`-rc.1` 而不是
# `-rc1`），因为 Cargo 与 Tauri 的 semver 实现都按点号分段比较，`rc.10 > rc.9` 才成立；
# 写成 `rc10` 就变成字符串比较，`rc10 < rc9`。
set -euo pipefail

V="${1:?用法：scripts/set-version.sh <版本>，例如 0.1.0-rc.1}"
if ! [[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "不是合法的 semver：$V" >&2
  exit 1
fi
cd "$(dirname "$0")/.."

node -e '
  const fs = require("fs"), v = process.argv[1];
  for (const f of ["package.json", "src-tauri/tauri.conf.json"]) {
    const j = JSON.parse(fs.readFileSync(f, "utf8"));
    j.version = v;
    fs.writeFileSync(f, JSON.stringify(j, null, 2) + "\n");
  }
' "$V"

# 演示数据里的版本号（只在 VITE_DEMO=1 的构建里生效，但截图用的就是那个构建）
sed -i -E "s|^(export const DEMO_LAUNCHER_VERSION = ')[^']*(')|\1$V\2|" src/lib/demo.ts

# Cargo.toml 只改 [package] 段里的那一行（依赖的 version = "2" 不能动）
awk -v v="$V" '
  /^\[/ { inpkg = ($0 == "[package]") }
  inpkg && /^version *=/ { print "version = \"" v "\""; next }
  { print }
' src-tauri/Cargo.toml > /tmp/Cargo.toml.new && mv /tmp/Cargo.toml.new src-tauri/Cargo.toml

echo "版本号已改成 $V："
grep -m1 '"version"' package.json
grep -m1 '"version"' src-tauri/tauri.conf.json
awk '/^\[package\]/{p=1} p && /^version *=/{print; exit}' src-tauri/Cargo.toml
grep -m1 'DEMO_LAUNCHER_VERSION' src/lib/demo.ts
echo
echo "记得跑一次 cargo check（或 cargo update -p hunter-launcher）把 Cargo.lock 也带上，"
echo "否则 CI 的 --locked 会报锁文件过期。"
