//! The reference library: embedded factory IRs plus anything the user imports.

use crate::analysis::quick_rt60;
use nice_plug::prelude::nice_error;
use rt_fft_convolver::resample;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};

const IPHONE_WAV: &[u8] = include_bytes!("../Presets/iPhone.wav");
const IPAD_WAV: &[u8] = include_bytes!("../Presets/iPad.wav");
const BEHRITONE_WAV: &[u8] = include_bytes!("../Presets/Behritone.wav");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Speaker,
    Space,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Phone,
    Tablet,
    Bluetooth,
    Laptop,
    Car,
    Bus,
    Club,
    Hall,
    Bed,
    Speaker,
}

#[derive(Clone, Debug)]
pub enum Source {
    Embedded(&'static [u8]),
    File(PathBuf),
}

/// Optional `manifest.json` entries in the user IR folder. Only `file` is
/// required; everything else overrides the filename-based inference.
#[derive(Clone, Debug, Deserialize)]
pub struct ManifestEntry {
    pub file: String,
    pub name: Option<String>,
    pub subtitle: Option<String>,
    /// "speakers" or "spaces".
    pub category: Option<String>,
    /// "phone", "car" or "room".
    pub diagram: Option<String>,
    pub tag: Option<String>,
}

fn load_manifest(dir: &Path) -> HashMap<String, ManifestEntry> {
    let text = match fs::read_to_string(dir.join("manifest.json")) {
        Ok(text) => text,
        Err(_) => return HashMap::new(),
    };
    let entries: Vec<ManifestEntry> = match serde_json::from_str(&text) {
        Ok(entries) => entries,
        Err(err) => {
            nice_error!("Roomcheck: ignoring invalid manifest.json: {err}");
            return HashMap::new();
        }
    };
    entries
        .into_iter()
        .map(|entry| (entry.file.to_lowercase(), entry))
        .collect()
}

#[derive(Clone, Debug)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub category: Category,
    pub icon: Icon,
    pub source: Source,
    pub channels: usize,
    pub sample_rate: u32,
    pub duration: f32,
    pub rt60: f32,
    /// True for the three factory references shipped inside the binary.
    pub factory: bool,
    tag_override: Option<String>,
    subtitle_override: Option<String>,
}

impl Preset {
    pub fn tag(&self) -> String {
        if let Some(tag) = &self.tag_override {
            return tag.clone();
        }
        match self.category {
            Category::Speaker => {
                if self.channels <= 1 {
                    "MONO".to_owned()
                } else if self.channels >= 4 {
                    "TRUE ST".to_owned()
                } else {
                    "STEREO".to_owned()
                }
            }
            Category::Space => crate::analysis::format_rt60(self.rt60),
        }
    }

    pub fn subtitle(&self) -> String {
        if let Some(subtitle) = &self.subtitle_override {
            return subtitle.clone();
        }
        match self.category {
            Category::Speaker => {
                if self.channels <= 1 {
                    "Mono playback".to_owned()
                } else {
                    "Stereo playback".to_owned()
                }
            }
            Category::Space => format!("{} · listening room", crate::analysis::format_rt60(self.rt60)),
        }
    }
}

pub fn user_ir_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ROOMCHECK_IR_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home).join("Documents/Roomcheck/IRs")
}

/// Read a WAV file into de-interleaved channels at its native sample rate.
pub fn decode_wav<R: Read + Seek>(reader: R) -> Result<(Vec<Vec<f32>>, u32), String> {
    let mut wav = hound::WavReader::new(reader).map_err(|e| e.to_string())?;
    let spec = wav.spec();
    let channels = spec.channels.max(1) as usize;
    let rate = spec.sample_rate.max(1);

    let mut frames: Vec<f32> = Vec::new();
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for s in wav.samples::<f32>() {
                frames.push(s.map_err(|e| e.to_string())?);
            }
        }
        hound::SampleFormat::Int => {
            let scale = match spec.bits_per_sample {
                8 => 1.0 / 128.0,
                16 => 1.0 / 32_768.0,
                24 => 1.0 / 8_388_608.0,
                _ => 1.0 / 2_147_483_648.0,
            };
            for s in wav.samples::<i32>() {
                frames.push(s.map_err(|e| e.to_string())? as f32 * scale);
            }
        }
    }

    let mut out: Vec<Vec<f32>> = (0..channels).map(|_| Vec::new()).collect();
    for (i, sample) in frames.iter().enumerate() {
        out[i % channels].push(*sample);
    }

    // Peak-normalise so imported IRs behave like the factory captures.
    let peak = out
        .iter()
        .flat_map(|c| c.iter())
        .fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 1e-6 && (peak - 1.0).abs() > 0.01 {
        let g = 1.0 / peak;
        for ch in &mut out {
            for s in ch.iter_mut() {
                *s *= g;
            }
        }
    }

    Ok((out, rate))
}

fn load_source(source: &Source) -> Result<(Vec<Vec<f32>>, u32), String> {
    match source {
        Source::Embedded(bytes) => decode_wav(Cursor::new(*bytes)),
        Source::File(path) => {
            let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            decode_wav(file)
        }
    }
}

/// Resample one channel with enough trailing zero padding for rubato's FFT
/// resampler to warm up. Without this, very short IRs (the 5 ms Behritone
/// capture) come back attenuated by more than 80 dB.
fn resample_channel(samples: &[f32], from: u32, to: u32) -> Result<Vec<f32>, String> {
    let expected = ((samples.len() as f64 * to as f64 / from as f64).round() as usize).max(1);
    let mut padded = Vec::with_capacity(samples.len() + 4096);
    padded.extend_from_slice(samples);
    padded.resize(samples.len() + 4096, 0.0);
    let full = resample(&padded, from, to).map_err(|e| format!("resample: {e}"))?;
    let mut out = full.into_iter().take(expected).collect::<Vec<_>>();
    out.resize(expected, 0.0);

    // Keep the original level; the padding only exists to stabilise the filter.
    let src_peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let dst_peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if dst_peak > 1e-9 && src_peak > 1e-9 {
        let gain = src_peak / dst_peak;
        for s in &mut out {
            *s *= gain;
        }
    }
    Ok(out)
}

/// Load and resample a preset to the host sample rate.
pub fn load_preset(preset: &Preset, target_sr: u32) -> Result<Vec<Vec<f32>>, String> {
    let (channels, native) = load_source(&preset.source)?;
    if native == target_sr || channels.is_empty() {
        return Ok(channels);
    }
    let mut out = Vec::with_capacity(channels.len());
    for ch in channels {
        out.push(resample_channel(&ch, native, target_sr)?);
    }
    Ok(out)
}

fn beautify(stem: &str) -> String {
    let lower = stem.to_ascii_lowercase();
    if lower.contains("iphone") {
        return "iPhone Speaker".to_owned();
    }
    if lower.contains("ipad") {
        return "iPad Speaker".to_owned();
    }
    if lower.contains("macbook") {
        return "MacBook Speaker".to_owned();
    }
    if lower.contains("behritone") {
        return "Behritone".to_owned();
    }
    let cleaned = stem.replace(['_', '-', '.'], " ");
    let mut out = String::with_capacity(cleaned.len());
    for (i, word) in cleaned.split_whitespace().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.extend(chars);
        }
    }
    if out.is_empty() {
        "Untitled reference".to_owned()
    } else {
        out
    }
}

fn classify(name: &str) -> (Category, Icon) {
    let n = name.to_ascii_lowercase();
    let space_words = [
        "car", "bus", "club", "hall", "room", "bed", "venue", "church", "cathedral", "studio",
        "chamber", "train", "plane", "cafe", "bar", "gym", "warehouse", "tunnel", "office",
        "kitchen", "bathroom", "space", "environment", "arena", "dome", "cave", "econoline",
        "navigator", "ford", "lincoln", "sedan",
    ];
    let speaker_words = [
        "phone", "ipad", "tablet", "laptop", "bluetooth", "speaker", "speakers", "monitor", "tv",
        "behritone", "auratone", "headphone", "macbook", "earbud", "boombox",
    ];

    let icon = if n.contains("ipad") || n.contains("tablet") {
        Icon::Tablet
    } else if n.contains("phone") {
        Icon::Phone
    } else if n.contains("bluetooth") || n.contains("bt") || n.contains("boombox") {
        Icon::Bluetooth
    } else if n.contains("laptop") || n.contains("macbook") {
        Icon::Laptop
    } else if n.contains("car") || n.contains("sedan") || n.contains("ford") || n.contains("econoline") || n.contains("navigator") || n.contains("lincoln") {
        Icon::Car
    } else if n.contains("bus") {
        Icon::Bus
    } else if n.contains("club") || n.contains("bar") {
        Icon::Club
    } else if n.contains("hall") || n.contains("church") || n.contains("cathedral") || n.contains("arena") || n.contains("dome") || n.contains("cave") {
        Icon::Hall
    } else if n.contains("bed") || n.contains("room") || n.contains("kitchen") || n.contains("bathroom") || n.contains("studio") {
        Icon::Bed
    } else {
        Icon::Speaker
    };

    if speaker_words.iter().any(|w| n.contains(w)) {
        (Category::Speaker, icon)
    } else if space_words.iter().any(|w| n.contains(w)) {
        (Category::Space, icon)
    } else {
        (Category::Speaker, Icon::Speaker)
    }
}

fn preset_from_file(path: &Path) -> Option<Preset> {
    let stem = path.file_stem()?.to_string_lossy().to_string();
    let name = beautify(&stem);
    let (channels, sample_rate) =
        fs::File::open(path).ok().and_then(|f| decode_wav(f).ok())?;
    let (rt60, duration) = quick_rt60(&channels, sample_rate as f32);
    let (mut category, mut icon) = classify(&name);
    let mut final_name = name;
    let mut subtitle_override = None;
    let mut tag_override = None;

    if let Some(dir) = path.parent() {
        let key = path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if let Some(entry) = load_manifest(dir).get(&key) {
            if let Some(n) = &entry.name {
                final_name = n.clone();
            }
            subtitle_override = entry.subtitle.clone();
            tag_override = entry.tag.clone();
            if let Some(cat) = entry.category.as_deref() {
                category = if cat.eq_ignore_ascii_case("spaces") {
                    Category::Space
                } else {
                    Category::Speaker
                };
            }
            if let Some(diagram) = entry.diagram.as_deref() {
                icon = if diagram.eq_ignore_ascii_case("phone") {
                    Icon::Phone
                } else if diagram.eq_ignore_ascii_case("car") {
                    Icon::Car
                } else {
                    Icon::Speaker
                };
            }
        }
    }

    Some(Preset {
        id: format!("file:{}", path.display()),
        name: final_name,
        category,
        icon,
        source: Source::File(path.to_path_buf()),
        channels: channels.len(),
        sample_rate,
        duration,
        rt60,
        factory: false,
        tag_override,
        subtitle_override,
    })
}

fn factory(byte: &'static [u8], id: &str, name: &str, icon: Icon) -> Option<Preset> {
    let (channels, sample_rate) = decode_wav(Cursor::new(byte)).ok()?;
    let (rt60, duration) = quick_rt60(&channels, sample_rate as f32);
    let count = channels.len();
    Some(Preset {
        id: id.to_owned(),
        name: name.to_owned(),
        category: Category::Speaker,
        icon,
        source: Source::Embedded(byte),
        channels: count,
        sample_rate,
        duration,
        rt60,
        factory: true,
        tag_override: None,
        subtitle_override: None,
    })
}

/// Scan factory + user references. Only headers and one decay pass per file.
pub fn scan() -> Vec<Preset> {
    let mut presets = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    if let Some(p) = factory(IPHONE_WAV, "factory:iphone", "iPhone Speaker", Icon::Phone) {
        seen_names.insert("iphone.wav".to_string());
        presets.push(p);
    }
    if let Some(p) = factory(IPAD_WAV, "factory:ipad", "iPad Speaker", Icon::Tablet) {
        seen_names.insert("ipad.wav".to_string());
        presets.push(p);
    }
    if let Some(p) = factory(
        BEHRITONE_WAV,
        "factory:behritone",
        "Behritone",
        Icon::Speaker,
    ) {
        seen_names.insert("behritone.wav".to_string());
        presets.push(p);
    }

    let mut scan_dirs = Vec::new();
    let local_dir = PathBuf::from("Presets");
    if local_dir.is_dir() {
        scan_dirs.push(local_dir);
    }
    let user_dir = user_ir_dir();
    if user_dir != PathBuf::from("Presets") {
        scan_dirs.push(user_dir);
    }

    for dir in scan_dirs {
        if let Ok(entries) = fs::read_dir(&dir) {
            let mut files: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .map(|e| e.eq_ignore_ascii_case("wav"))
                        .unwrap_or(false)
                })
                .collect();
            files.sort();
            for path in files.into_iter().take(200) {
                let Some(file_name) = path.file_name() else { continue };
                let lower_name = file_name.to_string_lossy().to_ascii_lowercase();
                if seen_names.contains(&lower_name) {
                    continue;
                }
                let too_big = fs::metadata(&path).map(|m| m.len() > 32 * 1024 * 1024).unwrap_or(true);
                if too_big {
                    continue;
                }
                if let Some(p) = preset_from_file(&path) {
                    seen_names.insert(lower_name);
                    presets.push(p);
                }
            }
        }
    }

    presets.sort_by(|a, b| {
        (a.category == Category::Space)
            .cmp(&(b.category == Category::Space))
            .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
    });
    presets
}

/// Copy an imported IR into the user directory and return the freshly scanned preset.
pub fn import(path: &Path) -> Result<Preset, String> {
    let dir = user_ir_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| "not a file".to_owned())?
        .to_string_lossy()
        .to_string();
    let dest = dir.join(&file_name);
    if dest != path {
        fs::copy(path, &dest).map_err(|e| format!("copy: {e}"))?;
    }
    preset_from_file(&dest).ok_or_else(|| "could not read that WAV".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_overrides_filename_inference() {
        let dir = std::env::temp_dir().join(format!("roomcheck-manifest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("my_room.wav");
        fs::write(&wav, BEHRITONE_WAV).unwrap();
        fs::write(
            dir.join("manifest.json"),
            r#"[{"file":"my_room.wav","name":"Basement","subtitle":"Concrete room","category":"spaces","diagram":"car","tag":"0.42 s"}]"#,
        )
        .unwrap();

        let preset = preset_from_file(&wav).unwrap();
        assert_eq!(preset.name, "Basement");
        assert_eq!(preset.subtitle(), "Concrete room");
        assert_eq!(preset.category, Category::Space);
        assert_eq!(preset.icon, Icon::Car);
        assert_eq!(preset.tag(), "0.42 s");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn factory_library_loads() {
        let lib = scan();
        assert!(lib.len() >= 3, "expected the three factory IRs");
        for preset in lib.iter().filter(|p| p.factory) {
            let channels = load_preset(preset, 48_000).expect("load");
            assert!(!channels.is_empty());
            assert!(channels[0].len() > 100);
            assert!(channels.iter().all(|c| c.iter().all(|s| s.is_finite())));
        }
        let iphone = lib.iter().find(|p| p.id == "factory:iphone").unwrap();
        assert_eq!(iphone.name, "iPhone Speaker");
        assert_eq!(iphone.channels, 1);
        assert_eq!(iphone.tag(), "MONO");
    }
}

#[cfg(test)]
mod inspect {
    use super::*;

    #[test]
    fn factory_irs_survive_resampling() {
        for preset in scan().iter().filter(|p| p.factory) {
            let channels = load_preset(preset, 48_000).unwrap();
            let peak = channels
                .iter()
                .flatten()
                .fold(0.0f32, |m, s| m.max(s.abs()));
            println!(
                "{}: peak {peak:.2} match gain {:+.2} dB",
                preset.name,
                20.0 * crate::analysis::analyze_ir(&channels, 48_000.0)
                    .loudness_match_gain
                    .log10()
            );
            assert!(
                (0.5..=2.0).contains(&peak),
                "{} resampled to peak {peak}",
                preset.name
            );
        }
    }
}
