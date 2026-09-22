#!/usr/bin/env python3
"""把覆盖文件退回 0.1.11 的样子（去掉重启策略与只为它存在的 llm-shim 段）。I12 验收脚本用。"""
import re
import sys

path = sys.argv[1]
s = open(path, encoding="utf-8").read()
s = re.sub(r"(?m)^ *restart: unless-stopped\n", "", s)
s = re.sub(r"(?m)^  llm-shim:\n", "", s)
s = re.sub(r"(?m)^#       4\) .*\n", "", s)
open(path, "w", encoding="utf-8").write(s)
