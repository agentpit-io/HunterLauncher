#!/usr/bin/env python3
"""把内置运行时的四个组件同步到腾讯云香港桶，并逐个按清单校验（I7）。

用法：
    runtime-mirror.py --check           # 只检查：官方地址与桶里的 sha256 是否都对得上清单
    runtime-mirror.py --sync            # 缺的传上去（已存在且校验通过的跳过）

清单的**唯一来源**是 `src-tauri/src/runtime/manifest.rs`——这里用正则把它读出来，
不另抄一份。抄一份就一定会有哪天只改了一边（这个项目已经在别处栽过两次）。
解析不出预期条数会直接失败，而不是「少同步几个」。

凭据从环境变量读：COS_SECRET_ID / COS_SECRET_KEY / COS_BUCKET / COS_REGION。
**缺任何一个就跳过并以 0 退出**（沿用 cos-upload.py 与签名步骤的同一条原则）。
"""
import argparse
import hashlib
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "src-tauri" / "src" / "runtime" / "manifest.rs"
# 清单里现在有 8 条（macOS 的 4 个组件 × 2 个架构）。改清单时这个数也要跟着改 ——
# 它是一道「我真的读全了吗」的看门断言，不是装饰
EXPECTED = 8

ITEM = re.compile(
    r"Item\s*\{\s*"
    r'component:\s*"(?P<component>[^"]+)",\s*'
    r'version:\s*"(?P<version>[^"]+)",\s*'
    r'os:\s*"(?P<os>[^"]+)",\s*'
    r'arch:\s*"(?P<arch>[^"]+)",\s*'
    r'file:\s*"(?P<file>[^"]+)",\s*'
    r'url:\s*"(?P<url>[^"]+)",\s*'
    r'(?:(?://[^\n]*\n\s*)*)'
    r'sha256:\s*"(?P<sha256>[0-9a-f]{64})",\s*'
    r"size:\s*(?P<size>[0-9_]+),",
    re.S,
)


def items():
    text = MANIFEST.read_text(encoding="utf-8")
    out = [m.groupdict() for m in ITEM.finditer(text)]
    for it in out:
        it["size"] = int(it["size"].replace("_", ""))
        it["key"] = f"runtime/{it['component']}/{it['version']}/{it['file']}"
    if len(out) != EXPECTED:
        print(f"::error::清单解析出 {len(out)} 条，期望 {EXPECTED} 条 —— manifest.rs 的写法变了？")
        sys.exit(1)
    return out


def sha256_url(url: str) -> tuple[str, int]:
    """流式下载并算 sha256，不落盘（CI 的磁盘不必为此多占 100 MB）。"""
    h = hashlib.sha256()
    n = 0
    with tempfile.NamedTemporaryFile() as f:
        r = subprocess.run(
            ["curl", "-fsSL", "--max-time", "1800", "-o", f.name, url],
            capture_output=True,
        )
        if r.returncode != 0:
            return ("", -1)
        with open(f.name, "rb") as fh:
            while chunk := fh.read(1 << 20):
                h.update(chunk)
                n += len(chunk)
    return (h.hexdigest(), n)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--sync", action="store_true")
    a = ap.parse_args()
    if not (a.check or a.sync):
        print(__doc__)
        return 2

    base = os.environ.get("CN_DOWNLOAD_BASE", "https://hunter-dl-hk-1253756459.cos.ap-hongkong.myqcloud.com")
    its = items()
    print(f"清单里 {len(its)} 个文件，合计 {sum(i['size'] for i in its):,} 字节")

    bad = []
    need_upload = []
    for it in its:
        got, n = sha256_url(f"{base}/{it['key']}")
        if got == it["sha256"]:
            print(f"  ✓ 桶里的 {it['key']} 与清单一致（{n:,} B）")
            continue
        if n < 0:
            print(f"  · 桶里还没有 {it['key']}")
        else:
            print(f"  ✗ 桶里的 {it['key']} sha256 对不上：期望 {it['sha256']}，实际 {got}")
            bad.append(it["key"])
        need_upload.append(it)

    if a.check:
        if bad:
            print(f"::error::桶里有 {len(bad)} 个文件与清单不一致，必须查清楚再说")
            return 1
        if need_upload:
            print(f"::warning::桶里还缺 {len(need_upload)} 个文件，跑 --sync 补上")
        return 0

    missing = [k for k in ("COS_SECRET_ID", "COS_SECRET_KEY", "COS_BUCKET", "COS_REGION") if not os.environ.get(k)]
    if missing:
        print(f"跳过同步：没有 {'、'.join(missing)}")
        return 0
    if not need_upload:
        print("桶里已经齐了，什么都不用做")
        return 0

    up = ROOT / "scripts" / "cos-upload.py"
    with tempfile.TemporaryDirectory() as d:
        for it in need_upload:
            local = Path(d) / it["file"]
            print(f"  ↓ 从官方地址取 {it['file']}")
            r = subprocess.run(["curl", "-fsSL", "--max-time", "1800", "-o", str(local), it["url"]])
            if r.returncode != 0:
                print(f"::error::下载 {it['url']} 失败")
                return 1
            h = hashlib.sha256(local.read_bytes()).hexdigest()
            if h != it["sha256"]:
                # **官方地址上的内容和清单对不上**：要么上游重打了包，要么路上出了事。
                # 两种都不该悄悄传上去 —— 桶里的副本必须与清单逐字节相同
                print(f"::error::{it['file']} 从官方地址下回来的 sha256 是 {h}，清单写的是 {it['sha256']}，不同步")
                return 1
            r = subprocess.run([sys.executable, str(up), str(local), it["key"]])
            if r.returncode != 0:
                return 1
    print("同步完成")
    return 0


if __name__ == "__main__":
    sys.exit(main())
