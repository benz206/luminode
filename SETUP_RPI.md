# WS2812B LED Controller (Raspberry Pi 4B + Ubuntu)

This project runs a smooth, low-CPU **WS2812B LED gradient animation** on a **Raspberry Pi 4B** using **Python + rpi_ws281x**, with **automatic startup on boot via systemd**.

It is designed specifically to work on **Ubuntu for Raspberry Pi**, including newer Python versions (3.12/3.13).

---

## ✨ Features

* GPIO 18 (PWM0) + DMA (stable WS2812B timing)
* Smooth HSV gradient animation
* 30 FPS (low CPU usage, low heat)
* Slower ambient transitions
* Auto-starts on boot
* Runs as a proper Linux service
* No Arduino / FastLED required

---

## 🧰 Hardware Requirements

* Raspberry Pi 4B
* WS2812B LED strip (GRB)
* External 5V power supply (⚠️ **do not power LEDs from Pi**)
* Common ground between Pi and PSU
* Recommended: 330–470Ω resistor on data line

---

## 📁 Directory Layout

The project is installed under `/opt` (important for systemd execution):

```
/opt/lighting/
├── main.py
├── run_leds.sh
├── venv/
│   └── bin/python
└── README.md
```

---

## 🐍 Python & OS Notes (Important)

* Ubuntu blocks system-wide `pip install`
* Python GPIO libraries lag behind newest Python releases
* **We intentionally avoid `board`, Blinka, and RPi.GPIO**
* `rpi_ws281x` works reliably on Python 3.13+

---

## 📦 1. Install OS Dependencies

```bash
sudo apt update
sudo apt install -y \
  python3-full \
  python3-venv \
  build-essential
```

---

## 🧪 2. Create Project Directory

```bash
sudo mkdir -p /opt/lighting
sudo chown -R root:root /opt/lighting
sudo chmod -R 755 /opt/lighting
```

Copy `main.py` into `/opt/lighting`.

---

## 🐍 3. Create Python Virtual Environment

```bash
cd /opt/lighting
sudo python3 -m venv venv
```

---

## 📥 4. Install Python Dependencies

```bash
sudo /opt/lighting/venv/bin/pip install --upgrade pip
sudo /opt/lighting/venv/bin/pip install rpi_ws281x
```

> ⚠️ Do **not** install `board`, `neopixel`, or `RPi.GPIO`
> They are unnecessary and break on newer Python versions.

---

## ▶️ 5. Test the Script Manually

```bash
sudo /opt/lighting/venv/bin/python /opt/lighting/main.py
```

If LEDs animate correctly, continue.

---

## 🖥️ 6. Bash Launcher Script

Create `/opt/lighting/run_leds.sh`:

```bash
sudo nano /opt/lighting/run_leds.sh
```

```bash
#!/bin/bash
set -e

PROJECT_DIR="/opt/lighting"
VENV_PY="$PROJECT_DIR/venv/bin/python"
SCRIPT="$PROJECT_DIR/main.py"

cd "$PROJECT_DIR"
exec "$VENV_PY" "$SCRIPT"
```

Make it executable:

```bash
sudo chmod +x /opt/lighting/run_leds.sh
```

Test:

```bash
sudo /opt/lighting/run_leds.sh
```

---

## ⚙️ 7. Create systemd Service (Auto-start on Boot)

Create the service file:

```bash
sudo nano /etc/systemd/system/leds.service
```

```ini
[Unit]
Description=WS2812B LED Gradient
After=network.target

[Service]
Type=simple
User=root
WorkingDirectory=/opt/lighting
ExecStart=/bin/bash /opt/lighting/run_leds.sh
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
```

---

## 🔄 8. Enable & Start the Service

```bash
sudo systemctl daemon-reexec
sudo systemctl daemon-reload
sudo systemctl enable leds.service
sudo systemctl start leds.service
```

---

## ✅ 9. Check Status

```bash
sudo systemctl status leds.service
```

Expected:

```
Active: active (running)
```

---

## 🔧 Useful Commands

| Action            | Command                               |
| ----------------- | ------------------------------------- |
| Stop              | `sudo systemctl stop leds.service`    |
| Start             | `sudo systemctl start leds.service`   |
| Restart           | `sudo systemctl restart leds.service` |
| Logs              | `journalctl -u leds.service -f`       |
| Disable autostart | `sudo systemctl disable leds.service` |

---

## 🌡️ Performance & Heat

* CPU usage: typically **1–5%**
* Temp increase: usually **+1–5°C**
* Safe for 24/7 operation

To monitor temperature:

```bash
watch -n 1 cat /sys/class/thermal/thermal_zone0/temp
```

---

## ⚠️ Power Safety Notes

* 259 WS2812B LEDs at full white ≈ **15A**
* Use an external **5V ≥10A PSU**
* Inject power every 1–2 meters
* Always share **ground**

---

## 🚀 Future Extensions

* Web control (FastAPI / WebSocket)
* Multiple animation modes
* Time-of-day brightness
* Graceful fade-out on shutdown
* Temperature-based dimming

---

## ✅ Final Notes

This setup:

* Works on **Ubuntu**
* Works on **Python 3.13**
* Survives reboots and SSH disconnects
* Uses correct Linux service practices
