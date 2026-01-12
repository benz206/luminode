#!/usr/bin/env python3
import time
import math
from rpi_ws281x import PixelStrip, Color

# =====================
# LED CONFIG
# =====================
LED_COUNT = 259
LED_PIN = 18          # GPIO 18 (PWM0)
LED_FREQ_HZ = 800000
LED_DMA = 10
LED_BRIGHTNESS = 250
LED_INVERT = False
LED_CHANNEL = 0

strip = PixelStrip(
    LED_COUNT,
    LED_PIN,
    LED_FREQ_HZ,
    LED_DMA,
    LED_INVERT,
    LED_BRIGHTNESS,
    LED_CHANNEL
)

strip.begin()

# =====================
# HELPERS
# =====================
def beatsin8(bpm, low=0, high=255):
    t = time.time()
    beat = math.sin(2 * math.pi * bpm * t / 60.0)
    return int((beat + 1) / 2 * (high - low) + low)

def hsv_to_rgb(h, s=255, v=255):
    h /= 255.0
    s /= 255.0
    v /= 255.0

    i = int(h * 6)
    f = h * 6 - i
    p = v * (1 - s)
    q = v * (1 - f * s)
    t = v * (1 - (1 - f) * s)
    i %= 6

    if i == 0: r, g, b = v, t, p
    elif i == 1: r, g, b = q, v, p
    elif i == 2: r, g, b = p, v, t
    elif i == 3: r, g, b = p, q, v
    elif i == 4: r, g, b = t, p, v
    else: r, g, b = v, p, q

    return int(r * 255), int(g * 255), int(b * 255)

def fill_gradient(start_hue, end_hue):
    for i in range(LED_COUNT):
        ratio = i / (LED_COUNT - 1)
        hue = int(start_hue + ratio * (end_hue - start_hue)) % 256
        r, g, b = hsv_to_rgb(hue)
        strip.setPixelColor(i, Color(g, r, b))  # GRB

# =====================
# MAIN LOOP
# =====================
try:
    while True:
        # Original: 5 and 7 BPM
        # Slowed by 1.5× → ~3.3 and ~4.7 BPM
        start = beatsin8(3.3)
        end   = beatsin8(4.7)

        fill_gradient(start, end)
        strip.show()

        # 30 FPS
        time.sleep(1 / 30)

except KeyboardInterrupt:
    for i in range(LED_COUNT):
        strip.setPixelColor(i, Color(0, 0, 0))
    strip.show()
