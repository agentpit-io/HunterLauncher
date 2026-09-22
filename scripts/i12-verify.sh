#!/usr/bin/env bash
# I12 的真机验收脚本（测试机上跑）。用法：bash scripts/i12-verify.sh <场景>
#
# 每个场景都**真的动这台机器上的 compose 项目 hunter**，跑完会把现场恢复。
# 只碰项目 `hunter`：旧栈 hunter-community、hca-* 一个都不碰（总控规则红线 9）。
#
# 场景：
#   r1-adopt     外部装好 + done=false → 启动器应判「进运行面板」并补写标记
#   r1-stopped   六个容器停掉 → 仍然进运行面板（显示已停止），不回向导
#   r1-fresh     容器与数据卷都没有 → 欢迎页
#   r1-datafound 容器没了、数据卷还在 → 「检测到上次的数据」
#   r2-compare   三层资源数字与 top / free / docker stats / df 对照
#   r3-cycle     停止（含运行环境）→ 启动：关键表行数前后一致，6/6 健康
#   r3plus-vmreboot  重启 docker 守护进程（模拟虚拟机重启）→ 容器该自己回来
#   r5-reuse     删容器留数据卷 → 重装应沿用（零数据重建）
#   r5-nosecrets 构造「缺密钥卷」
#   r5-downgrade 构造「数据比镜像新」
set -uo pipefail

BIN="${BIN:-$HOME/HunterLauncher/src-tauri/target/release/hunter-launcher}"
APP="$HOME/.hunter/app"
TOML="$HOME/.hunter/launcher.toml"
P=hunter

say() { printf '\n\033[1m== %s ==\033[0m\n' "$*"; }
row() { printf '%-42s %s\n' "$1" "$2"; }

dc() { docker compose -p "$P" --project-directory "$APP" -f "$APP/docker-compose.yml" -f "$APP/docker-compose.launcher.yml" "$@"; }

pg() { docker exec hunter-postgres-1 psql -U hunter -d hunter -tAc "$1" 2>/dev/null; }

key_counts() {
  # 「关键表行数」——停止/启动前后必须一字不差
  for t in users stocks hunter_config schema_migrations user_preference; do
    printf '%s=%s\n' "$t" "$(pg "select count(*) from $t" || echo 读不到)"
  done
}

case "${1:-}" in

r1-adopt)
  say "场景 R1①：外部装好、install.done=false → 应进运行面板并补写标记"
  dc up -d >/dev/null 2>&1
  sleep 5
  python3 - "$TOML" <<'PY'
import re, sys
p = sys.argv[1]
s = open(p).read()
s = re.sub(r'(?m)^done = true$', 'done = false', s)
s = re.sub(r'(?m)^adopted_from_running = true$', 'adopted_from_running = false', s)
open(p, 'w').write(s)
PY
  row "改之前 install.done" "$(grep -m1 '^done' "$TOML")"
  row "容器现状" "$(docker ps --filter "label=com.docker.compose.project=$P" -q | wc -l) 个在跑"
  "$BIN" --boot-state 2>&1 | tail -20
  echo "--- 改之后的 [install] ---"
  sed -n '/^\[install\]/,/^\[/p' "$TOML" | head -14
  ;;

r1-stopped)
  say "场景 R1②：六个容器停掉 → 仍然进运行面板，不回向导"
  dc stop >/dev/null 2>&1
  row "容器现状" "$(docker ps --filter "label=com.docker.compose.project=$P" -q | wc -l) 个在跑 / $(docker ps -a --filter "label=com.docker.compose.project=$P" -q | wc -l) 个存在"
  "$BIN" --boot-state 2>&1 | tail -20
  say "恢复"
  dc start >/dev/null 2>&1
  ;;

r1-fresh)
  say "场景 R1⑤：什么都没有 → 欢迎页（在一个空的 HUNTER_HOME 里跑，不动真环境）"
  T=$(mktemp -d)
  HUNTER_HOME="$T" "$BIN" --boot-state 2>&1 | tail -20
  rm -rf "$T"
  ;;

r1-datafound)
  say "场景 R1④：容器没了、数据卷还在 → 「检测到上次的数据」"
  dc down >/dev/null 2>&1
  row "容器" "$(docker ps -a --filter "label=com.docker.compose.project=$P" -q | wc -l) 个"
  row "数据卷" "$(docker volume ls --filter "label=com.docker.compose.project=$P" -q | wc -l) 个"
  "$BIN" --boot-state 2>&1 | tail -20
  say "恢复"
  dc up -d >/dev/null 2>&1
  ;;

r2-compare)
  say "场景 R2：三层资源与命令行对照"
  echo "--- 启动器给的 ---"
  "$BIN" --monitor 2>&1 | tail -40
  echo
  echo "--- 命令行实测 ---"
  row "CPU（top 一次采样）" "$(top -bn2 -d0.5 | awk '/^%Cpu/{l=$0} END{print l}')"
  row "内存（free -b）" "$(free -b | awk '/^Mem:/{printf "已用 %s / 总 %s", $3, $2}')"
  row "系统盘（df -B1 \$HOME）" "$(df -B1 "$HOME" | awk 'NR==2{printf "剩 %s / 总 %s（%s）", $4, $2, $6}')"
  row "内存压力（PSI）" "$(awk '/^some/{print $2}' /proc/pressure/memory 2>/dev/null || echo 无)"
  echo "--- docker stats ---"
  docker stats --no-stream --format '{{.Name}}\t{{.CPUPerc}}\t{{.MemUsage}}' \
    $(docker ps --filter "label=com.docker.compose.project=$P" --format '{{.Names}}')
  echo "--- 卷与镜像 ---"
  docker system df -v --format '{{json .Volumes}}' | python3 -c '
import json,sys
for v in json.load(sys.stdin):
    if v["Labels"] and "com.docker.compose.project=hunter," in v["Labels"]+",":
        print(f"  {v[\"Name\"]:34s} {v[\"Size\"]}")
'
  for i in web api opencode llm-shim; do
    docker image inspect "ghcr.io/agentpit-io/hunter-community-$i:1.2.0" --format "  {{.RepoTags}} {{.Size}}" 2>/dev/null
  done
  docker image inspect postgres:16-alpine redis:7-alpine --format "  {{.RepoTags}} {{.Size}}" 2>/dev/null
  ;;

r3-cycle)
  say "场景 R3：停止 → 启动，数据不变"
  echo "--- 停止前 ---"; key_counts | tee /tmp/i12-before.txt
  dc stop >/dev/null 2>&1
  row "停止后在跑的容器" "$(docker ps --filter "label=com.docker.compose.project=$P" -q | wc -l) 个"
  dc up -d >/dev/null 2>&1
  for i in $(seq 1 60); do
    n=$(docker ps --filter "label=com.docker.compose.project=$P" --filter health=healthy -q | wc -l)
    [ "$n" -ge 6 ] && break
    sleep 5
  done
  row "启动后健康" "$n / 6（等了 $((i*5)) 秒）"
  echo "--- 启动后 ---"; key_counts | tee /tmp/i12-after.txt
  if diff -q /tmp/i12-before.txt /tmp/i12-after.txt >/dev/null; then
    echo "✓ 关键表行数前后一字不差"
  else
    echo "✗ 行数变了："; diff /tmp/i12-before.txt /tmp/i12-after.txt
  fi
  ;;

r3plus-vmreboot)
  say "场景 R3+：重启 docker 守护进程（Linux 上最接近「虚拟机重启」的现场）"
  echo "--- 重启前 ---"
  docker ps --filter "label=com.docker.compose.project=$P" --format '{{.Names}}\t{{.Status}}'
  docker inspect hunter-web-1 --format '  web 的重启策略：{{.HostConfig.RestartPolicy.Name}}'
  sudo systemctl restart docker
  sleep 20
  echo "--- 重启后（没有人做任何操作）---"
  docker ps --filter "label=com.docker.compose.project=$P" --format '{{.Names}}\t{{.Status}}'
  row "自己回来的容器数" "$(docker ps --filter "label=com.docker.compose.project=$P" -q | wc -l) / 6"
  ;;

r3plus-autoup)
  say "场景 R3+②：容器停着、运行环境在跑 → 启动器起来应自动拉起"
  dc stop >/dev/null 2>&1
  python3 - "$TOML" <<'PY'
import re, sys
p=sys.argv[1]; s=open(p).read()
s=re.sub(r'(?m)^stopped_by_user = true$','stopped_by_user = false',s)
open(p,'w').write(s)
PY
  row "拉起前在跑" "$(docker ps --filter "label=com.docker.compose.project=$P" -q | wc -l) 个"
  row "stopped_by_user" "$(grep -m1 stopped_by_user "$TOML" || echo 没有这一项)"
  "$BIN" --boot-state >/dev/null 2>&1
  sleep 25
  row "启动器跑过之后在跑" "$(docker ps --filter "label=com.docker.compose.project=$P" -q | wc -l) 个"
  grep -E "开机自查容器" "$HOME/.hunter/logs/launcher.log" | tail -5
  ;;

r3plus-respect-stop)
  say "场景 R3+③：用户自己点的「停止」不该被自动拉起来"
  dc stop >/dev/null 2>&1
  python3 - "$TOML" <<'PY'
import re, sys
p=sys.argv[1]; s=open(p).read()
if 'stopped_by_user' in s:
    s=re.sub(r'(?m)^stopped_by_user = false$','stopped_by_user = true',s)
else:
    s=re.sub(r'(?m)^(\[install\]\n)', r'\1stopped_by_user = true\n', s)
open(p,'w').write(s)
PY
  row "stopped_by_user" "$(grep -m1 stopped_by_user "$TOML")"
  "$BIN" --boot-state >/dev/null 2>&1
  sleep 20
  row "启动器跑过之后在跑" "$(docker ps --filter "label=com.docker.compose.project=$P" -q | wc -l) 个（应为 0）"
  grep -E "开机自查容器" "$HOME/.hunter/logs/launcher.log" | tail -3
  say "恢复"
  dc up -d >/dev/null 2>&1
  ;;

r5-probe)
  say "场景 R5：检测已有数据（浅查 + 深查）"
  "$BIN" --data-check 2>&1 | tail -40
  echo
  "$BIN" --data-check --deep 2>&1 | tail -40
  ;;

r5-reuse)
  say "场景 R5①：删容器留数据卷 → 重装沿用，零数据重建"
  echo "--- 删之前 ---"; key_counts
  JWT_BEFORE=$(sha256sum <(grep '^JWT_SECRET=' "$APP/.env") | cut -c1-16)
  row "JWT_SECRET 指纹" "$JWT_BEFORE"
  VOLS_BEFORE=$(docker volume ls --filter "label=com.docker.compose.project=$P" -q | sort | tr '\n' ' ')
  row "数据卷" "$VOLS_BEFORE"
  dc down >/dev/null 2>&1
  row "删完容器后剩的卷" "$(docker volume ls --filter "label=com.docker.compose.project=$P" -q | wc -l) 个"
  "$BIN" --data-check 2>&1 | tail -20
  ;;

r5-nosecrets)
  say "场景 R5②：构造「缺密钥卷」"
  dc down >/dev/null 2>&1
  docker volume rm hunter_hunter_secrets >/dev/null 2>&1 && echo "已删掉密钥卷（数据库那个一个字节都没碰）"
  "$BIN" --data-check 2>&1 | tail -25
  say "恢复：重新 up（compose 会新建一个空的密钥卷）"
  dc up -d >/dev/null 2>&1
  ;;

r5-downgrade)
  say "场景 R5③：构造「数据比镜像新」"
  pg "insert into schema_migrations(filename, checksum, applied_at) values ('9999_from_the_future.sql','i12-test',now())" >/dev/null
  row "现在最大的迁移" "$(pg "select filename from schema_migrations order by filename desc limit 1")"
  "$BIN" --data-check --deep 2>&1 | tail -30
  say "恢复：把那条假迁移删掉"
  pg "delete from schema_migrations where checksum='i12-test'" >/dev/null
  row "恢复后最大的迁移" "$(pg "select filename from schema_migrations order by filename desc limit 1")"
  ;;

*)
  echo "用法：bash scripts/i12-verify.sh <场景>"
  sed -n '9,22p' "$0"
  exit 2
  ;;
esac
