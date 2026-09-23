#!/usr/bin/env python3
"""i13-verify.sh 用的 launcher.toml 小改写器（只动一行，不重排文件）。

用法：
  i13-toml.py <文件> get  <段> <键>
  i13-toml.py <文件> set  <段> <键> <值>

为什么不用 python-tomlkit：测试机上没有，而这里只需要「找到 [段] 下面那一行 键 = 值
并把它换掉」。写不回去的情况（段不存在）会在段末追加一行。

值的写法按类型猜：true/false 与纯数字不加引号，别的加双引号 ——
和 Rust 侧 `toml::to_string_pretty` 写出来的一致。
"""
import sys


def quote(v: str) -> str:
    if v in ("true", "false"):
        return v
    try:
        int(v)
        return v
    except ValueError:
        return '"' + v.replace('\\', '\\\\').replace('"', '\\"') + '"'


def main() -> int:
    path, op, section, key = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
    lines = open(path, encoding="utf-8").read().splitlines()
    head = f"[{section}]"
    in_sec = False
    idx = None
    sec_end = None
    for i, l in enumerate(lines):
        st = l.strip()
        if st.startswith("["):
            if in_sec and sec_end is None:
                sec_end = i
            in_sec = st == head
            continue
        if in_sec and st.split("=")[0].strip() == key:
            idx = i
    if in_sec and sec_end is None:
        sec_end = len(lines)

    if op == "get":
        if idx is None:
            print("")
            return 0
        v = lines[idx].split("=", 1)[1].strip()
        print(v.strip('"'))
        return 0

    val = sys.argv[5]
    new = f"{key} = {quote(val)}"
    if idx is not None:
        lines[idx] = new
    elif sec_end is not None:
        lines.insert(sec_end, new)
    else:
        lines += ["", head, new]
    open(path, "w", encoding="utf-8").write("\n".join(lines) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
