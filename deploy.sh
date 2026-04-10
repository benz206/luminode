#!/usr/bin/env bash
set -euo pipefail

cargo build --release
sudo systemctl stop leds.service
sudo cp target/release/leds /usr/local/bin/leds
sudo systemctl start leds.service
