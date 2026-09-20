#!/usr/bin/env bash
# 在无桌面环境的机器上用 Xvfb 驱动**真实的**启动器做 GUI 回归。
#
# 和 `screenshot.sh` 的分工：
#   screenshot.sh   演示模式、按页定位、只截图，给「界面长什么样」用；
#   gui-session.sh  真实数据、用 xdotool 一步步点，给「点下去真的会发生什么」用。
#
# 用法（一个会话里多次调用，状态放在 /tmp）：
#   BIN=/usr/bin/hunter-launcher bash scripts/gui-session.sh start ~/.hunter
#   bash scripts/gui-session.sh click 895 678 5     # 点一下，等 5 秒
#   bash scripts/gui-session.sh changed             # 断言界面真的变了
#   bash scripts/gui-session.sh shot out/10-运行面板.png
#   bash scripts/gui-session.sh stop
#
# 需要 Xvfb、xdotool、imagemagick（compare / import / convert）。
set -u

export DISPLAY="${DISPLAY_NUM:-:99}"
export WEBKIT_DISABLE_COMPOSITING_MODE=1
export WEBKIT_DISABLE_DMABUF_RENDERER=1
BIN="${BIN:-/usr/bin/hunter-launcher}"
WIDF=/tmp/hunter-gui-wid
BEFORE=/tmp/hunter-gui-before.png
AFTER=/tmp/hunter-gui-after.png

case "${1:-}" in
  start)
    pkill -f "Xvfb $DISPLAY" 2>/dev/null; pkill -f "$BIN" 2>/dev/null; sleep 1
    Xvfb "$DISPLAY" -screen 0 "${SCREEN:-1300x880x24}" >/dev/null 2>&1 &
    sleep 2
    HUNTER_HOME="${2:-$HOME/.hunter}" nohup "$BIN" > /tmp/hunter-gui-app.log 2>&1 &
    for _ in $(seq 1 40); do
      WID=$(xdotool search --name "Hunter Launcher" 2>/dev/null | tail -1)
      [ -n "${WID:-}" ] && break
      sleep 0.5
    done
    [ -z "${WID:-}" ] && { echo "窗口没出来："; cat /tmp/hunter-gui-app.log; exit 1; }
    echo "$WID" > $WIDF
    xdotool windowmove "$WID" 0 0; xdotool windowactivate "$WID" 2>/dev/null
    sleep 3
    echo "WID=$WID  BIN=$BIN  HUNTER_HOME=${2:-$HOME/.hunter}"
    ;;

  click)
    # 三个坑（M2 驱动 GUI 时逐个踩出来的，别改）：
    #  1) 必须先把指针挪开再挪过去，WebKit 要一次真实的 motion 才认这一下；
    #  2) 不能用 `xdotool click`（按下与抬起挨得太近会被吞掉），要 mousedown/sleep/mouseup；
    #  3) 不能用 --window 发点击（那是 XSendEvent 合成事件，WebKit 一律忽略）。
    WID=$(cat $WIDF)
    import -window "$WID" $BEFORE 2>/dev/null
    xdotool mousemove --window "$WID" 620 620
    sleep 0.3
    xdotool mousemove --window "$WID" "$2" "$3"
    sleep 0.5
    xdotool mousedown 1
    sleep 0.15
    xdotool mouseup 1
    sleep "${4:-1}"
    ;;

  # 「点完界面真的变了吗」（I2 报告第十节第 5 条的建议，I3 做出来的）。
  #
  # 为什么需要它：坐标是按截图量出来的，布局一改就会点空 —— 而点空之后**后面每一步
  # 都还会"成功"**，只是一直停在同一页上。I2 是靠最后一步的截图去重才发现的，
  # 前面整整一轮白跑；I3 第一遍的日志浮层也是这样，这一道当场就喊了出来。
  #
  # 判据是点击前后两张图的 RMSE：完全一致（0）就是没反应。
  # 注意它只能证明「有反应」，不能证明「反应对」—— 那还是要人看截图。
  changed)
    WID=$(cat $WIDF)
    import -window "$WID" $AFTER 2>/dev/null
    D=$(compare -metric RMSE $BEFORE $AFTER null: 2>&1 | sed 's/ .*//')
    D=${D%%.*}
    if [ -z "$D" ] || [ "$D" = "0" ]; then
      echo "    ⚠ 界面和点击前一模一样（RMSE=$D）—— 这一下大概率点空了"
      exit 1
    fi
    echo "    ✓ 界面变了（RMSE=$D）"
    ;;

  shot)
    WID=$(cat $WIDF)
    import -window "$WID" "$2"
    identify -format "%wx%h 颜色数 %k " "$2"
    convert "$2" -colorspace sRGB -format "亮度 %[fx:int(mean*1000)]‰ " info:
    convert "$2" -colorspace HSL -channel g -separate +channel -format "饱和度 %[fx:int(mean*1000)]‰\n" info:
    ;;

  type)
    # 不能用 --window（XSendEvent），也不能先 windowfocus（会把 DOM 里的焦点从输入框上打掉）。
    # 正确做法：靠前一步的点击拿到焦点，然后直接往当前焦点打字。
    xdotool type --delay 20 "$2"
    sleep 1
    ;;

  key) xdotool key "$2"; sleep "${3:-1}" ;;
  log) tail -40 /tmp/hunter-gui-app.log ;;
  stop)
    pkill -f "$BIN" 2>/dev/null
    pkill -f "Xvfb $DISPLAY" 2>/dev/null
    echo stopped
    ;;
  *) sed -n '2,20p' "$0"; exit 2 ;;
esac
