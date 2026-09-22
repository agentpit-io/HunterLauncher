#!/usr/bin/env python3
"""改 launcher.toml 里的一个布尔项（没有就加到 [install] 下面）。I12 验收脚本用。"""
import re
import sys

path, key, value = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(path, encoding="utf-8").read()
if re.search(rf"(?m)^{re.escape(key)} = ", s):
    s = re.sub(rf"(?m)^{re.escape(key)} = .*$", f"{key} = {value}", s)
else:
    s = re.sub(r"(?m)^(\[install\]\n)", rf"\1{key} = {value}\n", s, count=1)
open(path, "w", encoding="utf-8").write(s)
