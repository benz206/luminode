use std::f32::consts::TAU;
use std::thread::sleep;
use std::time::{Duration, Instant};
use smart_leds_trait::{SmartLedsWrite, RGB8};
use ws281x_rpi::Ws2812Rpi;

const LED_COUNT: usize = 259;
const FPS: u64 = 30;

fn main() {
    let mut strip = Ws2812Rpi::new(LED_COUNT as i32, 18).unwrap();

    // Precompute HSV → RGB table
    let hsv_lut = build_hsv_lut();

    let start = Instant::now();
    let bpm1 = 3.3 / 60.0 * TAU;
    let bpm2 = 4.7 / 60.0 * TAU;

    let mut leds = vec![RGB8 { r: 0, g: 0, b: 0 }; LED_COUNT];

    loop {
        let t = start.elapsed().as_secs_f32();

        let start_hue = beatsin(bpm1, t);
        let end_hue   = beatsin(bpm2, t);

        fill_gradient(
            &mut leds,
            start_hue,
            end_hue,
            &hsv_lut,
        );

        strip.write(leds.iter().copied()).unwrap();
        sleep(Duration::from_millis(1000 / FPS));
    }
}

#[inline(always)]
fn beatsin(freq: f32, t: f32) -> u8 {
    (((freq * t).sin() + 1.0) * 127.5) as u8
}

fn fill_gradient(
    leds: &mut [RGB8],
    start: u8,
    end: u8,
    lut: &[RGB8; 256],
) {
    let delta = end.wrapping_sub(start) as i16;
    let step = delta as f32 / (leds.len() - 1) as f32;

    let mut hue = start as f32;

    for led in leds.iter_mut() {
        *led = lut[hue as u8 as usize];
        hue += step;
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

