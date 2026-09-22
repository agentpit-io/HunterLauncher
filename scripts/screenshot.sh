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
# 截图前等多久。WebKitGTK 在软件渲染下首帧比较慢，宁可多等
SETTLE="${SETTLE:-9}"

# I5：向导改成「欢迎 → key → 一次授权 → 选模型 → 自动安装 → 完成」，
# 「拉取镜像」那一页没有了，换成 consent 与 auto 两页（见 src/state/machine.ts 文件头）
# I7 新增三页：auto-review（复核卡片）、auto-takeover-offer（「直接用它」那张不阻塞的卡片）、
# takeover（接管态的运行面板）
# I7 又新增两页：dashboard-lan / settings-lan（升级上来、网页端口还对局域网开着的老机器）
# I9 新增一页：error-builtin —— 上一次没装成功留下的内置运行时残骸
# （用户 Mac 上 0.1.8 那次的现场），同时也是「启动器已经替你做了这几步」那一块的样子
# I11 新增一页：error-recovered —— 错误页复查发现「其实已经好了」，
# 界面自己切到运行面板（截出来的是那一跳之后顶上那条绿色横幅）
PAGES=(welcome key consent model auto auto-need-user auto-review auto-takeover-offer docker docker-missing start done dashboard dashboard-quota-exhausted dashboard-lan takeover settings settings-lan logs feedback update error error-builtin error-recovered)

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
  sleep "$SETTLE"

  if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "进程已退出，日志："; cat "/tmp/hunter-shot-$page.log"; exit 1
  fi

  # 找到窗口并按它的几何裁剪，避免把 Xvfb 的黑边也截进去
  WID=$(xdotool search --name "Hunter Launcher" 2>/dev/null | tail -1 || true)
  if [ -n "$WID" ]; then
    import -window "$WID" "$OUT/$PREFIX-$page.png"
  else
    import -window root -crop "${W}x${H}+0+0" +repage "$OUT/$PREFIX-$page.png"
  fi

  kill "$APP_PID" 2>/dev/null || true
  wait "$APP_PID" 2>/dev/null || true
  sleep 1

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
