#!/usr/bin/env bash
# I14 的真机验收脚本（测试机上跑）。用法：bash scripts/i14-verify.sh <场景>
#
# 只碰 compose 项目 `hunter` 与 `$HOME/.hunter`：旧栈 hunter-community、
# hca-* 一个都不碰（总控规则红线 9）。跑完自己恢复现场。
#
# 场景：
#   f2-sparse      造一个稀疏的「虚拟机磁盘」→ 删除计划里的体积必须按实占算，
#                  而且「工作目录」与「运行环境」不能是同一个数字；警告不重复
#   f2-du          删除计划报的数和 `du -sb` 实测对账（差 5% 以内）
#   f3-kept-key    造「有 .env、没 launcher.toml」的现场 → `--auto` / `--headless`
#                  要沿用 .env 里那把 key，不再报「标准输入不是终端」
#   f3-bad-key     .env 里是一把格式不对的 key → 不许说「沿用」，要如实说原因
#   f4-reinstall   删除应用（保留数据）→ 重装 → 定时任务必须自己回来，
#                  而且过程流里要有那一句话
#   f1-assets      自更新的资产选择：真去拉一次 latest.json，逐个平台核后缀
#   f5-upgrade     Hunter 版本升级（升级前自动备份 → 拉新 tag → up → 健康）
#   f5-nosuch      拿一个根本不存在的 tag 去升级 → 停在第 ① 步，**不许**谎称回滚
#   f5-rollback    升级失败自动回滚：拿一个「compose 有、镜像没发布」的真 tag
#                  （默认 1.0.0）撞第 ⑤ 步 pull，整套写回旧的
#   restore-env    把这台机器恢复成「6/6 健康」的样子（出问题时手动兜底）
set -uo pipefail

BIN="${BIN:-$HOME/HunterLauncher/src-tauri/target/release/hunter-launcher}"
HOME_DIR="$HOME/.hunter"
APP="$HOME_DIR/app"
TOML="$HOME_DIR/launcher.toml"
OVL="$APP/docker-compose.launcher.yml"
P=hunter
KEYFILE="$HOME/.hunter-launcher-test/key"
UNIT=hunter-backup

say() { printf '\n\033[1m== %s ==\033[0m\n' "$*"; }
ok()  { printf '  \033[32m✓\033[0m %s\n' "$*"; }
bad() { printf '  \033[31m✗\033[0m %s\n' "$*"; }

dc() { docker compose -p "$P" --project-directory "$APP" -f "$APP/docker-compose.yml" -f "$OVL" "$@"; }
pg() { docker exec hunter-postgres-1 psql -U hunter -d hunter -tAc "$1" 2>/dev/null; }
# JWT_SECRET 只取指纹 —— 它和 key 一样，明文不该出现在任何输出里
jwt_fp() { grep -m1 '^JWT_SECRET=' "$APP/.env" | cut -d= -f2- | sha256sum | cut -c1-16; }

wait_healthy() {
  local n=0
  while [ $n -lt 72 ]; do
    [ "$(docker ps --filter "label=com.docker.compose.project=$P" --filter health=healthy -q | wc -l)" = 6 ] && return 0
    sleep 5; n=$((n+1))
  done
  return 1
}

case "${1:-}" in

# ───────────────────────────────────────────── F2 体积按实占算、两项分开
f2-sparse)
  say "F2 · 造一个稀疏的「虚拟机磁盘」：表观 40 GB、实占 0"
  # 测试机用的是系统 Docker，`builtin::installed()` 恒为 None —— 那样「运行环境」
  # 那一行根本不会出现，也就测不出「两行不能是同一个数」。所以这里把内置运行时的
  # 清单文件造出来（**只造这一个文件**，跑完就删；它只影响判定，不启动任何东西）
  RT="$HOME_DIR/runtime"
  mkdir -p "$RT"
  had_manifest=1
  if [ ! -f "$RT/installed.json" ]; then
    had_manifest=0
    printf '{"items":[],"installedAt":"2026-09-23 16:00:00","launcherVersion":"0.1.14"}\n' > "$RT/installed.json"
  fi
  cleanup_f2() {
    [ "$had_manifest" = 0 ] && rm -f "$RT/installed.json"
    rm -rf "$RT/_i14"
    rmdir "$RT" 2>/dev/null || true
  }
  trap cleanup_f2 EXIT

  mkdir -p "$RT/_i14"
  # 和 Colima 的 diffdisk 同一类：truncate 出来的文件表观很大、实占为零
  truncate -s 40G "$RT/_i14/diffdisk"
  # 再放一点真数据，好让「真写进去的要数出来」也有得比
  dd if=/dev/zero of="$RT/_i14/real.bin" bs=1M count=64 status=none

  # `du -sb` 是**表观大小**（-b 等于 --apparent-size），稀疏文件上它和 `ls -l` 一样虚高；
  # 实占要用 `du -s --block-size=1`（数的就是 st_blocks）。这两个数在这里差三个数量级，
  # 正好是 F2 那个 15 倍的放大版
  apparent=$(du -sb "$RT" | cut -f1)
  real=$(du -s --block-size=1 "$RT" | cut -f1)
  echo "    运行环境：表观（du -sb）$apparent 字节 / 实占（du -s --block-size=1）$real 字节"

  out="$("$BIN" --uninstall-plan 2>&1)"
  echo "$out" | sed 's/^/    /'

  a="$(echo "$out" | grep -E '^工作目录' | awk '{print $2, $3}')"
  b="$(echo "$out" | grep -E '^运行环境' | awk '{print $2, $3}')"
  echo "    工作目录 = [$a] · 运行环境 = [$b]"

  # 1) 不许出现按表观算出来的那个数（40 GB 级别）
  if echo "$out" | grep -qE '^(工作目录|运行环境) +[0-9.]+ (GB|TB)'; then
    bad "还是按表观算的（出现了 GB 级数字）"
  else
    ok "两行都不是 GB 级 —— 稀疏文件按实占算了"
  fi
  # 2) 两行不能一模一样
  if [ -n "$a" ] && [ "$a" = "$b" ]; then bad "两行还是同一个数：$a"; else ok "两行来自两处：$a vs $b"; fi
  # 3) 「运行环境」那一行要和 du -sb 对得上（差 5% 以内）
  # 3) 「运行环境」那一行要和实占对得上（差 5% 以内）
  rt_bytes=$(echo "$b" | awk '{print $1}')
  python3 - "$rt_bytes" "$real" <<'PY'
import sys
# `flow::human_bytes` 按 1000 进位（MB = 1e6），不是 MiB —— 对账要用同一把尺子
shown = float(sys.argv[1]) * 1e6
real = float(sys.argv[2])
d = abs(shown - real) / real
print("    界面 %.1f MB vs 实占 %.1f MB，差 %.2f%%" % (shown/1e6, real/1e6, d*100))
sys.exit(0 if d <= 0.05 else 1)
PY
  [ $? = 0 ] && ok "运行环境那一行和实占对得上（5% 以内）" || bad "运行环境那一行和实占差太多"
  # 4) 警告不重复
  c="$(echo "$out" | grep -c '删掉运行环境等于把数据一起删掉')"
  [ "$c" -le 1 ] && ok "「不能同时删运行环境」只说了 $c 遍" || bad "又说了 $c 遍"

  cleanup_f2; trap - EXIT
  ok "现场已恢复（$([ -f "$RT/installed.json" ] && echo "installed.json 是本来就有的，没动" || echo "造出来的文件都删了")）"
  ;;

f2-du)
  say "F2 · 和 du -sb 对账"
  real=$(du -s --block-size=1 "$HOME_DIR" | cut -f1)
  app=$(du -sb "$HOME_DIR" | cut -f1)
  out="$("$BIN" --uninstall-plan 2>&1)"
  echo "$out" | grep -E '^(工作目录|运行环境)' | sed 's/^/    /'
  echo "    实占（du -s --block-size=1）$real 字节 · 表观（du -sb）$app 字节"
  ;;

# ───────────────────────────────────────────── F3 沿用 .env 里那把 key
f3-kept-key)
  say "F3 · 造「.env 还在、launcher.toml 被重写过」的现场"
  [ -f "$APP/.env" ] || { bad "$APP/.env 不在，先装一次"; exit 1; }
  grep -q '^HUNTER_API_KEY=hunt_tools_' "$APP/.env" && ok ".env 里有一把 key（不打印它）" || { bad ".env 里没有 key"; exit 1; }

  # 关键：**标准输入不是终端**（管道），这正是 0.1.13 报 E_KEY_INVALID 的那个条件
  out="$(echo | "$BIN" --data-check 2>&1 | head -5)"
  echo "$out" | sed 's/^/    /'

  say "读 key 那一步（不真装，只看它拿不拿得到 key）"
  # --review 这条路也走 read_key，而且不会动这台机器上的任何东西
  out="$(echo | "$BIN" --review install_runtime --review-why "I14 F3 验收" 2>&1 | head -20)"
  echo "$out" | sed 's/^/    /'
  if echo "$out" | grep -q '沿用上次保留的 key'; then
    ok "沿用了 .env 里那把 key"
  elif echo "$out" | grep -q '标准输入不是终端'; then
    bad "还是报「标准输入不是终端」—— F3 没修好"
  else
    bad "既没说沿用、也没报那句话，人工看上面的原话"
  fi
  # key 不许出现在输出里（红线 2）
  echo "$out" | grep -q "$(cut -c1-20 "$KEYFILE" 2>/dev/null || echo __nope__)" \
    && bad "输出里出现了 key 的前 20 位" || ok "输出里没有 key 明文"
  ;;

f3-bad-key)
  say "F3 · .env 里是一把格式不对的 key → 不许说「沿用」"
  cp "$APP/.env" "$APP/.env.i14bak"
  sed -i 's|^HUNTER_API_KEY=.*|HUNTER_API_KEY=hunt_tools_这不是一把真的key|' "$APP/.env"
  out="$(echo | "$BIN" --review install_runtime --review-why "I14 F3 坏 key" 2>&1 | head -20)"
  echo "$out" | sed 's/^/    /'
  echo "$out" | grep -q '沿用上次保留的 key' && bad "格式都不对还说沿用" || ok "没有谎称沿用"
  mv "$APP/.env.i14bak" "$APP/.env"
  ok "已还原 .env"
  ;;

# ───────────────────────────────────────────── F4 重装之后定时任务自己回来
f4-reinstall)
  say "F3 + F4 · 删除应用（保留数据）→ 重装：key 要沿用、定时任务要自己回来"
  "$BIN" --schedule install 2>&1 | sed 's/^/    /'
  echo "    定时任务：$(systemctl --user is-enabled hunter-backup.timer 2>&1)"

  before_tables=$(pg "select count(*) from information_schema.tables where table_schema='public'")
  fp0=$(jwt_fp)
  vols_before=$(docker volume ls --filter "label=com.docker.compose.project=$P" -q | sort | md5sum)
  echo "    删之前：$before_tables 张表 · JWT_SECRET 指纹 $fp0"

  say "删除应用（只删应用，保留数据）"
  "$BIN" --uninstall app --confirm '删除应用' -y 2>&1 | sed 's/^/    /'

  say "断言：容器没了、卷还在、.env 还在、定时任务没了"
  echo "    容器 $(docker ps -a --filter "label=com.docker.compose.project=$P" -q | wc -l) 个"
  [ "$vols_before" = "$(docker volume ls --filter "label=com.docker.compose.project=$P" -q | sort | md5sum)" ] \
    && ok "数据卷一个没少" || bad "数据卷变了"
  [ -f "$APP/.env" ] && ok ".env 还在（key 与 JWT_SECRET 都在里面）" || bad ".env 被删了"
  if systemctl --user is-enabled hunter-backup.timer >/dev/null 2>&1; then
    bad "定时任务还在"
  else
    ok "定时任务已被移除"
  fi

  say "重装：**标准输入是管道**（这正是 0.1.13 报 E_KEY_INVALID 的那个条件），不给 --key-file"
  t0=$(date +%s)
  out="$(echo | "$BIN" --headless -y 2>&1)"
  echo "$out" | tail -40 | sed 's/^/    /'
  echo "    用时 $(( $(date +%s) - t0 )) 秒"

  # F3
  if echo "$out" | grep -q '沿用上次保留的 key'; then
    ok "F3：沿用了 .env 里那把 key，没有再问"
  elif echo "$out" | grep -q '标准输入不是终端'; then
    bad "F3：还是报「标准输入不是终端」"
  else
    bad "F3：既没说沿用也没报那句话，人工看上面的原话"
  fi
  # F4
  if echo "$out" | grep -qE '自动备份.*(挂上|没挂上)|没有装定时任务'; then
    ok "F4：过程流里说清了定时任务的结论 —— $(echo "$out" | grep -oE '(已按你的备份设置.*|自动备份的定时任务没挂上：.*|配置里自动备份是关着的.*)' | head -1)"
  else
    bad "F4：过程流里一个字都没说"
  fi
  if systemctl --user is-enabled hunter-backup.timer >/dev/null 2>&1; then
    ok "F4：定时任务已自动装回"
    systemctl --user list-timers hunter-backup.timer --no-pager 2>&1 | head -3 | sed 's/^/    /'
  else
    bad "F4：定时任务没有自己回来"
  fi

  wait_healthy && ok "6/6 健康" || bad "没等到 6/6"
  after_tables=$(pg "select count(*) from information_schema.tables where table_schema='public'")
  [ "$before_tables" = "$after_tables" ] && ok "数据原样沿用（仍然 $after_tables 张表）" \
                                         || bad "表数变了：$before_tables → $after_tables"
  fp1=$(jwt_fp)
  [ "$fp0" = "$fp1" ] && ok "JWT_SECRET 没变（$fp1），登录不会失效" || bad "JWT_SECRET 变了：$fp0 → $fp1"
  ;;

# ───────────────────────────────────────────── F1 资产选择
f1-assets)
  say "F1 · latest.json 里每个平台取到的是不是它自己那个包"
  u="https://hunter-dl-hk-1253756459.cos.ap-hongkong.myqcloud.com/launcher/latest.json"
  j="$(curl -sS --max-time 30 "$u")" || { bad "取不到 $u"; exit 1; }
  echo "$j" | python3 -c '
import json,sys
m=json.load(sys.stdin)
want={"linux-x86_64":".AppImage","windows-x86_64":"-setup.exe",
      "darwin-x86_64":".app.tar.gz","darwin-aarch64":".app.tar.gz"}
bad=0
print("  清单版本 " + m["version"])
for k,w in want.items():
    p=m.get("platforms",{}).get(k)
    if not p:
        print("  x " + k + " 不在清单里"); bad=1; continue
    url=p["url"]
    name=url.rsplit("/",1)[-1]
    if url.endswith(".deb"):
        print("  x " + k + " 指向 .deb：" + name); bad=1
    elif not url.endswith(w):
        print("  x " + k + " 后缀不对（要 " + w + "）：" + name); bad=1
    else:
        print("  OK " + k + " -> " + name)
sys.exit(bad)
'
  say "本机（Linux）--check-update 的原话"
  "$BIN" --check-update 2>&1 | sed 's/^/    /'
  ;;

# ───────────────────────────────────────────── F5 Hunter 版本升级
f5-upgrade)
  say "F5 · 升级 Hunter（升级前自动备份 → 拉新 tag → up → 健康）"
  echo "    升级前：$(pg "select count(*) from information_schema.tables where table_schema='public'") 张表"
  grep -m1 '^tag' "$TOML" | sed 's/^/    当前 /'
  t0=$(date +%s)
  "$BIN" --upgrade "${2:-}" -y 2>&1 | sed 's/^/    /'
  echo "    用时 $(( $(date +%s) - t0 )) 秒"
  grep -m1 '^tag' "$TOML" | sed 's/^/    升级后 /'
  wait_healthy && ok "6/6 健康" || bad "没等到 6/6"
  echo "    升级后：$(pg "select count(*) from information_schema.tables where table_schema='public'") 张表"
  ;;

f5-nosuch)
  say "F5 · 根本不存在的 tag：停在第 ① 步，**不许**谎称回滚"
  # `upgrade.rs` 文件头写着：①② 还没动任何东西，失败 = 什么都没变，
  # 这时候说「已回滚」就是在编。所以这一档要断言的恰恰是**没提回滚**。
  before_tag=$(grep -m1 '^tag' "$TOML" | cut -d'"' -f2)
  out="$("$BIN" --upgrade 9.9.9-i14-不存在 -y 2>&1)"; rc=$?
  echo "$out" | tail -12 | sed 's/^/    /'
  echo "    退出码 $rc"
  [ "$rc" != 0 ] && ok "如实失败（退出码 $rc）" || bad "不存在的版本居然报成功"
  echo "$out" | grep -qE '回滚|已经回到' && bad "①② 都没动过东西，不该提回滚" || ok "没有谎称回滚"
  [ "$before_tag" = "$(grep -m1 '^tag' "$TOML" | cut -d'"' -f2)" ] && ok "tag 原样没动（$before_tag）" || bad "tag 变了"
  ;;

f5-rollback)
  # 怎么才走得到第 ⑤ 步：
  #
  # 先试过两种「拿一个旧 tag 去撞」的造法，**都不成**，因为它们停在第 ① 步：
  #   · `9.9.9-i14-不存在` —— compose 根本取不到（见 f5-nosuch，那一档单独留着）
  #   · `1.0.0` / `1.0.1`  —— tag 在 GitHub 上真实存在、compose 也取得到，
  #     但那一版的 compose 里没有 `hunter-community-web` 这个镜像引用，
  #     严格校验（`config.rs` 的 `validate_compose`）当场把它拦下来了。
  # 而 hunter-community **已发布的每一个 tag，四个镜像都在 ghcr 上**
  #（1.2.0-rc1 / 1.1.0-rc1 / 1.1.0-rc2 逐个 `docker manifest inspect` 核过），
  # 所以「compose 合格、镜像却拉不到」这个组合在上游根本不存在。
  #
  # 于是换成造**拉取本身失败**，而且用的是 I6 在用户 Mac 上真撞过的那个失败：
  # `DOCKER_CONFIG` 指向一份写着不存在的 credsStore 的配置 → docker 客户端
  # 连凭据都取不到，`pull` 必然失败。这条失败 `flow::pull` 判定为「与下载源无关」
  # （`Code::CredHelper`），**不换源、直接返回** —— 正好是第 ⑤ 步的失败。
  # 这份配置只挂在这一条命令的环境变量上，机器上的 docker 配置一个字节都不动。
  TAG="${2:-1.2.0}"
  BADCFG=/tmp/i14-badcfg
  mkdir -p "$BADCFG"
  printf '{"credsStore": "i14nonexistent"}\n' > "$BADCFG/config.json"
  trap 'rm -rf "$BADCFG"' EXIT
  say "F5 · 升级失败自动回滚（目标 $TAG，第 ⑤ 步 pull 取不到凭据）"
  before_tag=$(grep -m1 '^tag' "$TOML" | cut -d'"' -f2)
  before_env=$(grep -m1 '^HUNTER_VERSION=' "$APP/.env")
  before_tables=$(pg "select count(*) from information_schema.tables where table_schema='public'")
  before_ids=$(docker ps --filter "label=com.docker.compose.project=$P" -q | sort | md5sum)
  before_ovl=$(md5sum "$OVL" | cut -d' ' -f1)
  before_backups=$("$BIN" --backups 2>&1 | grep -c '校验')
  echo "    升级前：tag=$before_tag · $before_env · $before_tables 张表 · 备份 $before_backups 份"

  t0=$(date +%s)
  out="$(DOCKER_CONFIG="$BADCFG" "$BIN" --upgrade "$TAG" -y 2>&1)"; rc=$?
  echo "$out" | tail -30 | sed 's/^/    /'
  echo "    退出码 $rc · 用时 $(( $(date +%s) - t0 )) 秒"

  [ "$rc" != 0 ] && ok "如实失败（退出码 $rc），没有谎称升级成功" || bad "拉不到镜像居然报成功"
  echo "$out" | grep -q '已写入新的 compose' && ok "走到了第 ③ 步（新 compose 写下去了）—— 这一次是真的需要回滚" \
                                            || bad "没走到第 ③ 步，这一档没测到回滚"
  echo "$out" | grep -qE '回滚|已经回到' && ok "输出里说了回滚" || bad "输出里没提回滚"
  echo "$out" | grep -qE '备份放到|已备份配置' && ok "动手之前先备份了一次" || bad "没看见升级前那次备份"

  after_tag=$(grep -m1 '^tag' "$TOML" | cut -d'"' -f2)
  [ "$before_tag" = "$after_tag" ] && ok "launcher.toml 的 tag 回到了 $after_tag" \
                                   || bad "tag 没回来：$before_tag → $after_tag"
  [ "$before_env" = "$(grep -m1 '^HUNTER_VERSION=' "$APP/.env")" ] \
    && ok ".env 的 HUNTER_VERSION 回来了" || bad ".env 没回来"
  [ "$before_ovl" = "$(md5sum "$OVL" | cut -d' ' -f1)" ] \
    && ok "覆盖文件逐字节回到了升级前" || bad "覆盖文件没回来"
  wait_healthy && ok "回滚之后 6/6 健康" || bad "回滚之后没等到 6/6"
  after_tables=$(pg "select count(*) from information_schema.tables where table_schema='public'")
  [ "$before_tables" = "$after_tables" ] && ok "数据没动（仍然 $after_tables 张表）" \
                                         || bad "表数变了：$before_tables → $after_tables"
  if [ "$before_ids" = "$(docker ps --filter "label=com.docker.compose.project=$P" -q | sort | md5sum)" ]; then
    ok "六个容器一个都没重建（容器 ID 全同）"
  else
    echo "    （容器 ID 变了 —— 回滚时 up -d 重建过，看上面的过程流）"
  fi
  say "这一次升级前的自动备份"
  "$BIN" --backups 2>&1 | tail -8 | sed 's/^/    /'
  rm -rf "$BADCFG"; trap - EXIT
  ;;

restore-env)
  say "把这台机器恢复成 6/6 健康"
  dc up -d 2>&1 | tail -5 | sed 's/^/    /'
  wait_healthy && ok "6/6 健康" || bad "没等到 6/6"
  ;;

*)
  sed -n '3,20p' "$0"
  exit 2
  ;;
esac
