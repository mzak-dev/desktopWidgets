//! What the speakers play (WASAPI loopback), for visualizers: frequency `bands` with their
//! `peaks`, `level`, `bass`, the waveform as an oscilloscope draws it (`wave`, -1..1), the
//! last second or so of bands for a waterfall (`history`, oldest first) and `active`. Built in because WebAssembly cannot reach WASAPI (ADR-0009).
//!
//! Capture runs only while a widget reads `audio`, and stops a few seconds after the last
//! one does. A widget waiting through silence still counts: it sleeps (no cadence) and the
//! capture thread wakes it when sound starts. Each widget has its own bands and smoothing,
//! from its params: `bands fmin fmax gain attack release peak_fall timebase`.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use super::{Cadence, DataSource, Notifier, SourceCx};
use crate::value::Value;

/// Samples per analysis: about 43 ms at 48 kHz, 23 Hz per bin.
pub const WINDOW: usize = 2048;
/// Samples kept: the window, and room before it to find where a wave starts.
pub const RING: usize = WINDOW * 2;
/// Points across `wave`.
pub const WAVE_POINTS: usize = 192;
/// How fast the waveform's scale follows a quieter passage: half way in this many seconds.
const WAVE_HALF_LIFE: f32 = 1.5;
/// The most the waveform is magnified, so hiss stays a flat line.
const WAVE_MAX_ZOOM: f32 = 20.0;
/// Rows of `history`, one every `HISTORY_ROW` seconds.
pub const HISTORY_ROWS: usize = 24;
const HISTORY_ROW: f32 = 0.06;
/// How often a sounding visualizer redraws.
pub const FRAME_MS: u32 = 33;
/// Sound this recent makes the source `active`.
const ACTIVE_FOR: Duration = Duration::from_millis(300);
/// A widget that read this recently is a reader.
const READER_TTL: Duration = Duration::from_secs(5);
/// With no readers this long, capture stops.
const STOP_AFTER: Duration = Duration::from_secs(3);
/// The quietest level shown, in dB below full scale.
const FLOOR_DB: f32 = 65.0;
/// Below this a sample is silence.
const SILENCE: f32 = 1e-4;

/// One widget's view, from its params.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub bands: usize,
    pub fmin: f32,
    pub fmax: f32,
    /// Multiplies the signal: 2 is +6 dB.
    pub gain: f32,
    /// How fast a band rises and falls, 0..1 per 1/30 s.
    pub attack: f32,
    pub release: f32,
    /// How fast a peak marker falls, in full heights per second.
    pub peak_fall: f32,
    /// Milliseconds of sound across `wave`.
    pub timebase: f32,
}

impl Settings {
    pub fn from_params(p: &BTreeMap<String, Value>) -> Settings {
        let n = |k: &str, d: f64| p.get(k).and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(d) as f32;
        let fmin = n("fmin", 40.0).clamp(10.0, 20_000.0);
        Settings {
            bands: n("bands", 32.0).round().clamp(1.0, 128.0) as usize,
            fmin,
            fmax: n("fmax", 16_000.0).clamp(fmin + 10.0, 24_000.0),
            gain: n("gain", 1.0).clamp(0.01, 100.0),
            attack: n("attack", 0.6).clamp(0.01, 1.0),
            release: n("release", 0.2).clamp(0.01, 1.0),
            peak_fall: n("peak_fall", 0.6).clamp(0.0, 20.0),
            timebase: n("timebase", 20.0).clamp(1.0, 60.0),
        }
    }
}

/// Amplitude (full scale 1) to 0..1 over the shown range.
fn height(amp: f32, gain: f32) -> f32 {
    let db = 20.0 * (amp * gain).max(1e-9).log10();
    ((db + FLOOR_DB) / FLOOR_DB).clamp(0.0, 1.0)
}

/// Magnitudes of `samples` (Hann-windowed), bin k at `k * rate / samples.len()`, scaled so a
/// full-scale sine reads 1.
pub fn spectrum(samples: &[f32], fft: &dyn Fft<f32>) -> Vec<f32> {
    let n = samples.len();
    let mut buf: Vec<Complex<f32>> = samples.iter().enumerate().map(|(i, s)| Complex::new(s * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()), 0.0)).collect();
    fft.process(&mut buf);
    // the Hann window halves a sine's amplitude, one side of the spectrum halves it again
    buf[..n / 2].iter().map(|c| c.norm() * 4.0 / n as f32).collect()
}

/// `s.bands` heights between `fmin` and `fmax`, spaced evenly in pitch (log frequency).
/// A band narrower than a bin takes the bin at its centre.
pub fn bands(mags: &[f32], rate: f32, s: &Settings) -> Vec<f32> {
    let n = mags.len().max(1) * 2;
    let bin = |f: f32| f * n as f32 / rate;
    let ratio = s.fmax / s.fmin;
    (0..s.bands)
        .map(|i| {
            let lo = s.fmin * ratio.powf(i as f32 / s.bands as f32);
            let hi = s.fmin * ratio.powf((i + 1) as f32 / s.bands as f32);
            let (a, b) = (bin(lo).ceil() as usize, bin(hi).floor() as usize);
            let amp = if a <= b && b < mags.len() {
                mags[a..=b].iter().copied().fold(0.0, f32::max)
            } else {
                mags.get(bin((lo * hi).sqrt()).round() as usize).copied().unwrap_or(0.0)
            };
            height(amp, s.gain)
        })
        .collect()
}

/// `points` values across the newest `span` of `samples`, each the mean of its stretch. Like
/// an oscilloscope's trigger, the span starts where the wave last rose through zero, so a
/// steady tone stands still from one frame to the next; without such a place, it is the
/// newest span. The crossing is found on a centred average, so treble adds no false starts,
/// and only after the wave has been below a tenth of its peak, so noise at zero adds none.
pub fn wave(samples: &[f32], span: usize, points: usize) -> Vec<f32> {
    let n = samples.len();
    if n < 2 || points == 0 {
        return vec![0.0; points];
    }
    let span = span.clamp(1, n * 3 / 4);
    let latest = n - span;
    let smooth = |i: usize| {
        let (a, b) = (i.saturating_sub(2), (i + 3).min(n));
        samples[a..b].iter().sum::<f32>() / (b - a) as f32
    };
    let below = -0.1 * samples.iter().fold(0f32, |m, x| m.max(x.abs()));
    let (mut armed, mut prev, mut start) = (false, 0.0, None);
    for i in 0..=latest {
        let y = smooth(i);
        armed |= y < below;
        if armed && prev < 0.0 && y >= 0.0 {
            start = Some(i);
            armed = false;
        }
        prev = y;
    }
    let start = start.unwrap_or(latest);
    (0..points)
        .map(|k| {
            let a = start + k * span / points;
            let b = (start + (k + 1) * span / points).clamp(a + 1, n);
            samples[a..b].iter().sum::<f32>() / (b - a) as f32
        })
        .collect()
}

/// One widget's moving picture: smoothed bands, falling peaks, level, bass and the waveform.
#[derive(Clone, Debug)]
pub struct Look {
    pub bands: Vec<f32>,
    pub peaks: Vec<f32>,
    pub level: f32,
    pub bass: f32,
    pub wave: Vec<f32>,
    /// The loudest of the waveform lately, which its scale follows.
    wave_peak: f32,
    /// Past bands, oldest first, always `HISTORY_ROWS` of them.
    pub history: VecDeque<Vec<f32>>,
    /// Seconds since the newest row.
    since_row: f32,
    at: Option<Instant>,
}

impl Look {
    fn new() -> Look {
        Look { bands: vec![], peaks: vec![], level: 0.0, bass: 0.0, wave: vec![], wave_peak: 0.0, history: VecDeque::new(), since_row: 0.0, at: None }
    }

    /// Moves toward `target` (heights, level, bass) as time has passed; returns the seconds.
    pub fn step(&mut self, target: &[f32], level: f32, bass: f32, now: Instant, s: &Settings) -> f32 {
        let dt = self.at.map_or(1.0 / 30.0, |t| now.saturating_duration_since(t).as_secs_f32().min(0.25));
        self.at = Some(now);
        // the rates are per 1/30 s; a longer frame moves further
        let frames = dt * 30.0;
        let rate = |k: f32| 1.0 - (1.0 - k).powf(frames);
        let (up, down) = (rate(s.attack), rate(s.release));
        let ease = |cur: f32, to: f32| cur + (to - cur) * if to > cur { up } else { down };
        self.bands.resize(target.len(), 0.0);
        self.peaks.resize(target.len(), 0.0);
        for (i, t) in target.iter().enumerate() {
            self.bands[i] = ease(self.bands[i], *t);
            self.peaks[i] = self.bands[i].max(self.peaks[i] - s.peak_fall * dt);
        }
        self.level = ease(self.level, level);
        self.bass = ease(self.bass, bass);
        // full of silence from the start, and again when the number of bands changes, so a
        // waterfall never changes its spacing
        if self.history.front().is_none_or(|r| r.len() != self.bands.len()) {
            self.history = std::iter::repeat_n(vec![0.0; self.bands.len()], HISTORY_ROWS).collect();
            self.since_row = 0.0;
        }
        // by the clock, not per frame: frames come faster while the widget is busy
        self.since_row += dt;
        if self.since_row >= HISTORY_ROW {
            self.since_row = (self.since_row - HISTORY_ROW).min(HISTORY_ROW);
            self.history.pop_front();
            self.history.push_back(self.bands.clone());
        }
        dt
    }

    /// Shows `raw` (from `wave`) scaled so the loudest of the last few seconds nearly fills
    /// the height, times `gain`: loopback can be quiet or loud, and a waveform is linear.
    /// The scale follows a louder sound at once and a quieter one slowly.
    pub fn fit_wave(&mut self, raw: &[f32], dt: f32, gain: f32) {
        let peak = raw.iter().fold(0f32, |m, x| m.max(x.abs()));
        self.wave_peak = peak.max(self.wave_peak * 0.5f32.powf(dt / WAVE_HALF_LIFE));
        let scale = gain * 0.9 / self.wave_peak.max(0.9 / WAVE_MAX_ZOOM);
        self.wave = raw.iter().map(|x| (x * scale).clamp(-1.0, 1.0)).collect();
    }

    /// Everything has fallen to rest: nothing moves until sound comes back.
    pub fn settled(&self) -> bool {
        self.bands.iter().chain(&self.peaks).chain([&self.level, &self.bass]).chain(self.history.iter().flatten()).all(|v| *v < 0.005) && self.wave.iter().all(|v| v.abs() < 0.005)
    }

    fn value(&self, active: bool) -> Value {
        let list = |v: &[f32]| Value::List(v.iter().map(|x| Value::Num(*x as f64)).collect());
        Value::obj([("bands", list(&self.bands)), ("peaks", list(&self.peaks)), ("level", Value::Num(self.level as f64)), ("bass", Value::Num(self.bass as f64)), ("wave", list(&self.wave)), ("history", Value::List(self.history.iter().map(|r| list(r)).collect())), ("active", Value::Bool(active))])
    }
}

/// Whether capture should go on: a widget read recently, or read when silence began and is
/// now asleep waiting for sound.
pub fn wanted(now: Instant, reads: impl IntoIterator<Item = Instant>, silent_since: Option<Instant>) -> bool {
    reads.into_iter().any(|r| now.saturating_duration_since(r) < READER_TTL || silent_since.is_some_and(|s| r + Duration::from_secs(1) >= s))
}

/// Interleaved samples to mono, as 32-bit float or 16-bit PCM.
pub fn to_mono(bytes: &[u8], channels: usize, float: bool) -> Vec<f32> {
    let channels = channels.max(1);
    let samples: Vec<f32> = if float {
        bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
    } else {
        bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect()
    };
    samples.chunks_exact(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect()
}

/// The latest samples and when sound was last heard, shared with the capture thread.
#[derive(Default)]
struct Ring {
    samples: VecDeque<f32>,
    rate: u32,
    last_sound: Option<Instant>,
    silent_since: Option<Instant>,
}

impl Ring {
    fn push(&mut self, mono: &[f32], now: Instant) -> bool {
        self.samples.extend(mono.iter().copied());
        let extra = self.samples.len().saturating_sub(RING);
        self.samples.drain(..extra);
        let woke = mono.iter().any(|s| s.abs() > SILENCE) && {
            let was_silent = self.last_sound.is_none_or(|t| now.saturating_duration_since(t) >= ACTIVE_FOR);
            self.last_sound = Some(now);
            self.silent_since = None;
            was_silent
        };
        if !woke && self.silent_since.is_none() && self.last_sound.is_none_or(|t| now.saturating_duration_since(t) >= ACTIVE_FOR) {
            self.silent_since = Some(now);
        }
        woke
    }

    fn active(&self, now: Instant) -> bool {
        self.last_sound.is_some_and(|t| now.saturating_duration_since(t) < ACTIVE_FOR)
    }
}

pub struct Audio {
    ring: Arc<Mutex<Ring>>,
    looks: Mutex<HashMap<String, Look>>,
    reads: Arc<Mutex<HashMap<String, Instant>>>,
    running: Arc<AtomicBool>,
    notify: Arc<Mutex<Option<Notifier>>>,
    fft: Arc<dyn Fft<f32>>,
}

impl Default for Audio {
    fn default() -> Self {
        Audio { ring: Arc::default(), looks: Mutex::default(), reads: Arc::default(), running: Arc::default(), notify: Arc::default(), fft: FftPlanner::new().plan_fft_forward(WINDOW) }
    }
}

impl Audio {
    fn ensure_capture(&self) {
        if self.running.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return;
        }
        let (ring, reads, running, notify) = (self.ring.clone(), self.reads.clone(), self.running.clone(), self.notify.clone());
        let spawned = std::thread::Builder::new().name("audio".into()).spawn(move || capture(&ring, &reads, &running, &notify));
        if spawned.is_err() {
            self.running.store(false, Ordering::SeqCst);
        }
    }
}

impl DataSource for Audio {
    fn name(&self) -> &str {
        "audio"
    }

    fn value(&self, cx: &SourceCx) -> Value {
        let now = Instant::now();
        self.reads.lock().unwrap().insert(cx.cfg.id.clone(), now);
        self.ensure_capture();
        let s = Settings::from_params(cx.params);
        let (active, samples, rate) = {
            let r = self.ring.lock().unwrap();
            (r.active(now), r.samples.iter().copied().collect::<Vec<f32>>(), r.rate)
        };
        let (target, level, bass, raw) = if active && samples.len() >= WINDOW && rate > 0 {
            let last = &samples[samples.len() - WINDOW..];
            let mags = spectrum(last, self.fft.as_ref());
            let rms = (last.iter().map(|x| x * x).sum::<f32>() / WINDOW as f32).sqrt();
            let bass = bands(&mags, rate as f32, &Settings { bands: 1, fmin: 20.0, fmax: 250.0, ..s.clone() })[0];
            let span = (s.timebase / 1000.0 * rate as f32).round() as usize;
            (bands(&mags, rate as f32, &s), height(rms * std::f32::consts::SQRT_2, s.gain), bass, wave(&samples, span, WAVE_POINTS))
        } else {
            (vec![0.0; s.bands], 0.0, 0.0, vec![0.0; WAVE_POINTS])
        };
        let mut looks = self.looks.lock().unwrap();
        let look = looks.entry(cx.cfg.id.clone()).or_insert_with(Look::new);
        let dt = look.step(&target, level, bass, now, &s);
        look.fit_wave(&raw, dt, s.gain);
        look.value(active)
    }

    /// While sound plays, and until a widget's bars have fallen to rest after it stops.
    fn cadence(&self, _field: &str, cx: &SourceCx) -> Option<Cadence> {
        let active = self.ring.lock().unwrap().active(Instant::now());
        let moving = self.looks.lock().unwrap().get(&cx.cfg.id).is_some_and(|l| !l.settled());
        (active || moving).then_some(Cadence::Millis(FRAME_MS))
    }

    fn retain(&self, live: &BTreeSet<String>) {
        self.looks.lock().unwrap().retain(|id, _| live.contains(id));
        self.reads.lock().unwrap().retain(|id, _| live.contains(id));
    }

    fn attach(&self, notify: Notifier) {
        *self.notify.lock().unwrap() = Some(notify);
    }
}

/// Captures until no widget wants it, reopening the device when it fails or the default
/// output changes.
fn capture(ring: &Mutex<Ring>, reads: &Mutex<HashMap<String, Instant>>, running: &AtomicBool, notify: &Mutex<Option<Notifier>>) {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    let still_wanted = || {
        let silent_since = ring.lock().unwrap().silent_since;
        wanted(Instant::now(), reads.lock().unwrap().values().copied(), silent_since)
    };
    let mut unwanted_since: Option<Instant> = None;
    let mut logged = false;
    loop {
        let result = loopback(ring, notify, &mut || {
            match (still_wanted(), unwanted_since) {
                (true, _) => unwanted_since = None,
                (false, None) => unwanted_since = Some(Instant::now()),
                (false, Some(t)) if t.elapsed() >= STOP_AFTER => return false,
                _ => {}
            }
            true
        });
        match result {
            Ok(()) => {
                // nobody reads: stop, unless a read came in while stopping
                running.store(false, Ordering::SeqCst);
                if !still_wanted() || running.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                    return;
                }
                unwanted_since = None;
            }
            Err(e) => {
                if !logged {
                    if let Some(n) = notify.lock().unwrap().as_ref() {
                        n.log(format!("audio capture: {e}"));
                    }
                    logged = true;
                }
                std::thread::sleep(Duration::from_secs(1));
                if !still_wanted() {
                    running.store(false, Ordering::SeqCst);
                    return;
                }
            }
        }
    }
}

/// One capture session on the default output. Returns Ok when `keep` says stop or, in
/// silence, when the default output changed (the caller reopens it); Err when the device fails.
fn loopback(ring: &Mutex<Ring>, notify: &Mutex<Option<Notifier>>, keep: &mut dyn FnMut() -> bool) -> windows::core::Result<()> {
    use windows::Win32::Media::Audio::{AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eConsole, eRender};
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};

    fn id_of(d: &IMMDevice) -> String {
        unsafe {
            let Ok(p) = d.GetId() else { return String::new() };
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        }
    }

    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let device_id = id_of(&device);
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let fmt = client.GetMixFormat()?;
        let f: WAVEFORMATEX = std::ptr::read_unaligned(fmt);
        let (tag, bits, channels, rate) = (f.wFormatTag, f.wBitsPerSample, f.nChannels as usize, f.nSamplesPerSec);
        // WAVE_FORMAT_IEEE_FLOAT, or WAVE_FORMAT_EXTENSIBLE naming float
        let float = tag == 3 || (tag == 0xFFFE && std::ptr::read_unaligned(fmt as *const WAVEFORMATEXTENSIBLE).SubFormat.data1 == 3);
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 200_000, 0, fmt, None);
        CoTaskMemFree(Some(fmt as *const _));
        init?;
        if !(float && bits == 32 || !float && bits == 16) {
            return Err(windows::core::Error::new(windows::Win32::Foundation::E_FAIL, format!("an output format it cannot read ({bits}-bit)")));
        }
        let cap: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        let frame_bytes = channels * bits as usize / 8;
        ring.lock().unwrap().rate = rate;
        let mut checked = Instant::now();
        let result = loop {
            if !keep() {
                break Ok(());
            }
            let silent = ring.lock().unwrap().silent_since.is_some_and(|t| t.elapsed() > Duration::from_secs(1));
            // in silence, look less often; the first sound wakes the widgets anyway
            std::thread::sleep(Duration::from_millis(if silent { 60 } else { 10 }));
            if silent && checked.elapsed() > Duration::from_secs(2) {
                checked = Instant::now();
                let now_id = devices.GetDefaultAudioEndpoint(eRender, eConsole).map(|d| id_of(&d));
                if now_id.is_ok_and(|id| id != device_id) {
                    break Ok(()); // the output changed: reopen on the new one
                }
            }
            let mut woke = false;
            loop {
                match cap.GetNextPacketSize() {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) => return Err(e),
                }
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let mono = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    vec![0.0; frames as usize]
                } else {
                    to_mono(std::slice::from_raw_parts(data, frames as usize * frame_bytes), channels, float)
                };
                cap.ReleaseBuffer(frames)?;
                woke |= ring.lock().unwrap().push(&mono, Instant::now());
            }
            if woke {
                if let Some(n) = notify.lock().unwrap().as_ref() {
                    n.changed();
                }
            }
        };
        let _ = client.Stop();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: f32, amp: f32) -> Vec<f32> {
        (0..WINDOW).map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / rate).sin()).collect()
    }

    fn params(p: &[(&str, f64)]) -> Settings {
        Settings::from_params(&p.iter().map(|(k, v)| (k.to_string(), Value::Num(*v))).collect())
    }

    #[test]
    fn a_tone_lights_its_own_band() {
        let fft = FftPlanner::new().plan_fft_forward(WINDOW);
        let mags = spectrum(&sine(1333.0, 48_000.0, 0.5), fft.as_ref());
        let s = params(&[("bands", 8.0), ("fmin", 100.0), ("fmax", 10_000.0)]);
        let b = bands(&mags, 48_000.0, &s);
        assert_eq!(b.len(), 8);
        let top = b.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(top, 4, "1.33 kHz sits in the fifth of eight bands from 100 Hz to 10 kHz: {b:?}");
        assert!(b[4] > 0.85 && b[0] < 0.3, "-6 dB near the top, the far bands low: {b:?}");
        let quiet = bands(&spectrum(&sine(1333.0, 48_000.0, 0.005), fft.as_ref()), 48_000.0, &s);
        assert!(quiet[4] < b[4] - 0.4, "40 dB quieter reads much lower");
        let louder = bands(&mags, 48_000.0, &params(&[("bands", 8.0), ("fmin", 100.0), ("fmax", 10_000.0), ("gain", 2.0)]));
        assert!(louder[4] >= b[4], "gain lifts it");
    }

    #[test]
    fn params_have_defaults_and_bounds() {
        let d = params(&[]);
        assert_eq!((d.bands, d.fmin, d.fmax, d.gain, d.timebase), (32, 40.0, 16_000.0, 1.0, 20.0));
        let wild = params(&[("bands", 5000.0), ("fmin", 900.0), ("fmax", 100.0), ("attack", 7.0), ("timebase", 900.0)]);
        assert_eq!((wild.bands, wild.fmax > wild.fmin, wild.attack, wild.timebase), (128, true, 1.0, 60.0));
    }

    #[test]
    fn bands_rise_fast_fall_slow_and_peaks_drift_down() {
        let s = params(&[("attack", 0.8), ("release", 0.1), ("peak_fall", 1.0)]);
        let t0 = Instant::now();
        let mut l = Look::new();
        l.step(&[1.0], 1.0, 1.0, t0, &s);
        assert!((l.bands[0] - 0.8).abs() < 1e-4, "{:?}", l.bands);
        let f = Duration::from_millis(33);
        l.step(&[0.0], 0.0, 0.0, t0 + f, &s);
        assert!(l.bands[0] > 0.6, "falls slowly: {:?}", l.bands);
        assert!((l.peaks[0] - 0.8 + 0.033).abs() < 0.01, "the peak falls at 1 height a second: {:?}", l.peaks);
        assert!(!l.settled());
        for i in 2..200 {
            l.step(&[0.0], 0.0, 0.0, t0 + f * i, &s);
        }
        assert!(l.settled(), "at rest after the sound stops: {:?}", l.bands);
    }

    #[test]
    fn a_row_of_history_every_60_ms_until_the_last_loud_one_has_gone() {
        let s = params(&[("bands", 2.0), ("attack", 1.0), ("release", 1.0), ("peak_fall", 20.0)]);
        let t0 = Instant::now();
        let ms = |m: u64| t0 + Duration::from_millis(m);
        let mut l = Look::new();
        l.step(&[1.0, 0.2], 1.0, 0.0, t0, &s);
        assert_eq!(l.history.len(), HISTORY_ROWS, "full from the start");
        assert!(l.history.iter().all(|r| r == &[0.0, 0.0]), "of silence");
        for i in 1..=6 {
            l.step(&[1.0, 0.2], 1.0, 0.0, ms(20 * i), &s);
        }
        assert_eq!(l.history.back(), Some(&vec![1.0, 0.2]), "the newest row last");
        assert_eq!((l.history.len(), l.history.iter().filter(|r| r[0] == 1.0).count()), (HISTORY_ROWS, 2), "a row every 60 ms in 153 ms of frames");
        for i in 7..=12 {
            l.step(&[0.0, 0.0], 0.0, 0.0, ms(20 * i), &s);
        }
        assert!(l.bands.iter().chain(&l.peaks).all(|v| *v == 0.0), "the bands are at rest: {:?} {:?}", l.bands, l.peaks);
        assert!(!l.settled(), "but loud rows still recede");
        for i in 13..=120 {
            l.step(&[0.0, 0.0], 0.0, 0.0, ms(20 * i), &s);
        }
        assert!(l.settled(), "until they have gone");
        l.step(&[0.5; 3], 0.0, 0.0, ms(2420), &s);
        assert!(l.history.len() == HISTORY_ROWS && l.history.iter().all(|r| r == &[0.0; 3]), "more bands start it again");
    }

    #[test]
    fn capture_runs_while_widgets_read_or_wait_through_silence() {
        // counted forward from now: a clock that started under ten minutes ago cannot go back 600 s
        let now = Instant::now() + Duration::from_secs(3600);
        let ago = |s: u64| now - Duration::from_secs(s);
        assert!(!wanted(now, [], None), "no widget reads audio");
        assert!(wanted(now, [ago(1)], None), "a widget reads it");
        assert!(!wanted(now, [ago(30)], None), "it stopped reading while sound played");
        assert!(wanted(now, [ago(600)], Some(ago(600))), "it fell asleep when silence began and waits for sound");
        assert!(!wanted(now, [ago(600)], Some(ago(60))), "it had stopped reading before the silence");
    }

    #[test]
    fn silence_and_sound_are_told_apart() {
        let t0 = Instant::now();
        let mut r = Ring::default();
        assert!(!r.push(&[0.0; 480], t0) && !r.active(t0));
        assert!(r.silent_since.is_some());
        assert!(r.push(&[0.3; 480], t0 + Duration::from_secs(1)), "sound after silence wakes the widgets");
        assert!(r.active(t0 + Duration::from_secs(1)) && r.silent_since.is_none());
        assert!(!r.push(&[0.3; 480], t0 + Duration::from_millis(1010)), "still sounding: no new wake");
        assert_eq!(r.samples.len(), 1440);
        r.push(&vec![0.0; RING + 100], t0 + Duration::from_secs(3));
        assert_eq!((r.samples.len(), r.active(t0 + Duration::from_secs(3))), (RING, false));
    }

    #[test]
    fn a_steady_tone_stands_still_on_the_scope() {
        // the same 220 Hz tone, caught at a different moment each frame
        let frames: Vec<Vec<f32>> = [0.0f32, 0.9, 2.3, 4.0]
            .iter()
            .map(|phase| (0..RING).map(|i| 0.5 * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0 + phase).sin()).collect())
            .collect();
        let span = 960; // 20 ms
        let waves: Vec<Vec<f32>> = frames.iter().map(|f| wave(f, span, 96)).collect();
        assert_eq!(waves[0].len(), 96);
        for w in &waves {
            assert!(w[0].abs() < 0.1 && w[1] > w[0], "it starts rising through zero: {:?}", &w[..4]);
            let diff = w.iter().zip(&waves[0]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            assert!(diff < 0.06, "every frame draws the same picture: off by {diff}");
        }
        let top = waves[0].iter().copied().fold(0.0, f32::max);
        assert!((top - 0.5).abs() < 0.03, "its height is the tone's: {top}");
    }

    #[test]
    fn a_wave_without_a_crossing_shows_the_newest_sound() {
        let mut ramp: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        let w = wave(&ramp, 100, 4);
        assert!(w.iter().zip([0.912, 0.937, 0.962, 0.987]).all(|(a, b)| (a - b).abs() < 1e-4), "the last 100 samples, 25 to a point: {w:?}");
        ramp.iter_mut().for_each(|x| *x = 0.0);
        assert_eq!(wave(&ramp, 100, 3), [0.0; 3], "silence is a flat line");
        assert_eq!(wave(&[], 100, 3), [0.0; 3]);
        assert_eq!(wave(&[0.5; 8], 100, 4).len(), 4, "a span longer than the samples is cut to fit");
    }

    #[test]
    fn the_scope_fills_its_height_whatever_the_volume() {
        let s = params(&[]);
        let quiet = [0.0, 0.05, -0.05, 0.02];
        let mut l = Look::new();
        l.fit_wave(&quiet, 1.0 / 30.0, s.gain);
        assert!((l.wave[1] - 0.9).abs() < 1e-4, "a quiet wave is magnified to 90%: {:?}", l.wave);
        l.fit_wave(&[0.0, 0.5, -0.5, 0.0], 1.0 / 30.0, s.gain);
        assert!((l.wave[1] - 0.9).abs() < 1e-4, "a louder one is shrunk at once: {:?}", l.wave);
        l.fit_wave(&quiet, 1.5, s.gain);
        assert!((l.wave[1] - 0.18).abs() < 1e-3, "after it, a quiet one grows back slowly: {:?}", l.wave);
        let mut hiss = Look::new();
        hiss.fit_wave(&[0.0, 0.001, -0.001], 1.0 / 30.0, 2.0);
        assert!(hiss.wave[1] < 0.05, "hiss is magnified at most 20 times: {:?}", hiss.wave);
        hiss.fit_wave(&[0.0, 0.9, -0.9], 1.0 / 30.0, 2.0);
        assert_eq!(hiss.wave[1..], [1.0, -1.0], "gain past the top is clipped");
        assert!(!hiss.settled());
        hiss.fit_wave(&[0.0; 3], 1.0 / 30.0, 2.0);
        assert!(hiss.settled(), "a flat line is at rest");
    }

    #[test]
    fn stereo_and_16_bit_mix_down_to_mono() {
        let f: Vec<u8> = [0.5f32, -0.5, 1.0, 0.0].iter().flat_map(|x| x.to_le_bytes()).collect();
        assert_eq!(to_mono(&f, 2, true), [0.0, 0.5]);
        let i: Vec<u8> = [16384i16, 16384].iter().flat_map(|x| x.to_le_bytes()).collect();
        assert_eq!(to_mono(&i, 1, false), [0.5, 0.5]);
    }
}
