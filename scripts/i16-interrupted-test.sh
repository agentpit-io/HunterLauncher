#!/usr/bin/env bash
# I16 · P1-2 的真机验收：**强退之后的中间态要认得出来**。
#
# 造的现场和客户 2026-09-25 强退之后留下的那个一模一样：
#
#   配置（.env / compose） → 一个本机没有镜像的版本
#   正在跑的容器           → 它原来那一版
#   新版本的镜像           → 本机不齐
#
# 判据全部是实测量：`.env` 里的 HUNTER_VERSION、`docker compose ps` 报的镜像 tag、
# `docker image inspect` 查出来的本机镜像。这个脚本只改 `.env` 里的一行版本号，
# **不碰容器、不碰卷**，测完写回去。
set -u

BIN="${BIN:-$HOME/HunterLauncher/src-tauri/target/release/hunter-launcher}"
HOME_DIR="${HOME_DIR:-$HOME/.hunter}"
# 一个这台机器上肯定没有镜像的版本号
FAKE_TAG="${FAKE_TAG:-9.9.9}"
SNAP="/tmp/i16-interrupted-snapshot"

rm -rf "$SNAP"; mkdir -p "$SNAP"
cp -a "$HOME_DIR/app/.env" "$SNAP/.env"
cp -a "$HOME_DIR/launcher.toml" "$SNAP/launcher.toml"
restore() {
  cp -a "$SNAP/.env" "$HOME_DIR/app/.env"
  cp -a "$SNAP/launcher.toml" "$HOME_DIR/launcher.toml"
  echo "已把 .env 与 launcher.toml 写回原样"
}
trap restore EXIT

echo "=== ① 现状：配置与容器一致时不许误报 ==="
echo -n "  .env 里的版本："; grep -E '^HUNTER_VERSION=' "$HOME_DIR/app/.env"
echo "  正在跑的容器用的镜像："
docker compose --project-name hunter ps --format json 2>/dev/null \
  | python3 -c 'import sys,json
for l in sys.stdin:
    l=l.strip()
    if not l: continue
    d=json.loads(l)
    print("   ", d.get("Service"), d.get("Image"))' || echo "   （读不到）"
HUNTER_HOME="$HOME_DIR" "$BIN" --boot-state 2>&1 | grep -iE "升级|interrupted" || echo "  ✓ 没有报「上一次升级没做完」"

echo
echo "=== ② 造中间态：把 .env 的版本改成 v$FAKE_TAG（本机没有这一版的镜像） ==="
sed -i -E "s/^HUNTER_VERSION=.*/HUNTER_VERSION=$FAKE_TAG/" "$HOME_DIR/app/.env"
grep -E '^HUNTER_VERSION=' "$HOME_DIR/app/.env"
echo "  本机有没有 v$FAKE_TAG 的镜像："
docker image inspect "ghcr.io/agentpit-io/hunter-community-web:$FAKE_TAG" >/dev/null 2>&1 \
  && echo "    有（那这个测试白造了）" || echo "    没有 ✓"

echo
echo "=== ③ 再问一次判定 ==="
HUNTER_HOME="$HOME_DIR" "$BIN" --boot-state 2>&1 | tail -30

echo
echo "=== ④ 「回退到正在跑的那一版」：不带 -y 只说要做什么 ==="
HUNTER_HOME="$HOME_DIR" "$BIN" --revert-to-running 2>&1 | tail -8
echo -n "  .env 现在还是："; grep -E '^HUNTER_VERSION=' "$HOME_DIR/app/.env"

echo
echo "=== ⑤ 真的回退（-y）。容器一个都不许动 ==="
BEFORE_IDS=$(docker ps --filter "label=com.docker.compose.project=hunter" -q | sort | tr '\n' ' ')
BEFORE_START=$(docker inspect -f '{{.State.StartedAt}}' $(docker ps --filter "label=com.docker.compose.project=hunter" -q) 2>/dev/null | sort | tr '\n' ' ')
HUNTER_HOME="$HOME_DIR" "$BIN" --revert-to-running -y 2>&1 | tail -12
echo -n "  .env 现在是："; grep -E '^HUNTER_VERSION=' "$HOME_DIR/app/.env"
AFTER_IDS=$(docker ps --filter "label=com.docker.compose.project=hunter" -q | sort | tr '\n' ' ')
AFTER_START=$(docker inspect -f '{{.State.StartedAt}}' $(docker ps --filter "label=com.docker.compose.project=hunter" -q) 2>/dev/null | sort | tr '\n' ' ')
[ "$BEFORE_IDS" = "$AFTER_IDS" ] && echo "  ✓ 六个容器的 ID 一个都没变" || echo "  ✗ 容器被重建了！"
[ "$BEFORE_START" = "$AFTER_START" ] && echo "  ✓ 六个容器的启动时刻一个都没变（没重启过）" || echo "  ✗ 有容器重启了！"
