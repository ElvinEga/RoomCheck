//! Host-visible parameters and smoothing.

use nice_plug::prelude::*;
use std::sync::Arc;

#[derive(Params)]
pub struct RoomcheckParams {
    /// Index into the reference library. The GUI writes this, the build thread reads it.
    #[id = "preset"]
    pub preset: IntParam,

    /// True while the user holds "Original" (button or space bar).
    #[id = "bypass"]
    pub bypass: BoolParam,

    /// How strongly the reference colours the programme material.
    #[id = "intensity"]
    pub intensity: FloatParam,

    /// Perceived listening distance (0 = at the source, 1 = across the room).
    #[id = "distance"]
    pub distance: FloatParam,

    /// Scales the IR tail.
    #[id = "tail"]
    pub tail: FloatParam,

    /// Mid/side width of the convolved signal.
    #[id = "width"]
    pub width: FloatParam,

    /// Dry / wet on the wet path.
    #[id = "mix"]
    pub mix: FloatParam,

    /// Output trim in decibels.
    #[id = "trim_db"]
    pub trim_db: FloatParam,

    #[id = "match_loudness"]
    pub match_loudness: BoolParam,

    #[id = "mono_sum"]
    pub mono_sum: BoolParam,
}

impl RoomcheckParams {
    pub fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl Default for RoomcheckParams {
    fn default() -> Self {
        Self {
            preset: IntParam::new("Reference", 0, IntRange::Linear { min: 0, max: 63 }),
            bypass: BoolParam::new("Listen original", false),
            intensity: FloatParam::new(
                "Intensity",
                1.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.5,
                },
            )
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_step_size(0.01)
            .with_value_to_string(formatters::v2s_f32_percentage(0)),
            distance: FloatParam::new(
                "Distance",
                0.4,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(25.0))
            .with_step_size(0.001)
            .with_value_to_string(formatters::v2s_f32_percentage(0)),
            tail: FloatParam::new(
                "Tail",
                1.0,
                FloatRange::Skewed {
                    min: 0.25,
                    max: 2.0,
                    factor: FloatRange::skew_factor(-0.6),
                },
            )
            .with_smoother(SmoothingStyle::Linear(25.0))
            .with_step_size(0.01)
            .with_value_to_string(Arc::new(|v| format!("{v:.2}×"))),
            width: FloatParam::new(
                "Width",
                1.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 2.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_step_size(0.01)
            .with_value_to_string(formatters::v2s_f32_percentage(0)),
            mix: FloatParam::new(
                "Mix",
                1.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 1.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_step_size(0.01)
            .with_value_to_string(formatters::v2s_f32_percentage(0)),
            trim_db: FloatParam::new(
                "Trim",
                0.0,
                FloatRange::Linear {
                    min: -18.0,
                    max: 18.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(15.0))
            .with_step_size(0.1)
            .with_value_to_string(Arc::new(|v| {
                if v.abs() < 0.05 {
                    "0.0 dB".to_owned()
                } else {
                    format!("{v:+.1} dB")
                }
            })),
            match_loudness: BoolParam::new("Match loudness", true),
            mono_sum: BoolParam::new("Mono sum", false),
        }
    }
}
