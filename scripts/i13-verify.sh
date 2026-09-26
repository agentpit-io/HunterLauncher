#!/usr/bin/env bash
# I13 的真机验收脚本（测试机上跑）。用法：bash scripts/i13-verify.sh <场景>
#
# 每个场景都**真的动这台机器上的 compose 项目 hunter**，跑完会把现场恢复。
# 只碰项目 `hunter`：旧栈 hunter-community、hca-* 一个都不碰（总控规则红线 9）。
#
# 场景：
#   r6-backup      做一次备份：pg_dump -Fc + 三个卷 + 配置 + meta.json，校验必须通过
#   r6-rotate      模拟 5 天的备份 → 按天轮换之后只剩最近 3 天（升级前备份另算）
#   r6-schedule    定时备份设成 2 分钟后 + 关掉启动器 → 到点自己生成并校验通过
#   r6-persistent  systemd 定时器的补跑（Persistent=true）：错过的那一次开机后要补上
#   r6-restore     恢复：先备份 → 改数据 → 恢复回去 → 行数与 JWT_SECRET 指纹都要还原
#   r6-diskfull    备份目录所在盘写满（真的 loop 设备）→ 失败提示 + 规则层结论
#   r4-plan        删除应用的两种范围各看一遍会动到什么（只读）
#   r4-confirm     输入框错一个字 → 拒绝执行
#   r4-app-only    删除应用 ①（保留数据）→ 重装：数据沿用、JWT_SECRET 不变
#   r4-all         删除应用 ②（先备份）→ 全新安装 → 从那份备份恢复
#   r7-disk        系统盘剩余告警（把阈值调到实测值之上造现场，数字全是真的）
#   r7-restart     一个服务在窗口内重启 ≥3 次 → 提醒并定位到它
#   r7-oom         构造一次真的 OOM（给 llm-shim 一个极小的内存上限）→ 严重提醒
#   r7-cleanup     一键腾空间：只清本项目旧镜像与悬空卷，别人的一个不碰
#   guard          守卫越界：删别的项目的卷 / prune / 动别人的定时任务，一律要被拒
#   seed           往库里塞几行真数据（空库上「行数前后一致」说明不了什么）
set -uo pipefail

BIN="${BIN:-$HOME/HunterLauncher/src-tauri/target/release/hunter-launcher}"
HOME_DIR="$HOME/.hunter"
APP="$HOME_DIR/app"
TOML="$HOME_DIR/launcher.toml"
OVL="$APP/docker-compose.launcher.yml"
BDIR="${BDIR:-$HOME/Hunter-backups}"
P=hunter
KEYFILE="$HOME/.hunter-launcher-test/key"

say() { printf '\n\033[1m== %s ==\033[0m\n' "$*"; }
row() { printf '%-46s %s\n' "$1" "$2"; }
ok()  { printf '  \033[32m✓\033[0m %s\n' "$*"; }
bad() { printf '  \033[31m✗\033[0m %s\n' "$*"; }

dc() { docker compose -p "$P" --project-directory "$APP" -f "$APP/docker-compose.yml" -f "$OVL" "$@"; }
pg() { docker exec hunter-postgres-1 psql -U hunter -d hunter -tAc "$1" 2>/dev/null; }

# 「关键表行数」—— 备份 / 恢复 / 删除前后必须对得上
# 实查这套库里真的有行的那几张（`pg_stat_user_tables` 排出来的）：
# schema_migrations 23 行、push_tasks 3 行、backtest_config 1 行，
# 外加我们自己 seed 进去的 hunter_config。**不选空表** —— 0 == 0 什么也证明不了
key_counts() {
  for t in hunter_config schema_migrations push_tasks backtest_config users; do
    printf '%s=%s\n' "$t" "$(pg "select count(*) from $t" || echo 读不到)"
  done
}
jwt_fp() { grep -m1 '^JWT_SECRET=' "$APP/.env" | cut -d= -f2- | sha256sum | cut -c1-16; }
py() { python3 "$(dirname "$0")/i13-toml.py" "$@"; }

wait_healthy() {
  local n=0
  while [ $n -lt 60 ]; do
    [ "$(docker ps --filter "label=com.docker.compose.project=$P" --filter health=healthy -q | wc -l)" = 6 ] && return 0
    sleep 5; n=$((n+1))
  done
  return 1
}

case "${1:-}" in

# 往库里塞几行真数据 —— 「行数前后一字不差」这种断言在一个空库上说明不了什么
seed)
  say "往 hunter_config 里塞几行，好让后面的「前后一致」有东西可比"
  # 列名是 k / v（实查 `\d hunter_config`，不是猜的 key / value）
  for i in 1 2 3 4 5 6 7; do
    pg "insert into hunter_config (k, v) values ('i13_seed_$i','v$i') on conflict (k) do update set v = excluded.v"
  done
  key_counts
  ;;

# ─────────────────────────────────────────────────────────────── R6 备份
r6-backup)
  say "R6 · 做一次备份并校验"
  before=$(key_counts); echo "$before"
  t0=$(date +%s)
  "$BIN" --backup
  rc=$?
  t1=$(date +%s)
  row "退出码" "$rc"
  row "耗时" "$((t1-t0)) 秒"
  last=$(ls -1dt "$BDIR"/hunter-* 2>/dev/null | head -1)
  row "备份目录" "$last"
  ls -la "$last"
  echo "--- meta.json ---"
  python3 -m json.tool "$last/meta.json" | head -80
  echo "--- 目录权限（必须 700）---"
  stat -c '%a %n' "$last" "$last/.env"
  echo "--- 校验：手工再跑一遍 pg_restore --list ---"
  docker exec -i hunter-postgres-1 pg_restore --list < "$last/hunter.dump" | grep -vc '^;'
  echo "--- 卷包里真的有东西吗 ---"
  for f in secrets user_skills opencode_data; do
    [ -f "$last/$f.tar.gz" ] && printf '%-24s %s 个文件\n' "$f.tar.gz" "$(tar -tzf "$last/$f.tar.gz" | wc -l)"
  done
  echo "--- 备份期间数据没被动过 ---"
  after=$(key_counts); echo "$after"
  [ "$before" = "$after" ] && ok "行数前后一字不差" || bad "行数变了"
  ;;

r6-rotate)
  say "R6 · 按天轮换：模拟 5 天之后只剩最近 3 天"
  mkdir -p "$BDIR"
  # 造 5 天的定时备份 + 同一天的第二份 + 3 份升级前备份
  for d in 19 20 21 22 23; do
    # 同一天的第二份放在 01:00 —— 必须早于「现在」，否则真做的那一份反而成了当天最早的
    for hm in 0000 0100; do
      [ "$hm" = 0100 ] && [ "$d" != 23 ] && continue
      id="hunter-202609${d}-${hm}-v1.2.0"
      mkdir -p "$BDIR/$id"
      cat > "$BDIR/$id/meta.json" <<JSON
{"id":"$id","tag":"1.2.0","at":"2026-09-${d} ${hm:0:2}:${hm:2:2}:00","kind":"scheduled","verified":true,"dumpBytes":1024,"totalBytes":1024}
JSON
      head -c 1024 /dev/zero > "$BDIR/$id/hunter.dump"
    done
  done
  for d in 01 02 03; do
    id="hunter-202609${d}-1000-v1.1.0"
    mkdir -p "$BDIR/$id"
    cat > "$BDIR/$id/meta.json" <<JSON
{"id":"$id","tag":"1.1.0","at":"2026-09-${d} 10:00:00","kind":"pre-upgrade","verified":true,"dumpBytes":1024,"totalBytes":1024}
JSON
  done
  echo "轮换前："; ls -1 "$BDIR" | sort
  py "$TOML" set backup keep_days 3
  "$BIN" --backups | head -20
  # 轮换发生在 --backup 之后；这里直接调一次真备份把它带起来
  "$BIN" --backup >/dev/null 2>&1
  echo "轮换后："; ls -1 "$BDIR" | sort
  echo "--- 断言 ---"
  n_sched=$(ls -1 "$BDIR" | grep -c 'v1.2.0')
  n_up=$(ls -1 "$BDIR" | grep -c 'v1.1.0')
  row "定时/手动备份剩几份（期望 3 天）" "$n_sched"
  row "升级前备份剩几份（期望 2）" "$n_up"
  ;;

r6-schedule)
  say "R6 · 定时备份：设成 2 分钟后，关掉启动器，到点自己跑"
  when=$(date -d '+2 minutes' '+%H:%M')
  row "现在" "$(date '+%H:%M:%S')"
  row "设定时间" "$when"
  py "$TOML" set backup enabled true
  py "$TOML" set backup time "$when"
  "$BIN" --schedule install
  echo "--- 系统里的单元文件 ---"
  cat ~/.config/systemd/user/hunter-backup.timer
  cat ~/.config/systemd/user/hunter-backup.service
  systemctl --user list-timers hunter-backup.timer --no-pager --all
  before=$(ls -1d "$BDIR"/hunter-* 2>/dev/null | wc -l)
  row "现在有几份备份" "$before"
  echo "--- 确认没有任何启动器进程在跑 ---"
  pkill -f 'hunter-launcher' 2>/dev/null
  sleep 1
  pgrep -af hunter-launcher || echo "  （一个都没有）"
  echo "--- 等到点（最多 240 秒）---"
  n=0
  while [ $n -lt 48 ]; do
    now=$(ls -1d "$BDIR"/hunter-* 2>/dev/null | wc -l)
    if [ "$now" -gt "$before" ]; then
      ok "$(date '+%H:%M:%S') 生成了新备份"
      break
    fi
    sleep 5; n=$((n+1))
  done
  last=$(ls -1dt "$BDIR"/hunter-* | head -1)
  row "新备份" "$last"
  python3 -c "import json,sys; m=json.load(open('$last/meta.json')); print('kind =',m['kind']); print('verified =',m['verified']); print('verifyNote =',m['verifyNote']); print('totalBytes =',m['totalBytes'])"
  echo "--- systemd 自己怎么说 ---"
  systemctl --user status hunter-backup.service --no-pager -n 20 2>&1 | head -30
  journalctl --user -u hunter-backup.service --no-pager -n 20 2>&1 | tail -20
  ;;

r6-persistent)
  say "R6 · systemd 定时器的补跑（Persistent=true）"
  #
  # **这个现场在这台机器上只能近似造。** `Persistent=true` 的语义是
  # 「开机 / 用户 manager 重新起来时，发现上一次该跑的时间已经过去了就补跑」——
  # 而 systemd 把「上次什么时候触发过」同时记在盘上（stamp 文件）**和自己进程的内存里**。
  # 第一遍的做法（stop → 删 stamp → start）不管用：内存里那一份还在
  # （实测 list-timers 的 LAST 仍然是 11:18）。真要走那条路只能重启这台机器，
  # 而这台机器上还跑着另外三套项目（总控规则红线 9）。
  #
  # 所以这里换一种造法：用**启动器生成的那两份单元文件的原文**另起一个一次性单元，
  # 它从来没触发过（没有任何 stamp、没有内存记录），OnCalendar 指向今天 00:00
  # —— 也就是「已经错过」。Persistent=true 该让它一 start 就补跑。
  # 跑的是同一个 --backup --scheduled。跑完就把这个一次性单元删干净。
  #
  "$BIN" --schedule install >/dev/null
  echo "--- 启动器生成的那两份单元（原文）---"
  cat ~/.config/systemd/user/hunter-backup.timer
  cat ~/.config/systemd/user/hunter-backup.service
  PROBE=hunter-backup-i13probe
  sed 's/^Unit=hunter-backup.service$/Unit='"$PROBE"'.service/; s/^OnCalendar=.*/OnCalendar=*-*-* 00:00:00/' \
    ~/.config/systemd/user/hunter-backup.timer > ~/.config/systemd/user/$PROBE.timer
  cp ~/.config/systemd/user/hunter-backup.service ~/.config/systemd/user/$PROBE.service
  echo "--- 一次性单元（只改了 Unit= 与 OnCalendar，Persistent 原样）---"
  grep -E 'OnCalendar|Persistent|Unit=' ~/.config/systemd/user/$PROBE.timer
  systemctl --user daemon-reload
  # **stamp 文件是关键**：Persistent 的语义是「和上一次真的跑过的时间比」。
  # 一个从来没跑过的定时器（没有 stamp）systemd 不会替它补跑 ——
  # 否则每装一个每日定时器都会当场跑一次。所以这里手工造一个
  # 「上一次是昨天跑的」的 stamp（stamp 文件的内容是空的，时间在 mtime 上）。
  mkdir -p ~/.local/share/systemd/timers
  : > ~/.local/share/systemd/timers/stamp-$PROBE.timer
  touch -d "$(date -d 'yesterday 00:00' '+%Y-%m-%d %H:%M:%S')" ~/.local/share/systemd/timers/stamp-$PROBE.timer
  ls -la --time-style=+%F\ %T ~/.local/share/systemd/timers/stamp-$PROBE.timer
  before=$(ls -1d "$BDIR"/hunter-* 2>/dev/null | wc -l)
  row "启动之前有几份备份" "$before"
  row "现在几点" "$(date '+%H:%M')（OnCalendar 是 00:00，也就是已经错过）"
  systemctl --user start $PROBE.timer
  n=0
  while [ $n -lt 36 ]; do
    now=$(ls -1d "$BDIR"/hunter-* 2>/dev/null | wc -l)
    if [ "$now" -gt "$before" ]; then ok "补跑了（$(date '+%H:%M:%S')）"; break; fi
    sleep 5; n=$((n+1))
  done
  row "现在有几份" "$(ls -1d "$BDIR"/hunter-* 2>/dev/null | wc -l)"
  systemctl --user list-timers $PROBE.timer --no-pager --all
  systemctl --user status $PROBE.service --no-pager -n 12 2>&1 | head -20
  echo "--- 收拾现场：一次性单元删干净 ---"
  systemctl --user stop $PROBE.timer 2>/dev/null
  rm -f ~/.config/systemd/user/$PROBE.timer ~/.config/systemd/user/$PROBE.service
  rm -f ~/.local/share/systemd/timers/stamp-$PROBE.timer
  systemctl --user daemon-reload
  systemctl --user list-timers --no-pager --all | grep -c i13probe || echo "  一次性单元已经不在了"
  ;;

r6-restore)
  say "R6 · 恢复：改数据 → 恢复回去 → 行数与 JWT 指纹都要还原"
  "$BIN" --backup || exit 1
  src=$(ls -1dt "$BDIR"/hunter-* | head -1)
  row "用这一份恢复" "$src"
  before=$(key_counts); echo "备份时：$before"
  fp0=$(jwt_fp); row "JWT_SECRET 指纹" "$fp0"
  echo "--- 故意改数据：插一行、再删一行（列名是 k / v，实查过）---"
  pg "insert into hunter_config (k, v) values ('i13_probe','x') on conflict (k) do nothing"
  pg "delete from hunter_config where k = 'i13_seed_1'"
  echo "改完共 $(pg "select count(*) from hunter_config") 行 · i13_probe $(pg "select count(*) from hunter_config where k='i13_probe'") 行 · i13_seed_1 $(pg "select count(*) from hunter_config where k='i13_seed_1'") 行"
  echo "--- 恢复 ---"
  t0=$(date +%s)
  "$BIN" --restore "$src" --confirm 恢复数据 -y
  rc=$?
  row "退出码" "$rc"
  row "耗时" "$(( $(date +%s) - t0 )) 秒"
  wait_healthy && ok "6/6 健康" || bad "没等到 6/6"
  after=$(key_counts); echo "恢复后：$after"
  [ "$before" = "$after" ] && ok "行数一字不差" || bad "行数对不上"
  echo "恢复后 i13_probe 应该没了（$(pg "select count(*) from hunter_config where k='i13_probe'") 行）· i13_seed_1 应该回来了（$(pg "select count(*) from hunter_config where k='i13_seed_1'") 行）"
  fp1=$(jwt_fp); row "JWT_SECRET 指纹（恢复后）" "$fp1"
  [ "$fp0" = "$fp1" ] && ok "JWT_SECRET 没变，登录不会失效" || bad "JWT_SECRET 变了"
  ;;

r6-diskfull)
  say "R6 · 备份目录所在盘写满（真的 loop 设备）"
  IMG=/tmp/i13-tiny.img
  MNT=/tmp/i13-tiny
  sudo umount "$MNT" 2>/dev/null; rm -f "$IMG"; mkdir -p "$MNT"
  dd if=/dev/zero of="$IMG" bs=1M count=12 status=none
  mkfs.ext4 -q "$IMG"
  sudo mount -o loop "$IMG" "$MNT"
  sudo chown "$USER:$USER" "$MNT"
  df -h "$MNT" | tail -1
  old_dir=$(py "$TOML" get backup dir)
  py "$TOML" set backup dir "$MNT"
  echo "--- 先把它填到只剩几百 KB ---"
  dd if=/dev/zero of="$MNT/filler" bs=1K count=9000 status=none 2>/dev/null
  df -h "$MNT" | tail -1
  echo "--- 现在做备份，期望失败并说清原因 ---"
  "$BIN" --backup 2>&1 | tail -20
  row "退出码" "$?"
  echo "--- 配置里记下来的失败 ---"
  grep -A3 '^\[backup\]' "$TOML" | head -12
  grep -E 'last_error|fail_streak' "$TOML"
  echo "--- 再失败一次 → fail_streak 到 2 → 规则层该给结论 ---"
  "$BIN" --backup >/dev/null 2>&1
  grep -E 'last_error|fail_streak' "$TOML"
  "$BIN" --alerts 2>&1 | sed -n '/backup-failed/,/^$/p'
  echo "--- 收拾现场 ---"
  py "$TOML" set backup dir "$old_dir"
  sudo umount "$MNT"; rm -f "$IMG"; rmdir "$MNT"
  ;;

# ─────────────────────────────────────────────────────────────── R4 删除
r4-plan)
  say "R4 · 两种范围各看一遍（只读）"
  "$BIN" --uninstall-plan app
  echo
  "$BIN" --uninstall-plan all --deep
  ;;

r4-confirm)
  say "R4 · 输入框错一个字 → 拒绝"
  for w in 删除应 删除引用 "删除应用。" "删除 应用" 删除应用和数据 ""; do
    out=$("$BIN" --uninstall app --confirm "$w" -y 2>&1 | tail -2)
    if echo "$out" | grep -q '逐字输入'; then
      ok "「$w」被拒：$(echo "$out" | head -1)"
    else
      bad "「$w」没被拒！$out"
    fi
  done
  ok "（正确的那一句不在这里试 —— 它归 r4-app-only）"
  ;;

r4-app-only)
  say "R4 · 删除应用①（保留数据）→ 重装应沿用"
  before=$(key_counts); echo "删除前：$before"
  fp0=$(jwt_fp); row "JWT_SECRET 指纹" "$fp0"
  vols_before=$(docker volume ls --filter "label=com.docker.compose.project=$P" -q | sort)
  "$BIN" --schedule install >/dev/null 2>&1
  row "定时任务装了吗" "$(systemctl --user is-enabled hunter-backup.timer 2>&1)"
  echo "--- 删 ---"
  "$BIN" --uninstall app --confirm 删除应用 -y
  echo "--- 断言：容器没了、卷还在、.env 还在、定时任务没了 ---"
  row "容器" "$(docker ps -a --filter "label=com.docker.compose.project=$P" -q | wc -l) 个"
  vols_after=$(docker volume ls --filter "label=com.docker.compose.project=$P" -q | sort)
  [ "$vols_before" = "$vols_after" ] && ok "6 个数据卷一个没少" || bad "数据卷变了"
  [ -f "$APP/.env" ] && ok ".env 还在（数据库口令与 JWT_SECRET 都在里面）" || bad ".env 被删了"
  row "launcher.toml 里 install.done" "$(grep -A2 '^\[install\]' "$TOML" | grep done || echo 没有)"
  row "备份设置还在吗" "$(grep -A4 '^\[backup\]' "$TOML" | head -5 | tr '\n' ' ')"
  row "定时任务" "$(systemctl --user is-enabled hunter-backup.timer 2>&1)"
  row "备份目录" "$(ls -1d "$BDIR"/hunter-* 2>/dev/null | wc -l) 份（一份都不该少）"
  echo "--- 重装（零点击）---"
  t0=$(date +%s)
  "$BIN" --auto --key-file "$KEYFILE" -y 2>&1 | tail -25
  row "重装耗时" "$(( $(date +%s) - t0 )) 秒"
  wait_healthy && ok "6/6 健康" || bad "没等到 6/6"
  after=$(key_counts); echo "重装后：$after"
  [ "$before" = "$after" ] && ok "数据原样沿用，一行都没重建" || bad "行数对不上"
  fp1=$(jwt_fp)
  [ "$fp0" = "$fp1" ] && ok "JWT_SECRET 没变（$fp1），登录不会失效" || bad "JWT_SECRET 变了：$fp0 → $fp1"
  ;;

r4-all)
  say "R4 · 删除应用②（先备份）→ 全新安装 → 从备份恢复"
  before=$(key_counts); echo "删除前：$before"
  fp0=$(jwt_fp); row "JWT_SECRET 指纹" "$fp0"
  echo "--- 删（带 --backup-first）---"
  "$BIN" --uninstall all --confirm 删除应用和数据 --backup-first -y
  src=$(ls -1dt "$BDIR"/hunter-* | head -1)
  row "删除前那份备份" "$src"
  echo "--- 断言：容器、卷、工作目录都没了；备份还在 ---"
  row "容器" "$(docker ps -a --filter "label=com.docker.compose.project=$P" -q | wc -l) 个"
  row "数据卷" "$(docker volume ls --filter "label=com.docker.compose.project=$P" -q | wc -l) 个"
  row "~/.hunter" "$([ -d "$HOME_DIR" ] && echo 还在 || echo 没了)"
  row "备份目录" "$(ls -1d "$BDIR"/hunter-* 2>/dev/null | wc -l) 份"
  row "别的项目的卷（不该被动）" "$(docker volume ls --format '{{.Name}}' | grep -c hunter-community || true)"
  echo "--- 全新安装 ---"
  t0=$(date +%s)
  "$BIN" --auto --key-file "$KEYFILE" -y 2>&1 | tail -20
  row "安装耗时" "$(( $(date +%s) - t0 )) 秒"
  wait_healthy && ok "6/6 健康" || bad "没等到 6/6"
  echo "全新安装后（应该是空库）：$(key_counts)"
  echo "--- 从那份备份恢复 ---"
  "$BIN" --restore "$src" --confirm 恢复数据 -y 2>&1 | tail -20
  wait_healthy && ok "6/6 健康" || bad "没等到 6/6"
  after=$(key_counts); echo "恢复后：$after"
  [ "$before" = "$after" ] && ok "数据完整恢复" || bad "行数对不上"
  fp1=$(jwt_fp)
  [ "$fp0" = "$fp1" ] && ok "JWT_SECRET 恢复了（$fp1），加密配置读得出来" || bad "JWT_SECRET 没恢复：$fp0 → $fp1"
  echo "--- 密钥卷真的回来了吗 ---"
  docker run --rm -v hunter_hunter_secrets:/v:ro postgres:16-alpine ls -la /v | head
  ;;

# ─────────────────────────────────────────────────────────────── R7 监测
r7-disk)
  say "R7 · 系统盘告警（阈值调到实测值之上造现场，数字全是真的）"
  df -B1 "$HOME" | tail -1
  free_gb=$(df -B1G --output=avail "$HOME" | tail -1 | tr -d ' ')
  row "系统盘实际剩余" "${free_gb} GB"
  old_w=$(py "$TOML" get monitor host_disk_warn_gb)
  old_c=$(py "$TOML" get monitor host_disk_crit_gb)
  py "$TOML" set monitor host_disk_warn_gb $((free_gb + 50))
  py "$TOML" set monitor host_disk_crit_gb $((free_gb + 20))
  rm -f "$HOME_DIR/monitor-state.json"
  "$BIN" --alerts 2>&1 | sed -n '/host-disk/,/^$/p'
  echo "--- 同一个问题 24 小时内不重复：再跑一次，notify 里不该再有它 ---"
  "$BIN" --alerts 2>&1 | tail -5
  echo "--- 一键清理能腾出多少（只看不删）---"
  "$BIN" --cleanup 2>&1 | tail -20
  py "$TOML" set monitor host_disk_warn_gb "$old_w"
  py "$TOML" set monitor host_disk_crit_gb "$old_c"
  ;;

r7-restart)
  say "R7 · 一个服务在窗口内反复重启"
  #
  # **不能用 `docker restart` 造这个现场**：`RestartCount` 只数「重启策略重新拉起来」
  # 的次数，手工 `docker restart` 一次都不算（第一遍就是这么跑的，四次之后它还是 0）。
  # 真的现场是「容器自己退出 → unless-stopped 把它拉回来」，所以这里让它真的崩。
  #
  rm -f "$HOME_DIR/monitor-state.json"
  before=$(docker inspect -f '{{.RestartCount}}' hunter-llm-shim-1)
  row "动手之前的 RestartCount" "$before"
  cp "$OVL" /tmp/i13-ovl.bak
  python3 - <<'PYX'
import os
p = os.path.expanduser('~/.hunter/app/docker-compose.launcher.yml')
s = open(p, encoding='utf-8').read()
s = s.replace('  llm-shim:\n', '  llm-shim:\n    entrypoint: ["sh", "-c", "exit 1"]\n', 1)
open(p, 'w', encoding='utf-8').write(s)
PYX
  grep -A3 'llm-shim:' "$OVL" | head -6
  dc up -d --force-recreate llm-shim 2>&1 | tail -2
  "$BIN" --alerts >/dev/null 2>&1        # 先采一次基线
  for i in 1 2 3 4 5 6; do
    sleep 5
    "$BIN" --alerts >/dev/null 2>&1      # 模拟界面在轮询（每一次都往状态文件里记一笔）
    printf '  第 %d 次采样：RestartCount=%s\n' "$i" "$(docker inspect -f '{{.RestartCount}}' hunter-llm-shim-1)"
  done
  echo "--- docker 怎么说 ---"
  docker inspect -f '{{.RestartCount}} {{.State.Status}} {{.State.ExitCode}}' hunter-llm-shim-1
  echo "--- 提醒 ---"
  "$BIN" --alerts 2>&1 | sed -n '/restart-llm-shim/,/^$/p'
  echo "--- 采样文件（窗口内的差值就是从这里算的）---"
  python3 -m json.tool "$HOME_DIR/monitor-state.json" | head -40
  echo "--- 收拾现场 ---"
  cp /tmp/i13-ovl.bak "$OVL"
  dc up -d --force-recreate llm-shim 2>&1 | tail -2
  wait_healthy && ok "六个服务恢复健康" || bad "没等到 6/6"
  ;;

r7-oom)
  say "R7 · 构造一次真的 OOM（给 llm-shim 一个极小的内存上限）"
  rm -f "$HOME_DIR/monitor-state.json"
  cp "$OVL" /tmp/i13-ovl.bak
  python3 - <<'PYX'
import re
p = __import__('os').path.expanduser('~/.hunter/app/docker-compose.launcher.yml')
s = open(p, encoding='utf-8').read()
s = s.replace('  llm-shim:\n', '  llm-shim:\n    mem_limit: 6m\n', 1)
open(p, 'w', encoding='utf-8').write(s)
PYX
  grep -A3 'llm-shim:' "$OVL" | head -6
  dc up -d --force-recreate llm-shim 2>&1 | tail -3
  sleep 20
  echo "--- docker 怎么说 ---"
  docker inspect -f '{{.State.OOMKilled}} {{.State.ExitCode}} {{.RestartCount}}' hunter-llm-shim-1
  echo "--- 提醒 ---"
  "$BIN" --alerts 2>&1 | sed -n '/oom-llm-shim/,/^$/p'
  echo "--- 收拾现场 ---"
  cp /tmp/i13-ovl.bak "$OVL"
  dc up -d --force-recreate llm-shim 2>&1 | tail -2
  wait_healthy && ok "六个服务恢复健康" || bad "没等到 6/6"
  ;;

r7-cleanup)
  say "R7 · 一键腾空间：只清本项目的东西"
  echo "--- 造一个「旧版本的镜像」（给现有镜像打个旧 tag，删它只会删掉这个 tag）---"
  docker tag ghcr.io/agentpit-io/hunter-community-api:1.2.0 ghcr.io/agentpit-io/hunter-community-api:1.1.0-i13test
  echo "--- 造一个悬空卷（带本项目标签、但 compose 里没有声明过）---"
  docker volume create --label com.docker.compose.project=hunter \
    --label com.docker.compose.volume=hunter_i13_orphan hunter_hunter_i13_orphan
  echo "--- 别人的镜像与卷（一个都不该被碰）---"
  other_img=$(docker image ls --format '{{.Repository}}:{{.Tag}}' | grep -v hunter-community | head -3)
  other_vol=$(docker volume ls --format '{{.Name}}' | grep -v '^hunter_' | wc -l)
  echo "$other_img"
  row "别人的卷有几个" "$other_vol"
  echo "--- 只看不删 ---"
  "$BIN" --cleanup 2>&1 | tail -20
  echo "--- 真清 ---"
  "$BIN" --cleanup -y 2>&1 | tail -15
  echo "--- 断言 ---"
  docker image ls --format '{{.Repository}}:{{.Tag}}' | grep -q 'hunter-community-api:1.1.0-i13test' \
    && bad "旧 tag 还在" || ok "旧 tag 清掉了"
  docker volume ls -q | grep -q hunter_hunter_i13_orphan && bad "悬空卷还在" || ok "悬空卷清掉了"
  row "本项目的 6 个数据卷还在吗" "$(docker volume ls --filter "label=com.docker.compose.project=$P" -q | wc -l)"
  row "别人的卷还有几个" "$(docker volume ls --format '{{.Name}}' | grep -vc '^hunter_')"
  for i in $other_img; do
    docker image inspect "$i" >/dev/null 2>&1 && ok "别人的镜像 $i 还在" || bad "别人的镜像 $i 没了！"
  done
  wait_healthy && ok "六个服务仍然健康" || bad "服务出问题了"
  ;;

guard)
  say "守卫越界：每一条都必须被拒"
  n_pass=0; n_fail=0
  # I16 修：原来只看输出里有没有「拒绝 / 不在 / 没过」这几个词。
  # 那是**按措辞判**，而措辞随时会变 —— 「要删除得逐字输入『删除应用和数据』」
  # 明明就是拒绝，却一个词都没命中，于是这一条从 I13 起一直在报假失败。
  # 现在**先看退出码**（拒绝一定非 0），措辞只作为补充线索。
  check() { # $1=说明 $2...=命令
    local what="$1"; shift
    local out rc
    out=$("$@" 2>&1); rc=$?
    if [ $rc -ne 0 ] || echo "$out" | grep -qE '拒绝|不在|没过|逐字输入'; then
      ok "$what（退出码 $rc）"; n_pass=$((n_pass+1))
    else
      bad "$what —— 没被拒！退出码 $rc：$out"; n_fail=$((n_fail+1))
    fi
  }
  # 1) 删应用要逐字输入
  check "空确认词删不了" "$BIN" --uninstall all --confirm "" -y
  # 2) 备份目录不能设在工作目录里（走的是 Rust 的那道校验）
  say "（下面几条走的是 Rust 单测覆盖的守卫，这里再在真二进制上确认一次）"
  # 3) 别的项目的卷：own_volume 必须认出来它不是我们的
  other=$(docker volume ls --format '{{.Name}}' | grep '^hunter-community' | head -1)
  row "拿来试的别人的卷" "${other:-（没有）}"
  # 通过 --cleanup 不会碰到它，用 plan 输出确认
  "$BIN" --cleanup 2>&1 | grep -E '悬空卷|旧镜像' | head -4
  docker volume inspect "$other" -f '{{index .Labels "com.docker.compose.project"}}' 2>/dev/null
  # 4) 别人的定时任务
  systemctl --user is-enabled hunter-backup.timer 2>&1 | head -1
  row "通过" "$n_pass"; row "没通过" "$n_fail"
  ;;

regress)
  say "I5–I12 回归"
  "$BIN" --boot-state
  echo; "$BIN" --monitor
  echo; "$BIN" --data-check
  echo; "$BIN" --check-net
  echo; "$BIN" --status
  ;;

*)
  sed -n '3,30p' "$0"
  ;;
esac
