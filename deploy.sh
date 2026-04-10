#!/usr/bin/env bash
set -euo pipefail

PI=${PI:-pi@raspberrypi.local}
REMOTE_DIR=~/luminode
REMOTE_BIN=/usr/local/bin/leds
SERVICE=leds.service

echo "==> Syncing source to $PI:$REMOTE_DIR..."
rsync -az --exclude target --exclude .git . "$PI:$REMOTE_DIR"

echo "==> Building on Pi..."
ssh "$PI" "cd $REMOTE_DIR && cargo build --release"

echo "==> Stopping $SERVICE..."
ssh "$PI" "sudo systemctl stop $SERVICE"

echo "==> Installing binary..."
ssh "$PI" "sudo cp $REMOTE_DIR/target/release/leds $REMOTE_BIN && sudo chmod 755 $REMOTE_BIN"

echo "==> Starting $SERVICE..."
ssh "$PI" "sudo systemctl start $SERVICE"

echo "==> Done. Live logs (Ctrl-C to exit):"
ssh "$PI" "journalctl -u $SERVICE -f"
