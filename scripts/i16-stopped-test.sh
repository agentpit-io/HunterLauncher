#!/usr/bin/env bash
# I16 追加 · 两条在 Linux 上就能真验的（测试机上跑）：
#
#   stopped   —— **六个容器全停下来**之后，开机判定该说「装好了，但容器都停着」，
#                而不是 0.1.15 那句「Hunter 在跑，但 … 还没就绪、网页打不开」（P0-4）。
#                这一条和 Windows 无关：`docker compose stop` 在哪个系统上都能造出
#                同一个现场（`ps` 里六个 `exited`），所以不用模拟、直接真停。
#   pulllocal —— 镜像本机已有时，那句话不许说成「拉取完成，用时 4 秒」（P2-5）。
#
# 只碰 compose 项目 `hunter` 与 `$HOME/.hunter`：旧栈一个都不碰（红线 9）。跑完恢复现场。
set -uo pipefail

BIN="${BIN:-$HOME/HunterLauncher/src-tauri/target/release/hunter-launcher}"
APP="$HOME/.hunter/app"
OVL="$APP/docker-compose.launcher.yml"
P=hunter
KEYFILE="$HOME/.hunter-launcher-test/key"

say() { printf '\n\033[1m== %s ==\033[0m\n' "$*"; }
ok()  { printf '  \033[32m✓\033[0m %s\n' "$*"; }
bad() { printf '  \033[31m✗\033[0m %s\n' "$*"; FAIL=1; }
FAIL=0

dc() { docker compose -p "$P" --project-directory "$APP" -f "$APP/docker-compose.yml" -f "$OVL" "$@"; }

case "${1:-}" in
stopped)
  say "① 先看现在的样子（应当 6/6 在跑）"
  dc ps --format '{{.Service}} {{.State}}' | sed 's/^/  /'

  say "② 真的把六个容器停下来（docker compose stop，不删容器、不碰卷）"
  dc stop >/dev/null 2>&1
  dc ps -a --format '{{.Service}} {{.State}}' | sed 's/^/  /'
  n_exit=$(dc ps -a --format '{{.State}}' | grep -c '^exited$')
  [ "$n_exit" -eq 6 ] && ok "六个全是 exited" || bad "只有 $n_exit 个 exited"

  say "③ 开机判定怎么说"
  out=$("$BIN" --boot-state 2>&1)
  echo "$out" | sed 's/^/  /'

  grep -q "都停着" <<<"$out" && ok "说的是「都停着」" || bad "没说「都停着」"
  grep -q "还没就绪" <<<"$out" && bad "不许把 exited 说成「还没就绪」" || ok "没有把 exited 说成「还没就绪」"
  grep -q "网页打不开" <<<"$out" && bad "容器都停着的时候不该再说「网页打不开」" || ok "没有多报一句「网页打不开」"
  # 「Hunter 在跑」是 0.1.15 在这个现场说的原话。这里只钉这一句 ——
  # 「只是没在跑」「配置与正在跑的容器对得上」里也有「在跑」两个字，
  # 拿「在跑」去 grep 会把对的话也判成错的
  grep -q "Hunter 在跑" <<<"$out" && bad "容器一个都没跑，不许说「Hunter 在跑」" \
    || ok "没有 0.1.15 那句「Hunter 在跑，但 … 还没就绪」"
  grep -q "没去探" <<<"$out" && ok "没去探的事没说成「没应答」" \
    || bad "容器都停着时不该报一个没做过的探测的结果"

  say "④ 恢复现场：把六个容器起回来"
  dc start >/dev/null 2>&1
  sleep 20
  dc ps --format '{{.Service}} {{.State}} {{.Status}}' | sed 's/^/  /'
  ;;

pulllocal)
  say "① 本机现在有哪些 hunter 镜像"
  docker images --format '{{.Repository}}:{{.Tag}}' | grep -E 'hunter-community|postgres:16-alpine|redis:7-alpine' | sed 's/^/  /'

  say "② 对着一套**本机已有**的镜像跑 --pull-only"
  out=$(HUNTER_KEY_FILE="$KEYFILE" "$BIN" --pull-only --key-file "$KEYFILE" -y 2>&1)
  echo "$out" | sed 's/^/  /'

  grep -q "本机都已有，没有下载" <<<"$out" && ok "说清楚了「本机都已有」" || bad "没说「本机都已有」"
  if grep -qE "拉取完成，用时" <<<"$out"; then
    bad "一个字节都没下却说「拉取完成」"
  else
    ok "没有谎称「拉取完成」"
  fi
  ;;

*)
  echo "用法：bash scripts/i16-stopped-test.sh {stopped|pulllocal}"; exit 2;;
esac

echo
[ "$FAIL" -eq 0 ] && echo "全部通过" || echo "有失败项"
exit "$FAIL"
