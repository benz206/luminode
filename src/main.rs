use std::thread::sleep;
use std::time::{Duration, Instant};
use smart_leds_trait::{SmartLedsWrite, RGB8};
use ws281x_rpi::Ws2812Rpi;

const LED_COUNT: usize = 259;
const FPS: u64 = 60;

fn main() {
    let mut strip = Ws2812Rpi::new(LED_COUNT as i32, 18).unwrap();

    let hsv_lut = build_hsv_lut();

    let mut leds = vec![RGB8 { r: 0, g: 0, b: 0 }; LED_COUNT];
    let frame_duration = Duration::from_secs_f64(1.0 / FPS as f64);
    let mut next_frame_at = Instant::now();
    let start = Instant::now();

    loop {
        let t = start.elapsed().as_secs_f64();

        let center_hue = (t * 8.0).rem_euclid(256.0);
        let spread = 60.0 + 30.0 * (t * 0.3).sin();

        fill_gradient(
            &mut leds,
            center_hue - spread,
            center_hue + spread,
            &hsv_lut,
        );

        strip.write(leds.iter().copied()).unwrap();
        let now = Instant::now();
        next_frame_at = if next_frame_at <= now {
            now + frame_duration
        } else {
            next_frame_at + frame_duration
        };
        sleep(next_frame_at.saturating_duration_since(now));
    }
}

fn fill_gradient(
    leds: &mut [RGB8],
    start: f64,
    end: f64,
    lut: &[RGB8; 256],
) {
    let step = (end - start) / (leds.len() - 1) as f64;
    let mut hue = start;

    for led in leds.iter_mut() {
        *led = sample_lut(hue, lut);
        hue += step;
    }
}

#[inline(always)]
fn sample_lut(hue: f64, lut: &[RGB8; 256]) -> RGB8 {
    let h = hue.rem_euclid(256.0);
    let h0 = h.floor();
    let i0 = h0 as usize & 255;
    let i1 = (i0 + 1) & 255;
    let f = (h - h0) as f32;

    let a = lut[i0];
    let b = lut[i1];

    RGB8 {
        r: (a.r as f32 + (b.r as f32 - a.r as f32) * f).round() as u8,
        g: (a.g as f32 + (b.g as f32 - a.g as f32) * f).round() as u8,
        b: (a.b as f32 + (b.b as f32 - a.b as f32) * f).round() as u8,
    }
}

fn build_hsv_lut() -> [RGB8; 256] {
    let mut lut = [RGB8 { r: 0, g: 0, b: 0 }; 256];
    for h in 0..256 {
        lut[h] = hsv_to_rgb(h as f32 / 255.0);
    }
    lut
}

fn hsv_to_rgb(h: f32) -> RGB8 {
    let i = (h * 6.0).floor() as i32;
    let f = h * 6.0 - i as f32;
    let q = (1.0 - f) * 255.0;
    let t = f * 255.0;

    match i % 6 {
        0 => RGB8 { r: 255, g: t as u8, b: 0 },
        1 => RGB8 { r: q as u8, g: 255, b: 0 },
        2 => RGB8 { r: 0, g: 255, b: t as u8 },
        3 => RGB8 { r: 0, g: q as u8, b: 255 },
        4 => RGB8 { r: t as u8, g: 0, b: 255 },
        _ => RGB8 { r: 255, g: 0, b: q as u8 },
    }
}
