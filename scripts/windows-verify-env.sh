#!/usr/bin/env bash
# HunterLauncher · Windows 真机验收的「环境开关」
#
#   bash scripts/windows-verify-env.sh status       # 看两台机器现在在不在
#   bash scripts/windows-verify-env.sh relay-up     # 拉起中继机（发安装包 + 收回传），并铺好两条通道
#   bash scripts/windows-verify-env.sh relay-down   # 收走中继机（验完就该收，它是临时机）
#   bash scripts/windows-verify-env.sh test-up      # 拉起 Windows 测试机（已在则只打印）
#   bash scripts/windows-verify-env.sh test-start   # 唤醒已有的 Windows 测试机
#
# ⚠️ 必须在**有谷歌云权限的机器**上跑。香港开发机虽然装了 gcloud，但服务账号
#    scope 不足（跑起来报 insufficient authentication scopes），所以这台是例外。
#
# 为什么中继机是「临时机」：
#   它只做两件事 —— (1) 用 python 静态服务把安装包发给 Windows 测试机；
#   (2) 收测试机 PUT 回来的进度与结果落盘。验收做完就没有用处，
#   而它上面还开着 2375 的容器接口（给 W2 当「真容器后台」用），长期留着是多余的敞口。
#   所以：用之前 relay-up，用完 relay-down。
set -euo pipefail

PROJECT="${HL_PROJECT:-candaigo}"
ZONE="${HL_ZONE:-us-central1-a}"
RELAY="${HL_RELAY:-hl-docker-relay}"
RELAY_TAG="hl-relay"
RELAY_MACHINE="e2-small"
RELAY_DISK_GB=20
WIN="${HL_WIN:-hunter-win-test}"
WIN_ZONE="${HL_WIN_ZONE:-us-central1-a}"
HERE="$(cd "$(dirname "$0")" && pwd)"

say() { echo "[$(TZ=Asia/Shanghai date '+%H:%M:%S')] $*"; }
die() { echo "❌ $*" >&2; exit 1; }

exists() { gcloud compute instances describe "$1" --zone="$2" --project="$PROJECT" > /dev/null 2>&1; }
internal_ip() {
  gcloud compute instances describe "$1" --zone="$2" --project="$PROJECT" \
    --format='value(networkInterfaces[0].networkIP)'
}

cmd_status() {
  for pair in "$RELAY:$ZONE" "$WIN:$WIN_ZONE"; do
    n="${pair%%:*}"; z="${pair##*:}"
    if exists "$n" "$z"; then
      printf '  %-18s 在    %s   内网 %s\n' "$n" "$z" "$(internal_ip "$n" "$z")"
    else
      printf '  %-18s 不在\n' "$n"
    fi
  done
}

cmd_relay_up() {
  if exists "$RELAY" "$ZONE"; then
    say "$RELAY 已经在，内网 IP $(internal_ip "$RELAY" "$ZONE")（只补铺通道）"
  else
    say "创建中继机 $RELAY（$RELAY_MACHINE · ${RELAY_DISK_GB}GB · Ubuntu 24.04）"
    gcloud compute instances create "$RELAY" \
      --project="$PROJECT" --zone="$ZONE" \
      --machine-type="$RELAY_MACHINE" \
      --image-family=ubuntu-2404-lts-amd64 --image-project=ubuntu-os-cloud \
      --boot-disk-size="${RELAY_DISK_GB}GB" --boot-disk-type=pd-standard \
      --tags="$RELAY_TAG" \
      --labels=purpose=hunter-launcher-windows-verify \
      --quiet
    say "等它起来…"
    for _ in $(seq 1 40); do
      sleep 10
      gcloud compute ssh "$RELAY" --zone="$ZONE" --project="$PROJECT" \
        --command='echo UP' 2>/dev/null | grep -q UP && break
    done
  fi

  say "装容器后台，并把它开在 2375（W2 要连一台真容器后台）"
  gcloud compute ssh "$RELAY" --zone="$ZONE" --project="$PROJECT" --command='
    set -e
    if ! command -v docker > /dev/null 2>&1; then
      curl -fsSL https://get.docker.com | sudo sh
    fi
    sudo mkdir -p /etc/systemd/system/docker.service.d
    sudo tee /etc/systemd/system/docker.service.d/override.conf > /dev/null <<EOF
[Service]
ExecStart=
ExecStart=/usr/bin/dockerd -H fd:// -H tcp://0.0.0.0:2375
EOF
    sudo systemctl daemon-reload
    sudo systemctl restart docker
    sleep 3
    curl -s -m 5 http://127.0.0.1:2375/version | head -c 80 || echo "容器后台没起来"
  '

  say "铺两条通道（8000 发安装包 / 8001 收回传）"
  gcloud compute scp "$HERE/winverify-relay-bootstrap.sh" "$RELAY:/tmp/hl-bootstrap.sh" \
    --zone="$ZONE" --project="$PROJECT" --quiet
  gcloud compute ssh "$RELAY" --zone="$ZONE" --project="$PROJECT" \
    --command='bash /tmp/hl-bootstrap.sh'

  local ip; ip="$(internal_ip "$RELAY" "$ZONE")"
  say "中继机就绪 · 内网 IP $ip"
  say "把要发的安装包拷上去：gcloud compute scp <包> $RELAY:/tmp/hl/ --zone=$ZONE"
  say "测试机侧按 docs/开发文档/I20-Windows验收手册.md §四、§五 操作"
}

cmd_relay_down() {
  if ! exists "$RELAY" "$ZONE"; then say "$RELAY 本来就不在，无需清理"; return 0; fi
  say "收走中继机 $RELAY（开机盘随实例一起删；上面只有临时文件与容器）"
  gcloud compute instances delete "$RELAY" --zone="$ZONE" --project="$PROJECT" --quiet
  say "已收走"
}

cmd_test_up() {
  if exists "$WIN" "$WIN_ZONE"; then
    say "$WIN 已经在，内网 IP $(internal_ip "$WIN" "$WIN_ZONE")"
    say "要唤醒它：$0 test-start"
    return 0
  fi
  say "创建 Windows 测试机 $WIN（Windows Server 2022 · e2-standard-2 · 100GB，无桌面）"
  gcloud compute instances create "$WIN" \
    --project="$PROJECT" --zone="$WIN_ZONE" \
    --machine-type=e2-standard-2 \
    --image-family=windows-2022 --image-project=windows-cloud \
    --boot-disk-size=100GB --boot-disk-type=pd-standard \
    --tags="$WIN" --quiet
  say "就绪，内网 IP $(internal_ip "$WIN" "$WIN_ZONE")"
}

cmd_test_start() {
  exists "$WIN" "$WIN_ZONE" || die "$WIN 不存在，先跑 $0 test-up"
  say "唤醒 $WIN"
  gcloud compute instances start "$WIN" --zone="$WIN_ZONE" --project="$PROJECT" --quiet
  say "内网 IP $(internal_ip "$WIN" "$WIN_ZONE")"
}

case "${1:-status}" in
  status)     cmd_status ;;
  relay-up)   cmd_relay_up ;;
  relay-down) cmd_relay_down ;;
  test-up)    cmd_test_up ;;
  test-start) cmd_test_start ;;
  *) die "用法: $0 {status|relay-up|relay-down|test-up|test-start}" ;;
esac
