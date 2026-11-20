//! Real-time engine + the background thread that prepares IRs.
//!
//! The audio thread never touches the filesystem, never allocates on the hot
//! path, and never locks. Everything expensive happens in `spawn_worker`.

use crate::analysis::{analyze_ir, shape_ir, IrAnalysis};
use crate::params::RoomcheckParams;
use crate::presets::{self, Preset};
use ebur128_stream::{Analyzer, AnalyzerBuilder, Channel, Mode};
use nice_plug::prelude::*;
use nice_plug::util;
use rt_fft_convolver::{DenormalGuard, UniformPartitionEngine};
use std::f32::consts::FRAC_PI_2;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

pub const BLOCK: usize = 128;
pub const SPECTRUM_BANDS: usize = 76;
const FFT_SIZE: usize = 1024;
const CROSSFADE_SECS: f32 = 0.03;

fn store_f32(atom: &AtomicU32, value: f32) {
    atom.store(value.to_bits(), Ordering::Relaxed);
}

fn load_f32(atom: &AtomicU32) -> f32 {
    f32::from_bits(atom.load(Ordering::Relaxed))
}

#[derive(Default)]
pub struct Meters {
    original: AtomicU32,
    environment: AtomicU32,
}

impl Meters {
    pub fn new() -> Self {
        Self {
            original: AtomicU32::new((-70.0f32).to_bits()),
            environment: AtomicU32::new((-70.0f32).to_bits()),
        }
    }

    pub fn original(&self) -> f32 {
        load_f32(&self.original)
    }

    pub fn environment(&self) -> f32 {
        load_f32(&self.environment)
    }

    /// Published by the audio thread; also used to seed UI snapshots.
    pub fn set(&self, original: f32, environment: f32) {
        store_f32(&self.original, original);
        store_f32(&self.environment, environment);
    }
}

// ---------------------------------------------------------------------------
// Build requests
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct BuildRequest {
    generation: u64,
    index: usize,
    sample_rate: u32,
    tail: f32,
    distance: f32,
    intensity: f32,
}

pub struct WorkerHandles {
    pub convolvers: rtrb::Consumer<PreparedConvolver>,
    pub analysis: triple_buffer::Output<Arc<IrAnalysis>>,
}

struct CachedIr {
    preset_id: String,
    sample_rate: u32,
    channels: Vec<Vec<f32>>,
}

pub fn spawn_worker(
    params: Arc<RoomcheckParams>,
    library: Arc<RwLock<Vec<Preset>>>,
    sample_rate: Arc<AtomicU32>,
    generation: Arc<AtomicU64>,
) -> WorkerHandles {
    let (mut conv_tx, conv_rx) = rtrb::RingBuffer::<PreparedConvolver>::new(4);
    let (mut analysis_tx, analysis_out) = triple_buffer::triple_buffer(&Arc::new(IrAnalysis::default()));

    thread::Builder::new()
        .name("roomcheck-ir-build".to_owned())
        .spawn(move || {
            let mut cache: Option<CachedIr> = None;
            let mut applied: Option<(usize, u32, u32, u32, u32)> = None;

            loop {
                // IR preparation is background work; polling at ~28 Hz keeps
                // knob drags responsive without ever touching the audio thread.
                thread::sleep(Duration::from_millis(35));

                let sr = sample_rate.load(Ordering::Relaxed);
                if sr == 0 {
                    continue;
                }
                let index = {
                    let len = library.read().map(|lib| lib.len()).unwrap_or(0);
                    (params.preset.value().max(0) as usize).min(len.saturating_sub(1))
                };
                let key = (
                    index,
                    sr,
                    params.tail.value().to_bits(),
                    params.distance.value().to_bits(),
                    params.intensity.value().to_bits(),
                );
                if applied == Some(key) {
                    continue;
                }
                applied = Some(key);

                let g = generation.fetch_add(1, Ordering::Relaxed) + 1;
                let req = BuildRequest {
                    generation: g,
                    index,
                    sample_rate: sr,
                    tail: params.tail.value(),
                    distance: params.distance.value(),
                    intensity: params.intensity.value(),
                };
                let preset = match library.read() {
                    Ok(lib) => lib.get(req.index).cloned(),
                    Err(_) => None,
                };
                let Some(preset) = preset else { continue };

                let needs_load = cache
                    .as_ref()
                    .map(|c| c.preset_id != preset.id || c.sample_rate != req.sample_rate)
                    .unwrap_or(true);

                if needs_load {
                    match presets::load_preset(&preset, req.sample_rate) {
                        Ok(channels) => {
                            cache = Some(CachedIr {
                                preset_id: preset.id.clone(),
                                sample_rate: req.sample_rate,
                                channels,
                            });
                        }
                        Err(err) => {
                            nice_error!("Roomcheck: could not load IR: {err}");
                            continue;
                        }
                    }
                }

                let Some(cached) = cache.as_ref() else {
                    continue;
                };

                let sr = req.sample_rate as f32;
                let shaped: Vec<Vec<f32>> = cached
                    .channels
                    .iter()
                    .map(|ch| shape_ir(ch, sr, req.tail, req.distance, req.intensity))
                    .collect();

                let analysis = analyze_ir(&shaped, sr);
                let gain = analysis.loudness_match_gain;
                let prepared = PreparedConvolver {
                    slot: Slot::new(&shaped, gain),
                };

                // Drop stale results if the user kept dragging. Force another
                // poll so the latest settings still get built.
                if req.generation != generation.load(Ordering::Relaxed) {
                    applied = None;
                    continue;
                }

                let _ = analysis_tx.write(Arc::new(analysis));
                if conv_tx.push(prepared).is_err() {
                    nice_error!("Roomcheck: convolver ring full, dropping build");
                }
            }
        })
        .expect("failed to spawn Roomcheck build thread");

    WorkerHandles {
        convolvers: conv_rx,
        analysis: analysis_out,
    }
}

// ---------------------------------------------------------------------------
// Prepared convolver
// ---------------------------------------------------------------------------

type Engine = UniformPartitionEngine;

pub struct Slot {
    ll: Option<Engine>,
    lr: Option<Engine>,
    rl: Option<Engine>,
    rr: Option<Engine>,
    gain: f32,
    wet_l: [f32; BLOCK],
    wet_r: [f32; BLOCK],
    tmp: [f32; BLOCK],
}

impl Slot {
    pub fn new(channels: &[Vec<f32>], gain: f32) -> Self {
        let mut slot = Self {
            ll: None,
            lr: None,
            rl: None,
            rr: None,
            gain,
            wet_l: [0.0; BLOCK],
            wet_r: [0.0; BLOCK],
            tmp: [0.0; BLOCK],
        };
        match channels.len() {
            0 => {}
            1 => {
                slot.ll = Some(Engine::new(&channels[0], BLOCK));
                slot.rr = Some(Engine::new(&channels[0], BLOCK));
            }
            2 => {
                slot.ll = Some(Engine::new(&channels[0], BLOCK));
                slot.rr = Some(Engine::new(&channels[1], BLOCK));
            }
            _ => {
                slot.ll = Some(Engine::new(&channels[0], BLOCK));
                slot.lr = Some(Engine::new(&channels[1], BLOCK));
                slot.rl = Some(Engine::new(&channels[2], BLOCK));
                slot.rr = Some(Engine::new(&channels[3], BLOCK));
            }
        }
        slot
    }

    fn process(&mut self, in_l: &[f32; BLOCK], in_r: &[f32; BLOCK]) {
        let Slot {
            ll,
            lr,
            rl,
            rr,
            wet_l,
            wet_r,
            tmp,
            ..
        } = self;
        wet_l.fill(0.0);
        wet_r.fill(0.0);

        let mut apply = |engine: &mut Option<Engine>,
                         input: &[f32; BLOCK],
                         dst: &mut [f32; BLOCK]| {
            if let Some(engine) = engine {
                tmp.fill(0.0);
                engine.process(input, tmp);
                for i in 0..BLOCK {
                    dst[i] += tmp[i];
                }
            }
        };

        apply(ll, in_l, wet_l);
        apply(lr, in_l, wet_r);
        apply(rl, in_r, wet_l);
        apply(rr, in_r, wet_r);
    }
}

pub struct PreparedConvolver {
    slot: Slot,
}

// ---------------------------------------------------------------------------
// Real-time processor
// ---------------------------------------------------------------------------

pub struct RoomcheckDsp {
    sample_rate: f32,
    active: Option<Slot>,
    fading: Option<Slot>,
    fade_pos: usize,
    fade_len: usize,

    in_l: [f32; BLOCK],
    in_r: [f32; BLOCK],
    dry_l: [f32; BLOCK],
    dry_r: [f32; BLOCK],
    pos: usize,

    bypass_ramp: f32,

    conv_rx: rtrb::Consumer<PreparedConvolver>,
    spectrum_tx: triple_buffer::Input<[f32; SPECTRUM_BANDS]>,
    meters: Arc<Meters>,

    lufs_orig: Option<Analyzer>,
    lufs_env: Option<Analyzer>,

    fft: Option<Arc<dyn realfft::RealToComplex<f32>>>,
    fft_input: Vec<f32>,
    fft_output: Vec<realfft::num_complex::Complex32>,
    spectrum_ring: [f32; FFT_SIZE],
    spectrum_pos: usize,
    spectrum_fill: usize,
    window: Vec<f32>,
}

impl RoomcheckDsp {
    pub fn new(
        conv_rx: rtrb::Consumer<PreparedConvolver>,
        spectrum_tx: triple_buffer::Input<[f32; SPECTRUM_BANDS]>,
        meters: Arc<Meters>,
    ) -> Self {
        Self {
            sample_rate: 48_000.0,
            active: None,
            fading: None,
            fade_pos: 0,
            fade_len: 1,
            in_l: [0.0; BLOCK],
            in_r: [0.0; BLOCK],
            dry_l: [0.0; BLOCK],
            dry_r: [0.0; BLOCK],
            pos: 0,
            bypass_ramp: 1.0,
            conv_rx,
            spectrum_tx,
            meters,
            lufs_orig: None,
            lufs_env: None,
            fft: None,
            fft_input: vec![0.0; FFT_SIZE],
            fft_output: Vec::new(),
            spectrum_ring: [0.0; FFT_SIZE],
            spectrum_pos: 0,
            spectrum_fill: 0,
            window: (0..FFT_SIZE)
                .map(|i| {
                    let x = i as f32 / (FFT_SIZE - 1) as f32;
                    0.5 - 0.5 * (2.0 * std::f32::consts::PI * x).cos()
                })
                .collect(),
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.fade_len = ((self.sample_rate * CROSSFADE_SECS) as usize).max(BLOCK);

        let make = || {
            AnalyzerBuilder::new()
                .sample_rate(self.sample_rate as u32)
                .channels(&[Channel::Left, Channel::Right])
                .modes(Mode::Momentary)
                .build()
                .ok()
        };
        self.lufs_orig = make();
        self.lufs_env = make();

        let mut planner = realfft::RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        self.fft_output = vec![realfft::num_complex::Complex32::default(); FFT_SIZE / 2 + 1];
        self.fft = Some(fft);
    }

    pub fn reset(&mut self) {
        self.active = None;
        self.fading = None;
        self.in_l.fill(0.0);
        self.in_r.fill(0.0);
        self.dry_l.fill(0.0);
        self.dry_r.fill(0.0);
        self.pos = 0;
        self.spectrum_ring.fill(0.0);
        self.spectrum_fill = 0;
        if let Some(a) = self.lufs_orig.as_mut() {
            a.reset();
        }
        if let Some(a) = self.lufs_env.as_mut() {
            a.reset();
        }
    }

    pub fn start_fade(&mut self, next: PreparedConvolver) {
        self.fading = self.active.take();
        self.active = Some(next.slot);
        self.fade_pos = 0;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        params: &RoomcheckParams,
        metering: bool,
    ) {
        let frames = left.len().min(right.len());
        if frames == 0 {
            return;
        }

        // A new convolver may be waiting.
        if let Ok(next) = self.conv_rx.pop() {
            if self.active.is_some() {
                self.start_fade(next);
            } else {
                self.active = Some(next.slot);
                self.fade_pos = self.fade_len;
                // Fade the very first reference in instead of dropping it on
                // top of the dry signal.
                self.bypass_ramp = 0.0;
            }
        }

        if self.active.is_none() {
            // Nothing loaded yet: pass audio through untouched.
            return;
        }

        let _denormals = DenormalGuard::new();

        let mono_sum = params.mono_sum.value();
        let match_loudness = params.match_loudness.value();
        let bypass_target = if params.bypass.value() { 0.0 } else { 1.0 };
        let ramp_step = 1.0 / (self.sample_rate * 0.015).max(1.0);


        for i in 0..frames {
            let dry_in_l = left[i];
            let dry_in_r = right[i];

            let (cond_l, cond_r) = if mono_sum {
                let m = 0.5 * (dry_in_l + dry_in_r);
                (m, m)
            } else {
                (dry_in_l, dry_in_r)
            };

            // Delayed dry, so dry and wet stay sample-aligned.
            let dry_l = self.dry_l[self.pos];
            let dry_r = self.dry_r[self.pos];
            self.dry_l[self.pos] = cond_l;
            self.dry_r[self.pos] = cond_r;
            self.in_l[self.pos] = cond_l;
            self.in_r[self.pos] = cond_r;

            let idx = self.pos;
            let mut wet_l;
            let mut wet_r;
            {
                let active = self.active.as_ref().expect("checked above");
                let active_gain = if match_loudness { active.gain } else { 1.0 };
                wet_l = active.wet_l[idx] * active_gain;
                wet_r = active.wet_r[idx] * active_gain;
                if let Some(fading) = self.fading.as_ref() {
                    let fading_gain = if match_loudness { fading.gain } else { 1.0 };
                    let t = (self.fade_pos as f32 / self.fade_len as f32).clamp(0.0, 1.0);
                    let fade_in = (t * FRAC_PI_2).sin();
                    let fade_out = (t * FRAC_PI_2).cos();
                    wet_l = wet_l * fade_in + fading.wet_l[idx] * fading_gain * fade_out;
                    wet_r = wet_r * fade_in + fading.wet_r[idx] * fading_gain * fade_out;
                }
            }

            // Mid/side width.
            let width = params.width.smoothed.next();
            let mid = 0.5 * (wet_l + wet_r);
            let side = 0.5 * (wet_l - wet_r) * width;
            let proc_l = mid + side;
            let proc_r = mid - side;

            let trim = util::db_to_gain(params.trim_db.smoothed.next());
            let mix = params.mix.smoothed.next().clamp(0.0, 1.0);

            let env_l = dry_l * (1.0 - mix) + proc_l * trim * mix;
            let env_r = dry_r * (1.0 - mix) + proc_r * trim * mix;

            if metering {
                if let Some(a) = self.lufs_orig.as_mut() {
                    let _ = a.push_interleaved(&[dry_l, dry_r]);
                }
                if let Some(a) = self.lufs_env.as_mut() {
                    let _ = a.push_interleaved(&[env_l, env_r]);
                }
                self.push_spectrum(0.5 * (dry_l + dry_r));
            }

            if self.bypass_ramp < bypass_target {
                self.bypass_ramp = (self.bypass_ramp + ramp_step).min(bypass_target);
            } else if self.bypass_ramp > bypass_target {
                self.bypass_ramp = (self.bypass_ramp - ramp_step).max(bypass_target);
            }

            left[i] = dry_l * (1.0 - self.bypass_ramp) + env_l * self.bypass_ramp;
            right[i] = dry_r * (1.0 - self.bypass_ramp) + env_r * self.bypass_ramp;

            self.pos += 1;
            if self.pos == BLOCK {
                self.pos = 0;
                let in_l = self.in_l;
                let in_r = self.in_r;
                if let Some(active) = self.active.as_mut() {
                    active.process(&in_l, &in_r);
                }
                if let Some(fading) = self.fading.as_mut() {
                    fading.process(&in_l, &in_r);
                }
                if self.fading.is_some() {
                    self.fade_pos += BLOCK;
                    if self.fade_pos >= self.fade_len {
                        self.fading = None;
                        self.fade_pos = self.fade_len;
                    }
                }
            }
        }

        if metering {
            if let Some(a) = self.lufs_orig.as_mut() {
                store_f32(
                    &self.meters.original,
                    a.snapshot().momentary_lufs().unwrap_or(-70.0) as f32,
                );
            }
            if let Some(a) = self.lufs_env.as_mut() {
                store_f32(
                    &self.meters.environment,
                    a.snapshot().momentary_lufs().unwrap_or(-70.0) as f32,
                );
            }
        }
    }

    fn push_spectrum(&mut self, sample: f32) {
        self.spectrum_ring[self.spectrum_pos] = sample;
        self.spectrum_pos = (self.spectrum_pos + 1) % FFT_SIZE;
        self.spectrum_fill = (self.spectrum_fill + 1).min(FFT_SIZE);
        if self.spectrum_fill < FFT_SIZE {
            return;
        }

        let Some(fft) = self.fft.as_ref() else {
            return;
        };
        for i in 0..FFT_SIZE {
            let src = (self.spectrum_pos + i) % FFT_SIZE;
            self.fft_input[i] = self.spectrum_ring[src] * self.window[i];
        }
        if fft.process(&mut self.fft_input, &mut self.fft_output).is_err() {
            return;
        }

        let mut bands = [0.0f32; SPECTRUM_BANDS];
        let bin_hz = self.sample_rate / FFT_SIZE as f32;
        for (i, band) in bands.iter_mut().enumerate() {
            let t = i as f32 / (SPECTRUM_BANDS - 1) as f32;
            let f = 20.0 * 1000.0f32.powf(t);
            let centre = (f / bin_hz).round() as usize;
            let lo = centre.saturating_sub(1).clamp(1, FFT_SIZE / 2);
            let hi = (centre + 1).clamp(1, FFT_SIZE / 2);
            let mut acc = 0.0;
            for b in lo..=hi {
                acc += self.fft_output[b].norm_sqr();
            }
            let mag = (acc / 3.0).sqrt() / (FFT_SIZE as f32 * 0.25);
            let db = 20.0 * mag.max(1e-8).log10();
            *band = ((db + 72.0) / 72.0).clamp(0.0, 1.0);
        }
        let _ = self.spectrum_tx.write(bands);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta_ir() -> Vec<f32> {
        let mut ir = vec![0.0f32; 256];
        ir[0] = 1.0;
        ir
    }

    #[test]
    fn slot_convolves_without_nan() {
        let ir = delta_ir();
        let mut slot = Slot::new(&[ir.clone()], 1.0);
        let mut input = [0.0f32; BLOCK];
        input[0] = 1.0;
        slot.process(&input, &input);
        assert!(slot.wet_l.iter().all(|s| s.is_finite()));
        assert!(slot.wet_l[0] > 0.5, "delta should pass through, got {}", slot.wet_l[0]);
    }

    #[test]
    fn engine_runs_end_to_end() {
        let (mut producer, consumer) = rtrb::RingBuffer::<PreparedConvolver>::new(2);
        let (spectrum_in, _spectrum_out) = triple_buffer::triple_buffer(&[0.0f32; SPECTRUM_BANDS]);
        let meters = Arc::new(Meters::new());
        let mut dsp = RoomcheckDsp::new(consumer, spectrum_in, meters);
        dsp.set_sample_rate(48_000.0);

        producer
            .push(PreparedConvolver {
                slot: Slot::new(&[delta_ir()], 1.0),
            })
            .unwrap();

        let params = RoomcheckParams::default();
        let mut left = vec![0.0f32; 512];
        let mut right = vec![0.0f32; 512];
        left[0] = 1.0;
        right[0] = 1.0;
        dsp.process(&mut left, &mut right, &params, true);

        assert!(left.iter().all(|s| s.is_finite()));
        assert!(left.iter().any(|s| s.abs() > 0.1), "no audio came out");
        assert!(right.iter().all(|s| s.is_finite()));
    }
}
