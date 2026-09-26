#!/usr/bin/env bash
# I16 · P0-1 的真机验收：**拉取卡住要能自己发现**。
#
# 造的现场和客户 2026-09-25 那次一样：manifest 读得到（所以镜像总量算得出来），
# 拉层的时候一个字节都不来，而且**不报错**。原来的启动器会一直等下去。
#
# 用完全独立的 HUNTER_HOME，不碰这台机器上跑着的那两套 Hunter。
set -u

BIN="${BIN:-$HOME/HunterLauncher/src-tauri/target/release/hunter-launcher}"
PORT="${PORT:-5999}"
# **用这台机器上真正那份工作目录。**
#
# 不能另起一个空目录：compose 项目名 `hunter` 被现有那一套占着，
# 启动器的 `guard_project_owner` 会当场拦下来（拦得对 —— 那条闸门是 P0-5 的成果）。
# 所以这里的做法是：**先把 ~/.hunter/app 与 launcher.toml 整份快照一下，
# 测完原样写回去**。`--pull-only` 全程不碰容器，六个服务一直跑着。
HOME_DIR="${HOME_DIR:-$HOME/.hunter}"
SNAP="${SNAP:-/tmp/i16-hunter-snapshot}"
STALL_SECS="${STALL_SECS:-20}"
OUT="${OUT:-/tmp/i16-stall}"

mkdir -p "$OUT"
rm -rf "$SNAP"
mkdir -p "$SNAP"
cp -a "$HOME_DIR/app" "$SNAP/app"
cp -a "$HOME_DIR/launcher.toml" "$SNAP/launcher.toml"
echo "已把 $HOME_DIR/app 与 launcher.toml 快照到 $SNAP"

restore() {
  rm -rf "$HOME_DIR/app"
  cp -a "$SNAP/app" "$HOME_DIR/app"
  cp -a "$SNAP/launcher.toml" "$HOME_DIR/launcher.toml"
  echo "已把 $HOME_DIR 的配置写回快照那一份"
}

echo "=== 给假 registry 签一张自签证书 ==="
# Docker Engine 29 对 127.0.0.1:PORT 也直接发 TLS 握手（实测原话：
# `tls: first record does not look like a TLS handshake`），
# 「本机地址自动当成 insecure」那条退路没有了。让它信任自签证书的办法是
# 把 CA 放进 /etc/docker/certs.d/<host:port>/ca.crt —— **不需要重启 daemon**。
CERTDIR="$OUT/certs"
mkdir -p "$CERTDIR"
openssl req -x509 -newkey rsa:2048 -nodes -days 2 \
  -keyout "$CERTDIR/key.pem" -out "$CERTDIR/cert.pem" \
  -subj "/CN=127.0.0.1" -addext "subjectAltName=IP:127.0.0.1" >/dev/null 2>&1
sudo mkdir -p "/etc/docker/certs.d/127.0.0.1:$PORT"
sudo cp "$CERTDIR/cert.pem" "/etc/docker/certs.d/127.0.0.1:$PORT/ca.crt"
echo "  已装到 /etc/docker/certs.d/127.0.0.1:$PORT/ca.crt"

echo "=== 起假 registry（manifest 正常，blob 永远不吐字节） ==="
python3 "$(dirname "$0")/fake-stalling-registry.py" "$PORT" "$CERTDIR/cert.pem" "$CERTDIR/key.pem" > "$OUT/registry.log" 2>&1 &
FAKE=$!
cleanup() {
  kill $FAKE 2>/dev/null
  sudo rm -rf "/etc/docker/certs.d/127.0.0.1:$PORT"
  restore
}
trap cleanup EXIT
sleep 1
curl -sk -m 5 "https://127.0.0.1:$PORT/v2/" && echo "  ← /v2/ 应答正常"
echo "  manifest 读一遍（这就是「748 MB 算得出来」那一步）："
curl -sk -m 5 "https://127.0.0.1:$PORT/v2/agentpit/hunter-community-web/manifests/1.2.2" \
  | python3 -c 'import json,sys; m=json.load(sys.stdin); print("    层数", len(m["layers"]), "· 合计", sum(l["size"] for l in m["layers"])//1000000, "MB")'
echo "  blob 拉 8 秒看看能收到几个字节（应当是 0）："
timeout 8 curl -sk -o "$OUT/blob.bin" "https://127.0.0.1:$PORT/v2/agentpit/hunter-community-web/blobs/sha256:$(printf '0%.0s' {1..64})"
echo "    收到 $(stat -c%s "$OUT/blob.bin" 2>/dev/null || echo 0) 字节"

echo
echo "=== 把静默阈值调到 ${STALL_SECS} 秒（生产默认 90） ==="
python3 - "$HOME_DIR/launcher.toml" "$STALL_SECS" <<'PYEOF'
import sys, re
p, secs = sys.argv[1], sys.argv[2]
s = open(p).read()
if 'pull_stall_secs' in s:
    s = re.sub(r'pull_stall_secs\s*=\s*\d+', f'pull_stall_secs = {secs}', s)
else:
    s = re.sub(r'^\[hunter\]$', f'[hunter]\npull_stall_secs = {secs}', s, count=1, flags=re.M)
open(p, 'w').write(s)
print('  launcher.toml 里 pull_stall_secs =', secs)
PYEOF

echo
echo "=== 真跑一次 --pull-only（点名了假源，所以不该换源） ==="
START=$(date +%s)
HUNTER_HOME="$HOME_DIR" \
  timeout 300 "$BIN" --headless --pull-only -y \
  --key-file "$HOME/.hunter-launcher-test/key" \
  --registry "127.0.0.1:$PORT/agentpit" --tag 1.2.2 \
  > "$OUT/pull.log" 2>&1
RC=$?
END=$(date +%s)
echo "  退出码 $RC · 用时 $((END-START)) 秒"
echo
echo "=== 输出 ==="
cat "$OUT/pull.log"
echo
echo "=== 还有没有 docker compose pull 活着（应当一个都没有） ==="
pgrep -af "compose.*pull" || echo "  没有残留进程 ✓"
