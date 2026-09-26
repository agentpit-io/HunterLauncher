#!/usr/bin/env python3
"""
**一个「manifest 正常、blob 一个字节都不给」的假 registry**（I16 的测试台）。

## 它复现的是什么

2026-09-25 客户 Mac 上的现场：升级 Hunter 1.2.0 → 1.2.2，
界面卡在「正在拉取新版本的镜像…」一个多小时不动。截图里能确定的：

* 前面每一步都成功了，而且**算出了「六个镜像合计约 748 MB」**；
* 然后就停在那里。

「748 MB」这个数字只有逐个读镜像 manifest 才算得出来 —— 说明到镜像仓库的
**元数据请求是通的**，卡住的是**拉层（blob）**那一步。这种连接不报错：
TCP 连上了、进程活着、就是不传字节。于是 `docker compose pull` 会一直等下去，
而启动器原来只认「报错 → 换源重试」，一次都没触发。

这个脚本就是那种服务器：

| 路径 | 行为 |
|---|---|
| `GET /v2/` | 200，握手通过 |
| `GET /v2/<repo>/manifests/<ref>` | **正常返回**一份合法的 manifest（层大小加起来几百 MB） |
| `GET|HEAD /v2/<repo>/blobs/<digest>` | 回 200 与 `Content-Length`，然后**一个字节都不发**，挂着不断开 |

docker 于是会把每一层都显示成 `Downloading  0B/…`，进度条有分母、没有分子 ——
和客户那台机器上看到的一模一样。

## 怎么用

```bash
python3 fake-stalling-registry.py 5999 [证书.pem 私钥.pem] &
hunter-launcher --headless --pull-only --registry 127.0.0.1:5999/agentpit
```

给了证书就起 HTTPS。**Docker Engine 29 必须走 HTTPS**：实测它对
`127.0.0.1:5999` 也直接发 TLS 握手，不再有「本机地址自动当成 insecure」
那条退路（原话：`tls: first record does not look like a TLS handshake`）。
让它信任自签证书的办法是把 CA 放到 `/etc/docker/certs.d/127.0.0.1:5999/ca.crt`
—— 这一步**不需要重启 docker daemon**（`scripts/i16-stall-test.sh` 就是这么做的）。

**只监听 127.0.0.1。** 这是一个会把连接挂死的服务，不该对外开放。
"""

import hashlib
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# 每一层假装有多大。六个镜像 × 4 层 × 31 MB ≈ 744 MB —— 量级照着客户那次的 748 MB 来，
# 好让日志里的数字看着就是同一类现场。
LAYER_BYTES = 31 * 1000 * 1000
LAYERS_PER_IMAGE = 4

def sha256(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


# 层的摘要是**编出来的**（用层序号填充）。这一条可以编：blob 永远不会传出
# 一个字节，docker 根本走不到校验层内容那一步。
#
# **manifest 与 config 的摘要不能编** —— 实测 Docker Engine 29 收到 manifest
# 之后立刻按内容算一遍 sha256 跟 `Docker-Content-Digest` 对账，对不上就报
# `unexpected commit digest …: failed precondition`，那是「拉取失败」，
# 不是我们要复现的「拉取卡住」。所以下面那两处都用真的 sha256。
def digest(repo: str, i: int) -> str:
    seed = f"{repo}:{i}".encode()
    h = 0
    for b in seed:
        h = (h * 131 + b) % (1 << 64)
    return "sha256:" + f"{h:016x}" * 4


def config_blob(repo: str) -> bytes:
    return json.dumps(
        {
            "architecture": "amd64",
            "os": "linux",
            "config": {},
            "rootfs": {"type": "layers", "diff_ids": [digest(repo, i) for i in range(LAYERS_PER_IMAGE)]},
        },
        separators=(",", ":"),
    ).encode()


def manifest(repo: str) -> bytes:
    cfg = config_blob(repo)
    return json.dumps(
        {
            "schemaVersion": 2,
            "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
            "config": {
                "mediaType": "application/vnd.docker.container.image.v1+json",
                "size": len(cfg),
                "digest": sha256(cfg),
            },
            "layers": [
                {
                    "mediaType": "application/vnd.docker.image.rootfs.diff.tar.gzip",
                    "size": LAYER_BYTES,
                    "digest": digest(repo, i),
                }
                for i in range(LAYERS_PER_IMAGE)
            ],
        },
        separators=(",", ":"),
    ).encode()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # noqa: D102
        sys.stderr.write("[fake-registry] %s\n" % (fmt % args))

    # ── 路由 ────────────────────────────────────────────────────────────
    def route(self):
        p = self.path.split("?")[0]
        if p in ("/v2", "/v2/"):
            return ("ping", None, None)
        if not p.startswith("/v2/"):
            return (None, None, None)
        rest = p[len("/v2/") :]
        for kind in ("manifests", "blobs"):
            marker = "/" + kind + "/"
            if marker in rest:
                repo, ref = rest.split(marker, 1)
                return (kind, repo, ref)
        return (None, None, None)

    def do_HEAD(self):
        self.handle_any(body=False)

    def do_GET(self):
        self.handle_any(body=True)

    def handle_any(self, body: bool):
        kind, repo, ref = self.route()
        if kind == "ping":
            self.send_response(200)
            self.send_header("Content-Length", "2")
            self.send_header("Docker-Distribution-Api-Version", "registry/2.0")
            self.end_headers()
            if body:
                self.wfile.write(b"{}")
            return

        if kind == "manifests":
            # config blob 也从这条路要（docker 会按 digest 去 blobs 取），
            # 但 manifest 本身必须**当场正常返回** —— 那正是「748 MB 算得出来」的原因
            data = manifest(repo)
            self.send_response(200)
            self.send_header("Content-Type", "application/vnd.docker.distribution.manifest.v2+json")
            self.send_header("Docker-Content-Digest", sha256(data))
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            if body:
                self.wfile.write(data)
            return

        if kind == "blobs":
            # config blob 要真给，否则 docker 在「拉层」之前就报错了 ——
            # 那就变成「拉取失败」，不是我们要复现的「拉取卡住」
            if ref == sha256(config_blob(repo)):
                data = config_blob(repo)
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                if body:
                    self.wfile.write(data)
                return
            # ── 这里就是那个病 ──
            # 200 + Content-Length 都给，然后**一个字节都不发**，也不断开。
            # docker 会显示 `Downloading 0B/31MB` 然后永远停在那里。
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(LAYER_BYTES))
            self.end_headers()
            self.log_message("blob %s 已接受连接，接下来一个字节都不发", ref[:19])
            try:
                while True:
                    time.sleep(3600)
            except Exception:
                return
            return

        self.send_response(404)
        self.send_header("Content-Length", "0")
        self.end_headers()


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 5999
    cert = sys.argv[2] if len(sys.argv) > 2 else None
    key = sys.argv[3] if len(sys.argv) > 3 else None
    srv = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    srv.daemon_threads = True
    scheme = "http"
    if cert and key:
        import ssl

        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        ctx.load_cert_chain(cert, key)
        srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
        scheme = "https"
    sys.stderr.write(
        f"[fake-registry] 监听 {scheme}://127.0.0.1:{port}（manifest 正常，blob 永远不吐字节）\n"
    )
    srv.serve_forever()


if __name__ == "__main__":
    main()
