#!/usr/bin/env python3
"""把 Rust 源码里的**字符串字面量**抠出来。

给 `check-fonts.py` 用：界面上可能显示的 Rust 文案只可能来自字符串字面量，
注释再多也不会上屏。用一个小扫描器而不是正则，是因为正则分不清
「注释里的引号」和「真的字符串」—— 第一版用正则，被注释里的一个引号带跑，
误报了 20 个字（其中「踩」「违」「盯」全都只出现在注释里）。

处理的东西：普通字符串（含 `\\` 转义）、`r"…"` / `r#"…"#` 原始字符串（任意个 `#`）、
字符字面量、`//` 行注释、`/* */` 块注释（Rust 的块注释可以嵌套）。
"""
from typing import List


def string_literals(src: str) -> List[str]:
    out: List[str] = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]

        # 行注释
        if c == "/" and i + 1 < n and src[i + 1] == "/":
            j = src.find("\n", i)
            i = n if j < 0 else j + 1
            continue

        # 块注释（可嵌套）
        if c == "/" and i + 1 < n and src[i + 1] == "*":
            depth, i = 1, i + 2
            while i < n and depth:
                if src.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif src.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            continue

        # 原始字符串 r"…" / r#"…"# / br#"…"#
        if c in "rb":
            k = i
            if src[k] == "b" and k + 1 < n and src[k + 1] == "r":
                k += 1
            if src[k] == "r":
                h = k + 1
                while h < n and src[h] == "#":
                    h += 1
                if h < n and src[h] == '"':
                    hashes = "#" * (h - k - 1)
                    end = src.find('"' + hashes, h + 1)
                    if end < 0:
                        break
                    out.append(src[h + 1 : end])
                    i = end + 1 + len(hashes)
                    continue

        # 普通字符串
        if c == '"':
            j = i + 1
            buf = []
            while j < n:
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == '"':
                    break
                buf.append(src[j])
                j += 1
            out.append("".join(buf))
            i = j + 1
            continue

        # 字符字面量：跳过，免得 '"' 把后面的代码当成字符串
        if c == "'":
            if src.startswith("'\\", i):
                j = src.find("'", i + 2)
                i = n if j < 0 else j + 1
                continue
            if i + 2 < n and src[i + 2] == "'":
                i += 3
                continue
            i += 1  # 生命周期标注 'a，照常往下走
            continue

        i += 1
    return out
