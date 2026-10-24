#!/usr/bin/env bash

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/artifacts/copycat-relay"

command -v zig >/dev/null || { echo "找不到 zig，请先安装并加入 PATH" >&2; exit 1; }

cd "$ROOT"
cargo zigbuild --release -p copycat-relay \
    --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl

rm -rf "$OUT"
mkdir -p "$OUT/bin/amd64" "$OUT/bin/arm64"
cp target/x86_64-unknown-linux-musl/release/copycat-relay "$OUT/bin/amd64/"
cp target/aarch64-unknown-linux-musl/release/copycat-relay "$OUT/bin/arm64/"
cp deploy/docker/Dockerfile deploy/docker/compose.yml deploy/docker/setup.sh "$OUT/"
cp deploy/install-relay.sh deploy/copycat-relay.service "$OUT/"

TAR="$ROOT/artifacts/copycat-relay.tar"
tar -C "$ROOT/artifacts" --owner=0 --group=0 --exclude=copycat-relay/bin -cf "$TAR" copycat-relay
tar -C "$ROOT/artifacts" --owner=0 --group=0 --mode=0755 -rf "$TAR" copycat-relay/bin
gzip -f "$TAR"

echo "已生成 $OUT 与 artifacts/copycat-relay.tar.gz"
