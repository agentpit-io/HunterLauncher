#!/usr/bin/env python3
"""清掉 COS 下载桶里已经没人会去下的旧安装包（待办池 P1-17）。

用法：
    cos-prune.py                 真删
    cos-prune.py --dry-run       只列出会删什么，不动手

凭据与 cos-upload.py 同一套环境变量：COS_SECRET_ID / COS_SECRET_KEY / COS_BUCKET / COS_REGION。
**缺任何一个就跳过并以 0 退出** —— 没配 secrets 的 fork 上不该让流水线红。

## 为什么要清

`release.yml` 只往 `/launcher/<版本>/` 加东西，从不删。一个版本的六个安装包加起来
100 MB 出头，发十几个 rc 之后桶里会堆几个 GB，而其中绝大多数没有任何人会再去下。

## 删什么、不删什么

规则只有两条，都往「宁可留着」的方向偏 —— 删错了会让已经拿到链接的人 404，
而留着只是多占点钱不多的存储：

1. **正式版一个都不删。** `0.1.0`、`0.1.2` 这种不带后缀的，永远留着。
2. **预发布版（`0.1.0-rc.2` 这种）只在「它的正式版已经发出来了」时才删。**
   正式版一出，对应的那几个 rc 就彻底没用了 —— 谁也不会特地去装 `0.1.0-rc.1`。
   正式版还没发的，全留着（那是当前正在测的东西）；另外**无论如何保留最新的那个
   预发布版**，免得某次误判把正在用的包删掉。

`launcher/latest.json` 本身不在任何版本目录里，规则上碰不到它；代码里还是显式排除一次。

## 这个脚本不认识的东西一律不动

桶里除了 `launcher/` 还可能放别的（镜像同步的产物之类）。只扫 `launcher/<看起来像版本号>/`
这一层，别的前缀连列都不列。
"""
import os
import re
import sys

PREFIX = "launcher/"
# 无论如何保留的预发布版个数（按版本序从新到旧）。
# 取 1 而不是更大：真正的保险是上面那条「正式版还没发的预发布版一个都不删」，
# 这一条只是防止规则本身算错时把「最新的那个包」也一起带走。
KEEP_RECENT = 1

# 0.1.0 / 0.1.0-rc.2 / 1.2.3-beta.10
VERSION_RE = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$")


def parse(v: str):
    """版本字符串 → (主, 次, 补, 预发布段或 None)；不是版本号就返回 None。"""
    m = VERSION_RE.match(v)
    if not m:
        return None
    return (int(m[1]), int(m[2]), int(m[3]), m[4])


def sort_key(v: str):
    """排序用。预发布版排在同号正式版**前面**（semver 的规则）。"""
    p = parse(v)
    assert p is not None
    major, minor, patch, pre = p
    if pre is None:
        return (major, minor, patch, 1, ())
    # 预发布段按点号分段比，数字段按数值比，其余按字符串比
    segs = tuple((0, int(s), "") if s.isdigit() else (1, 0, s) for s in pre.split("."))
    return (major, minor, patch, 0, segs)


def decide(versions):
    """输入桶里现有的版本号列表，返回 (要留的, 要删的)。纯函数，有单测。"""
    known = [v for v in versions if parse(v) is not None]
    finals = {v for v in known if parse(v)[3] is None}
    final_bases = {".".join(str(x) for x in parse(v)[:3]) for v in finals}

    pres = sorted((v for v in known if parse(v)[3] is not None), key=sort_key, reverse=True)
    recent = set(pres[:KEEP_RECENT])

    drop = []
    for v in pres:
        base = ".".join(str(x) for x in parse(v)[:3])
        if base in final_bases and v not in recent:
            drop.append(v)
    keep = [v for v in known if v not in set(drop)]
    return sorted(keep, key=sort_key), sorted(drop, key=sort_key)


def self_test() -> int:
    """规则本身的测试。CI 里跑一次 —— 这个脚本会真删东西，逻辑不能只靠眼看。"""
    cases = [
        # （桶里现有的版本, 应当被删的）
        # 0.1.0 已经发了正式版 → 它的 rc 只留最新那个
        (["0.1.0-rc.1", "0.1.0-rc.2", "0.1.0", "0.1.1"], ["0.1.0-rc.1"]),
        (
            ["0.1.0-rc.1", "0.1.0-rc.2", "0.1.0-rc.3", "0.1.0", "0.1.1"],
            ["0.1.0-rc.1", "0.1.0-rc.2"],
        ),
        # 只有一个 rc、正式版已发：仍然留着（KEEP_RECENT 的保险）
        (["0.1.0-rc.1", "0.1.0"], []),
        (["0.2.0-rc.1", "0.2.0-rc.2", "0.2.0-rc.3", "0.2.0-rc.4"], []),  # 正式版还没发，一个不删
        (["1.0.0", "1.1.0", "2.0.0"], []),  # 正式版永远不删
        ([], []),
    ]
    bad = 0
    for versions, want in cases:
        keep, drop = decide(versions)
        if drop != sorted(want, key=sort_key):
            print(f"  ✗ {versions} → 删 {drop}，期望 {want}")
            bad += 1
        elif sorted(keep + drop, key=sort_key) != sorted(versions, key=sort_key):
            print(f"  ✗ {versions} → 留+删 对不上原始清单")
            bad += 1
        else:
            print(f"  ✓ {versions} → 删 {drop or '（无）'}")
    # 预发布版排在同号正式版前面
    assert sort_key("0.1.0-rc.2") < sort_key("0.1.0"), "预发布版必须排在正式版前面"
    assert sort_key("0.1.0-rc.2") < sort_key("0.1.0-rc.10"), "rc 段的数字要按数值比"
    assert parse("latest.json") is None and parse("0.1") is None
    print("cos-prune 自测：全部通过" if not bad else f"cos-prune 自测：{bad} 条不通过")
    return 1 if bad else 0


def main() -> int:
    if "--self-test" in sys.argv[1:]:
        return self_test()
    dry = "--dry-run" in sys.argv[1:]
    missing = [
        k
        for k in ("COS_SECRET_ID", "COS_SECRET_KEY", "COS_BUCKET", "COS_REGION")
        if not os.environ.get(k)
    ]
    if missing:
        print(f"跳过 COS 清理：没有 {'、'.join(missing)}（fork 或未配 secrets 的仓库属于正常情况）")
        return 0

    import boto3
    from botocore.config import Config

    bucket, region = os.environ["COS_BUCKET"], os.environ["COS_REGION"]
    s3 = boto3.client(
        "s3",
        endpoint_url=f"https://cos.{region}.myqcloud.com",
        aws_access_key_id=os.environ["COS_SECRET_ID"],
        aws_secret_access_key=os.environ["COS_SECRET_KEY"],
        region_name=region,
        config=Config(s3={"addressing_style": "virtual"}, signature_version="s3v4"),
    )

    # 列出 launcher/ 下的所有对象，按「版本目录」归组
    by_version: dict[str, list[tuple[str, int]]] = {}
    loose = 0
    token = None
    while True:
        kw = {"Bucket": bucket, "Prefix": PREFIX}
        if token:
            kw["ContinuationToken"] = token
        r = s3.list_objects_v2(**kw)
        for o in r.get("Contents", []):
            rest = o["Key"][len(PREFIX):]
            if "/" not in rest:
                loose += 1  # launcher/latest.json 这类，永远不碰
                continue
            ver = rest.split("/", 1)[0]
            if parse(ver) is None:
                loose += 1
                continue
            by_version.setdefault(ver, []).append((o["Key"], o["Size"]))
        if not r.get("IsTruncated"):
            break
        token = r.get("NextContinuationToken")

    if not by_version:
        print(f"{PREFIX} 下没有看得懂的版本目录，什么都不做（另有 {loose} 个对象不在规则范围内）")
        return 0

    keep, drop = decide(list(by_version))
    print(f"桶 {bucket} · {PREFIX} 下共 {len(by_version)} 个版本，另有 {loose} 个对象不在规则范围内")
    print(f"  保留 {len(keep)}：{'、'.join(keep) or '（无）'}")
    print(f"  清理 {len(drop)}：{'、'.join(drop) or '（无）'}")
    if not drop:
        print("没有要删的。规则：正式版全留；预发布版只在同号正式版已发布、"
              f"且不是最新的 {KEEP_RECENT} 个时才删。")
        return 0

    keys = [k for v in drop for k, _ in by_version[v]]
    freed = sum(n for v in drop for _, n in by_version[v])
    for k in keys:
        print(f"  {'将删' if dry else '删'} {k}")
    if dry:
        print(f"--dry-run：没有真删。会释放 {freed:,} 字节。")
        return 0
    # **一个一个删，不用批量的 DeleteObjects**。
    # 实测（2026-09-20 第一次真跑）：COS 对批量删除要求带 `Content-MD5` 头，
    # 而 boto3 不带，于是整批直接失败：
    #     InvalidRequest: Missing required header for this request: Content-MD5
    # 这和 M4 撞到的「COS 不支持 boto3 的分片上传」是同一类问题 —— S3 兼容不等于全兼容。
    # 单个 `DeleteObject` 没有这个要求。我们一次最多也就几十个对象，逐个删完全够。
    failed = 0
    for k in keys:
        try:
            s3.delete_object(Bucket=bucket, Key=k)
        except Exception as e:  # noqa: BLE001 —— 删不掉一个不该让整条流水线红
            failed += 1
            print(f"::warning::删不掉 {k}：{e}")
    print(f"清理完成：{len(keys) - failed} 个对象，释放约 {freed:,} 字节"
          + (f"；{failed} 个没删掉" if failed else ""))
    return 0


if __name__ == "__main__":
    sys.exit(main())
