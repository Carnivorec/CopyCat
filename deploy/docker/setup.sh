#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")"

SUDO=""
[[ $EUID -ne 0 ]] && SUDO="sudo"

if ! docker compose version >/dev/null 2>&1; then
    echo "需要 Docker 与 Compose 插件（docker compose）" >&2
    exit 1
fi

if grep -Eq 'COPYCAT_RELAY_KEY: *""' compose.yml; then
    echo "compose.yml 中还没有填写中转密钥（COPYCAT_RELAY_KEY）。" >&2
    echo "请在 CopyCat 电脑端“设置 → 中转服务器”中生成，填入后重新运行。" >&2
    exit 1
fi

$SUDO install -d -m 0700 -o 65532 -g 65532 data

IMAGE=$(sed -nE 's/^ *image: *([^ ]+)$/\1/p' compose.yml | head -n1)
if [[ -d bin ]]; then
    docker build -t "$IMAGE" .
else
    docker compose pull
fi
docker compose up -d
sleep 2
docker compose logs --tail 5 relay

PORT=$(sed -nE 's/^ *- "([0-9]+):8443\/udp"$/\1/p' compose.yml | head -n1)
cat <<EOF

完成。中转服务器已在端口 ${PORT:-8443} 上运行（上面的日志中应有“中转服务器已启动”）。
云服务商的安全组需放行 UDP ${PORT:-8443} 与 TCP ${PORT:-8443}；
域名托管在 Cloudflare 时，中转子域名必须设为“仅 DNS”（灰色云朵）。
查看日志：docker compose logs -f
EOF
