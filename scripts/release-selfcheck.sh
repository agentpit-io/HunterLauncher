#!/usr/bin/env bash
# 发布后自查：把产物**从 Release 页真的下回来**再验，不看本地那一份。
#
#   用法: scripts/release-selfcheck.sh 0.1.9 [tag]
#
# I8 之前这一套是手敲的，每次都要重想一遍查什么；I9 固化成脚本，
# 以后每次发版跑一次就行。任何一条不过整个脚本退出码非 0。
#
# 查这些：
#   1. Release 是 prerelease（红线 7：包没有代码签名，不能标正式版）
#   2. 产物齐全：三平台的包 + 自更新用的 .sig + latest.json
#   3. dmg 里 Info.plist 的版本 / 版权（mac 用户看到的版本号来自这里）
#   4. .deb 的 Version 字段、解出来的二进制 --version
#   5. 腾讯云香港的副本存在且字节数与 GitHub 一致（国内用户走这条路）
#   6. COS 那份 latest.json 的 url 指向 COS 而不是 GitHub
set -uo pipefail

V="${1:?用法: release-selfcheck.sh <版本号> [tag]}"
TAG="${2:-launcher-v$V}"
REPO="${REPO:-agentpit-io/HunterLauncher}"
CN_BASE="${CN_DOWNLOAD_BASE:-https://hunter-dl-hk-1253756459.cos.ap-hongkong.myqcloud.com}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

fail=0
skipped=0
ok()   { printf '  ✅ %s\n' "$*"; }
bad()  { printf '  ❌ %s\n' "$*"; fail=1; }
step() { printf '\n── %s ──\n' "$*"; }
# 期望值 实测值 说明
eq()   { if [ "$1" = "$2" ]; then ok "$3 = $2"; else bad "$3：期望 $1，实测 $2"; fi; }

step "1. Release 元信息"
# 走公开 REST API 而不是 gh：这个脚本要能在测试机上跑（那台没有 gh，
# 而 `--version` 那一条只有装了 webkit2gtk 的机器跑得了）。仓库是公开的，
# 不带 token 也能读；有 GH_TOKEN 就带上，免得撞 60 次/小时的匿名限额。
meta="$WORK/rel.json"
AUTH=()
[ -n "${GH_TOKEN:-${GITHUB_TOKEN:-}}" ] && AUTH=(-H "Authorization: Bearer ${GH_TOKEN:-$GITHUB_TOKEN}")
if ! curl -sSfL --max-time 60 "${AUTH[@]}" \
     -H "Accept: application/vnd.github+json" \
     "https://api.github.com/repos/$REPO/releases/tags/$TAG" > "$meta" 2>"$WORK/err"; then
  bad "取不到 Release $TAG：$(cat "$WORK/err")"; exit 1
fi
eq true "$(jq -r .prerelease "$meta")" "prerelease"
eq "$TAG" "$(jq -r .tag_name "$meta")" "tag_name"

# 产物清单**另外**取一次，不能用上面那份 release 对象里的 `.assets`。
# I12 踩到的（0.1.12 发完当场）：`/releases/tags/<tag>` 这个端点会在发布后
# 相当长一段时间里返回 `"assets": []`，而同一时刻
# `/releases/<id>/assets` 已经把 12 个产物全列出来、个个 `state=uploaded`、
# 浏览器地址也真的下得动（实测 .deb 4622798 字节，和 COS 那份一模一样）。
# 上面那份只用来读 prerelease / tag_name 这类不会变的字段。
rid=$(jq -r .id "$meta")
if ! curl -sSfL --max-time 60 "${AUTH[@]}" \
     -H "Accept: application/vnd.github+json" \
     "https://api.github.com/repos/$REPO/releases/$rid/assets?per_page=100" \
     | jq '{assets: .}' > "$WORK/assets.json" 2>"$WORK/err"; then
  bad "取不到 Release $TAG 的产物清单：$(cat "$WORK/err")"; exit 1
fi
meta_rel="$meta"
meta="$WORK/assets.json"

step "2. 产物清单"
jq -r '.assets[].name' "$meta" | sort | sed 's/^/     /'
n=$(jq '.assets | length' "$meta")
echo "     共 $n 个"
# 一个平台少一个 .sig，那个平台的自更新就是坏的
for want in "hunter-launcher_${V}_amd64.deb" \
            "hunter-launcher_${V}_amd64.deb.sig" \
            "hunter-launcher_${V}_amd64.AppImage" \
            "hunter-launcher_${V}_amd64.AppImage.sig" \
            "hunter-launcher_${V}_universal.dmg" \
            "hunter-launcher_${V}_universal.app.tar.gz" \
            "hunter-launcher_${V}_universal.app.tar.gz.sig" \
            "hunter-launcher_${V}_x64-setup.exe" \
            "hunter-launcher_${V}_x64-setup.exe.sig" \
            "hunter-launcher_${V}_x64_en-US.msi" \
            "hunter-launcher_${V}_x64_en-US.msi.sig" \
            "latest.json"; do
  if jq -e --arg w "$want" '.assets[] | select(.name==$w)' "$meta" >/dev/null; then
    ok "$want"
  else
    bad "缺 $want"
  fi
done

dl() { # 资产名 → 下到 $WORK，回声路径（下不到就回声空）
  local name="$1" url
  url=$(jq -r --arg n "$name" '.assets[] | select(.name==$n) | .browser_download_url' "$meta")
  [ -n "$url" ] && [ "$url" != "null" ] || return 0
  curl -sSfL --max-time 600 -o "$WORK/$name" "$url" >/dev/null 2>&1 \
    && printf '%s' "$WORK/$name"
}

step "3. dmg 里的 Info.plist（mac 用户看到的版本号）"
dmg="$(dl "hunter-launcher_${V}_universal.dmg")"
if [ -n "$dmg" ] && [ -s "$dmg" ]; then
  # Linux 上没有 hdiutil；dmg 是 APFS 镜像，够新的 7-Zip 能把里面的 .app 抽出来。
  # 版本很讲究：Ubuntu 24.04 自带的 7-Zip 23.01 读不了 APFS（测试机上实测「解不开」），
  # 26.02 可以。所以优先找 7zz，再退回 PATH 上的 7z。
  SEVENZ=""
  for c in "$HOME/bin/7zz" 7zz 7z 7za; do
    command -v "$c" >/dev/null 2>&1 && { SEVENZ="$c"; break; }
  done
  if [ -z "$SEVENZ" ]; then
    printf '  ⚠️  dmg 的 Info.plist：未测 —— 本机没有 7z/7zz\n'; skipped=1
  elif "$SEVENZ" x -y -o"$WORK/dmg" "$dmg" >/dev/null 2>&1; then
    plist="$(find "$WORK/dmg" -path '*.app/Contents/Info.plist' -print -quit)"
    if [ -n "$plist" ]; then
      # Tauri 产出的是二进制 plist，用 python 的 plistlib 读
      python3 - "$plist" <<'PY' > "$WORK/plist.txt" 2>&1
import plistlib, sys
d = plistlib.load(open(sys.argv[1], "rb"))
for k in ("CFBundleShortVersionString", "CFBundleVersion",
          "NSHumanReadableCopyright", "CFBundleIdentifier", "LSMinimumSystemVersion"):
    print(f"{k}={d.get(k, '<缺>')}")
PY
      sed 's/^/     /' "$WORK/plist.txt"
      g() { grep -m1 "^$1=" "$WORK/plist.txt" | cut -d= -f2-; }
      eq "$V" "$(g CFBundleShortVersionString)" "CFBundleShortVersionString"
      eq "$V" "$(g CFBundleVersion)"            "CFBundleVersion"
      [ -n "$(g NSHumanReadableCopyright)" ] && [ "$(g NSHumanReadableCopyright)" != "<缺>" ] \
        && ok "NSHumanReadableCopyright = $(g NSHumanReadableCopyright)" \
        || bad "NSHumanReadableCopyright 缺失"
    else
      bad "dmg 里找不到 *.app/Contents/Info.plist"
    fi
  else
    # 分清「产物坏了」和「这台机器的工具太老」：后者是未测，不是不通过
    printf '  ⚠️  dmg 的 Info.plist：未测 —— %s 解不开这个 dmg（多半是版本太老读不了 APFS，需要 7-Zip ≥ 24）\n' \
      "$("$SEVENZ" 2>&1 | grep -om1 '7-Zip[^:]*')"; skipped=1
  fi
else
  bad "dmg 下不下来"
fi

step "4. .deb 的版本与二进制 --version"
deb="$(dl "hunter-launcher_${V}_amd64.deb")"
if [ -n "$deb" ] && [ -s "$deb" ]; then
  eq "$V" "$(dpkg-deb -f "$deb" Version)" ".deb Version"
  if dpkg-deb -x "$deb" "$WORK/deb" 2>/dev/null; then
    binp="$(find "$WORK/deb" -type f -name 'hunter-launcher' -perm -u+x -print -quit)"
    if [ -n "$binp" ]; then
      out="$("$binp" --version 2>&1 | head -1)"
      # 开发机上没有 webkit2gtk（总控规则：这台不装系统包），跑不起来不是产物的问题。
      # 红线 5：测不到就写「未测」和原因，不许含糊过去，也不许当成通过。
      case "$out" in
        *"error while loading shared libraries"*|*"cannot open shared object"*)
          printf '  ⚠️  二进制 --version：未测 —— 本机缺 %s，这一条要在测试机上跑\n' \
            "$(printf '%s' "$out" | grep -o 'lib[^:]*\.so[^:]*' | head -1)"
          skipped=1
          ;;
        *) eq "hunter-launcher $V" "$out" "二进制 --version" ;;
      esac
    else
      bad ".deb 里找不到 hunter-launcher 可执行文件"
    fi
  else
    bad ".deb 解不开"
  fi
else
  bad ".deb 下不下来"
fi

step "5. 腾讯云香港的副本（国内用户走这条）"
# 与 GitHub 上那一份逐字节比大小；只比大小不比 sha 是因为 COS 不回 etag 以外的摘要，
# 而分块上传的 etag 不是 md5 —— 大小对不上就一定是传坏了
for a in "hunter-launcher_${V}_universal.dmg" \
         "hunter-launcher_${V}_amd64.deb" \
         "hunter-launcher_${V}_x64-setup.exe"; do
  gh_size=$(jq -r --arg a "$a" '.assets[] | select(.name==$a) | .size' "$meta")
  hdr=$(curl -sSI --max-time 60 "$CN_BASE/launcher/$V/$a")
  code=$(printf '%s' "$hdr" | head -1 | awk '{print $2}')
  cn_size=$(printf '%s' "$hdr" | tr -d '\r' | grep -i '^content-length:' | tail -1 | awk '{print $2}')
  if [ "$code" = "200" ] && [ -n "$gh_size" ] && [ "$gh_size" = "$cn_size" ]; then
    ok "$a · 200 · $cn_size 字节（与 GitHub 一致）"
  else
    bad "$a · HTTP $code · COS $cn_size vs GitHub $gh_size"
  fi
done

step "6. COS 那份 latest.json"
if curl -sS --max-time 60 -o "$WORK/cn.json" "$CN_BASE/launcher/latest.json"; then
  eq "$V" "$(jq -r .version "$WORK/cn.json")" "latest.json version"
  for p in linux-x86_64 windows-x86_64 darwin-x86_64 darwin-aarch64; do
    u=$(jq -r --arg p "$p" '.platforms[$p].url // "<缺>"' "$WORK/cn.json")
    case "$u" in
      "$CN_BASE"/*) ok "$p → COS" ;;
      "<缺>")       bad "$p 这个平台在 latest.json 里没有（该平台永远收不到更新）" ;;
      *)            bad "$p 指向的不是 COS：$u" ;;
    esac
  done
else
  bad "COS 的 latest.json 取不到"
fi

step "小结"
if [ $fail -ne 0 ]; then
  echo "  有不通过的项，见上面的 ❌"
elif [ $skipped -ne 0 ]; then
  echo "  没有不通过的项，但有「未测」的（⚠️）—— 那几条要换台机器跑（版本 $V · tag $TAG）"
else
  echo "  全部通过（版本 $V · tag $TAG）"
fi
exit $fail
