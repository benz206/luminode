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
/// Poll every 2 s for tighter drift correction (beatmap is cached per-track).
const POLL_SECS: u64 = 2;

/// Beat-synced colour palette — one colour per section kind.
const PALETTE: [[u8; 3]; 6] = [
    [0,   100, 255],  // electric blue  (intro / bridge)
    [255,   0, 180],  // hot magenta    (chorus)
    [0,   220, 170],  // teal           (verse / outro)
    [255, 160,   0],  // amber          (buildup)
    [140,   0, 255],  // purple         (drop)
    [255,  80,  30],  // coral          (breakdown)
];

// ── Config ─────────────────────────────────────────────────────────────────────

struct Config {
    luminode_url: String,
    spotify_token_file: PathBuf,
    spotify_client_id: String,
}

impl Config {
    fn load() -> Self {
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

// ── Beatmap ────────────────────────────────────────────────────────────────────

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum SectionKind {
    Intro,
    Verse,
    Chorus,
    Buildup,
    Drop,
    Breakdown,
    Bridge,
    Outro,
    Unknown,
}

impl SectionKind {
    fn from_str(s: &str) -> Self {
        match s {
            "intro"     => SectionKind::Intro,
            "verse"     => SectionKind::Verse,
            "chorus"    => SectionKind::Chorus,
            "buildup"   => SectionKind::Buildup,
            "drop"      => SectionKind::Drop,
            "breakdown" => SectionKind::Breakdown,
            "bridge"    => SectionKind::Bridge,
            "outro"     => SectionKind::Outro,
            _           => SectionKind::Unknown,
        }
    }

    fn palette_index(self) -> Option<usize> {
        match self {
            SectionKind::Intro     => Some(0),
            SectionKind::Verse     => Some(2),
            SectionKind::Chorus    => Some(1),
            SectionKind::Buildup   => Some(3),
            SectionKind::Drop      => Some(4),
            SectionKind::Breakdown => Some(5),
            SectionKind::Bridge    => Some(0),
            SectionKind::Outro     => Some(2),
            SectionKind::Unknown   => None,
        }
    }
}

struct SectionEntry {
    start_beat: usize,
    end_beat: usize,
    kind: SectionKind,
    palette_index: usize,
}

struct BeatmapData {
    beat_times_ms: Vec<u32>,
    is_downbeat: Vec<bool>,
    beat_palette_index: Vec<usize>,
    calibration_ms: i32,
    sections: Vec<SectionEntry>,
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

        let mut bar_index = vec![0usize; n];
        let mut bar = 0usize;
        for i in 0..n {
            if i > 0 && is_downbeat[i] {
                bar += 1;
            }
            bar_index[i] = bar;
        }

        // Build sorted section list with resolved end beats.
        let mut raw_sections: Vec<(usize, SectionKind)> = raw.sections
            .iter()
            .map(|s| (s.start_beat as usize, SectionKind::from_str(&s.kind)))
            .collect();
        raw_sections.sort_by_key(|&(b, _)| b);

        let sections: Vec<SectionEntry> = raw_sections
            .iter()
            .enumerate()
            .map(|(i, &(start_beat, kind))| {
                let end_beat = raw_sections.get(i + 1).map(|&(b, _)| b).unwrap_or(n);
                let palette_index = kind.palette_index()
                    .unwrap_or_else(|| bar_index[start_beat.min(n - 1)] % PALETTE.len());
                SectionEntry { start_beat, end_beat, kind, palette_index }
            })
            .collect();

        let beat_palette_index: Vec<usize> = (0..n)
            .map(|i| {
                sections.iter().rev()
                    .find(|s| s.start_beat <= i)
                    .map(|s| s.palette_index)
                    .unwrap_or_else(|| bar_index[i] % PALETTE.len())
            })
            .collect();

        BeatmapData {
            beat_times_ms,
            is_downbeat,
            beat_palette_index,
            calibration_ms: raw.calibration_ms,
            sections,
        }
    }
}

// ── Shared playback state ──────────────────────────────────────────────────────

#[derive(Default)]
struct PlaybackState {
    track_id: Option<String>,
    is_playing: bool,
    /// Spotify's reported progress at the time polled_at was captured.
    progress_ms: u32,
    /// Instant representing when progress_ms was accurate (≈ midpoint of RTT).
    polled_at: Option<Instant>,
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
    section_kind: SectionKind,
    /// How far into the current section: 0.0 = section start, 1.0 = section end.
    section_phase: f32,
}

fn beat_state_at(bm: &BeatmapData, position_ms: u32) -> BeatState {
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

    // Locate the active section and compute how far we are into it.
    let (section_kind, section_phase) = bm.sections.iter().rev()
        .find(|s| s.start_beat <= idx)
        .map(|s| {
            let span = (s.end_beat.saturating_sub(s.start_beat)).max(1) as f32;
            let sp = (idx - s.start_beat) as f32 / span;
            (s.kind, sp.clamp(0.0, 1.0))
        })
        .unwrap_or((SectionKind::Unknown, 0.0));

    BeatState {
        phase,
        is_downbeat: bm.is_downbeat[idx],
        palette_index: bm.beat_palette_index[idx],
        section_kind,
        section_phase,
    }
}

// ── Rendering ──────────────────────────────────────────────────────────────────

#[inline(always)]
fn set_all(leds: &mut [RGB8], r: u8, g: u8, b: u8) {
    for led in leds.iter_mut() {
        *led = RGB8 { r, g, b };
    }
}

/// **Drop**: maximum-energy strobe — ultra-sharp attack, power-cube decay,
/// white burst on every beat (not just downbeats).
fn fill_drop(leds: &mut [RGB8], bs: &BeatState) {
    let c = PALETTE[bs.palette_index];
    // Very fast decay: power-3 for extra snap.
    let flash = if bs.phase < 0.03 {
        1.0f32
    } else {
        ((1.0 - bs.phase) * 1.6).max(0.0).powi(3)
    };
    // White burst on every beat; stronger on downbeats.
    let white_window = if bs.is_downbeat { 0.12 } else { 0.06 };
    let white = if bs.phase < white_window {
        let intensity = if bs.is_downbeat { 0.85 } else { 0.55 };
        (1.0 - bs.phase / white_window) * intensity
    } else {
        0.0
    };
    set_all(
        leds,
        (c[0] as f32 * flash + 255.0 * white).min(255.0) as u8,
        (c[1] as f32 * flash + 255.0 * white).min(255.0) as u8,
        (c[2] as f32 * flash + 255.0 * white).min(255.0) as u8,
    );
}

/// **Buildup**: intensity and sharpness escalate with section_phase (0 → 1),
/// conveying mounting tension leading into the drop.
fn fill_buildup(leds: &mut [RGB8], bs: &BeatState) {
    let c = PALETTE[bs.palette_index];
    // Brightness floor rises 15 % → 60 % as buildup progresses.
    let floor = 0.15 + 0.45 * bs.section_phase;
    // Decay gets sharper (power 2 → 4).
    let power = 2.0 + 2.0 * bs.section_phase as f64;
    let decay = ((1.0 - bs.phase as f64) * 1.4).max(0.0).powf(power) as f32;
    let flash = (decay + floor).min(1.0);
    // Downbeat white burst also intensifies toward the end.
    let white = if bs.is_downbeat && bs.phase < 0.1 {
        (1.0 - bs.phase / 0.1) * (0.25 + 0.5 * bs.section_phase)
    } else {
        0.0
    };
    set_all(
        leds,
        (c[0] as f32 * flash + 255.0 * white).min(255.0) as u8,
        (c[1] as f32 * flash + 255.0 * white).min(255.0) as u8,
        (c[2] as f32 * flash + 255.0 * white).min(255.0) as u8,
    );
}

/// **Chorus**: punchy standard beat-sync with an enhanced downbeat white burst.
fn fill_chorus(leds: &mut [RGB8], bs: &BeatState) {
    let c = PALETTE[bs.palette_index];
    let flash = if bs.phase < 0.05 {
        1.0f32
    } else {
        ((1.0 - bs.phase) * 1.4).max(0.0).powi(2)
    };
    let white = if bs.is_downbeat && bs.phase < 0.10 {
        (1.0 - bs.phase / 0.10) * 0.65
    } else {
        0.0
    };
    set_all(
        leds,
        (c[0] as f32 * flash + 255.0 * white).min(255.0) as u8,
        (c[1] as f32 * flash + 255.0 * white).min(255.0) as u8,
        (c[2] as f32 * flash + 255.0 * white).min(255.0) as u8,
    );
}

/// **Verse**: gentle beat-sync at 70 % brightness — calm, steady.
fn fill_verse(leds: &mut [RGB8], bs: &BeatState) {
    let c = PALETTE[bs.palette_index];
    let flash = if bs.phase < 0.05 {
        0.70f32
    } else {
        ((1.0 - bs.phase) * 1.0).max(0.0).powi(2) * 0.70
    };
    set_all(
        leds,
        (c[0] as f32 * flash).min(255.0) as u8,
        (c[1] as f32 * flash).min(255.0) as u8,
        (c[2] as f32 * flash).min(255.0) as u8,
    );
}

/// **Breakdown**: slow sine-shaped breath — one long inhale/exhale per beat,
/// minimal brightness, giving the strip a "resting" feel.
fn fill_breakdown(leds: &mut [RGB8], bs: &BeatState) {
    let c = PALETTE[bs.palette_index];
    let pulse = (std::f32::consts::PI * (1.0 - bs.phase)).sin() * 0.35 + 0.04;
    set_all(
        leds,
        (c[0] as f32 * pulse).min(255.0) as u8,
        (c[1] as f32 * pulse).min(255.0) as u8,
        (c[2] as f32 * pulse).min(255.0) as u8,
    );
}

/// **Intro / outro / bridge**: rainbow breathing with a very gentle beat accent.
fn fill_soft(leds: &mut [RGB8], bs: &BeatState, t: f64, lut: &[RGB8; 256]) {
    let center = (t * 6.0).rem_euclid(256.0);
    let spread = 50.0 + 20.0 * (t * 0.25).sin();
    fill_gradient(leds, center - spread, center + spread, lut);
    // Slight brightness lift on each beat.
    if bs.phase < 0.5 {
        let boost = 1.0 + (1.0 - bs.phase * 2.0) * 0.25;
        for led in leds.iter_mut() {
            led.r = (led.r as f32 * boost).min(255.0) as u8;
            led.g = (led.g as f32 * boost).min(255.0) as u8;
            led.b = (led.b as f32 * boost).min(255.0) as u8;
        }
    }
}

/// **Default** beat-sync (unknown section kind — same as previous behaviour).
fn fill_beat_synced(leds: &mut [RGB8], bs: &BeatState) {
    let c = PALETTE[bs.palette_index];
    let flash = if bs.phase < 0.05 {
        1.0f32
    } else {
        ((1.0 - bs.phase) * 1.4).max(0.0).powi(2)
    };
    let white = if bs.is_downbeat && bs.phase < 0.08 {
        (1.0 - bs.phase / 0.08) * 0.55
    } else {
        0.0
    };
    set_all(
        leds,
        (c[0] as f32 * flash + 255.0 * white).min(255.0) as u8,
        (c[1] as f32 * flash + 255.0 * white).min(255.0) as u8,
        (c[2] as f32 * flash + 255.0 * white).min(255.0) as u8,
    );
}

/// Dispatch to the correct fill function based on section kind.
fn fill_section(leds: &mut [RGB8], bs: &BeatState, t: f64, lut: &[RGB8; 256]) {
    match bs.section_kind {
        SectionKind::Drop                                                    => fill_drop(leds, bs),
        SectionKind::Buildup                                                 => fill_buildup(leds, bs),
        SectionKind::Chorus                                                  => fill_chorus(leds, bs),
        SectionKind::Verse                                                   => fill_verse(leds, bs),
        SectionKind::Breakdown                                               => fill_breakdown(leds, bs),
        SectionKind::Intro | SectionKind::Outro | SectionKind::Bridge       => fill_soft(leds, bs, t, lut),
        SectionKind::Unknown                                                 => fill_beat_synced(leds, bs),
    }
}

/// Rainbow breathing — used when no beatmap is active or playback is paused.
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

/// Fetches currently-playing track.  Returns `(track_id, progress_ms,
/// is_playing, polled_at)` where `polled_at` is the Instant corresponding to
/// when `progress_ms` was accurate (≈ midpoint of the HTTP round-trip), so
/// that the render thread's linear extrapolation stays phase-accurate.
fn fetch_current_track(auth: &SpotifyAuth) -> Option<(String, u32, bool, Instant)> {
    // Capture wall time before the request so we can bracket the latency.
    let t0 = Instant::now();
    let resp = ureq::get("https://api.spotify.com/v1/me/player/currently-playing")
        .set("Authorization", &format!("Bearer {}", auth.access_token))
        .call();

    match resp {
        Ok(r) if r.status() == 204 => None,
        Ok(r) => {
            let body: serde_json::Value = serde_json::from_reader(r.into_reader()).ok()?;
            // Measure total round-trip (including body) now that we have the data.
            let rtt_ms = t0.elapsed().as_millis() as u64;
            if body["currently_playing_type"].as_str() != Some("track") {
                return None;
            }
            let id = body["item"]["id"].as_str()?.to_owned();
            let progress_ms = body["progress_ms"].as_u64().unwrap_or(0) as u32;
            let playing = body["is_playing"].as_bool().unwrap_or(false);
            // The server generated progress_ms at ~t0 + rtt/2 (midpoint of RTT).
            // Storing polled_at = t0 + rtt/2 means elapsed() gives the correct
            // extrapolation offset with no further adjustment.
            let polled_at = t0 + Duration::from_millis(rtt_ms / 2);
            Some((id, progress_ms, playing, polled_at))
        }
        Err(e) => {
            eprintln!("[spotify] poll error: {}", e);
            None
        }
    }
}

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
            eprintln!(
                "[beatmap] loaded {} beats, {} sections for {}",
                raw.timing.beat_deltas_ms.len() + 1,
                raw.sections.len(),
                track_id
            );
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
                if last_track_id.is_some() {
                    eprintln!("[spotify] playback stopped");
                    last_track_id = None;
                }
                let mut g = state.lock().unwrap();
                g.is_playing = false;
                g.track_id = None;
                g.beatmap = None;
            }

            Some((track_id, progress_ms, is_playing, polled_at)) => {
                let beatmap = if last_track_id.as_deref() != Some(&track_id) {
                    eprintln!("[spotify] track: {}", track_id);
                    last_track_id = Some(track_id.clone());
                    fetch_beatmap(&config.luminode_url, &track_id)
                } else {
                    state.lock().unwrap().beatmap.clone()
                };

                let mut g = state.lock().unwrap();
                g.track_id = Some(track_id);
                g.is_playing = is_playing;
                g.progress_ms = progress_ms;
                g.polled_at = Some(polled_at);
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

    {
        let state = Arc::clone(&state);
        thread::spawn(move || poll_loop(config, state));
    }

    loop {
        let t = start.elapsed().as_secs_f64();

        let (playing, pos_ms, beatmap) = {
            let g = state.lock().unwrap();
            (g.is_playing, g.current_position_ms(), g.beatmap.clone())
        };

        if playing {
            if let Some(ref bm) = beatmap {
                let bs = beat_state_at(bm, pos_ms);
                fill_section(&mut leds, &bs, t, &hsv_lut);
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
