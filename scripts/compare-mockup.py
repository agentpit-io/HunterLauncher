#!/usr/bin/env python3
"""
把 Xvfb 截出来的实现截图与视觉稿并排拼成对照图。

视觉稿是 2 倍图且四周有画布留白，先按窗体位置裁出来（(80,70)-(2280,1450)），
再和实现截图缩到同一高度并排。标注用 ASCII，
避免在没有中文字体的机器上画成豆腐块。

用法：python3 scripts/compare-mockup.py [截图目录] [输出目录]
"""
import os
import sys

from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MOCK_DIR = os.path.join(ROOT, "docs", "design")
FONT = os.path.join(ROOT, "src", "assets", "fonts", "noto-sans-sc-subset.woff2")

# 视觉稿里窗体的位置（2360×1520 的图上）
WIN_BOX = (80, 70, 2280, 1450)
TARGET_H = 860
LABEL_H = 34
BG = (5, 8, 15)
FG = (142, 166, 194)

PAIRS = [
    ("m1-key.png", "hunter-launcher-01-输入key.jpg", "对照-01-输入key.png"),
    ("m1-pull.png", "hunter-launcher-02-拉取镜像.jpg", "对照-02-拉取镜像.png"),
    ("m1-dashboard.png", "hunter-launcher-03-运行面板.jpg", "对照-03-运行面板.png"),
]


def load_font(size: int):
    # woff2 PIL 读不了，退回默认位图字体；标题只有四个字，糊一点不影响判读
    for path in ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",):
        if os.path.exists(path):
            try:
                return ImageFont.truetype(path, size)
            except OSError:
                pass
    return ImageFont.load_default()


def scaled(im: Image.Image, h: int) -> Image.Image:
    w = round(im.width * h / im.height)
    return im.resize((w, h), Image.LANCZOS)


def labelled(im: Image.Image, text: str, font) -> Image.Image:
    out = Image.new("RGB", (im.width, im.height + LABEL_H), BG)
    out.paste(im, (0, LABEL_H))
    d = ImageDraw.Draw(out)
    d.text((10, 9), text, fill=FG, font=font)
    return out


def main() -> int:
    shots = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "docs", "screenshots", "M1")
    out_dir = sys.argv[2] if len(sys.argv) > 2 else os.path.join(shots, "对照")
    os.makedirs(out_dir, exist_ok=True)
    font = load_font(20)

    for shot_name, mock_name, out_name in PAIRS:
        shot_path = os.path.join(shots, shot_name)
        mock_path = os.path.join(MOCK_DIR, mock_name)
        for p in (shot_path, mock_path):
            if not os.path.exists(p):
                print(f"缺文件：{p}", file=sys.stderr)
                return 1

        mock = scaled(Image.open(mock_path).convert("RGB").crop(WIN_BOX), TARGET_H)
        shot = scaled(Image.open(shot_path).convert("RGB"), TARGET_H)
        a = labelled(mock, "mockup (plan/UI)", font)
        b = labelled(shot, "M1 build (Xvfb screenshot)", font)

        gap = 16
        canvas = Image.new("RGB", (a.width + gap + b.width + gap * 2, a.height + gap * 2), BG)
        canvas.paste(a, (gap, gap))
        canvas.paste(b, (gap + a.width + gap, gap))
        dest = os.path.join(out_dir, out_name)
        canvas.save(dest)
        print(f"  {dest}  {canvas.width}×{canvas.height}")

    print(f"完成：{out_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
