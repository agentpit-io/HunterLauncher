#!/usr/bin/env python3
"""
从三张视觉稿里逐像素量出设计令牌（颜色、字号、间距、圆角、结构尺寸）。

src/styles/tokens.css 里的每一个数值都是这个脚本量出来的，改令牌前先跑一遍它对账。

方法：
  · 颜色：对纯色区域取**众数**（避开 JPEG 压缩噪点），对文字取区域内**最亮像素**
    （文字是抗锯齿的，峰值最接近真实字色）。
  · 尺寸：沿一行/一列扫亮度跃迁找边界。视觉稿是 2 倍图（2360×1520 对应约 1180×760
    逻辑像素），所以所有像素测量值 ÷2 才是写进令牌的逻辑像素。

用法：python3 scripts/sample-mockup.py [视觉稿目录]
默认目录：../plan/UI 或 docs/design
"""
import os
import sys
from collections import Counter

try:
    from PIL import Image
except ImportError:
    print("需要 Pillow：pip install Pillow", file=sys.stderr)
    raise SystemExit(1)

FILES = {
    "01": "hunter-launcher-01-输入key.jpg",
    "02": "hunter-launcher-02-拉取镜像.jpg",
    "03": "hunter-launcher-03-运行面板.jpg",
}


def hexof(rgb):
    return "#%02x%02x%02x" % rgb


def mode_color(im, box, label, top=2):
    data = list(im.crop(box).getdata())
    total = len(data)
    parts = " | ".join(f"{hexof(c)}({100 * n / total:.0f}%)" for c, n in Counter(data).most_common(top))
    print(f"{label:24s} {parts}")


def brightest(im, box, label):
    data = sorted(im.crop(box).getdata(), key=sum)
    print(f"{label:24s} 最亮 {hexof(data[-1])}")


def edges_x(gray, y, x0, x1, th=8):
    px = gray.load()
    return [x for x in range(x0 + 1, x1) if abs(px[x, y] - px[x - 1, y]) > th]


def edges_y(gray, x, y0, y1, th=8):
    px = gray.load()
    return [y for y in range(y0 + 1, y1) if abs(px[x, y] - px[x, y - 1]) > th]


def main() -> int:
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    candidates = [
        sys.argv[1] if len(sys.argv) > 1 else None,
        os.path.join(here, "..", "plan", "UI"),
        os.path.join(here, "docs", "design"),
    ]
    base = next((d for d in candidates if d and os.path.exists(os.path.join(d, FILES["01"]))), None)
    if base is None:
        print("找不到视觉稿目录", file=sys.stderr)
        return 1

    rgb = {k: Image.open(os.path.join(base, v)).convert("RGB") for k, v in FILES.items()}
    gray = {k: v.convert("L") for k, v in rgb.items()}
    i1, i3 = rgb["01"], rgb["03"]

    print("=== 底色与边框（图 01） ===")
    mode_color(i1, (10, 10, 60, 60), "窗外底色 bg-app")
    mode_color(i1, (600, 100, 1000, 135), "窗口/内容区 bg-window")
    mode_color(i1, (200, 1000, 400, 1100), "侧栏 bg-sidebar")
    mode_color(i1, (1000, 840, 1140, 880), "卡片/输入框 bg-card")
    mode_color(rgb["02"], (690, 1215, 2170, 1340), "日志框 bg-log")
    print("卡片左边框（y=800，逐像素）:",
          [hexof(i1.getpixel((x, 800))) for x in range(676, 684)])
    print("次按钮边框（图 03，y=1298）:",
          [hexof(i3.getpixel((x, 1298))) for x in range(1428, 1434)])

    print("\n=== 琥珀金与灰蓝 ===")
    mode_color(i1, (1990, 1300, 2180, 1360), "主按钮 amber")
    mode_color(i1, (1990, 556, 2100, 620), "已验证底 amber-soft")
    mode_color(i1, (140, 608, 165, 635), "当前步骤点", 3)
    mode_color(i1, (140, 368, 165, 392), "已完成步骤点", 3)
    mode_color(rgb["02"], (1020, 850, 1950, 865), "完成态进度条 slate-done")
    mode_color(rgb["02"], (1750, 650, 1950, 665), "进度条轨道 slate-track")

    print("\n=== 文字 ===")
    brightest(i1, (676, 255, 1160, 300), "主标题 text")
    brightest(rgb["02"], (676, 630, 900, 665), "次级标题 text-2")
    brightest(i1, (680, 345, 1750, 375), "正文 text-body")
    brightest(rgb["02"], (700, 1255, 1580, 1285), "日志 text-dim")
    brightest(i3, (1420, 780, 1560, 810), "环境键 text-label")
    brightest(rgb["02"], (676, 668, 830, 700), "副标题 text-muted")
    brightest(i1, (205, 265, 390, 295), "侧栏 slogan text-faint")
    brightest(i1, (190, 360, 300, 395), "已完成步骤名 step-done")
    brightest(i1, (676, 1305, 940, 1345), "链接 amber-text")

    print("\n=== 结构尺寸（÷2 = 逻辑像素） ===")
    g1 = gray["01"]
    print("窗口左右边缘:", edges_x(g1, 760, 0, 120, 5), edges_x(g1, 760, 2200, 2359, 5))
    print("窗口上下边缘:", edges_y(g1, 1200, 0, 120, 5), edges_y(g1, 1200, 1350, 1519, 5))
    print("标题栏下边线:", edges_y(g1, 1500, 120, 220))
    print("侧栏分隔线  :", edges_x(g1, 1100, 500, 700))
    print("统计卡 x 边界:", [e for e in edges_x(g1, 780, 600, 2300, 7) if e < 700 or e > 1150][:12])
    print("输入框 y 边界:", edges_y(g1, 1900, 500, 680, 7))
    print("主按钮 y 边界:", [edges_y(g1, 2100, 1250, 1420, 10)[0], edges_y(g1, 2100, 1250, 1420, 10)[-1]])
    print("底部分隔线  :", edges_y(g1, 1000, 1200, 1300, 6))
    print("进度条高(图02):", edges_y(gray["02"], 1200, 640, 680, 25))

    print("\n=== 圆角（看左上角每一行的左边界收敛速度） ===")
    px = g1.load()
    for y in range(678, 700, 2):
        xs = [x for x in range(670, 720) if px[x, y] > 24]
        print(f"  卡片 y={y}: 左边界 {xs[0] if xs else '-'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
