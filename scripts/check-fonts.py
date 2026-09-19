#!/usr/bin/env python3
"""
检查「有没有新加的中文没被字体子集覆盖」。CI 里跑，**不需要 fontTools**。

背景：启动器的字体是本地打包的**子集**（见 scripts/build-fonts.py），
只含仓库里出现过的字形。加一句新中文却忘了重新生成子集，结果不是报错，
而是界面上多一个豆腐块 —— M3 的第一张运行面板截图上「平台数据□给已配置」
的那个方框就是这么来的（那次漏的是 Rust 源码里的字，当时的脚本只扫了 src/）。

做法：按和 build-fonts.py 完全相同的规则重新收一遍字符集，
与生成时记下的 src/assets/fonts/charset.txt 对账。多出来的字就是没覆盖的。

用法：python3 scripts/check-fonts.py
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from importlib import util as _util

_spec = _util.spec_from_file_location(
    "build_fonts", os.path.join(os.path.dirname(os.path.abspath(__file__)), "build-fonts.py")
)
_bf = _util.module_from_spec(_spec)
_spec.loader.exec_module(_bf)


def main() -> int:
    if not os.path.exists(_bf.CHARSET_OUT):
        print(f"找不到 {_bf.CHARSET_OUT}，先跑一次 scripts/build-fonts.py", file=sys.stderr)
        return 1
    with open(_bf.CHARSET_OUT, encoding="utf-8") as f:
        # 只去掉行尾的换行 —— 不能用 strip()，字符集里第一个字符就是半角空格
        recorded = set("".join(l.rstrip("\n") for l in f if not l.startswith("#")))
    current = set(_bf.collect_chars())
    missing = sorted(current - recorded)
    if missing:
        print("以下字符出现在源码里，但字体子集没覆盖：", file=sys.stderr)
        print("".join(missing), file=sys.stderr)
        print(
            "\n重新生成：/tmp/fonttools-venv/bin/python scripts/build-fonts.py"
            "\n（装 fontTools：python3 -m venv /tmp/fonttools-venv && "
            "/tmp/fonttools-venv/bin/pip install fonttools brotli）",
            file=sys.stderr,
        )
        return 1
    print(f"OK：{len(current)} 个字符全部被字体子集覆盖")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
