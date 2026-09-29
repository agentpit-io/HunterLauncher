#!/usr/bin/env bash
# I17 现场造景 —— 在本机那套 **测试栈**（compose 项目名 `hunter`）上逐条过验收标准。
#
# 来源：客户 2026-09-29 上传的启动器日志 HL-GFV764（Windows · 0.1.17 · 1.2.2→1.2.3 升级被打断）。
# 方案里这几条**在 Linux 上就能真造出来**（和 I16 的 5.5 一个道理：`docker compose stop`
# 在哪个系统上都是同一个现场），所以不模拟、直接真停、真备份、真改 .env。
#
# 面板那一屏依赖的三个结论（说不说「运行中」/ 给不给地址 / 给不给时长）由
# `src-tauri/tests/i17_live.rs` 这个 `#[ignore]` 测试原样打印出来（`--status` 不印时长），
# 断言都落在它打印的那几行 `PANEL / INTERRUPTED / BOOT` 上。
#
# 用法：
#   bash scripts/i17-verify.sh <场景> [场景…]
# 场景：
#   a11   只留 postgres 在跑         → 面板必须说「已停止」，不给地址、不给时长（A1-1）
#   a13   六个容器全停               → 同上（A1-3，客户 10:44 的现场）
#   a21   停止状态下备份一次         → 结束后六个仍是 exited，不留孤儿 postgres（A2-1）
#   a23   运行中备份一次             → postgres 不许被误停（A2-3）
#   a31   配置 1.2.2 / 镜像缺 / 全停 → 出「上一次升级没做完」卡片（A3-1）
#   a32   点「继续升到 1.2.2」        → 拉齐四个镜像、起来、hunter.tag 跟上（A3-2）
#   a33   点「回退到 1.2.0」         → 配置回到 1.2.0，卡片消失（A3-3）
#   a34   配置超前但镜像齐           → 不出卡片；启动后 hunter.tag 被补齐（A3-4）
#   a35   容器在跑 + 配置超前        → 卡片照旧出现（A3-5，不许回归）
#   a42   当前源没有这一版           → 改写 .env **之前**就拦下（A4-2）
#   a46   非升级状态                 → 退出拦截判据必须为 None（A4-6）
#   all   按上面的顺序全跑一遍
#
# 容器只动 `-p hunter` 这一套；**不碰**这台机器上的 hca-* / xinghe-* / mixplode-*（总控规则红线 1）。
set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BIN:-$REPO/src-tauri/target/release/hunter-launcher}"
APP="${APP:-$HOME/.hunter/app}"
HOME_DIR="${HOME_DIR:-$HOME/.hunter}"
P=hunter
TAG="${TAG:-1.2.0}"          # 本机有镜像的那一版（造景的起点）
AHEAD="${AHEAD:-1.2.2}"      # 本机没有镜像、但 GHCR 上有的一版（当「升级到一半」的那一版）
KEYFILE="${KEYFILE:-$HOME/.hunter-launcher-test/key}"
export PATH="$HOME/.cargo/bin:$HOME/.npm-global/bin:$PATH"

snap=""      # 场景开始前的 .env / launcher.toml 快照
PROBE=""     # 上一次 probe 的输出（面板会拿到什么）

say() { printf '\n\033[1m== %s ==\033[0m\n' "$*"; }
ok()  { printf '  \033[32m✓\033[0m %s\n' "$*"; }
bad() { printf '  \033[31m✗\033[0m %s\n' "$*"; FAIL=1; }
FAIL=0

dc() { docker compose -p "$P" --project-directory "$APP" -f "$APP/docker-compose.yml" -f "$APP/docker-compose.launcher.yml" "$@"; }
states() { dc ps -a --format '{{.Service}} {{.State}}' | sort; }
n_running() { dc ps --format '{{.State}}' | grep -c '^running$'; }
n_exited()  { dc ps -a --format '{{.State}}' | grep -c '^exited$'; }
# 面板那一屏真正会拿到的值（A1-x / A3-x / A4-6 的判据都在这里）
probe() {
  PROBE="$(nice -n 10 ionice -c 3 cargo test --manifest-path "$REPO/src-tauri/Cargo.toml" --locked \
    --test i17_live -- --ignored --nocapture 2>/dev/null)"
}
pget() { grep -E "^$*=" <<<"$PROBE" | head -1 | cut -d= -f2-; }
pshow() { grep -E "^($1) " <<<"$PROBE" | sed 's/^/    /'; }

snapshot() { snap="$(mktemp -d)"; cp -a "$HOME_DIR/app/.env" "$snap/.env"; cp -a "$HOME_DIR/launcher.toml" "$snap/launcher.toml"; }
restore() {
  [ -n "$snap" ] || return 0
  cp -a "$snap/.env" "$HOME_DIR/app/.env"; cp -a "$snap/launcher.toml" "$HOME_DIR/launcher.toml"
  rm -rf "$snap"; snap=""
}
set_env_version() { sed -i -E "s/^HUNTER_VERSION=.*/HUNTER_VERSION=$1/" "$HOME_DIR/app/.env"; }
set_cfg_tag()     { sed -i -E "s/^tag = .*/tag = \"$1\"/" "$HOME_DIR/launcher.toml"; }
get_env_version() { grep -E '^HUNTER_VERSION=' "$HOME_DIR/app/.env" | cut -d= -f2; }
get_cfg_tag()     { grep -E '^tag = ' "$HOME_DIR/launcher.toml" | head -1 | cut -d'"' -f2; }

wait_healthy() {
  for _ in $(seq 1 60); do
    [ "$(dc ps --format '{{.Health}}' 2>/dev/null | grep -c healthy)" -eq 6 ] && return 0
    sleep 5
  done
  return 1
}

# ── A1-1 / A1-3 ─────────────────────────────────────────────────────────
case_a11() {
  say "A1-1 造现场：docker compose stop web api opencode llm-shim redis（只留 postgres）"
  dc stop web api opencode llm-shim redis >/dev/null 2>&1
  states | sed 's/^/    /'
  [ "$(n_running)" -eq 1 ] && ok "只有 1 个在跑" || bad "在跑的有 $(n_running) 个，现场没造对"

  say "A1-1 面板会拿到什么"
  probe; pshow 'PANEL|BOOT'
  [ "$(pget PANEL running)" = "false" ] && ok "running=false（不说「运行中」）" || bad "running=$(pget PANEL running)"
  [ "$(pget PANEL web_url)" = "None" ] && ok "web_url=None（不给「打开 Hunter」）" || bad "web_url 有值 —— 会给出一个打不开的地址"
  [ "$(pget PANEL uptime_seconds)" = "None" ] && ok "uptime_seconds=None（不显示假时长）" || bad "还给了时长 $(pget PANEL uptime_seconds)"
  grep -q "^PANEL posture=Partial" <<<"$PROBE" && ok "posture=Partial（有服务不正常）" || bad "posture 不是 Partial"
  grep -q "运行中" <<<"$(pget 'BOOT headline')" && bad "开机判定还在说「运行中」" || ok "开机判定没说「运行中」"

  say "A1-1 恢复现场：把五个容器起回来"
  dc start >/dev/null 2>&1; wait_healthy && ok "6/6 健康" || bad "没起全"
}

case_a13() {
  say "A1-3 造现场：六个容器全停（客户 10:44 的现场）"
  dc stop >/dev/null 2>&1
  states | sed 's/^/    /'
  [ "$(n_exited)" -eq 6 ] && ok "六个全是 exited" || bad "只有 $(n_exited) 个 exited"

  say "A1-3 面板会拿到什么"
  probe; pshow 'PANEL|BOOT'
  [ "$(pget PANEL running)" = "false" ] && ok "running=false" || bad "running 不是 false"
  [ "$(pget PANEL web_url)" = "None" ] && ok "不给地址" || bad "给了地址"
  [ "$(pget PANEL uptime_seconds)" = "None" ] && ok "不给时长" || bad "给了时长"
  grep -q "^BOOT posture=Stopped" <<<"$PROBE" && ok "posture=Stopped" || bad "posture 不是 Stopped"

  say "A1-3 恢复现场"
  dc start >/dev/null 2>&1; wait_healthy && ok "6/6 健康" || bad "没起全"
}

# ── A2-1 / A2-3 ─────────────────────────────────────────────────────────
case_a21() {
  say "A2-1 造现场：Hunter 停止状态（六个全停）"
  dc stop >/dev/null 2>&1
  [ "$(n_exited)" -eq 6 ] && ok "六个全是 exited" || bad "现场没造对"

  say "A2-1 备份一次"
  local rc=0
  "$BIN" --backup 2>&1 | tail -14 | sed 's/^/    /' || rc=$?
  echo "    退出码=$rc"

  say "A2-1 备份之后容器什么状态（必须六个仍全 exited）"
  states | sed 's/^/    /'
  if [ "$(n_exited)" -eq 6 ]; then ok "六个仍是 exited，没有留下孤儿 postgres"
  else bad "备份后还有 $(n_running) 个在跑 —— 孤儿 postgres 又出现了"; fi
  if tail -400 "$HOME_DIR/logs/launcher.log" | grep -q "已停回去"; then
    ok "启动器日志里有「已停回去」那一行"
    tail -400 "$HOME_DIR/logs/launcher.log" | grep "已停回去" | tail -2 | sed 's/^/    /'
  else bad "日志里没有「已停回去」"; fi

  say "A2-1 恢复现场"
  dc start >/dev/null 2>&1; wait_healthy && ok "6/6 健康" || bad "没起全"
}

case_a23() {
  say "A2-3 运行中（6/6 健康）备份一次 —— postgres 不许被误停"
  wait_healthy && ok "起点是 6/6 健康" || bad "起点不是 6/6，这次测不准"
  "$BIN" --backup 2>&1 | tail -8 | sed 's/^/    /'
  states | sed 's/^/    /'
  [ "$(n_running)" -eq 6 ] && ok "六个都还在跑（lease 是空动作）" || bad "备份停了不该停的容器"
  sleep 5; probe
  [ "$(pget PANEL running)" = "true" ] && ok "面板仍是「运行中」" || bad "面板不再是运行中"
}

# ── A3-1 / A3-2 / A3-3：中间态 ──────────────────────────────────────────
case_a31() {
  say "A3-1 造现场：.env 的 HUNTER_VERSION 改成 $AHEAD（本机没有这一版的镜像）+ 六个全停"
  snapshot
  set_env_version "$AHEAD"
  echo "    .env  HUNTER_VERSION=$(get_env_version)"
  echo "    toml  hunter.tag       =$(get_cfg_tag)"
  dc stop >/dev/null 2>&1
  [ "$(n_exited)" -eq 6 ] && ok "六个全是 exited" || bad "现场没造对"

  say "A3-1 面板会拿到什么"
  probe; pshow 'PANEL|BOOT|INTERRUPTED'
  grep -q "^INTERRUPTED yes" <<<"$PROBE" && ok "出了「上一次升级没做完」卡片" || bad "卡片没出现"
  [ "$(pget INTERRUPTED config_tag)" = "$AHEAD" ] && ok "config_tag=$AHEAD" || bad "config_tag 不对"
  [ "$(pget INTERRUPTED running_tag)" = "None" ] && ok "running_tag=None（全停时没有「正在跑的那一版」）" || bad "running_tag 不该有值"
  [ "$(pget INTERRUPTED last_good_tag)" = "$TAG" ] && ok "last_good_tag=$TAG（回退目标）" || bad "last_good_tag 不对"
  [ -n "$(pget INTERRUPTED missing)" ] && ok "列出了缺哪几个镜像：$(pget INTERRUPTED missing)" || bad "没列缺镜"
  if grep -q '点「启动」就能用' <<<"$PROBE"; then bad "面板还在说「点『启动』就能用」—— 这正是客户被误导的那句话"
  else ok "没有出现「点『启动』就能用」"; fi
  grep -q "^BOOT line=但配置那一版 v$AHEAD 的镜像本机还缺" <<<"$PROBE" \
    && ok "开机判定写明了「配置那一版还缺几个镜像」" || bad "开机判定没提缺镜"
}

case_a32() {
  say "A3-2 点「继续升到 v$AHEAD」：拉齐四个镜像并启动（走升级那条真路径）"
  local rc=0
  "$BIN" --upgrade "$AHEAD" -y 2>&1 | tail -22 | sed 's/^/    /' || rc=$?
  echo "    退出码=$rc"
  echo "    .env  HUNTER_VERSION=$(get_env_version)"
  echo "    toml  hunter.tag       =$(get_cfg_tag)"
  dc ps --format '{{.Service}} {{.State}} {{.Status}}' | sed 's/^/    /'
  [ "$(get_cfg_tag)" = "$AHEAD" ] && ok "hunter.tag 跟上了（$AHEAD）" || bad "hunter.tag 还是 $(get_cfg_tag)"
  probe
  grep -q "^INTERRUPTED no" <<<"$PROBE" && ok "卡片消失" || bad "卡片还在"
  grep -q "^PANEL running=true" <<<"$PROBE" && ok "面板「运行中」" || bad "面板不是运行中"
  docker image inspect "ghcr.io/agentpit-io/hunter-community-web:$AHEAD" >/dev/null 2>&1 \
    && ok "v$AHEAD 的 web 镜像已经在本机" || bad "镜像没拉下来"
}

case_a33() {
  say "A3-3 卡片点「回退到 v$TAG」：配置回到 $TAG，卡片消失"
  set_env_version "$AHEAD"
  echo "    先把现场摆回中间态：.env=$(get_env_version) / toml=$(get_cfg_tag)"
  probe; grep -q "^INTERRUPTED yes" <<<"$PROBE" && ok "中间态成立" || bad "没造出中间态"

  say "    命令行上同一条路：--revert-to-running"
  "$BIN" --revert-to-running 2>&1 | tail -5 | sed 's/^/    /'
  "$BIN" --revert-to-running -y 2>&1 | tail -6 | sed 's/^/    /'
  echo "    .env  HUNTER_VERSION=$(get_env_version)"
  echo "    toml  hunter.tag       =$(get_cfg_tag)"
  [ "$(get_env_version)" = "$TAG" ] && ok ".env 回到 $TAG" || bad ".env 还是 $(get_env_version)"
  probe
  grep -q "^INTERRUPTED no" <<<"$PROBE" && ok "卡片消失" || bad "卡片还在"
  pshow 'BOOT'
  restore
}

case_a34() {
  say "A3-4 配置超前（$AHEAD）但镜像齐、仍全停 → 不出卡片；启动后 hunter.tag 被补齐"
  snapshot
  set_env_version "$AHEAD"; set_cfg_tag "$TAG"
  docker image inspect "ghcr.io/agentpit-io/hunter-community-web:$AHEAD" >/dev/null 2>&1 \
    && ok "v$AHEAD 的镜像本机已有（上一条拉下来的）" || bad "镜像不齐，这条测不了"
  dc stop >/dev/null 2>&1
  probe; pshow 'INTERRUPTED|BOOT'
  grep -q "^INTERRUPTED no" <<<"$PROBE" && ok "不出卡片（「缺镜为空」就是不出卡的主防线）" || bad "误报了卡片"

  say "    --start：起来之后 hunter.tag 应该被补齐成 $AHEAD"
  "$BIN" --start 2>&1 | tail -6 | sed 's/^/    /'
  wait_healthy >/dev/null 2>&1
  echo "    toml  hunter.tag =$(get_cfg_tag)"
  [ "$(get_cfg_tag)" = "$AHEAD" ] && ok "hunter.tag 补齐成 $AHEAD" || bad "hunter.tag 还是 $(get_cfg_tag)"
  probe; grep -q "^PANEL running=true" <<<"$PROBE" && ok "起来后面板「运行中」" || bad "没起来"
  restore
}

case_a35() {
  say "A3-5 容器正在跑 + 配置超前 → 卡片照旧出现（I16 覆盖的现场，不许回归）"
  snapshot
  dc start >/dev/null 2>&1; wait_healthy >/dev/null 2>&1
  set_env_version "$AHEAD"; set_cfg_tag "$TAG"
  probe; pshow 'INTERRUPTED|PANEL'
  grep -q "^INTERRUPTED yes" <<<"$PROBE" && ok "卡片照旧出现" || bad "卡片不见了 —— 回归了"
  [ "$(pget INTERRUPTED running_tag)" = "Some(\"$TAG\")" ] && ok "running_tag=$TAG（跑着的那一版）" || bad "running_tag 不对"
  restore
  dc start >/dev/null 2>&1; wait_healthy >/dev/null 2>&1
}

# ── A4-x ────────────────────────────────────────────────────────────────
case_a42() {
  say "A4-2 当前源上没有这一版（国内源 1.2.3 实测 404）→ 必须在改配置**之前**拦下"
  snapshot
  local before_env before_toml
  before_env="$(get_env_version)"; before_toml="$(get_cfg_tag)"
  sed -i -E 's/^registry_id = .*/registry_id = "tencent"/' "$HOME_DIR/launcher.toml"
  echo "    镜像源 registry_id=$(grep -E '^registry_id' "$HOME_DIR/launcher.toml" | cut -d'"' -f2)"

  local rc=0
  "$BIN" --upgrade 1.2.3 -y 2>&1 | tail -12 | sed 's/^/    /' || rc=$?
  echo "    退出码=$rc"
  [ "$rc" -ne 0 ] && ok "非零退出（没往下走）" || bad "居然成功了"
  echo "    .env  HUNTER_VERSION=$(get_env_version)（改前 $before_env）"
  echo "    toml  hunter.tag       =$(get_cfg_tag)（改前 $before_toml）"
  [ "$(get_env_version)" = "$before_env" ] && ok ".env 一个字节没改" || bad ".env 被改了"
  [ "$(get_cfg_tag)" = "$before_toml" ] && ok "hunter.tag 一个字节没改" || bad "hunter.tag 被改了"
  restore
}

case_a46() {
  say "A4-6 非升级状态：退出拦截判据必须是 None（一个提示都不许有）"
  probe; pshow 'QUIT_GUARD'
  grep -q "^QUIT_GUARD armed=None" <<<"$PROBE" && ok "没有升级在跑" || bad "有升级标记残留"
  grep -q "^QUIT_GUARD guard=None" <<<"$PROBE" && ok "判据为 None → 直接退出，不弹任何提示" || bad "判据不是 None"

  say "A4-6 对照：把「升级进行中」这一个窗口打开，必须有那一句话（单测）"
  local n
  n="$(nice -n 10 ionice -c 3 cargo test --manifest-path "$REPO/src-tauri/Cargo.toml" --locked \
      --lib -- 退出 2>&1 | grep -c 'test result: ok')"
  [ "$n" = "1" ] && ok "quit_guard 的两条单测都过了" || bad "quit_guard 的单测没过"
}

# ── 总控 ────────────────────────────────────────────────────────────────
[ -x "$BIN" ] || { echo "找不到 $BIN（先 pnpm tauri:demo 或 cargo build --release）"; exit 2; }
echo "I17 现场造景 · 项目名 $P · 工作目录 $APP · 起点版本 $TAG · 超前版本 $AHEAD"
date '+%F %T %Z'

for c in "$@"; do
  case "$c" in
    a11) case_a11;; a13) case_a13;; a21) case_a21;; a23) case_a23;;
    a31) case_a31;; a32) case_a32;; a33) case_a33;; a34) case_a34;; a35) case_a35;;
    a42) case_a42;; a46) case_a46;;
    all) case_a11; case_a13; case_a21; case_a23; case_a31; case_a32; case_a33; case_a34; case_a35; case_a42; case_a46;;
    *) echo "不认识的场景：$c"; exit 2;;
  esac
done

echo
[ "$FAIL" -eq 0 ] && echo "全部通过" || echo "有失败项"
exit "$FAIL"
