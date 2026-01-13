# Luminode – WS2812B LED Controller (Rust + systemd)

This project runs a **WS2812B LED animation** on a **Raspberry Pi** using **Rust**, with **automatic startup on boot via systemd**.

It’s one compiled binary:

- no Python / venv
- no runtime dependencies
- no Cargo at boot

## Key properties

- Native Rust binary (low CPU usage)
- Uses PWM + DMA (GPIO 18)
- Stable WS2812B timing
- Auto-starts on boot
- Clean start / stop / restart workflow via systemd

## Project layout

Rust project:

```
~/luminode/
├── Cargo.toml
├── Cargo.lock
├── src/
│   └── main.rs
└── target/
    └── release/
        └── leds
```

Installed binary:

```
/usr/local/bin/leds
```

systemd service:

```
/etc/systemd/system/leds.service
```

## 1) Build the Rust binary

```bash
cd ~/luminode
cargo build --release
```

Output:

```
~/luminode/target/release/leds
```

## 2) Test the binary manually

WS2812B access requires root:

```bash
sudo ~/luminode/target/release/leds
```

Stop with `Ctrl+C`.

## 3) Install the binary system-wide

```bash
sudo cp ~/luminode/target/release/leds /usr/local/bin/leds
sudo chmod +x /usr/local/bin/leds
```

Test:

```bash
sudo /usr/local/bin/leds
```

## 4) Create the systemd service

Create:

```bash
sudo nano /etc/systemd/system/leds.service
```

Paste:

```ini
[Unit]
Description=WS2812B LED Controller (Rust)
After=multi-user.target

[Service]
Type=simple
User=root
ExecStart=/usr/local/bin/leds
Restart=always
RestartSec=2
Nice=10

[Install]
WantedBy=multi-user.target
```

## 5) Enable and start on boot

```bash
sudo systemctl daemon-reload
sudo systemctl enable leds.service
sudo systemctl start leds.service
```

Status:

```bash
sudo systemctl status leds.service
```

Logs:

```bash
journalctl -u leds.service -f
```

## Updating the Rust code

```bash
cd ~/luminode
cargo build --release
sudo cp target/release/leds /usr/local/bin/leds
sudo systemctl restart leds.service
```

## License

GNU General Public License v3.0
