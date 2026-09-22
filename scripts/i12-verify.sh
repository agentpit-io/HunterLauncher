#!/usr/bin/env bash
# I12 的真机验收脚本（测试机上跑）。用法：bash scripts/i12-verify.sh <场景>
#
# 每个场景都**真的动这台机器上的 compose 项目 hunter**，跑完会把现场恢复。
# 只碰项目 `hunter`：旧栈 hunter-community、hca-* 一个都不碰（总控规则红线 9）。
#
# 场景：
#   r1-adopt         外部装好 + done=false → 应判「进运行面板」并补写标记
#   r1-stopped       六个容器停掉 → 仍然进运行面板（显示已停止），不回向导
#   r1-fresh         容器与数据卷都没有 → 欢迎页
#   r1-datafound     容器没了、数据卷还在 → 「检测到上次的数据」
#   r1-gui           真界面：打开启动器**不闪欢迎页**，直接到运行面板
#   r2-compare       三层资源数字与 top / free / docker stats / df 对照
#   r3-cycle         停止 → 启动：关键表行数前后一致，6/6 健康
#   r3-one           只重启一个服务：别的五个容器一个都不动
#   r3plus-overlay   老覆盖文件（没有重启策略）→ 启动器补写、但一个容器都不动
#   r3plus-vmreboot  重启 docker 守护进程（最接近「虚拟机重启」的现场）→ 容器该自己回来
#   r3plus-autoup    容器停着、运行环境在跑 → 启动器起来自动拉起
#   r3plus-respect   用户自己点的停止不该被自动拉起来
#   r5-probe         检测已有数据（浅查 + 深查）
#   r5-reuse         删容器留数据卷 → 重装应沿用（零数据重建）
#   r5-nosecrets     构造「缺密钥卷」
#   r5-downgrade     构造「数据比镜像新」
set -uo pipefail

BIN="${BIN:-$HOME/HunterLauncher/src-tauri/target/release/hunter-launcher}"
APP="$HOME/.hunter/app"
TOML="$HOME/.hunter/launcher.toml"
OVL="$APP/docker-compose.launcher.yml"
LOG="$HOME/.hunter/logs/launcher.log"
P=hunter

say() { printf '\n\033[1m== %s ==\033[0m\n' "$*"; }
row() { printf '%-44s %s\n' "$1" "$2"; }
count_up() { docker ps --filter "label=com.docker.compose.project=$P" -q | wc -l; }
created_at() { docker ps -a --filter "label=com.docker.compose.project=$P" --format '{{.Names}} {{.CreatedAt}}' | sort; }

dc() { docker compose -p "$P" --project-directory "$APP" -f "$APP/docker-compose.yml" -f "$OVL" "$@"; }
pg() { docker exec hunter-postgres-1 psql -U hunter -d hunter -tAc "$1" 2>/dev/null; }

# 「关键表行数」—— 停止 / 启动前后必须一字不差
key_counts() {
  for t in users stocks hunter_config schema_migrations user_preference; do
    printf '%s=%s\n' "$t" "$(pg "select count(*) from $t" || echo 读不到)"
  done
}

# 改 launcher.toml 里的一个布尔项（没有就加到 [install] 下面）
set_flag() { python3 "$(dirname "$0")/i12-setflag.py" "$TOML" "$1" "$2"; }

# 真界面：R3+ 的两道自愈都挂在窗口起来那一段（lib.rs 的 setup），命令行路径走不到
gui_start() {
  export DISPLAY=:99
  export WEBKIT_DISABLE_COMPOSITING_MODE=1
  export WEBKIT_DISABLE_DMABUF_RENDERER=1
  pkill -f "hunter-launcher" 2>/dev/null
  pkill -f "Xvfb :99" 2>/dev/null
  sleep 1
  Xvfb :99 -screen 0 1300x880x24 >/dev/null 2>&1 &
  sleep 2
  nohup "$BIN" > /tmp/i12-gui.log 2>&1 &
  local WID=""
  for _ in $(seq 1 40); do
    WID=$(xdotool search --name "Hunter Launcher" 2>/dev/null | tail -1)
    [ -n "$WID" ] && break
    sleep 0.5
  done
  if [ -z "$WID" ]; then echo "窗口没出来"; tail -20 /tmp/i12-gui.log; return 1; fi
  row "窗口" "$WID"
}
gui_shot() {
  mkdir -p "$(dirname "$1")"
  import -window "$(xdotool search --name 'Hunter Launcher' | tail -1)" "$1" 2>/dev/null && echo "截图 $1"
}
gui_stop() { pkill -f "hunter-launcher" 2>/dev/null; pkill -f "Xvfb :99" 2>/dev/null; sleep 1; }

case "${1:-}" in

r1-adopt)
  say "场景 R1①：外部装好、install.done=false → 应进运行面板并补写标记"
  dc up -d >/dev/null 2>&1
  sleep 5
  set_flag done false
  set_flag adopted_from_running false
  row "改之前 install.done" "$(grep -m1 '^done' "$TOML")"
  row "容器现状" "$(count_up) 个在跑"
  "$BIN" --boot-state 2>&1 | tail -22
  echo "--- 改之后的 [install] ---"
  sed -n '/^\[install\]/,/^\[/p' "$TOML" | head -14
  ;;

r1-stopped)
  say "场景 R1②：六个容器停掉 → 仍然进运行面板，不回向导"
  dc stop >/dev/null 2>&1
  row "容器现状" "$(count_up) 个在跑 / $(docker ps -a --filter "label=com.docker.compose.project=$P" -q | wc -l) 个存在"
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

r1-gui)
  say "场景 R1⑥：真界面 —— 打开启动器不闪欢迎页，直接到运行面板"
  dc up -d >/dev/null 2>&1
  set_flag done false
  row "开之前 install.done" "$(grep -m1 '^done' "$TOML")"
  gui_start || exit 1
  gui_shot /tmp/i12-shots/01-开机第一帧.png
  sleep 6
  gui_shot /tmp/i12-shots/02-六秒后.png
  row "开之后 install.done" "$(grep -m1 '^done' "$TOML")"
  row "adopted_from_running" "$(grep -m1 '^adopted_from_running' "$TOML")"
  grep -E "开机判定|复查看到" "$LOG" | tail -4
  gui_stop
  ;;

r2-compare)
  say "场景 R2：三层资源与命令行对照"
  echo "--- 启动器给的 ---"
  "$BIN" --monitor 2>&1 | tail -45
  echo
  echo "--- 命令行实测（同一时刻）---"
  row "CPU（top 两次采样）" "$(top -bn2 -d0.5 | awk '/^%Cpu/{l=$0} END{print l}')"
  row "内存（free -b）" "$(free -b | awk '/^Mem:/{printf "已用 %s / 总 %s", $3, $2}')"
  row "系统盘（df -B1 \$HOME）" "$(df -B1 "$HOME" | awk 'NR==2{printf "剩 %s / 总 %s（挂在 %s）", $4, $2, $6}')"
  row "内存压力（PSI some）" "$(awk '/^some/{print $2, $3, $4}' /proc/pressure/memory 2>/dev/null || echo 无)"
  echo "--- docker stats ---"
  docker stats --no-stream --format '{{.Name}}\t{{.CPUPerc}}\t{{.MemUsage}}' \
    $(docker ps --filter "label=com.docker.compose.project=$P" --format '{{.Names}}')
  echo "--- 卷（docker system df -v，只筛本项目）---"
  docker system df -v --format '{{json .Volumes}}' | python3 "$(dirname "$0")/i12-vols.py"
  echo "--- 镜像 ---"
  for i in web api opencode llm-shim; do
    docker image inspect "ghcr.io/agentpit-io/hunter-community-$i:1.2.0" --format "  {{index .RepoTags 0}} {{.Size}}" 2>/dev/null
  done
  docker image inspect postgres:16-alpine redis:7-alpine --format "  {{index .RepoTags 0}} {{.Size}}" 2>/dev/null
  ;;

r3-cycle)
  say "场景 R3：停止 → 启动，数据不变"
  echo "--- 停止前 ---"; key_counts | tee /tmp/i12-before.txt
  dc stop >/dev/null 2>&1
  row "停止后在跑的容器" "$(count_up) 个"
  dc up -d >/dev/null 2>&1
  n=0
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

r3-one)
  say "场景 R3：只重启一个服务 —— 别的五个容器一个都不动"
  BEFORE=$(created_at)
  ID_BEFORE=$(docker ps -aq --filter "label=com.docker.compose.project=$P" | sort | tr '\n' ' ')
  dc restart opencode
  sleep 8
  AFTER=$(created_at)
  ID_AFTER=$(docker ps -aq --filter "label=com.docker.compose.project=$P" | sort | tr '\n' ' ')
  if [ "$BEFORE" = "$AFTER" ] && [ "$ID_BEFORE" = "$ID_AFTER" ]; then
    echo "✓ 六个容器的 ID 与创建时间一字不差（restart 只换进程，不换容器）"
  else
    echo "✗ 容器被重建过："; diff <(echo "$BEFORE") <(echo "$AFTER")
  fi
  docker ps --filter "label=com.docker.compose.project=$P" --format '{{.Names}}\t{{.Status}}'
  ;;

r3plus-overlay)
  say "场景 R3+①：0.1.11 生成的老覆盖文件（没有重启策略）→ 启动器补写，但不动容器"
  python3 "$(dirname "$0")/i12-strip-restart.py" "$OVL"
  row "补写前 restart 行数" "$(grep -c 'restart:' "$OVL")"
  BEFORE=$(created_at)
  gui_start || exit 1
  sleep 20
  row "补写后 restart 行数" "$(grep -c 'restart:' "$OVL")"
  AFTER=$(created_at)
  if [ "$BEFORE" = "$AFTER" ]; then
    echo "✓ 六个容器的创建时间一字不差（补写只改文件）"
  else
    echo "✗ 容器被动过："; diff <(echo "$BEFORE") <(echo "$AFTER")
  fi
  grep -E "开机自查覆盖文件" "$LOG" | tail -3
  gui_stop
  echo "--- 补写之后的覆盖文件 ---"
  cat "$OVL"
  ;;

r3plus-vmreboot)
  say "场景 R3+①验证：重启 docker 守护进程（Linux 上最接近「虚拟机重启」的现场）"
  echo "--- 重启前 ---"
  docker ps --filter "label=com.docker.compose.project=$P" --format '{{.Names}}\t{{.Status}}'
  for c in hunter-web-1 hunter-llm-shim-1 hunter-postgres-1; do
    row "$c 的重启策略" "$(docker inspect "$c" --format '{{.HostConfig.RestartPolicy.Name}}' 2>/dev/null)"
  done
  sudo systemctl restart docker
  sleep 25
  echo "--- 重启后（没有人做任何操作）---"
  docker ps --filter "label=com.docker.compose.project=$P" --format '{{.Names}}\t{{.Status}}'
  row "自己回来的容器数" "$(count_up) / 6"
  ;;

r3plus-autoup)
  say "场景 R3+②：容器停着、运行环境在跑 → 启动器起来应自动拉起（Xvfb 真界面）"
  dc stop >/dev/null 2>&1
  set_flag stopped_by_user false
  row "拉起前在跑" "$(count_up) 个"
  row "stopped_by_user" "$(grep -m1 stopped_by_user "$TOML" || echo '（配置里还没有这一项）')"
  T0=$(date +%s)
  gui_start || exit 1
  n=0
  for _ in $(seq 1 24); do
    n=$(count_up)
    [ "$n" -ge 6 ] && break
    sleep 5
  done
  row "启动器起来 $(( $(date +%s) - T0 )) 秒后在跑" "$n 个"
  grep -E "开机自查容器|开机自查覆盖文件" "$LOG" | tail -6
  gui_stop
  ;;

r3plus-respect)
  say "场景 R3+③：用户自己点的「停止」不该被自动拉起来（Xvfb 真界面）"
  dc stop >/dev/null 2>&1
  set_flag stopped_by_user true
  row "stopped_by_user" "$(grep -m1 stopped_by_user "$TOML")"
  gui_start || exit 1
  sleep 45
  row "启动器起来 45 秒后在跑" "$(count_up) 个（应为 0）"
  grep -E "开机自查容器" "$LOG" | tail -3
  gui_stop
  say "恢复"
  set_flag stopped_by_user false
  dc up -d >/dev/null 2>&1
  ;;

r5-probe)
  say "场景 R5：检测已有数据（浅查 + 深查）"
  "$BIN" --data-check 2>&1 | tail -30
  echo
  "$BIN" --data-check --deep 2>&1 | tail -30
  ;;

r5-reuse)
  say "场景 R5①：删容器留数据卷 → 重装沿用，零数据重建"
  echo "--- 删之前 ---"; key_counts
  JWT_BEFORE=$(grep '^JWT_SECRET=' "$APP/.env" | sha256sum | cut -c1-16)
  row "JWT_SECRET 指纹（sha256 前 16 位）" "$JWT_BEFORE"
  row "数据卷" "$(docker volume ls --filter "label=com.docker.compose.project=$P" -q | sort | tr '\n' ' ')"
  dc down >/dev/null 2>&1
  row "删完容器后剩的卷" "$(docker volume ls --filter "label=com.docker.compose.project=$P" -q | wc -l) 个"
  "$BIN" --data-check --deep 2>&1 | tail -25
  say "重新起（模拟重装的最后一步：数据卷原样接上）"
  dc up -d >/dev/null 2>&1
  n=0
  for i in $(seq 1 60); do
    n=$(docker ps --filter "label=com.docker.compose.project=$P" --filter health=healthy -q | wc -l)
    [ "$n" -ge 6 ] && break
    sleep 5
  done
  row "重新起之后健康" "$n / 6（等了 $((i*5)) 秒）"
  echo "--- 重新起之后 ---"; key_counts
  row "JWT_SECRET 指纹" "$(grep '^JWT_SECRET=' "$APP/.env" | sha256sum | cut -c1-16)（之前是 $JWT_BEFORE）"
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
  sed -n '7,24p' "$0"
  exit 2
  ;;
esac
