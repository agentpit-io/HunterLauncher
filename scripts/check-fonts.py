#!/usr/bin/env python3
"""
检查「界面上可能显示的中文，字体子集有没有全覆盖」。CI 里跑，**不需要 fontTools**。

背景：启动器的字体是本地打包的**子集**（见 `scripts/build-fonts.py`），
只含仓库里出现过的字形。加一句新中文却忘了重新生成子集，结果不是报错，
而是界面上多一个豆腐块 —— M3 的第一张运行面板截图上「平台数据□给已配置」
的那个方框就是这么来的（当时的生成脚本只扫了 `src/`，而那句话是 Rust 拼的）。

## 这个检查和生成脚本是**两套口径**，这是有意的

| | 生成脚本 `build-fonts.py` | 这个检查 |
|---|---|---|
| 前端 `.ts/.tsx/.css/.html` | 全文 | 全文 |
| Rust `.rs` | 去掉**整行**注释之后的全文（行尾注释还留着） | 只取**字符串字面量**（用 `rust_strings.py` 里的小扫描器） |

生成的是**超集**，要求的是**子集** —— 所以正常情况下检查必然通过；
一旦生成脚本的那个「去整行注释」的近似判断出了偏差（比如哪天有人写了一行以 `//` 开头的
跨行字符串续行），这个用真扫描器做的检查就会把它抓出来。

用法：python3 scripts/check-fonts.py
"""
import os
import re
import sys
from importlib import util as _util

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)

from rust_strings import string_literals  # noqa: E402

_spec = _util.spec_from_file_location("build_fonts", os.path.join(HERE, "build-fonts.py"))
_bf = _util.module_from_spec(_spec)
_spec.loader.exec_module(_bf)

CJK_RE = re.compile(r"[㐀-䶿一-鿿豈-﫿]")


def chars_that_can_reach_the_ui() -> set:
    """界面上**可能显示**的字符集合。"""
    chars = set(_bf.BASE)
    paths = [os.path.join(ROOT, "index.html")]
    for dirpath, _dirs, files in os.walk(_bf.SRC):
        if os.path.join("src", "assets") in dirpath:
            continue
        paths += [
            os.path.join(dirpath, f)
            for f in files
            if f.endswith((".ts", ".tsx", ".css", ".html"))
        ]
    for path in paths:
        with open(path, encoding="utf-8") as f:
            chars |= set(CJK_RE.findall(f.read()))

    # Rust：只有字符串字面量能上屏，注释不能
    for dirpath, _dirs, files in os.walk(os.path.join(ROOT, "src-tauri", "src")):
        for f in files:
            if not f.endswith(".rs"):
                continue
            with open(os.path.join(dirpath, f), encoding="utf-8") as fh:
                for lit in string_literals(fh.read()):
                    chars |= set(CJK_RE.findall(lit))
    return chars


def main() -> int:
    if not os.path.exists(_bf.CHARSET_OUT):
        print(f"找不到 {_bf.CHARSET_OUT}，先跑一次 scripts/build-fonts.py", file=sys.stderr)
        return 1
    with open(_bf.CHARSET_OUT, encoding="utf-8") as f:
        # 只去掉行尾的换行 —— 不能用 strip()，字符集里第一个字符就是半角空格
        recorded = set("".join(l.rstrip("\n") for l in f if not l.startswith("#")))

    needed = chars_that_can_reach_the_ui()
    missing = sorted(needed - recorded)
    if missing:
        print("以下字符可能显示在界面上，但字体子集没覆盖：", file=sys.stderr)
        print("".join(missing), file=sys.stderr)
        print(
            "\n重新生成：/tmp/fonttools-venv/bin/python scripts/build-fonts.py"
            "\n（装 fontTools：python3 -m venv /tmp/fonttools-venv && "
            "/tmp/fonttools-venv/bin/pip install fonttools brotli）",
            file=sys.stderr,
        )
        return 1
    print(f"OK：界面上可能出现的 {len(needed)} 个字符全部被字体子集（{len(recorded)} 个）覆盖")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
