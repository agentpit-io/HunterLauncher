#!/usr/bin/env bash
#
# 两源 × 四镜像 × 指定 tag，**全部可拉才算通过**（I17 · 方案 §4.2①）。
#
# 用法：
#   scripts/verify-mirrors.sh 1.2.3            # 查 1.2.3（前导 v 可有可无）
#   scripts/verify-mirrors.sh 1.2.4 --only cn  # 只查国内源
#   scripts/verify-mirrors.sh 1.2.4 --only ghcr
#
# 退出码：0 = 两源四个镜像全齐；1 = 有缺的（缺哪个打哪个）。
#
# ## 它为什么存在
#
# 2026-09-29 客户那次升级失败不是「没人同步镜像」，而是**同步窗口追不上发布**：
# 国内源是每 6 小时的定时同步（`.github/workflows/cn-mirror.yml`），
# 而升级按钮在 GitHub Release 出现那一刻就亮了 —— v1.2.3 发布到客户点升级只隔 8 分钟，
# 国内源那时还是 404，启动器按设计自动换到 GHCR 重试，用户就在那中间点了退出。
#
# 方案 §4.2① 定的规矩：**「两源齐了」是发布完成的最后一条勾**，不是可选项。
# 发完版手动触发一次同步，然后跑这个脚本确认。所以它只做两件事：
# **发现**（哪个源缺哪个镜像）与**拦截**（非零退出，让发布清单上那一勾打不下去）。
#
# ## 明确不做
#
# **不推镜像、不同步、不需要任何账号。** 国内源的同步由另一条链路负责
# （用户 2026-09-29 明确），这里只探。探测用的是 registry v2 的标准匿名流程：
# `GET /v2/` → 换一个匿名 token → `GET /v2/<repo>/manifests/<tag>`，
# 只看 HTTP 状态码，不下载任何一层（方案 §4.1 那段 curl 就是它）。
set -uo pipefail

TAG="${1:-}"
ONLY="both"
if [ "${2:-}" = "--only" ]; then
  ONLY="${3:-both}"
fi

if [ -z "$TAG" ] || [ "$TAG" = "-h" ] || [ "$TAG" = "--help" ]; then
  sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
fi

# 前导 v 去掉：`.env` 里的 `HUNTER_VERSION` 与镜像 tag 都不带 v
TAG="${TAG#v}"

# 四个自家镜像（postgres / redis 不查：它们来自 Docker Hub 官方库与国内源的 library 路径，
# 与 Hunter 的发布节奏无关）
SERVICES=(hunter-community-web hunter-community-api hunter-community-opencode hunter-community-llm-shim)

# 两个源。**主机名与仓库路径一个字节都不许改**（改了就是换源事故）——
# 它们与 `src-tauri/src/registry.rs` 的 CANDIDATES 是同一对，
# `scripts/check-retired-mirrors.sh` 另外盯着「不许用已停用的广州区地址」。
#
# 每项：`名字|registry 主机|仓库前缀|换 token 的地址`
SOURCES=(
  "GHCR|ghcr.io|agentpit-io|https://ghcr.io/token?service=ghcr.io&scope=repository:REPO:pull"
  "中国国内云服务|hkccr.ccs.tencentyun.com|agentpit|https://hkccr.ccs.tencentyun.com/service/token?service=token-service&scope=repository:REPO:pull"
)

# manifests 的 Accept：三样都要列上（OCI index / docker manifest list / 单架构 v2）。
# 少列一样，多架构 manifest 会被 registry 按单架构回，探出来的结果是假的。
ACCEPT='Accept: application/vnd.oci.image.index.v1+json,application/vnd.docker.distribution.manifest.list.v2+json,application/vnd.docker.distribution.manifest.v2+json'

TIMEOUT="${TIMEOUT:-20}"

command -v curl >/dev/null || { echo "找不到 curl" >&2; exit 1; }
command -v python3 >/dev/null || { echo "找不到 python3" >&2; exit 1; }

# 换一个匿名 token。拿不到就返回空串 —— 调用方按 401/403 处理，**不猜成「有」**。
anon_token() {
  curl -fsS --max-time "$TIMEOUT" "$1" 2>/dev/null \
    | python3 -c 'import sys,json
try:
    d = json.load(sys.stdin)
except Exception:
    print(""); raise SystemExit
tok = d.get("token") or d.get("access_token") or ""
print(tok)'
}

# 探一个 manifest，回一个 HTTP 状态码（拿不到就回 000）。
probe_manifest() {
  local host="$1" repo="$2" token="$3"
  if [ -n "$token" ]; then
    curl -s -o /dev/null -w '%{http_code}' --max-time "$TIMEOUT" \
      -H "Authorization: Bearer $token" -H "$ACCEPT" \
      "https://$host/v2/$repo/manifests/$TAG"
  else
    curl -s -o /dev/null -w '%{http_code}' --max-time "$TIMEOUT" \
      -H "$ACCEPT" \
      "https://$host/v2/$repo/manifests/$TAG"
  fi
}

echo "校验镜像源：tag=$TAG"
echo "（只探测，不推任何东西；国内源的同步由另一条链路负责）"
echo

TOTAL=0
OK=0
MISSING_ROWS=()

for entry in "${SOURCES[@]}"; do
  IFS='|' read -r label host prefix token_url <<<"$entry"
  case "$ONLY" in
    cn)   [ "$host" = "ghcr.io" ] && continue ;;
    ghcr) [ "$host" != "ghcr.io" ] && continue ;;
  esac
  echo "── $label（$host/$prefix）"
  for svc in "${SERVICES[@]}"; do
    repo="$prefix/$svc"
    TOTAL=$((TOTAL + 1))
    tok="$(anon_token "${token_url//REPO/$repo}")"
    code="$(probe_manifest "$host" "$repo" "$tok")"
    if [ "$code" = "200" ]; then
      OK=$((OK + 1))
      printf '   ✓ %-34s %s\n' "$svc" "$TAG"
    else
      # 401/403 是「换不到 token / 不给看」，404 是「这一版确实没有」——
      # 两种都算不可用，但说清是哪种，免得排查时找错方向
      case "$code" in
        401|403) why="拿不到匿名 token（HTTP $code）" ;;
        404)     why="这一版在源上不存在（HTTP 404）" ;;
        000)     why="连不上（超时或 DNS 失败）" ;;
        *)       why="HTTP $code" ;;
      esac
      printf '   ✗ %-34s %s —— %s\n' "$svc" "$TAG" "$why"
      MISSING_ROWS+=("$label|$host/$repo:$TAG|$why")
    fi
  done
  echo
done

echo "──────────────────────────────────────────────"
echo "结果：$OK / $TOTAL 可拉"

if [ "${#MISSING_ROWS[@]}" -eq 0 ]; then
  echo "两源四个镜像都齐了 —— 这一条可以打勾。"
  exit 0
fi

echo
echo "缺这些："
for row in "${MISSING_ROWS[@]}"; do
  IFS='|' read -r label ref why <<<"$row"
  echo "  · $label  $ref  —— $why"
done
echo
echo "国内源缺的话：先手动触发一次同步，再跑一遍这个脚本。"
echo "  gh workflow run cn-mirror.yml --repo agentpit-io/HunterLauncher"
echo "两源齐了才算这次发布完成（方案 §4.2①：这一条是发布清单的最后一勾）。"
exit 1
