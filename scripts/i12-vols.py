#!/usr/bin/env python3
"""从 `docker system df -v --format '{{json .Volumes}}'` 里只挑出本项目的卷。I12 验收脚本用。"""
import json
import sys

for v in json.load(sys.stdin) or []:
    labels = (v.get("Labels") or "") + ","
    if "com.docker.compose.project=hunter," in labels:
        print(f'  {v["Name"]:34s} {v["Size"]}')
