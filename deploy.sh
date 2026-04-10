#!/usr/bin/env bash
set -euo pipefail

TARGET=armv7-unknown-linux-gnueabihf
PI=${PI:-pi@raspberrypi.local}
REMOTE_BIN=/usr/local/bin/leds
SERVICE=leds.service

echo "==> Building for $TARGET..."
cross build --release --target "$TARGET"

echo "==> Stopping $SERVICE..."
ssh "$PI" "sudo systemctl stop $SERVICE"

echo "==> Copying binary..."
scp "target/$TARGET/release/leds" "$PI:/tmp/leds"
ssh "$PI" "sudo mv /tmp/leds $REMOTE_BIN && sudo chmod 755 $REMOTE_BIN"

echo "==> Starting $SERVICE..."
ssh "$PI" "sudo systemctl start $SERVICE"

echo "==> Done. Live logs (Ctrl-C to exit):"
ssh "$PI" "journalctl -u $SERVICE -f"
