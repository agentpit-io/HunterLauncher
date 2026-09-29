#!/usr/bin/env bash
#
# I18 · P1：**发布闸门 —— 镜像先就位，再点亮升级入口。**
#
# 用法：
#   scripts/gate-mirrors.sh                # 同步 + 校验「hunter-community 最新 Release」
#   scripts/gate-mirrors.sh 1.2.3          # 指定 hunter 版本（前导 v 可有可无）
#   scripts/gate-mirrors.sh --self-test    # 本地化验轮询 / 超时 / 失败判定（不联网）
#
# 退出码：0 = 镜像已就位（这一步可以打勾）；1 = 没就位 / 同步失败 / 等超时。
#
# ## 它为什么存在
#
# 2026-09-29 09:05 客户那台 Windows 上，升级失败的直接原因不是网络、不是操作，
# 而是**国内源上还没有 v1.2.3**：国内源是每 6 小时的定时同步
# （`.github/workflows/cn-mirror.yml`），而升级按钮在 Release 出现那一刻就亮了。
# 10:38 实测 `hkccr…/hunter-community-api:1.2.3: not found` → 启动器按设计换到 GHCR，
# 客户等不下去点了「一起停止」，机器留在中间态。
#
# I17 只做到「发现并拦一下」（`scripts/verify-mirrors.sh`）。这一轮把它**接进发布链路**：
# `.github/workflows/release.yml` 在 `gh release create`（那一步才是「点亮」）之前先跑这个脚本，
# 它做三件事 ——
#
#   ① 触发一次国内源同步（`cn-mirror.yml` 支持 `workflow_dispatch` 带 `tags` 入参）；
#   ② **等它跑完**（有上限，见下），失败就不往下走；
#   ③ 用 `scripts/verify-mirrors.sh` 做最后一道校验，非零退出就停。
#
# 三件事都过了才允许建 Release。**宁可晚发，不把一个「客户点升级就 404」的版本放出去。**
#
# ## 上限（写进迭代报告的那个数）
#
# `GATE_WAIT_SECS` 默认 1800 秒（30 分钟）。依据：2026-09-29 13:44 那次
# `cn-mirror.yml` 实测跑了 **15 分 7 秒**（四个 hunter 镜像 + postgres/redis）。
# 给到 30 分钟是它的两倍，够一次重试；再久就是卡住了，早点红比挂着强。
# `GATE_POLL_SECS` 默认 20 秒——`gh run view` 一次一个 API 调用，20 秒的粒度对
# 一个要跑十几分钟的作业足够了，也不会把 API 配额烧掉。
#
# ## 明确不做
#
# **不推镜像、不改同步逻辑**（国内源的同步由另一条链路负责，用户 2026-09-29 明确）。
# 这里只负责「触发 — 等 — 核」三件事。
set -uo pipefail

REPO="${GATE_REPO:-agentpit-io/HunterLauncher}"
WORKFLOW="${GATE_WORKFLOW:-cn-mirror.yml}"
WAIT_SECS="${GATE_WAIT_SECS:-1800}"
POLL_SECS="${GATE_POLL_SECS:-20}"
# 这两个可替换点**只为 `--self-test` 存在**：真机上它们就是 `gh` 与仓库里那个脚本
GH="${GATE_GH:-gh}"
VERIFY="${GATE_VERIFY:-$(dirname "$0")/verify-mirrors.sh}"

log() { echo "$@"; }
die() {
  echo "::error::$*" >&2
  echo "$*" >&2
  exit 1
}

# ── 参数 ──────────────────────────────────────────────────────────────────
SELF_TEST=0
WANT=""
for a in "$@"; do
  case "$a" in
    --self-test) SELF_TEST=1 ;;
    -h | --help)
      sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
      exit 2
      ;;
    -*) die "认不得的参数：$a" ;;
    *) WANT="$a" ;;
  esac
done

# ── 纯逻辑：一次 gh run list 的结论该怎么判 ──────────────────────────────
#
# 抽出来是因为它**是本脚本唯一有分支的地方**，也是最容易写错的地方
# （R4 那轮就在「完成但失败」上摔过一次：「completed」被当成了「成功」）。
#
# 入参：`status`（queued/in_progress/completed/…）与 `conclusion`
# （success/failure/cancelled/skipped/…），都可能是空串。
# 出参：wait / ok / fail 三种之一。
verdict() {
  local status="$1" conclusion="$2"
  if [ -z "$status" ]; then
    echo "fail" # 读不到状态：不猜成「成功」
    return
  fi
  if [ "$status" != "completed" ]; then
    echo "wait"
    return
  fi
  case "$conclusion" in
    success) echo "ok" ;;
    # skipped 也算没过：同步没做（例如没配凭据），凭什么说镜像齐了
    *) echo "fail" ;;
  esac
}

# ── 取要同步的版本 ────────────────────────────────────────────────────────
#
# 不给参数时问 hunter-community 的**最新正式 Release**（与 `cn-mirror.yml`
# 不带入参时的口径完全一致：`gh api …/releases/latest`，去掉前导 v）。
resolve_version() {
  if [ -n "$WANT" ]; then
    echo "${WANT#v}"
    return 0
  fi
  local tag
  tag="$("$GH" api "repos/agentpit-io/hunter-community/releases/latest" --jq .tag_name 2>/dev/null)"
  [ -n "$tag" ] || return 1
  echo "${tag#v}"
}

main() {
  local ver
  if ! ver="$(resolve_version)"; then
    die "取不到要同步的 hunter 版本（hunter-community 那边没有正式 Release，或者 gh api 没通）。"
  fi
  [ -n "$ver" ] || die "要同步的 hunter 版本是空的，停。"

  log "镜像闸门：hunter $ver · 仓库 $REPO · 等上限 ${WAIT_SECS} 秒"

  # ① 先记下「派发之前最新那一次 run 的 id」——用来认出我们自己触发的那一次。
  #    `gh workflow run` 不回 run id，只能这样认（比按时间戳认稳：
  #    GitHub 给的 createdAt 只到秒，而派发与列表之间可能有几秒漂移）。
  local prev
  prev="$("$GH" run list --repo "$REPO" --workflow "$WORKFLOW" --limit 1 \
    --json databaseId --jq '.[0].databaseId // -1' 2>/dev/null)"
  case "$prev" in
    '' | *[!0-9-]*) prev=-1 ;;
  esac

  # ② 触发同步
  log "触发 $WORKFLOW（tags=$ver）…"
  "$GH" workflow run "$WORKFLOW" --repo "$REPO" -f "tags=$ver" ||
    die "触发 $WORKFLOW 失败（没权限、或者这个 workflow 不在默认分支上）。"

  # ③ 等它跑完。**这是本脚本的主循环。**
  local waited=0 status conclusion id line v
  while :; do
    line="$("$GH" run list --repo "$REPO" --workflow "$WORKFLOW" --limit 10 \
      --json databaseId,status,conclusion \
      --jq "[.[] | select(.databaseId != $prev)] | sort_by(.databaseId) | last | [(.databaseId // -1), (.status // \"\"), (.conclusion // \"\")] | @tsv" \
      2>/dev/null)"
    id="$(printf '%s' "$line" | cut -f1)"
    status="$(printf '%s' "$line" | cut -f2)"
    conclusion="$(printf '%s' "$line" | cut -f3)"

    v="$(verdict "$status" "$conclusion")"
    case "$v" in
      ok)
        log "同步完成（run $id，success）。"
        break
        ;;
      fail)
        die "国内源同步没有成功（run ${id:-?}：status=${status:-?} conclusion=${conclusion:-?}）。镜像没就位，不发 Release。"
        ;;
      wait)
        if [ "$waited" -ge "$WAIT_SECS" ]; then
          die "等了 ${WAIT_SECS} 秒，国内源同步还没跑完（run ${id:-?}：${status:-?}）。镜像没就位，不发 Release。"
        fi
        log "同步进行中（run $id：$status），已等 ${waited} 秒…"
        sleep "$POLL_SECS"
        # 计步至少按 1 秒算：`--self-test` 会把 POLL_SECS 设成 0（不真等），
        # 那时若按 0 累加就永远到不了上限，超时那条路会变成死循环
        waited=$((waited + (POLL_SECS > 0 ? POLL_SECS : 1)))
        ;;
    esac
  done

  # ④ 最后一道校验：**两个源、四个镜像，全都能拉才算过。**
  #    非零退出直接停 —— 这一步不许被绕过、也不许被 `|| true` 吞掉。
  log "校验两源四个镜像（$ver）…"
  bash "$VERIFY" "$ver" || die "镜像源校验没过（见上面缺了哪些）。镜像没就位，不发 Release。"

  log "镜像闸门通过：$ver 在两个源上都齐了 —— 这一版可以点亮升级入口了。"
}

# ── 自测：证明这套「触发 — 等 — 核」不是空跑 ──────────────────────────────
#
# 真发布在这台机器上跑不了（要真的推镜像、真的建 Release）。能验的是**判定逻辑**：
# 用一个假的 `gh` 与假的校验脚本，把三条路都走一遍 ——
#
#   · 同步成功 + 校验通过 → 0；
#   · 同步**完成但失败** → 非零，而且**校验脚本一次都不许被调用**
#     （同步都失败了还去核镜像，等于用一次巧合掩盖一次失败）；
#   · 一直不完成 → 等超时后非零；
#   · `-f tags=<版本>` 有没有拼对（拼错就是同步了别的版本，而校验的是这个版本）。
self_test() {
  local tmp fails=0
  tmp="$(mktemp -d)"
  # `${tmp:-}`：`tmp` 是 local，EXIT 那一刻已经出作用域了，不这么写会 unbound
  trap 'rm -rf "${tmp:-}"' EXIT

  # 假的 gh：按 FAKE 那几件事回话，并把收到的参数记进 $tmp/calls
  cat >"$tmp/gh" <<'FAKE'
#!/usr/bin/env bash
echo "$*" >>"${FAKE_CALLS:?}"
case "$1 $2" in
  "api repos/agentpit-io/hunter-community/releases/latest")
    [ "${FAKE_MODE}" = "api-fail" ] && exit 1
    echo "v1.2.3"
    ;;
  "run list")
    case "${FAKE_MODE}" in
      ok|verify-fail) printf '%s\t%s\t%s\n' 4242 completed success ;;
      fail) printf '%s\t%s\t%s\n' 4242 completed failure ;;
      skipped) printf '%s\t%s\t%s\n' 4242 completed skipped ;;
      timeout) printf '%s\t%s\t%s\n' 4242 in_progress "" ;;
      *) printf '%s\t%s\t%s\n' -1 "" "" ;;
    esac
    ;;
  "workflow run") exit 0 ;;
  *) exit 0 ;;
esac
FAKE
  chmod +x "$tmp/gh"

  # 假的校验脚本：把「被调用过」记下来，按 MODE 决定退出码
  cat >"$tmp/verify" <<'FAKE'
#!/usr/bin/env bash
echo "$1" >>"${FAKE_VERIFY_CALLS:?}"
[ "${FAKE_MODE}" = "verify-fail" ] && exit 1
exit 0
FAKE
  chmod +x "$tmp/verify"

  run_case() { # 名字 · 期望退出码 · 模式（模式决定假 gh 怎么回话）
    local name="$1" want="$2" mode="$3"
    local calls="$tmp/calls-$mode" vcalls="$tmp/vcalls-$mode"
    : >"$calls"
    : >"$vcalls"
    # GATE_WAIT_SECS=1 + GATE_POLL_SECS=0：timeout 那条路一轮就判超时，不会真等
    FAKE_CALLS="$calls" FAKE_VERIFY_CALLS="$vcalls" FAKE_MODE="$mode" \
      GATE_GH="$tmp/gh" GATE_VERIFY="$tmp/verify" GATE_POLL_SECS=0 GATE_WAIT_SECS=1 \
      bash "$0" >/dev/null 2>&1
    local got=$?
    if [ "$got" -ne "$want" ]; then
      echo "✗ $name：期望退出码 $want，实际 $got"
      fails=$((fails + 1))
    else
      echo "✓ $name：退出码 $got"
    fi
    # `-f tags=<版本>` 有没有拼对：拼错就是同步了别的版本，而校验的是这个版本
    if ! grep -q -- "-f tags=1.2.3" "$calls"; then
      echo "✗ $name：触发同步时没带上 -f tags=1.2.3"
      echo "  实际：$(cat "$calls")"
      fails=$((fails + 1))
    fi
    # 同步没成功的那几条路：**校验脚本一次都不许被调用** ——
    # 同步都失败了还去核镜像，等于拿一次巧合掩盖一次失败
    if [ "$mode" = "fail" ] || [ "$mode" = "timeout" ]; then
      if [ -s "$vcalls" ]; then
        echo "✗ $name：同步都没成功，却还去核了镜像"
        fails=$((fails + 1))
      fi
    fi
    return 0
  }

  run_case "同步成功 · 校验通过" 0 ok
  run_case "同步完成但失败" 1 fail
  run_case "同步被跳过" 1 skipped
  run_case "一直没跑完 · 等超时" 1 timeout
  run_case "同步成功但校验说镜像不齐" 1 verify-fail

  if [ "$fails" -ne 0 ]; then
    echo
    echo "自测没过：$fails 条"
    return 1
  fi
  echo
  echo "自测全过（5 条）。"
  return 0
}

if [ "$SELF_TEST" = "1" ]; then
  self_test
  exit $?
fi

main
