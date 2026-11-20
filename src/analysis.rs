//! Offline IR analysis and shaping.
//!
//! Everything in here runs on the build thread. It may allocate freely.

use realfft::RealFftPlanner;
use std::f32::consts::PI;

pub const FREQ_POINTS: usize = 240;
pub const DECAY_POINTS: usize = 200;

#[derive(Clone, Debug)]
pub struct EarlyReflection {
    pub t: f32,
    pub db: f32,
}

#[derive(Clone, Debug)]
pub struct IrAnalysis {
    /// 240 log-spaced dB points from 20 Hz to 20 kHz, 1/6-octave smoothed,
    /// normalised so the midband sits at 0 dB.
    pub freq_response_db: Vec<f32>,
    /// 200 points from 0 to `decay_span_sec`, in dB from the IR peak.
    pub decay_db: Vec<f32>,
    pub decay_span_sec: f32,
    pub ir_length_sec: f32,
    pub rt60_sec: f32,
    pub channels: usize,
    pub early: Vec<EarlyReflection>,
    /// Linear makeup gain that equalises pink-noise loudness against a flat IR.
    pub loudness_match_gain: f32,
}

impl Default for IrAnalysis {
    fn default() -> Self {
        Self {
            freq_response_db: vec![0.0; FREQ_POINTS],
            decay_db: vec![-60.0; DECAY_POINTS],
            decay_span_sec: 0.5,
            ir_length_sec: 0.0,
            rt60_sec: 0.0,
            channels: 2,
            early: Vec::new(),
            loudness_match_gain: 1.0,
        }
    }
}

fn mono_mix(channels: &[Vec<f32>]) -> Vec<f32> {
    let len = channels.first().map_or(0, |c| c.len());
    let mut out = vec![0.0f32; len];
    if channels.is_empty() {
        return out;
    }
    let scale = 1.0 / channels.len() as f32;
    for ch in channels {
        for (i, s) in ch.iter().take(len).enumerate() {
            out[i] += s * scale;
        }
    }
    out
}

/// Full analysis of an already-resampled, already-shaped IR set.
pub fn analyze_ir(channels: &[Vec<f32>], sample_rate: f32) -> IrAnalysis {
    let mono = mono_mix(channels);
    let len = mono.len().max(1);
    let ir_length_sec = len as f32 / sample_rate;

    let fft_size = len.next_power_of_two().clamp(8192, 65_536);
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(fft_size);

    let mut buf = vec![0.0f32; fft_size];
    let copy_len = len.min(fft_size);
    buf[..copy_len].copy_from_slice(&mono[..copy_len]);
    let mut spectrum = r2c.make_output_vec();
    let _ = r2c.process(&mut buf, &mut spectrum);

    let bin_hz = (sample_rate / fft_size as f32).max(1e-6);
    let mag_sq: Vec<f32> = spectrum.iter().map(|c| c.norm_sqr()).collect();

    // 1/6-octave smoothing, 20 Hz .. 20 kHz.
    let half_band = 2.0f32.powf(1.0 / 12.0);
    let mut raw_db = vec![0.0f32; FREQ_POINTS];
    let mut mid_sum = 0.0f32;
    let mut mid_count = 0usize;
    for (i, slot) in raw_db.iter_mut().enumerate() {
        let t = i as f32 / (FREQ_POINTS - 1) as f32;
        let f = 20.0 * 1000.0f32.powf(t);
        let lo = (((f / half_band) / bin_hz).floor() as usize).clamp(1, mag_sq.len() - 1);
        let hi = (((f * half_band) / bin_hz).ceil() as usize).clamp(lo, mag_sq.len() - 1);
        let mut acc = 0.0;
        for b in lo..=hi {
            acc += mag_sq[b];
        }
        let avg = (acc / (hi - lo + 1) as f32).max(1e-15);
        *slot = 10.0 * avg.log10();
        if (250.0..=4000.0).contains(&f) {
            mid_sum += avg;
            mid_count += 1;
        }
    }
    let mid_ref = 10.0 * (mid_sum / mid_count.max(1) as f32).max(1e-15).log10();
    let freq_response_db = raw_db
        .iter()
        .map(|db| (db - mid_ref).clamp(-30.0, 12.0))
        .collect();

    // Pink-weighted loudness equalisation. A flat IR gives exactly 1.0.
    let mut weighted_ref = 0.0f32;
    let mut weighted_ir = 0.0f32;
    for (b, power) in mag_sq.iter().enumerate().skip(1) {
        let f = b as f32 * bin_hz;
        let w = 1.0 / f;
        weighted_ref += w;
        weighted_ir += w * power;
    }
    let loudness_match_gain = if weighted_ir > 1e-12 {
        (weighted_ref / weighted_ir).sqrt().clamp(0.06, 16.0)
    } else {
        1.0
    };

    // Schroeder backward integration.
    let mut schroeder = vec![0.0f32; len];
    let mut running = 0.0f32;
    for i in (0..len).rev() {
        running += mono[i] * mono[i];
        schroeder[i] = running;
    }
    let max_energy = schroeder[0].max(1e-15);

    let mut t5 = None;
    let mut t25 = None;
    for (i, &e) in schroeder.iter().enumerate() {
        let db = 10.0 * (e / max_energy).max(1e-12).log10();
        if t5.is_none() && db <= -5.0 {
            t5 = Some(i as f32 / sample_rate);
        }
        if t5.is_some() && db <= -25.0 {
            t25 = Some(i as f32 / sample_rate);
            break;
        }
    }
    let rt60_sec = match (t5, t25) {
        (Some(a), Some(b)) if b > a => (b - a) * 3.0,
        _ => ir_length_sec,
    };

    // Speaker IRs are far shorter than any reverb. Give them a sensible window.
    let decay_span_sec = if ir_length_sec < 0.12 {
        (ir_length_sec * 1.15).max(0.03)
    } else {
        (rt60_sec * 1.4).clamp(0.12, ir_length_sec.max(0.12))
    };
    let mut decay_db = vec![-60.0f32; DECAY_POINTS];
    for (i, slot) in decay_db.iter_mut().enumerate() {
        let t = i as f32 / (DECAY_POINTS - 1) as f32 * decay_span_sec;
        let idx = (t * sample_rate) as usize;
        let db = if idx < len {
            10.0 * (schroeder[idx] / max_energy).max(1e-9).log10()
        } else {
            -60.0
        };
        *slot = db.clamp(-60.0, 0.0);
    }

    // A handful of early reflections for the decay card's tick marks.
    let peak = mono.iter().fold(0.0f32, |m, s| m.max(s.abs())).max(1e-9);
    let mut early = Vec::new();
    let scan = len.min((0.08 * sample_rate) as usize);
    let mut i = 1;
    while i + 1 < scan && early.len() < 6 {
        let a = mono[i - 1].abs();
        let b = mono[i].abs();
        let c = mono[i + 1].abs();
        if b > a && b >= c && b > peak * 0.12 && b > 1e-7 {
            early.push(EarlyReflection {
                t: i as f32 / sample_rate,
                db: 20.0 * (b / peak).log10(),
            });
            i += (sample_rate * 0.004) as usize; // don't stack ticks together
        } else {
            i += 1;
        }
    }

    IrAnalysis {
        freq_response_db,
        decay_db,
        decay_span_sec,
        ir_length_sec,
        rt60_sec,
        channels: channels.len(),
        early,
        loudness_match_gain,
    }
}

/// Cheap RT60 estimate used when scanning the library for sidebar tags.
pub fn quick_rt60(channels: &[Vec<f32>], sample_rate: f32) -> (f32, f32) {
    let mono = mono_mix(channels);
    let len = mono.len();
    if len == 0 {
        return (0.0, 0.0);
    }
    let mut running = 0.0f32;
    let mut schroeder = vec![0.0f32; len];
    for i in (0..len).rev() {
        running += mono[i] * mono[i];
        schroeder[i] = running;
    }
    let max = schroeder[0].max(1e-15);
    let mut t5 = None;
    let mut t20 = None;
    for (i, &e) in schroeder.iter().enumerate() {
        let db = 10.0 * (e / max).max(1e-12).log10();
        if t5.is_none() && db <= -5.0 {
            t5 = Some(i as f32 / sample_rate);
        }
        if t5.is_some() && db <= -20.0 {
            t20 = Some(i as f32 / sample_rate);
            break;
        }
    }
    let rt60 = match (t5, t20) {
        (Some(a), Some(b)) if b > a => (b - a) * 4.0,
        _ => len as f32 / sample_rate,
    };
    (rt60, len as f32 / sample_rate)
}

/// Shapes an IR for the current Distance / Tail / Intensity settings.
pub fn shape_ir(raw: &[f32], sample_rate: f32, tail: f32, distance: f32, intensity: f32) -> Vec<f32> {
    let mut out = raw.to_vec();
    let n = out.len();
    if n == 0 {
        return out;
    }

    // Tail: exponential re-decay anchored halfway through the IR.
    if (tail - 1.0).abs() > 0.005 {
        let tau = (n as f32 / sample_rate).max(0.02) * 0.5;
        let delta = 1.0 / tail.max(0.1) - 1.0;
        for (i, s) in out.iter_mut().enumerate() {
            let t = i as f32 / sample_rate;
            *s *= (-delta * t / tau).exp().clamp(0.0, 4.0);
        }
    }

    // Distance: gentle air absorption, never a hard filter.
    let d = distance.clamp(0.0, 1.0);
    if d > 0.001 {
        let cutoff = (20_000.0 - 11_000.0 * d).clamp(2_000.0, 20_000.0);
        let alpha = 1.0 - (-2.0 * PI * cutoff / sample_rate).exp();
        let mut lp = 0.0f32;
        for s in &mut out {
            lp += alpha * (*s - lp);
            *s = lp;
        }
    }

    // Intensity: blend the IR toward a unit impulse without touching the dry path.
    if (intensity - 1.0).abs() > 0.005 {
        let direct = (1.0 - intensity).clamp(-0.0, 1.0);
        for s in &mut out {
            *s *= intensity.max(0.0);
        }
        out[0] += direct;
    }

    // Keep the engine's input in a sane range.
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 4.0 {
        let scale = 4.0 / peak;
        for s in &mut out {
            *s *= scale;
        }
    }
    for s in &mut out {
        if !s.is_finite() {
            *s = 0.0;
        }
    }
    out
}

/// Format an RT60 for display, matching the design's "< 0.05 s" convention.
pub fn format_rt60(rt60: f32) -> String {
    if rt60 <= 0.05 {
        "< 0.05 s".to_owned()
    } else if rt60 < 1.0 {
        format!("{rt60:.2} s")
    } else {
        format!("{rt60:.1} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shaped_speaker_ir_is_sane() {
        // 20 ms of decaying noise, like a small speaker capture.
        let sr = 48_000.0;
        let mut ir = vec![0.0f32; (sr * 0.02) as usize];
        for (i, s) in ir.iter_mut().enumerate() {
            let t = i as f32 / sr;
            *s = (-t * 220.0).exp() * (i as f32 * 0.7).sin() * 0.5;
        }
        let shaped = shape_ir(&ir, sr, 1.0, 0.4, 1.0);
        let analysis = analyze_ir(&[shaped], sr);
        assert_eq!(analysis.freq_response_db.len(), FREQ_POINTS);
        assert_eq!(analysis.decay_db.len(), DECAY_POINTS);
        assert!(analysis.ir_length_sec > 0.019);
        assert!(analysis.rt60_sec > 0.0 && analysis.rt60_sec < 2.0);
        assert!(analysis.loudness_match_gain.is_finite());
        assert!(analysis.loudness_match_gain >= 0.06 && analysis.loudness_match_gain <= 16.0);
        assert!(analysis.freq_response_db.iter().all(|v| v.is_finite()));
        assert!(analysis.decay_db.iter().all(|v| *v <= 0.0));
    }

    #[test]
    fn flat_ir_matches_unity_loudness() {
        let mut ir = vec![0.0f32; 4096];
        ir[0] = 1.0;
        let analysis = analyze_ir(&[ir], 48_000.0);
        let gain_db = 20.0 * analysis.loudness_match_gain.log10();
        assert!(gain_db.abs() < 1.0, "flat IR gain was {gain_db} dB");
    }
}
