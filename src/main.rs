use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use smart_leds_trait::{SmartLedsWrite, RGB8};
use ws281x_rpi::Ws2812Rpi;
use serde::{Deserialize, Serialize};

const LED_COUNT: usize = 259;
const FPS: u64 = 60;
const POLL_SECS: u64 = 5;

/// Beat-synced colour palette.  Each bar gets one colour from this list
/// (cycling), applied as a punchy flash on every beat.
const PALETTE: [[u8; 3]; 6] = [
    [0,   100, 255],  // electric blue
    [255,   0, 180],  // hot magenta
    [0,   220, 170],  // teal
    [255, 160,   0],  // amber
    [140,   0, 255],  // purple
    [255,  80,  30],  // coral
];

// ── Config ─────────────────────────────────────────────────────────────────────

struct Config {
    luminode_url: String,
    spotify_token_file: PathBuf,
    spotify_client_id: String,
}

impl Config {
    fn load() -> Self {
        // Load .env if it exists alongside the binary or in the current dir.
        let _ = dotenvy::dotenv();
        Config {
            luminode_url: std::env::var("LUMINODE_URL")
                .unwrap_or_else(|_| "https://luminode.bzhou.ca".to_string()),
            spotify_token_file: PathBuf::from(
                std::env::var("SPOTIFY_TOKEN_FILE").unwrap_or_else(|_| {
                    "/home/pi/.local/share/luminode-sync/spotify_token.json".to_string()
                }),
            ),
            spotify_client_id: std::env::var("SPOTIFY_CLIENT_ID").unwrap_or_default(),
        }
    }
}

// ── Spotify token ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SpotifyAuth {
    access_token: String,
    refresh_token: String,
    expires_at_epoch_secs: u64,
    #[serde(default)]
    client_id: Option<String>,
}

impl SpotifyAuth {
    fn load(path: &PathBuf) -> Option<Self> {
        let s = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&s).ok()
    }

    fn save(&self, path: &PathBuf) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, s);
        }
    }

    fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now + 60 >= self.expires_at_epoch_secs
    }
}

// ── Beatmap (minimal deserialization) ─────────────────────────────────────────

/// Minimal subset of the beatmap wire format needed for LED sync.
/// rmp-serde skips any fields not declared here.
#[derive(Deserialize)]
struct RawBeatmap {
    timing: RawTiming,
    #[serde(default)]
    calibration_ms: i32,
    #[serde(default)]
    sections: Vec<RawSection>,
}

#[derive(Deserialize)]
struct RawTiming {
    first_beat_ms: u32,
    beat_deltas_ms: Vec<u16>,
    downbeat_bits: Vec<u8>,
}

#[derive(Deserialize)]
struct RawSection {
    start_beat: u16,
    kind: String,
    #[allow(dead_code)]
    energy: u8,
}

/// Map a section kind string to a PALETTE index.
fn section_palette_index(kind: &str) -> Option<usize> {
    match kind {
        "intro"     => Some(0), // electric blue — soft entrance
        "verse"     => Some(2), // teal — calm sections
        "chorus"    => Some(1), // hot magenta — high energy
        "buildup"   => Some(3), // amber — building tension
        "drop"      => Some(4), // purple — intense
        "breakdown" => Some(5), // coral — breakdown
        "bridge"    => Some(0), // electric blue
        "outro"     => Some(2), // teal — winding down
        _           => None,    // fallback: use bar index
    }
}

struct BeatmapData {
    /// Absolute beat timestamps (ms from track start).
    beat_times_ms: Vec<u32>,
    /// Whether each beat is a downbeat (bar start).
    is_downbeat: Vec<bool>,
    /// PALETTE index for each beat, driven by section kind (falls back to bar index).
    beat_palette_index: Vec<usize>,
    /// Signed timing correction applied to all beat positions.
    calibration_ms: i32,
}

impl BeatmapData {
    fn from_raw(raw: RawBeatmap) -> Self {
        let t = &raw.timing;
        let n = t.beat_deltas_ms.len() + 1;

        let mut beat_times_ms = Vec::with_capacity(n);
        let mut cur = t.first_beat_ms;
        beat_times_ms.push(cur);
        for &d in &t.beat_deltas_ms {
            cur += d as u32;
            beat_times_ms.push(cur);
        }

        let is_downbeat: Vec<bool> = (0..n)
            .map(|i| {
                t.downbeat_bits
                    .get(i / 8)
                    .map(|&b| (b >> (i % 8)) & 1 == 1)
                    .unwrap_or(false)
            })
            .collect();

        // Compute bar index (used as fallback when no sections are available).
        let mut bar_index = vec![0usize; n];
        let mut bar = 0usize;
        for i in 0..n {
            if i > 0 && is_downbeat[i] {
                bar += 1;
            }
            bar_index[i] = bar;
        }

        // Build a sorted list of (start_beat, palette_index) from sections.
        // Sections missing from the plan fall back to None (use bar index).
        let mut section_map: Vec<(usize, Option<usize>)> = raw.sections
            .iter()
            .map(|s| (s.start_beat as usize, section_palette_index(&s.kind)))
            .collect();
        section_map.sort_by_key(|&(b, _)| b);

        // For each beat, find the active section and assign a palette index.
        let beat_palette_index: Vec<usize> = (0..n)
            .map(|i| {
                // Walk backwards to find the last section start ≤ i.
                let active = section_map.iter().rev().find(|&&(sb, _)| sb <= i);
                match active {
                    Some(&(_, Some(pi))) => pi,
                    _ => bar_index[i] % PALETTE.len(),
                }
            })
            .collect();

        BeatmapData {
            beat_times_ms,
            is_downbeat,
            beat_palette_index,
            calibration_ms: raw.calibration_ms,
        }
    }
}

// ── Shared playback state ──────────────────────────────────────────────────────

#[derive(Default)]
struct PlaybackState {
    track_id: Option<String>,
    is_playing: bool,
    /// Playback position at the time of the last poll.
    progress_ms: u32,
    /// Wall-clock instant when progress_ms was captured.
    polled_at: Option<Instant>,
    /// Present when a beatmap for the current track was found.
    beatmap: Option<Arc<BeatmapData>>,
}

impl PlaybackState {
    /// Estimated current playback position, extrapolated from the last poll.
    fn current_position_ms(&self) -> u32 {
        if !self.is_playing {
            return self.progress_ms;
        }
        let elapsed = self
            .polled_at
            .map(|t| t.elapsed().as_millis() as u32)
            .unwrap_or(0);
        self.progress_ms.saturating_add(elapsed)
    }
}

// ── Beat state ─────────────────────────────────────────────────────────────────

struct BeatState {
    /// 0.0 = exactly on the beat, 1.0 = just before the next beat.
    phase: f32,
    is_downbeat: bool,
    palette_index: usize,
}

fn beat_state_at(bm: &BeatmapData, position_ms: u32) -> BeatState {
    // Apply calibration offset.
    let pos = if bm.calibration_ms >= 0 {
        position_ms.saturating_sub(bm.calibration_ms as u32)
    } else {
        position_ms.saturating_add((-bm.calibration_ms) as u32)
    };

    let times = &bm.beat_times_ms;
    let idx = match times.binary_search(&pos) {
        Ok(i) => i,
        Err(0) => 0,
        Err(i) if i >= times.len() => times.len() - 1,
        Err(i) => i - 1,
    };

    let beat_start = times[idx];
    let beat_end = times.get(idx + 1).copied().unwrap_or(beat_start + 500);
    let phase = if beat_end > beat_start {
        pos.saturating_sub(beat_start) as f32 / (beat_end - beat_start) as f32
    } else {
        0.0
    }
    .clamp(0.0, 1.0);

    BeatState {
        phase,
        is_downbeat: bm.is_downbeat[idx],
        palette_index: bm.beat_palette_index[idx],
    }
}

// ── Rendering ──────────────────────────────────────────────────────────────────

/// Beat-synced flash: every beat punches in with the bar's colour, decays
/// sharply, and downbeats get a white overlay for emphasis.
fn fill_beat_synced(leds: &mut [RGB8], bs: &BeatState) {
    let color = PALETTE[bs.palette_index];

    // Envelope: instant attack, quadratic decay over the beat.
    let flash: f32 = if bs.phase < 0.05 {
        1.0
    } else {
        ((1.0 - bs.phase) * 1.4).max(0.0).powi(2)
    };

    // Downbeats add a white burst over the first 8% of the beat.
    let white: f32 = if bs.is_downbeat && bs.phase < 0.08 {
        (1.0 - bs.phase / 0.08) * 0.55
    } else {
        0.0
    };

    let r = (color[0] as f32 * flash + 255.0 * white).min(255.0) as u8;
    let g = (color[1] as f32 * flash + 255.0 * white).min(255.0) as u8;
    let b = (color[2] as f32 * flash + 255.0 * white).min(255.0) as u8;

    for led in leds.iter_mut() {
        *led = RGB8 { r, g, b };
    }
}

/// Original rainbow breathing — used when no beatmap is active.
fn fill_rainbow_breathing(leds: &mut [RGB8], t: f64, lut: &[RGB8; 256]) {
    let center = (t * 8.0).rem_euclid(256.0);
    let spread = 60.0 + 30.0 * (t * 0.3).sin();
    fill_gradient(leds, center - spread, center + spread, lut);
}

fn fill_gradient(leds: &mut [RGB8], start: f64, end: f64, lut: &[RGB8; 256]) {
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

// ── Spotify polling thread ─────────────────────────────────────────────────────

fn try_refresh_token(auth: &mut SpotifyAuth, client_id: &str, path: &PathBuf) {
    let result = ureq::post("https://accounts.spotify.com/api/token")
        .send_form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", auth.refresh_token.as_str()),
            ("client_id", client_id),
        ]);

    match result {
        Ok(resp) => {
            if let Ok(body) = serde_json::from_reader::<_, serde_json::Value>(resp.into_reader()) {
                if let Some(at) = body["access_token"].as_str() {
                    auth.access_token = at.to_owned();
                }
                let expires_in = body["expires_in"].as_u64().unwrap_or(3600);
                auth.expires_at_epoch_secs = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    + expires_in;
                if let Some(rt) = body["refresh_token"].as_str() {
                    auth.refresh_token = rt.to_owned();
                }
                auth.save(path);
                eprintln!("[spotify] token refreshed");
            }
        }
        Err(e) => eprintln!("[spotify] token refresh failed: {}", e),
    }
}

/// Returns (track_id, progress_ms, is_playing) or None.
fn fetch_current_track(auth: &SpotifyAuth) -> Option<(String, u32, bool)> {
    let resp = ureq::get("https://api.spotify.com/v1/me/player/currently-playing")
        .set("Authorization", &format!("Bearer {}", auth.access_token))
        .call();

    match resp {
        Ok(r) if r.status() == 204 => None,
        Ok(r) => {
            let body: serde_json::Value = serde_json::from_reader(r.into_reader()).ok()?;
            if body["currently_playing_type"].as_str() != Some("track") {
                return None;
            }
            let id = body["item"]["id"].as_str()?.to_owned();
            let progress = body["progress_ms"].as_u64().unwrap_or(0) as u32;
            let playing = body["is_playing"].as_bool().unwrap_or(false);
            Some((id, progress, playing))
        }
        Err(e) => {
            eprintln!("[spotify] poll error: {}", e);
            None
        }
    }
}

/// Fetches and parses a beatmap from the luminode-sync API.
fn fetch_beatmap(base_url: &str, track_id: &str) -> Option<Arc<BeatmapData>> {
    let url = format!("{}/beatmap/{}", base_url, track_id);
    let resp = ureq::get(&url).call();
    match resp {
        Ok(r) if r.status() == 200 => {
            let mut bytes = Vec::new();
            r.into_reader().read_to_end(&mut bytes).ok()?;
            let raw: RawBeatmap = rmp_serde::from_slice(&bytes)
                .map_err(|e| eprintln!("[beatmap] parse error for {}: {}", track_id, e))
                .ok()?;
            eprintln!("[beatmap] loaded {} beats for {}", raw.timing.beat_deltas_ms.len() + 1, track_id);
            Some(Arc::new(BeatmapData::from_raw(raw)))
        }
        Ok(r) if r.status() == 404 => {
            eprintln!("[beatmap] none for {}", track_id);
            None
        }
        Ok(r) => {
            eprintln!("[beatmap] unexpected {} for {}", r.status(), track_id);
            None
        }
        Err(e) => {
            eprintln!("[beatmap] fetch error: {}", e);
            None
        }
    }
}

fn poll_loop(config: Config, state: Arc<Mutex<PlaybackState>>) {
    let token_path = config.spotify_token_file.clone();

    let mut auth = match SpotifyAuth::load(&token_path) {
        Some(a) => a,
        None => {
            eprintln!(
                "[spotify] token not found at {} — running rainbow only",
                token_path.display()
            );
            return;
        }
    };

    // Prefer SPOTIFY_CLIENT_ID env; fall back to what was saved in the token file.
    let client_id = if config.spotify_client_id.is_empty() {
        auth.client_id.clone().unwrap_or_default()
    } else {
        config.spotify_client_id.clone()
    };

    if client_id.is_empty() {
        eprintln!("[spotify] no client_id — set SPOTIFY_CLIENT_ID in .env");
        return;
    }

    let mut last_track_id: Option<String> = None;

    loop {
        if auth.is_expired() {
            try_refresh_token(&mut auth, &client_id, &token_path);
        }

        match fetch_current_track(&auth) {
            None => {
                // Nothing playing or not a track.
                if last_track_id.is_some() {
                    eprintln!("[spotify] playback stopped");
                    last_track_id = None;
                }
                let mut g = state.lock().unwrap();
                g.is_playing = false;
                g.track_id = None;
                g.beatmap = None;
            }

            Some((track_id, progress_ms, is_playing)) => {
                // Fetch beatmap only on track change (including first run).
                let beatmap = if last_track_id.as_deref() != Some(&track_id) {
                    eprintln!("[spotify] track: {}", track_id);
                    last_track_id = Some(track_id.clone());
                    fetch_beatmap(&config.luminode_url, &track_id)
                } else {
                    // Re-use existing beatmap from state (cheap Arc clone).
                    state.lock().unwrap().beatmap.clone()
                };

                let mut g = state.lock().unwrap();
                g.track_id = Some(track_id);
                g.is_playing = is_playing;
                g.progress_ms = progress_ms;
                g.polled_at = Some(Instant::now());
                g.beatmap = beatmap;
            }
        }

        thread::sleep(Duration::from_secs(POLL_SECS));
    }
}

// ── Main ───────────────────────────────────────────────────────────────────────

fn main() {
    let config = Config::load();
    let mut strip = Ws2812Rpi::new(LED_COUNT as i32, 18).unwrap();
    let hsv_lut = build_hsv_lut();

    let mut leds = vec![RGB8 { r: 0, g: 0, b: 0 }; LED_COUNT];
    let frame_dur = Duration::from_secs_f64(1.0 / FPS as f64);
    let start = Instant::now();
    let mut next_frame = Instant::now();

    let state: Arc<Mutex<PlaybackState>> = Arc::new(Mutex::new(PlaybackState::default()));

    // Spotify + beatmap polling thread.
    {
        let state = Arc::clone(&state);
        thread::spawn(move || poll_loop(config, state));
    }

    loop {
        let t = start.elapsed().as_secs_f64();

        // Snapshot just enough state to render — hold the lock as briefly as possible.
        let (playing, pos_ms, beatmap) = {
            let g = state.lock().unwrap();
            (g.is_playing, g.current_position_ms(), g.beatmap.clone())
        };

        if playing {
            if let Some(ref bm) = beatmap {
                let bs = beat_state_at(bm, pos_ms);
                fill_beat_synced(&mut leds, &bs);
            } else {
                fill_rainbow_breathing(&mut leds, t, &hsv_lut);
            }
        } else {
            fill_rainbow_breathing(&mut leds, t, &hsv_lut);
        }

        strip.write(leds.iter().copied()).unwrap();

        let now = Instant::now();
        next_frame = if next_frame <= now {
            now + frame_dur
        } else {
            next_frame + frame_dur
        };
        thread::sleep(next_frame.saturating_duration_since(now));
    }
}
