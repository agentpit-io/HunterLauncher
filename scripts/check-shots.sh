#!/usr/bin/env bash
# 一批截图的自查。用法：bash scripts/check-shots.sh <目录> [另一个目录 …]
#
# 五道校验。前三道抓「看起来像一张正常图片的白屏」，后两道抓**截错了页**：
#
#   1) 颜色数太少          = 纯色块
#   2) 平均亮度太高        = 白屏（本项目的界面是深海军蓝，亮度一定很低）
#   3) 饱和度近似 0        = 灰度图，说明底色没上（#0a101c 不是灰色）
#   4) 两张内容完全一样    = 多半是点击没生效，同一页截了好几次
#   5) 和「开机那一屏」几乎一样 = 这一页压根没渲染出来（I16 加的，见下）
#
# 第四道是 I1 收尾时加进建议里的（I1 报告第十节第 6 条）：那一轮第一遍跑出来的
# 四张浮层截图 **md5 完全相同**（坐标点空了，四次都截了同一页），而前三道校验
# 全都通过 —— 它们只能抓白屏，抓不到「截了同一页四次」。
#
# **第五道是 I16 加的**（待办池 P2-36）。本轮新加两页演示态时，
# `store.tsx` 里那张「演示页名 → 状态机状态」的表忘了登记，两页就停在开机那一屏；
# 前四道**全过** —— 那两张既不是纯色、亮度饱和度都正常，md5 也各不相同
# （开机那一屏上有个转着的东西，每次截到的相位都不一样）。
# 按 md5 去重抓不到「这一页其实没渲染出来」，按「和 booting 那张有多像」就抓得到。
#
# 需要 imagemagick。
set -euo pipefail

[ $# -ge 1 ] || { echo "用法：bash scripts/check-shots.sh <目录> [目录 …]"; exit 2; }

bad=0
for dir in "$@"; do
  [ -d "$dir" ] || { echo "目录不存在：$dir"; exit 2; }
  mapfile -t files < <(find "$dir" -maxdepth 1 -name '*.png' | sort)
  if [ ${#files[@]} -eq 0 ]; then echo "$dir 下一张 png 都没有"; exit 2; fi
  echo "== $dir（${#files[@]} 张）=="

  for f in "${files[@]}"; do
    size=$(stat -c%s "$f")
    colors=$(identify -format %k "$f")
    mean=$(convert "$f" -colorspace sRGB -format "%[fx:int(mean*1000)]" info:)
    sat=$(convert "$f" -colorspace HSL -channel g -separate +channel -format "%[fx:int(mean*1000)]" info:)
    printf '   %-52s %8sB  颜色数 %-6s 亮度 %s‰  饱和度 %s‰\n' "$(basename "$f")" "$size" "$colors" "$mean" "$sat"
    if [ "$colors" -lt 200 ]; then echo "   ✗ 颜色数只有 $colors，疑似没渲染出来"; bad=$((bad+1)); fi
    if [ "$mean" -gt 350 ]; then echo "   ✗ 平均亮度 ${mean}‰ 偏高，疑似白屏（样式没加载）"; bad=$((bad+1)); fi
    if [ "$sat" -lt 20 ]; then echo "   ✗ 平均饱和度 ${sat}‰ 近似灰度，疑似底色没上"; bad=$((bad+1)); fi
  done

  # ⑤ 和「开机那一屏」几乎一样的（I16 · 待办池 P2-36）。
  #    开机屏是唯一一张「哪一页都可能退化成它」的图：状态机没认出这个演示页名时，
  #    界面就停在那里。RMSE 归一化到 [0,1]，这条线是**实测定的**（I16，39 张图）：
  #      · 真的没渲染出来（拿 booting 改一个像素冒充）→ 1.1e-05
  #      · 和 booting 最像的一张真页（backup-restore）→ 0.075
  #    取 0.03：比误报的下限低 2.5 倍，比漏报的上限高三个数量级。
  boot=$(find "$dir" -maxdepth 1 -name '*-booting.png' | head -1)
  if [ -n "$boot" ]; then
    for f in "${files[@]}"; do
      [ "$f" = "$boot" ] && continue
      # `compare` 图不一样时**退出码就是非 0**（那是它的正常输出方式，不是出错），
      # 而这个脚本开着 `set -e` —— 不接 `|| true` 的话第一张图就把脚本带走了
      rmse=$(compare -metric RMSE "$boot" "$f" null: 2>&1 || true)
      # **科学计数法也要认**：一模一样的两张图 compare 报的是 `0.718 (1.09562e-05)`，
      # 字符集里漏了 `e` 和 `-` 就匹配不上，于是 rmse 是空的、这一张被静悄悄跳过
      # —— 加这道检查的第一版就是这么漏掉那张冒充图的
      rmse=$(sed -n 's/.*(\([0-9.eE+-]*\)).*/\1/p' <<<"$rmse")
      [ -n "$rmse" ] || continue
      if awk -v v="$rmse" 'BEGIN{exit !(v+0 < 0.03)}'; then
        echo "   ✗ $(basename "$f") 和开机那一屏几乎一样（RMSE $rmse）—— 这一页多半没渲染出来"
        echo "     （新加演示页时先在 src/state/store.tsx 的 DEMO_STATES 里登记一条）"
        bad=$((bad+1))
      fi
    done
  fi

  # 内容完全一样的两张。按 md5 分组，任何一组里多于一个就是问题。
  dup=$(md5sum "${files[@]}" | sort | awk '{c[$1]=c[$1]" "$2; n[$1]++} END {for (h in n) if (n[h]>1) print "   ✗ 这几张内容完全一样：" c[h]}')
  if [ -n "$dup" ]; then
    echo "$dup"
    echo "   （多半是点击没生效，同一页被截了好几次 —— I1 第一遍就是这样）"
    bad=$((bad+1))
  fi
done

if [ "$bad" -gt 0 ]; then
  echo "截图自查：$bad 处不通过"
  exit 1
fi
echo "截图自查：全部通过（含「不许有两张一模一样」）"
