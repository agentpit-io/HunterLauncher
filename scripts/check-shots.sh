#!/usr/bin/env bash
# 一批截图的自查。用法：bash scripts/check-shots.sh <目录> [另一个目录 …]
#
# 四道校验。前三道抓「看起来像一张正常图片的白屏」，第四道抓**截错了页**：
#
#   1) 颜色数太少        = 纯色块
#   2) 平均亮度太高      = 白屏（本项目的界面是深海军蓝，亮度一定很低）
#   3) 饱和度近似 0      = 灰度图，说明底色没上（#0a101c 不是灰色）
#   4) 两张内容完全一样  = 多半是点击没生效，同一页截了好几次
#
# 第四道是 I1 收尾时加进建议里的（I1 报告第十节第 6 条）：那一轮第一遍跑出来的
# 四张浮层截图 **md5 完全相同**（坐标点空了，四次都截了同一页），而前三道校验
# 全都通过 —— 它们只能抓白屏，抓不到「截了同一页四次」。
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
