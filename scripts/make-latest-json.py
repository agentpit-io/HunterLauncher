#!/usr/bin/env python3
"""从三平台的构建产物拼出 Tauri updater 的 `latest.json`（技术方案 §10）。

用法：
    make-latest-json.py <产物目录> <版本> <下载地址前缀> <输出文件> [--notes 文件] [--pub-date ISO8601]

产物目录里要有各平台的**可更新包**与它的 `.sig`：

    hunter-launcher_<版本>_amd64.AppImage      + .sig   → linux-x86_64
    hunter-launcher_<版本>_x64-setup.exe       + .sig   → windows-x86_64
    hunter-launcher_<版本>_universal.app.tar.gz + .sig  → darwin-x86_64 / darwin-aarch64

注意 macOS 那一条：updater 用的是 `.app.tar.gz`，**不是** `.dmg`（dmg 要用户手动拖，
不可能就地替换）。通用二进制一个包同时喂给两个 target 键，这是 Tauri 官方的写法。

`.deb` 不在清单里：Tauri 的 updater 在 Linux 上只能就地替换 AppImage，
`.deb` 装在 /usr 下要 root。deb 用户走的是 selfupdate.rs 里那条「下好 + 给一条命令」的路，
下载地址按固定规则拼，不读这个清单。

生成两份的用法：GitHub 一份（地址指向 Release 资产），COS 一份（地址指向香港下载桶），
差别只有 `<下载地址前缀>` 这个参数。
"""
import argparse
import json
import sys
from pathlib import Path

# 文件名后缀 → Tauri updater 的 target 键。一个包可以对应多个键（通用二进制）
TARGETS = [
    ("_amd64.AppImage", ["linux-x86_64"]),
    ("_x64-setup.exe", ["windows-x86_64"]),
    ("_universal.app.tar.gz", ["darwin-x86_64", "darwin-aarch64"]),
]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("dist")
    ap.add_argument("version")
    ap.add_argument("base_url", help="下载地址前缀，不带末尾斜杠")
    ap.add_argument("out")
    ap.add_argument("--notes", default=None, help="Release Notes 文件；没有就留空字符串")
    ap.add_argument("--pub-date", default=None, help="ISO 8601；不给就不写这个字段")
    a = ap.parse_args()

    dist = Path(a.dist)
    platforms: dict[str, dict[str, str]] = {}
    seen: list[str] = []
    for suffix, keys in TARGETS:
        pkg = next((p for p in sorted(dist.rglob(f"*{suffix}")) if p.is_file()), None)
        if pkg is None:
            continue
        sig = pkg.with_name(pkg.name + ".sig")
        if not sig.exists():
            # 有包没签名 = updater 装不了它。宁可让流水线红，也不要发一个装不上的清单
            print(f"::error::{pkg.name} 没有对应的 .sig（tauri.conf.json 的 createUpdaterArtifacts 开了吗？）")
            return 1
        for k in keys:
            platforms[k] = {
                "signature": sig.read_text().strip(),
                "url": f"{a.base_url.rstrip('/')}/{pkg.name}",
            }
        seen.append(pkg.name)

    if not platforms:
        print(f"::error::{dist} 里一个可更新包都没有找到")
        return 1

    manifest: dict[str, object] = {
        "version": a.version,
        "notes": Path(a.notes).read_text().strip() if a.notes and Path(a.notes).exists() else "",
        "platforms": platforms,
    }
    if a.pub_date:
        manifest["pub_date"] = a.pub_date

    Path(a.out).write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    print(f"写出 {a.out}")
    for name in seen:
        print(f"  {name}")
    print(f"  平台键：{'、'.join(sorted(platforms))}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
