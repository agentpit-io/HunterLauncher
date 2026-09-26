#!/usr/bin/env bash
# I16 追加 · 选源探测的真机验收：**元数据通、层数据下不来的源要判不可用**。
#
# ## 复现的是谁的现场
#
# 2026-09-26 那位 Windows 用户：他机器上 GHCR 的 manifest 又快又通
# （于是每次选源都选中 GHCR），可层数据一个字节都下不来 ——
# 升级因此卡了一个多小时，前后卡了三次。
#
# 这个分裂是**真实存在的**，不需要编：GHCR 的 manifest 在 `ghcr.io` 上，
# 层数据是 307 跳到 `pkg-containers.githubusercontent.com` 上的（本脚本会打印那个 307）。
# 所以复现它最忠实的办法不是造一个假 registry，而是**只把层那一侧堵掉**：
# 在 `/etc/hosts` 里把 `pkg-containers.githubusercontent.com` 指到
# 198.51.100.1（TEST-NET-2，不可路由，连上去只会一直等），`ghcr.io` 一个字都不动。
#
# 于是应当看到：
#   * `probe(ghcr)`    → available=false，原因写「元数据能拿到，但层数据下不来」
#   * `probe(tencent)` → available=true，并且给得出实测的 data_ms
#   * `choose()`       → 选腾讯云
#
# ## 为什么不用 scripts/fake-stalling-registry.py 来验这一条
#
# 试过，走不通，原因值得记一笔：启动器的 HTTP 客户端是 ureq，而 ureq 用的是
# **webpki-roots（编译进二进制的 Mozilla 根证书表）**，不读系统信任库
# （`cargo tree -i webpki-roots` 能看到；rustls-platform-verifier 只在
# tauri-plugin-updater 那条链上）。把自签 CA 装进 /usr/local/share/ca-certificates
# 对它没有任何作用，报的是 `invalid peer certificate: UnknownIssuer`。
# 假 registry 那条路照旧用来验 `docker compose pull` 的卡死判定
# （docker 读 /etc/docker/certs.d，见 scripts/i16-stall-test.sh）。
#
# **只改 /etc/hosts 一行，退出时原样写回。不碰这台机器的防火墙（红线 9）。**
set -u

OUT="${OUT:-/tmp/i16-probe}"
REPO="${REPO:-$HOME/HunterLauncher}"
LAYER_HOST="pkg-containers.githubusercontent.com"
BLACKHOLE="198.51.100.1"   # TEST-NET-2（RFC 5737），保证不可路由

mkdir -p "$OUT"

echo "=== 1/3 先证明这个分裂是真的：GHCR 的层是 307 跳到另一个主机上的 ==="
TOK=$(curl -s "https://ghcr.io/token?scope=repository:agentpit-io/hunter-community-web:pull&service=ghcr.io" \
      | python3 -c 'import json,sys;print(json.load(sys.stdin)["token"])')
curl -s -o /dev/null -D- -m 20 -H "Authorization: Bearer $TOK" \
  "https://ghcr.io/v2/agentpit-io/hunter-community-web/blobs/sha256:4f4fb700ef54461cfa02571ae0db9a0dc1e0cdb5577484a6d75e68dc38e8acc1" \
  | grep -iE "^HTTP/|^location:" | cut -c1-120 | sed 's/^/  /'

echo
echo "=== 2/3 把层那一侧堵掉（只动 /etc/hosts 一行） ==="
sudo cp /etc/hosts "$OUT/hosts.bak"
restore() {
  sudo cp "$OUT/hosts.bak" /etc/hosts
  echo "已把 /etc/hosts 写回原样："
  grep -c "$LAYER_HOST" /etc/hosts || echo "  里面已经没有 $LAYER_HOST 了 ✓"
}
trap restore EXIT
echo "$BLACKHOLE $LAYER_HOST" | sudo tee -a /etc/hosts >/dev/null
echo "  已加：$BLACKHOLE $LAYER_HOST"
echo "  验一下（应当连不上 / 一直等）："
timeout 8 curl -s -o /dev/null -w "    curl 退出前 http=%{http_code} 用时 %{time_total}s\n" \
  "https://$LAYER_HOST/" || echo "    curl 超时退出（正是要的现场）"

echo
echo "=== 3/3 跑那条真机单测 ==="
cd "$REPO/src-tauri" || exit 1
# shellcheck disable=SC1090
source "$HOME/.cargo/env"
HL_BLOCKED_LAYER_HOST="$LAYER_HOST" \
  cargo test --lib -- --ignored --nocapture probe_真机 2>&1 | tee "$OUT/probe-test.log"
exit "${PIPESTATUS[0]}"
