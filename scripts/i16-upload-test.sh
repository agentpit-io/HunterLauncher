#!/usr/bin/env bash
# I16 · P0-2 的真机验收：**一键上传日志**。
#
# 四件事：
#   ① 不带 -y 时**一个请求都不发**，只把将要上传的正文原样打出来；
#   ② 把那份正文逐条筛一遍：不许有 key、口令、主机名、用户名；
#   ③ 真发一次，走**线上**接口，拿到真的 traceCode；
#   ④ 连打到触发 429，验限流的文案。
#
# 这台机器的用户名是 support、主机名是 hunter-test-01.…，
# 两个都要在正文里找不到 —— 这一条不能只靠单测（总控规则「红线 1」的同一条道理）。
set -u

BIN="${BIN:-$HOME/HunterLauncher/src-tauri/target/release/hunter-launcher}"
HOME_DIR="${HOME_DIR:-$HOME/.hunter}"
OUT="${OUT:-/tmp/i16-upload}"
mkdir -p "$OUT"

USER_NAME="$(id -un)"
HOST_NAME="$(hostname)"
HOST_SHORT="${HOST_NAME%%.*}"

echo "=== ① 不带 -y：只看，不发 ==="
HUNTER_HOME="$HOME_DIR" "$BIN" --upload-logs \
  --stage upgrade --code E_PULL_STALLED \
  --summary "升级时从腾讯云香港拉镜像 90 秒没有数据进来" \
  > "$OUT/preview.txt" 2>&1
echo "  退出码 $? · 输出 $(wc -l < "$OUT/preview.txt") 行 → $OUT/preview.txt"
grep -E "这一次\*\*什么都没有发出去|这一次什么都没有发出去" "$OUT/preview.txt" \
  && echo "  ✓ 明确说了这一次没发" || echo "  ✗ 没说「没发」"

echo
echo "=== ② 人工核一遍要传的正文 ==="
# 只看「将要上传的正文」那一段（`  | ` 开头的行）
sed -n '/将要上传的正文/,/正文到此为止/p' "$OUT/preview.txt" > "$OUT/body.txt"
echo "  正文 $(wc -l < "$OUT/body.txt") 行"
fail=0
check() { # 名字 模式
  local n
  n=$(grep -c -- "$2" "$OUT/body.txt" || true)
  if [ "$n" -gt 0 ]; then
    echo "  ✗ $1：命中 $n 行"
    grep -n -- "$2" "$OUT/body.txt" | head -3
    fail=1
  else
    echo "  ✓ $1：一处都没有"
  fi
}
# key 明文：`hunt_tools_` 后面还跟着字母数字（打过码的是 `hunt_tools_****`）
check "hunt_tools_ 后跟真实字符" "hunt_tools_[A-Za-z0-9]"
check "口令（未打码的 PASSWORD=）" "PASSWORD=[A-Za-z0-9]"
check "口令（未打码的 SECRET=）" "SECRET=[A-Za-z0-9]"
check "口令（未打码的 PASS=）" "PASS=[A-Za-z0-9]"
check "主机名 $HOST_SHORT" "$HOST_SHORT"
check "用户名 $USER_NAME" "$USER_NAME"
echo "  正文里 key 的样子（应当都是打过码的）："
grep -oE "hunt_tools_[^ ,\"]*" "$OUT/body.txt" | sort -u | head -5 || echo "    （一处都没有）"

echo
echo "=== ③ 真发一次（线上接口） ==="
HUNTER_HOME="$HOME_DIR" "$BIN" --upload-logs -y \
  --stage upgrade --code E_PULL_STALLED \
  --summary "I16 验收：升级时从腾讯云香港拉镜像 90 秒没有数据进来" \
  2>&1 | tee "$OUT/upload-1.txt" | grep -E "追踪码|HTTP|machineId"
echo

echo "=== ④ 连打到触发 429（同机器每小时最多 5 次） ==="
for i in 2 3 4 5 6 7; do
  printf "  第 %d 次：" "$i"
  HUNTER_HOME="$HOME_DIR" "$BIN" --upload-logs -y --stage ratelimit-probe \
    > "$OUT/upload-$i.txt" 2>&1
  grep -oE "✓ 追踪码：[A-Z0-9-]+|HTTP [0-9]+" "$OUT/upload-$i.txt" | tr '\n' ' '
  echo
done
echo
echo "  429 那一次的完整说法："
grep -l "HTTP 429" "$OUT"/upload-*.txt | head -1 | xargs -r grep -A2 "HTTP 429" | head -6
echo
echo "  所有输出留在 $OUT/"
exit $fail
