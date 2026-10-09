#!/usr/bin/env bash
# 在**中继机**上执行：把「发安装包」与「收回传」两条通道铺好。
#
# 一般不用手跑 —— 由 scripts/windows-verify-env.sh relay-up 送上来执行。
# 唯一需要人记住的约定：安装包放进 /tmp/hl/，测试机就从
# http://<中继机内网IP>:8000/<文件名> 取；测试机把进度与结果 PUT 到
# http://<中继机内网IP>:8001/p/<名字>，落盘在 /tmp/hl/rep/<名字>。
set -euo pipefail

DIR=/tmp/hl
mkdir -p "$DIR/rep"

cat > "$DIR/sink.py" <<'PYEOF'
#!/usr/bin/env python3
# 结果收集器：测试机把每一步的进度与文件 PUT 到这里，原样落盘到 /tmp/hl/rep/
import http.server, socketserver, os, datetime, urllib.parse

D = '/tmp/hl/rep'
os.makedirs(D, exist_ok=True)


class H(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def _log(self, m):
        try:
            with open(os.path.join(D, '_access.log'), 'a') as f:
                f.write('%s %s\n' % (datetime.datetime.utcnow().isoformat() + 'Z', m))
        except Exception:
            pass

    def _store(self):
        try:
            p = urllib.parse.urlparse(self.path).path
            name = p.rstrip('/').split('/')[-1] or 'index'
            name = ''.join(c for c in name if c.isalnum() or c in '._-') or 'x'
            n = int(self.headers.get('Content-Length') or 0)
            body = self.rfile.read(n) if n else b''
            with open(os.path.join(D, name), 'wb') as f:
                f.write(body)
            self._log('PUT %s -> %s (%d bytes)' % (p, name, len(body)))
        except Exception as e:
            self._log('ERR %r' % (e,))
        self.send_response(200)
        self.send_header('Content-Length', '2')
        self.end_headers()
        self.wfile.write(b'ok')

    def do_PUT(self):
        self._store()

    def do_POST(self):
        self._store()

    def do_GET(self):
        self._log('GET %s' % self.path)
        self.send_response(200)
        self.send_header('Content-Length', '2')
        self.end_headers()
        self.wfile.write(b'ok')

    def log_message(self, *a):
        pass


socketserver.ThreadingTCPServer.allow_reuse_address = True
srv = socketserver.ThreadingTCPServer(('0.0.0.0', 8001), H)
srv.serve_forever()
PYEOF

# 清掉上一轮的进程（这两条 pkill 写在文件里，是为了避开「命令串自匹配、把自己 ssh 会话杀掉」的坑）
pkill -f "$DIR/sink\.py" 2>/dev/null || true
pkill -f "http.server 8000" 2>/dev/null || true
sleep 1

cd "$DIR"
setsid nohup python3 -m http.server 8000 > "$DIR/http.log" 2>&1 < /dev/null &
setsid nohup python3 "$DIR/sink.py" > "$DIR/sink.log" 2>&1 < /dev/null &
sleep 2

echo "--- 监听 ---"
if ss -lntp 2>/dev/null | grep -E ':800[01]'; then :; else echo "❌ 8000/8001 没起来，看 $DIR/http.log 与 $DIR/sink.log"; fi

echo "--- 自测回传通道 ---"
curl -s -m 5 -X PUT --data-binary "selftest" http://127.0.0.1:8001/p/selftest > /dev/null && echo "  PUT 成功"
[ "$(cat "$DIR/rep/selftest" 2>/dev/null)" = "selftest" ] && echo "  ✅ 落盘正确（$DIR/rep/selftest）"

echo
echo "下一步：把安装包放进 $DIR/ ，测试机即可从 http://<本机内网IP>:8000/<文件名> 下载"
