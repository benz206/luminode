use std::f32::consts::TAU;
use std::thread::sleep;
use std::time::{Duration, Instant};
use ws281x_rpi::{ControllerBuilder, ChannelBuilder};

const LED_COUNT: usize = 259;
const FPS: u64 = 30;

fn main() {
    let mut controller = ControllerBuilder::new()
        .channel(
            ChannelBuilder::new()
                .pin(18)
                .count(LED_COUNT as i32)
                .brightness(250)
                .build(),
        )
        .build()
        .unwrap();

    // Precompute HSV → RGB table
    let hsv_lut = build_hsv_lut();

    let start = Instant::now();
    let bpm1 = 3.3 / 60.0 * TAU;
    let bpm2 = 4.7 / 60.0 * TAU;

    loop {
        let t = start.elapsed().as_secs_f32();

        let start_hue = beatsin(bpm1, t);
        let end_hue   = beatsin(bpm2, t);

        fill_gradient(
            &mut controller.channels[0].leds,
            start_hue,
            end_hue,
            &hsv_lut,
        );

        controller.render().unwrap();
        sleep(Duration::from_millis(1000 / FPS));
    }
}

#[inline(always)]
fn beatsin(freq: f32, t: f32) -> u8 {
    (((freq * t).sin() + 1.0) * 127.5) as u8
}

fn fill_gradient(
    leds: &mut [u32],
    start: u8,
    end: u8,
    lut: &[(u8, u8, u8); 256],
) {
    let delta = end.wrapping_sub(start) as i16;
    let step = delta as f32 / (leds.len() - 1) as f32;

    let mut hue = start as f32;

    for led in leds.iter_mut() {
        let (r, g, b) = lut[hue as u8 as usize];
        *led = ((g as u32) << 16) | ((r as u32) << 8) | b as u32;
        hue += step;
    }
}

fn build_hsv_lut() -> [(u8, u8, u8); 256] {
    let mut lut = [(0, 0, 0); 256];
    for h in 0..256 {
        lut[h] = hsv_to_rgb(h as f32 / 255.0);
    }
    lut
}

fn hsv_to_rgb(h: f32) -> (u8, u8, u8) {
    let i = (h * 6.0).floor() as i32;
    let f = h * 6.0 - i as f32;
    let q = (1.0 - f) * 255.0;
    let t = f * 255.0;

    match i % 6 {
        0 => (255, t as u8, 0),
        1 => (q as u8, 255, 0),
        2 => (0, 255, t as u8),
        3 => (0, q as u8, 255),
        4 => (t as u8, 0, 255),
        _ => (255, 0, q as u8),
    }
}

