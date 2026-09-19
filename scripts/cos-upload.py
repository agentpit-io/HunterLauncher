#!/usr/bin/env python3
"""把文件传到腾讯云 COS（香港区下载桶）。

用法：
    cos-upload.py <本地文件或目录> <桶内前缀>
    cos-upload.py dist/ launcher/0.1.0/
    cos-upload.py latest.json launcher/latest.json

凭据从环境变量读：COS_SECRET_ID / COS_SECRET_KEY / COS_BUCKET / COS_REGION。
**缺任何一个就跳过并以 0 退出** —— 没配 secrets 的 fork 上，同步这一步不该让整条流水线红
（与签名步骤同一条原则，见总控规则红线 7）。

为什么用 boto3 而不是 coscmd：COS 支持 S3 兼容接口，boto3 在 runner 上是现成的，
少装一个包。**必须用 virtual-hosted 寻址**（`<桶>.cos.<区>.myqcloud.com/<键>`）——
COS 明确拒绝 path-style（会回 `PathStyleDomainForbidden`），这一点实测过。

地址与目录约定见 plan/国内镜像与下载源.md：
    /launcher/latest.json            国内版自更新清单
    /launcher/<版本>/<安装包>        三平台安装包与 .sig
"""
import mimetypes
import os
import sys
from pathlib import Path

REQUIRED = ["COS_SECRET_ID", "COS_SECRET_KEY", "COS_BUCKET", "COS_REGION"]

# 安装包不能被浏览器当网页打开，json/yml 要能直接读
CONTENT_TYPES = {
    ".json": "application/json; charset=utf-8",
    ".yml": "text/yaml; charset=utf-8",
    ".yaml": "text/yaml; charset=utf-8",
    ".sig": "text/plain; charset=utf-8",
    ".deb": "application/vnd.debian.binary-package",
    ".appimage": "application/octet-stream",
    ".exe": "application/octet-stream",
    ".msi": "application/octet-stream",
    ".dmg": "application/octet-stream",
    ".gz": "application/gzip",
}


def content_type(p: Path) -> str:
    ext = p.suffix.lower()
    if ext in CONTENT_TYPES:
        return CONTENT_TYPES[ext]
    return mimetypes.guess_type(p.name)[0] or "application/octet-stream"


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    src, prefix = Path(sys.argv[1]), sys.argv[2]

    missing = [k for k in REQUIRED if not os.environ.get(k)]
    if missing:
        print(f"跳过 COS 同步：没有 {'、'.join(missing)}（fork 或未配 secrets 的仓库属于正常情况）")
        return 0
    if not src.exists():
        print(f"::error::要上传的路径不存在：{src}")
        return 1

    import boto3
    from botocore.config import Config
    from boto3.s3.transfer import TransferConfig

    bucket, region = os.environ["COS_BUCKET"], os.environ["COS_REGION"]
    s3 = boto3.client(
        "s3",
        endpoint_url=f"https://cos.{region}.myqcloud.com",
        aws_access_key_id=os.environ["COS_SECRET_ID"],
        aws_secret_access_key=os.environ["COS_SECRET_KEY"],
        region_name=region,
        # COS 只认 virtual-hosted 寻址，path-style 会被拒（PathStyleDomainForbidden）
        config=Config(s3={"addressing_style": "virtual"}, signature_version="s3v4"),
    )

    # **必须关掉分片上传**。boto3 默认超过 8 MB 就切成 multipart，而 COS 的 UploadPart
    # 要求带 Content-Length，boto3 不带 —— 于是大文件必然失败：
    #   (MissingContentLength) when calling the UploadPart operation
    # 78 MB 的 AppImage 第一次发 rc.1 就是栽在这里。
    # 单次 PutObject 在 COS 上支持到 5 GB，我们最大的包才 80 MB，够用。
    no_multipart = TransferConfig(multipart_threshold=5 * 1024**3, multipart_chunksize=5 * 1024**3)

    files = sorted(p for p in src.rglob("*") if p.is_file()) if src.is_dir() else [src]
    base = f"https://{bucket}.cos.{region}.myqcloud.com"
    total = 0
    for f in files:
        key = f"{prefix.rstrip('/')}/{f.relative_to(src).as_posix()}" if src.is_dir() else prefix
        s3.upload_file(
            str(f), bucket, key,
            ExtraArgs={"ContentType": content_type(f)},
            Config=no_multipart,
        )
        n = f.stat().st_size
        total += n
        print(f"  ↑ {key}  ({n:,} B)  {base}/{key}")
    print(f"COS 同步完成：{len(files)} 个文件，{total:,} 字节")
    return 0


if __name__ == "__main__":
    sys.exit(main())
