#!/usr/bin/env bash
# 在无桌面环境的机器上用 Xvfb 真实启动启动器并逐页截图（总控规则「前端完成的标准」）。
#
# 用法：  bash scripts/screenshot.sh <输出目录>
# 前置：  可执行文件必须由 `VITE_DEMO=1 pnpm exec tauri build --no-bundle` 产出。
#         **不能**用裸的 `cargo build --release`：那样不带 custom-protocol 特性，
#         产物会去连 devUrl 而不是用内嵌前端，窗口就是一片白（M1 踩过这个坑）。
#         需要 Xvfb、x11-utils、xdotool、imagemagick
set -euo pipefail

OUT="${1:-docs/screenshots/M1}"
# 文件名前缀跟着输出目录走（docs/screenshots/M4 → m4-*.png）。
# M1~M3 都硬编码成 m1-，到 M4 就对不上了 —— 一个里程碑一套图，名字得看得出是哪一套
PREFIX="${PREFIX:-$(basename "$OUT" | tr 'A-Z' 'a-z')}"
BIN="${BIN:-src-tauri/target/release/hunter-launcher}"
W="${W:-1180}"
H="${H:-760}"
DISPLAY_NUM="${DISPLAY_NUM:-:99}"
# **等窗口出来的上限（秒）**，不是固定的 sleep 时长。WebKitGTK 在软件渲染下首帧很慢，
# 而且快慢随机器负载变化：2026-09-29 在 mixplode-hk-01 上实测，同一台机器同一页，
# 窗口出现要 **10~25 秒**、出现之后再等 **约 5 秒**才真的画出来（在那之前窗口是纯色）。
# 原来写死 `sleep 9`，截到的是一张纯黑的图，而 `check-shots.sh` 只会说
# 「颜色数只有 1」—— 它说得对，但说不出原因是「等得不够」。
# 现在三个等待全部改成**探到条件就走**：窗口出现 → 真的画出来（颜色数）→ resize 后重画。
SETTLE="${SETTLE:-45}"
# 上面每一步最多等多少秒
PAINT="${PAINT:-20}"

# I5：向导改成「欢迎 → key → 一次授权 → 选模型 → 自动安装 → 完成」，
# 「拉取镜像」那一页没有了，换成 consent 与 auto 两页（见 src/state/machine.ts 文件头）
# I7 新增三页：auto-review（复核卡片）、auto-takeover-offer（「直接用它」那张不阻塞的卡片）、
# takeover（接管态的运行面板）
# I7 又新增两页：dashboard-lan / settings-lan（升级上来、网页端口还对局域网开着的老机器）
# I9 新增一页：error-builtin —— 上一次没装成功留下的内置运行时残骸
# （用户 Mac 上 0.1.8 那次的现场），同时也是「启动器已经替你做了这几步」那一块的样子
# I11 新增一页：error-recovered —— 错误页复查发现「其实已经好了」，
# 界面自己切到运行面板（截出来的是那一跳之后顶上那条绿色横幅）
# I12 新增两页：booting（打开启动器的第一屏「正在检查 Hunter 状态」，
# 它在真机上停留不到 1 秒，但那正是「不再闪欢迎页」的证据，要留档）、
# data-found（容器没了、数据卷还在时的「检测到上次的数据」）。
# dashboard 那一页这一轮也变了 —— 上半部多了三层资源（R2）
# I13 新增五页：
#   dashboard-alert —— 运行面板顶上那条异常提醒（R7）
#   backup / backup-restore —— 「备份与恢复」那一页，以及它上面逐字输入「恢复数据」的弹窗（R6）
#   uninstall / uninstall-all —— 删除应用的两档（R4）。第二档要摆出数据概况与「先备份一次」
# I14 新增两页：key-kept —— 「只删除应用，保留数据」之后重装时的 key 页，
# 上面是「沿用上次保留的 key（hunt_tools_****）」那张卡片，输入框收起来（F3）；
# key-shape —— 留着的那一项不是一把 key 的样子，沿用不了，但界面上要说明为什么（F3 · 3.4）
# I16 新增四页：
#   error-stalled —— 「拉着拉着不动了」那一档（客户 Mac 上 0.1.15 的现场）。
#     和 error（E_START_TIMEOUT）分开截：这一档的说法完全不同，而且多一个「上传日志给我们」
#   upload-preview / upload-done —— 一键上传日志的两屏：
#     传之前把要传的东西原样摆出来；传完把追踪码用一行大字显示出来
#   dashboard-interrupted —— 强退之后留下的中间态（配置一版、容器另一版、镜像不齐）
# I16 追加两页（都来自 2026-09-26 那位 Windows 客户的诊断包）：
#   dashboard-stopped —— 六个容器全 exited。看的是顶上那行大字与副标题**不再打架**
#     （0.1.15 在这个现场写的是「Hunter 运行中」＋「v1.2.2 · 容器已停止」）
# R1 追加一页：
#   dashboard-runtime-down —— 用户 2026-09-28 报的那一幕：没起 Docker 打开软件。
#     看的是「不再被判成没装过」—— 进的是运行面板（不是欢迎页），
#     顶上一块说清「装过、只是运行时没在跑」，主按钮是「启动运行时」
#   dashboard-schedule-broken —— 定时备份根本没挂上的那条红横幅（0.1.15 界面上一个字都没有）
# I17 新增四页：
#   dashboard-interrupted-stopped —— **六个容器全停**的那种中间态（客户 2026-09-29 的现场
#     HL-GFV764）。0.1.17 恰好判不出这一格：卡片不出现，面板反而说「点『启动』就能用」。
#     和 dashboard-interrupted 分开截 —— 全停时回退目标与措辞都不一样
#   （`update-no-tag` 这一页**不在自动截图里**：演示模式下自动打开前置体检弹窗没做成，
#     截出来会和普通的更新页一模一样。那一张改成真点两下点出来的 `i17-update-preflight.png`，
#     见 I17 报告 8.3。页面本身还在（`HUNTER_DEMO_PAGE=update-no-tag` 能进），只是不进这一套图）
#   quit-guard —— 升级进行中点退出被拦下的那一句（P0-4）。U2 之后这是唯一会弹的提示
#   （dashboard 那一页这一轮也变了：起停/重启/检查更新/退出启动器 全在首屏一级按钮区）
# R5 新增两页（都是 U-03 选盘卡片）：
#   disk —— 有得选的样子：默认位置 / 为什么是它 / 要占多少 / 还剩多少 / 上限 vs 实占
#   disk-none —— **一块合格的盘都没有**（S-01 返回 None）。看的是界面有没有话说，
#     而不是静默失败或者瞎选一块
# 这一轮还改了三页既有页面：docker-missing（主按钮）、consent（三个勾）、
# settings（授权卡 + 人话审计），它们本来就在这套图里，跟着一起重截
# I18 新增两页（U5 升级过程要看得见网速与剩余时间）：
#   update-running —— 升级进行中：`已下 / 总计` + 百分比 + 当前网速 + 预计剩余
#     + 「本次已下载」（**用 netBytes，不是 downloadedBytes**）
#   update-retry —— 换源重试那一档（attempt=2）：出现「第 N 次尝试（已自动换源）」，
#     而 ETA 已经作废重算，显示的是「剩余时间未知」**而不是一个编出来的秒数**（A5-3/A5-4）
PAGES=(booting data-found welcome key key-kept key-shape consent model auto auto-need-user auto-review auto-takeover-offer docker docker-missing disk disk-none start done dashboard dashboard-alert dashboard-quota-exhausted dashboard-lan dashboard-stopped dashboard-runtime-down dashboard-schedule-broken dashboard-interrupted dashboard-interrupted-stopped quit-guard upload-preview upload-done backup backup-restore uninstall uninstall-all takeover settings settings-lan logs feedback update update-running update-retry error error-stalled error-builtin error-recovered)

# 只截指定的几页（改了一两页时不用把四十几页重跑一遍）：
#   ONLY="dashboard update" bash scripts/screenshot.sh docs/screenshots/I17
# 留空 = 全跑。**注意它只影响截哪几页**，`check-shots.sh` 仍然扫整个目录，
# 所以局部出图之后不要拿它当「这一套图是完整的」。
if [ -n "${ONLY:-}" ]; then read -r -a PAGES <<<"$ONLY"; fi

mkdir -p "$OUT"
[ -x "$BIN" ] || { echo "找不到可执行文件：$BIN"; exit 1; }

# Xvfb 的画布比窗口大一圈，这样窗口不会被裁掉；截完再按窗口几何裁出来
SCREEN_W=$((W + 120))
SCREEN_H=$((H + 120))

pkill -f "Xvfb $DISPLAY_NUM" 2>/dev/null || true
sleep 1
Xvfb "$DISPLAY_NUM" -screen 0 "${SCREEN_W}x${SCREEN_H}x24" >/dev/null 2>&1 &
XVFB_PID=$!
trap 'kill $XVFB_PID 2>/dev/null || true' EXIT
sleep 2

export DISPLAY="$DISPLAY_NUM"
# M0 §7.2：这两个变量在测试机上不是必须的，但它们是零成本的保险 ——
# 「白屏截图」这种故障在 CI 上只会表现成一张看起来正常的图片，极难发现
export WEBKIT_DISABLE_COMPOSITING_MODE=1
export WEBKIT_DISABLE_DMABUF_RENDERER=1

for page in "${PAGES[@]}"; do
  echo "== $page =="
  HUNTER_DEMO_PAGE="$page" "$BIN" >/tmp/hunter-shot-$page.log 2>&1 &
  APP_PID=$!

  # 等主窗口真的出来（最多 SETTLE 秒）。没出来就说清楚是等超时了，不要把黑屏当成
  # 「这一页长这样」交上去 —— 下面那道「颜色数只有 1」的校验只会说结果、说不出原因。
  WID=""
  for _ in $(seq 1 "$SETTLE"); do
    sleep 1
    kill -0 "$APP_PID" 2>/dev/null || break
    WID=$(xdotool search --name "Hunter Launcher" 2>/dev/null | tail -1 || true)
    [ -n "$WID" ] && break
  done
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "进程已退出，日志："; cat "/tmp/hunter-shot-$page.log"; exit 1
  fi
  if [ -z "$WID" ]; then
    echo "等了 $SETTLE 秒主窗口还没出来（应用日志）："; cat "/tmp/hunter-shot-$page.log"; exit 1
  fi

  # **把窗口摆成 W×H**：程序自己的 `inner_size` 是视觉稿的 1180×760，
  # 要不要按别的尺寸出图（例如 I17 要验「1440×900 首屏不滚动」）由调用方的 W/H 说了算。
  # 窗口是可缩放的（`min_inner_size` 980×660），X 下没有 WM 也照样 resize 得动。
  xdotool windowmove "$WID" 0 0 2>/dev/null || true
  xdotool windowsize "$WID" "$W" "$H" 2>/dev/null || true

  # **等它真的画出来**：判据是「颜色数 ≥ 200」。界面是深海军蓝 + 琥珀金，
  # 正常一页的颜色数在两千以上；纯色（还没画、或白屏）是 1。
  # 这两道等待都要，因为窗口一出现就截的话拿到的是一张纯色图。
  # 判据与下面 `check-shots.sh` 那三道用的是同一个数，不是另一把尺子。
  for _ in $(seq 1 "$PAINT"); do
    import -window "$WID" "$OUT/$PREFIX-$page.png" 2>/dev/null || true
    COLORSN=$(identify -format %k "$OUT/$PREFIX-$page.png" 2>/dev/null || echo 0)
    if [ "${COLORSN:-0}" -ge 200 ]; then break; fi
    sleep 1
  done
  if [ "${COLORSN:-0}" -lt 200 ]; then
    echo "等了 $PAINT 秒这一页还没画出来（颜色数 ${COLORSN:-0}）；应用日志："
    cat "/tmp/hunter-shot-$page.log"
    exit 1
  fi
  # 再稳一下：刚画出来的那一帧可能还在加载字体（中文会先出豆腐块再换真字）
  sleep 2
  import -window "$WID" "$OUT/$PREFIX-$page.png"

  kill "$APP_PID" 2>/dev/null || true
  wait "$APP_PID" 2>/dev/null || true
  # 等上一页的窗口真的消失再起下一页 —— 否则下面那次 `xdotool search` 会抓到
  # **正在死掉的那一页**，截出来的就是上一页的图（`check-shots.sh` 的「两张一模一样」
  # 只能告诉你出了这件事，说不出是哪一步抓错了）
  for _ in $(seq 1 10); do
    [ -z "$(xdotool search --name 'Hunter Launcher' 2>/dev/null)" ] && break
    sleep 1
  done

  SIZE=$(stat -c%s "$OUT/$PREFIX-$page.png")
  COLORS=$(identify -format %k "$OUT/$PREFIX-$page.png")
  MEAN=$(convert "$OUT/$PREFIX-$page.png" -colorspace sRGB -format "%[fx:int(mean*1000)]" info:)
  SAT=$(convert "$OUT/$PREFIX-$page.png" -colorspace HSL -channel g -separate +channel -format "%[fx:int(mean*1000)]" info:)
  echo "   $OUT/$PREFIX-$page.png  ${SIZE}B  颜色数 ${COLORS}  平均亮度 ${MEAN}‰  平均饱和度 ${SAT}‰"
  # 三道校验，专门用来抓「看起来像一张正常图片的白屏」：
  #   1) 颜色数太少 = 纯色块
  #   2) 平均亮度太高 = 白屏（本项目的界面是深海军蓝，亮度一定很低）
  #   3) 饱和度为 0 = 灰度图，说明底色没上（#0a101c 不是灰色）
  if [ "$COLORS" -lt 200 ]; then echo "颜色数只有 $COLORS，疑似没渲染出来"; exit 1; fi
  if [ "$MEAN" -gt 350 ]; then echo "平均亮度 ${MEAN}‰ 偏高，疑似白屏（样式没加载）"; exit 1; fi
  if [ "$SAT" -lt 20 ]; then echo "平均饱和度 ${SAT}‰ 近似灰度，疑似底色没上"; exit 1; fi
done

# 最后再整批过一遍 —— 单张的三道校验抓不到「同一页截了好几次」（I1 报告第十节第 6 条）
bash "$(dirname "$0")/check-shots.sh" "$OUT"

echo "全部完成：$OUT"
