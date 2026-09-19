#!/usr/bin/env python3
"""
生成本地打包的字体子集（总控规则：字体本地打包，启动器要离线可用）。

做法：扫描 src/ 下所有源码里出现过的字符，从 Google Fonts 仓库取两套**可变字体**的原始 TTF，
用 fontTools 按这份字符集做子集化 + woff2 压缩，产物存进 src/assets/fonts/，
同时写出 src/styles/fonts.css。

为什么是子集而不是整套：Noto Sans SC 完整一档就有 10 MB 上下，会把 .deb 从 2.3 MB 撑到几十 MB；
而启动器的界面文案是**有限且写死在仓库里**的，子集足够。
子集没覆盖到的字（例如 Docker 报错里冒出来的生僻字）按 tokens.css 里列的 font-family
回落到系统中文字体，不会变成豆腐块。

为什么用可变字体：一个文件就覆盖 400/500/600/700 全部字重，比每档一个文件小得多。

字体许可：Noto Sans SC 与 JetBrains Mono 均为 SIL Open Font License 1.1，
许可证全文随字体一起放在 src/assets/fonts/ 下（OFL 要求随分发附带）。

依赖（只有重新生成字体时才需要，CI 与日常开发都不需要 —— 产物已提交进仓库）：
    python3 -m venv /tmp/fonttools-venv && /tmp/fonttools-venv/bin/pip install fonttools brotli
运行：
    /tmp/fonttools-venv/bin/python scripts/build-fonts.py
"""
import os
import re
import sys
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "src")
OUT_DIR = os.path.join(SRC, "assets", "fonts")
CACHE = os.path.join("/tmp", "hunter-font-cache")
CSS_OUT = os.path.join(SRC, "styles", "fonts.css")
CHARSET_OUT = os.path.join(OUT_DIR, "charset.txt")

GF = "https://raw.githubusercontent.com/google/fonts/main"

# 一定要覆盖的基础集合：ASCII 可打印 + 中文标点 + 界面里用到的符号
BASE = set(chr(c) for c in range(0x20, 0x7F))
BASE |= set("·—–…、。，；：？！（）〈〉《》「」『』【】“”‘’％×÷≤≥≈→←↑↓✓✕•°")

CJK_RE = re.compile(r"[㐀-䶿一-鿿豈-﫿]")

FONTS = [
    {
        "name": "Noto Sans SC",
        "url": f"{GF}/ofl/notosanssc/NotoSansSC%5Bwght%5D.ttf",
        "src": "NotoSansSC-var.ttf",
        "out": "noto-sans-sc-subset.woff2",
        "license": ("LICENSE-Noto-Sans-SC.txt", f"{GF}/ofl/notosanssc/OFL.txt"),
        "cjk": True,
    },
    {
        "name": "JetBrains Mono",
        "url": f"{GF}/ofl/jetbrainsmono/JetBrainsMono%5Bwght%5D.ttf",
        "src": "JetBrainsMono-var.ttf",
        "out": "jetbrains-mono-subset.woff2",
        "license": ("LICENSE-JetBrains-Mono.txt", f"{GF}/ofl/jetbrainsmono/OFL.txt"),
        "cjk": False,  # 等宽只用来显示数字、路径、日志、镜像名，拉丁就够
    },
]


def collect_chars() -> str:
    """把界面上**可能显示出来**的中文字符全收进来。

    两个来源，缺一不可：

    1. `src/` 与 `index.html` —— 前端自己的文案；
    2. **`src-tauri/src/*.rs`** —— Rust 侧的字符串。这一条是 M3 补的：
       错误说明、诊断包的小标题、「平台数据供给已配置」这类由 Rust 拼好再交给界面显示的话，
       都不在前端源码里。漏了它们的结果是界面上冒出豆腐块 —— M3 的第一张运行面板截图上
       「平台数据□给已配置」的那个方框就是这么来的。

    Rust 源码里的注释也会被一起扫进来。多几百个字形对 woff2 的体积影响很小
    （中文子集本来就以千计），而漏一个字就是界面上一个方块，宁可多扫。
    """
    chars = set(BASE)
    paths = [os.path.join(ROOT, "index.html")]
    for dirpath, _dirs, files in os.walk(SRC):
        if os.path.join("src", "assets") in dirpath:
            continue
        paths += [os.path.join(dirpath, f) for f in files if f.endswith((".ts", ".tsx", ".css", ".html"))]
    rust_src = os.path.join(ROOT, "src-tauri", "src")
    for dirpath, _dirs, files in os.walk(rust_src):
        paths += [os.path.join(dirpath, f) for f in files if f.endswith(".rs")]
    for path in paths:
        with open(path, encoding="utf-8") as f:
            chars |= set(CJK_RE.findall(f.read()))
    return "".join(sorted(chars))


def download(url: str, dest: str) -> str:
    if os.path.exists(dest):
        return dest
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    print(f"  下载 {url}")
    urllib.request.urlretrieve(url, dest)
    return dest


def main() -> int:
    try:
        from fontTools import subset
    except ImportError:
        print("缺少 fontTools，见本文件顶部注释里的安装命令", file=sys.stderr)
        return 1

    os.makedirs(OUT_DIR, exist_ok=True)
    text = collect_chars()
    cjk = len(CJK_RE.findall(text))
    print(f"字符集：{len(text)} 个（其中汉字 {cjk} 个）")

    # 把这一次用到的字符集记下来，给 scripts/check-fonts.py 在 CI 里对账用。
    # 没有它的话，「加了一句中文却忘了重新生成字体」只有等到有人截图时才会发现
    # —— M3 的第一张运行面板截图上那个豆腐块就是这么来的。
    with open(CHARSET_OUT, "w", encoding="utf-8") as f:
        f.write("# 由 scripts/build-fonts.py 生成。记录字体子集覆盖了哪些字符，\n")
        f.write("# scripts/check-fonts.py 用它检查「有没有新加的中文没被字体覆盖」。\n")
        f.write(text + "\n")

    css = [
        "/* 由 scripts/build-fonts.py 生成，请勿手改。",
        "   两个都是可变字体的子集，一个文件覆盖全部字重；",
        "   子集只含界面用到的字形，未覆盖的字回落到 tokens.css 里列的系统中文字体。",
        "   Noto Sans SC 与 JetBrains Mono 均为 SIL OFL 1.1，许可证全文见 src/assets/fonts/。 */",
        "",
    ]

    for font in FONTS:
        raw = download(font["url"], os.path.join(CACHE, font["src"]))
        chars = text if font["cjk"] else "".join(sorted(BASE))
        out = os.path.join(OUT_DIR, font["out"])
        subset.main([
            raw,
            f"--text={chars}",
            "--flavor=woff2",
            f"--output-file={out}",
            "--layout-features=kern,liga,calt,tnum",
            "--no-hinting",
            "--desubroutinize",
            "--name-IDs=1,2,3,4,5,6",
            "--notdef-outline",
            "--recommended-glyphs",
        ])
        size = os.path.getsize(out)
        print(f"  {font['out']:30s} {size / 1024:7.1f} KB")
        css.append(f"""@font-face {{
  font-family: '{font["name"]}';
  font-style: normal;
  font-weight: 100 900;
  font-display: block;
  src: url('../assets/fonts/{font["out"]}') format('woff2');
}}
""")

        lic_name, lic_url = font["license"]
        lic_path = os.path.join(OUT_DIR, lic_name)
        if not os.path.exists(lic_path):
            urllib.request.urlretrieve(lic_url, lic_path)
            print(f"  {lic_name}")

    with open(CSS_OUT, "w", encoding="utf-8") as f:
        f.write("\n".join(css))
    print(f"写出 {CSS_OUT}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
