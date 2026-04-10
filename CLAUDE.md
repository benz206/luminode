# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Luminode is a WS2812B LED controller for Raspberry Pi, written in Rust. It runs as a systemd service and drives 259 addressable RGB LEDs via GPIO 18 (PWM0 + DMA for timing precision). The entire program is a single file: `src/main.rs`.

It integrates with **luminode-sync** (the beatmap service at `luminode.bzhou.ca`) to sync LEDs to music: when Spotify is playing a track with a beatmap, the LEDs flash in time with the beat; otherwise they fall back to rainbow breathing.

## Build & Deploy

```bash
# Build (on dev machine or Pi)
cargo build --release
# Output: target/release/leds

# Cross-compile for Pi (requires `cross`)
cross build --release --target armv7-unknown-linux-gnueabihf

# Deploy to Pi and restart service
sudo cp target/release/leds /usr/local/bin/leds
sudo systemctl restart leds.service

# Run manually (requires root for GPIO/DMA access)
sudo ./target/release/leds

# View live logs
journalctl -u leds.service -f
```

There are no automated tests — validation is done by running on hardware.

## Configuration

Copy `.env.example` to `.env` (alongside the binary or in the working dir):

```
LUMINODE_URL=https://luminode.bzhou.ca
SPOTIFY_TOKEN_FILE=/home/pi/.local/share/luminode-sync/spotify_token.json
SPOTIFY_CLIENT_ID=<your-spotify-client-id>
```

The Spotify token file is generated once by `beatmap-cli auth --client-id <id>` from the luminode-sync repo.

Alternatively, set these as environment variables in the systemd unit file.

## Architecture

`src/main.rs` implements two concurrent threads:

### Render thread (main thread, 60 FPS)
- Reads `PlaybackState` (quick lock snapshot) each frame
- **Beat-synced mode** (when `is_playing && beatmap present`):
  - Computes estimated playback position (`progress_ms + elapsed since last poll`)
  - Finds current beat via binary search on precomputed beat timestamps
  - Calls `fill_beat_synced()` — flashes all LEDs in the bar's colour with a punchy attack/decay envelope; downbeats get a white overlay burst
- **Rainbow breathing mode** (paused, no track, no beatmap):
  - Calls `fill_rainbow_breathing()` — same rotating gradient as before

### Spotify polling thread (background, every 5 s)
- Loads `SpotifyAuth` from the token file; auto-refreshes when near expiry
- `GET /v1/me/player/currently-playing` → updates `PlaybackState`
- On track change: `GET {LUMINODE_URL}/beatmap/{spotify_id}` → parses MessagePack beatmap
- Updates shared `Arc<Mutex<PlaybackState>>` (lock held only for the write, not during HTTP)

### Colour palette (`PALETTE` constant)
Six vivid colours cycling per bar (every downbeat):
electric blue · hot magenta · teal · amber · purple · coral

## Seeding beatmaps

Run once to populate the beatmap service with all locally-saved tracks:

```bash
cd ../luminode-sync
SPOTIFY_CLIENT_ID=xxx SPOTIFY_CLIENT_SECRET=yyy \
LUMINODE_URL=https://luminode.bzhou.ca \
python scripts/seed_beatmaps.py
```

## Key Constraints

- **Root required**: GPIO and DMA access require `sudo`
- **GPIO 18 only**: PWM0 is the only pin with DMA support reliable enough for WS2812B timing
- **No runtime deps**: Compiled binary is self-contained — do not add dependencies that require runtime installations on the Pi. All Rust crates (`ureq`, `serde`, `rmp-serde`, `dotenvy`) compile into the binary.
- **259 LEDs**: This is a hardware constant tied to the physical LED strip length
- **Single file**: Keep the entire program in `src/main.rs`
- **No async runtime**: The render loop blocks for ~2 ms on each GPIO write. Polling runs in a plain `std::thread` with blocking `ureq` HTTP calls. Do not add tokio.
