#!/usr/bin/env bash

set -euo pipefail

BIN_SRC="${1:-target/release/copycat-relay}"
KEY="${2:-${COPYCAT_RELAY_KEY:-}}"
DATA_DIR=/var/lib/copycat-relay
PORT="${COPYCAT_PORT:-8443}"
HERE="$(cd "$(dirname "$0")" && pwd)"

if [[ $EUID -ne 0 ]]; then
    echo "请以 root 运行" >&2
    exit 1
fi
if [[ ! -x "$BIN_SRC" ]]; then
    echo "找不到可执行文件 $BIN_SRC，请先 cargo build --release -p copycat-relay" >&2
    exit 1
fi
if [[ "$KEY" != ccr1-* ]]; then
    echo "缺少中转密钥：请在 CopyCat 电脑端“设置 → 中转服务器”生成，作为第二个参数传入" >&2
    exit 1
fi

id copycat >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin copycat
install -m 0755 "$BIN_SRC" /usr/local/bin/copycat-relay
install -d -m 0700 -o copycat -g copycat "$DATA_DIR"
sed "s/0\.0\.0\.0:8443/0.0.0.0:${PORT}/" "$HERE/copycat-relay.service" >/etc/systemd/system/copycat-relay.service
chmod 0644 /etc/systemd/system/copycat-relay.service

umask 077
printf 'COPYCAT_RELAY_KEY=%s\n' "$KEY" >/etc/copycat-relay.env
chmod 0600 /etc/copycat-relay.env

if command -v ufw >/dev/null 2>&1 && ufw status | grep -q "Status: active"; then
    ufw allow "${PORT}/udp"
    ufw allow "${PORT}/tcp"
fi

systemctl daemon-reload
systemctl enable copycat-relay
systemctl restart copycat-relay
systemctl --no-pager --lines=5 status copycat-relay || true

cat <<EOF

完成。中转服务器已在端口 ${PORT} 上运行，设备组中的设备会自动连上（电脑端填写的地址端口需为 ${PORT}）。

云服务商的安全组（防火墙）中需放行 UDP ${PORT} 与 TCP ${PORT}。
查看日志：journalctl -u copycat-relay -f
EOF
