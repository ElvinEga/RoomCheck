//! Roomcheck — hear your mix the way the world will hear it.
//!
//! A reference plugin: convolve the master bus with measured speaker and
//! environment impulse responses, with click-free switching, loudness matching
//! and a fixed listening level.

use nice_plug::editor::dpi::LogicalSize;
use nice_plug::prelude::*;
use nice_plug_egui::{create_egui_editor, EguiEditor, EguiEditorState, EguiNiceSettings, RepaintNotifier};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

mod analysis;
mod dsp;
mod editor;
mod params;
mod presets;

use dsp::{Meters, RoomcheckDsp, BLOCK, SPECTRUM_BANDS};
use editor::{RoomcheckEditor, UiShared};
use params::RoomcheckParams;

const WINDOW_W: u32 = 1160;
const WINDOW_H: u32 = 740;

pub struct Roomcheck {
    params: Arc<RoomcheckParams>,
    editor_state: Arc<EguiEditorState>,
    shared: Arc<UiShared>,
    dsp: RoomcheckDsp,
    sample_rate: Arc<AtomicU32>,
    #[allow(dead_code)]
    generation: Arc<AtomicU64>,
    repaint: RepaintNotifier,
}

impl Default for Roomcheck {
    fn default() -> Self {
        let library = Arc::new(RwLock::new(presets::scan()));
        let params = RoomcheckParams::arc();
        let sample_rate = Arc::new(AtomicU32::new(0));
        let generation = Arc::new(AtomicU64::new(0));

        let worker = dsp::spawn_worker(
            params.clone(),
            library.clone(),
            sample_rate.clone(),
            generation.clone(),
        );

        let meters = Arc::new(Meters::new());
        let (spectrum_in, spectrum_out) = triple_buffer::triple_buffer(&[0.0f32; SPECTRUM_BANDS]);

        let dsp = RoomcheckDsp::new(worker.convolvers, spectrum_in, meters.clone());

        let shared = Arc::new(UiShared {
            analysis: Mutex::new(worker.analysis),
            spectrum: Mutex::new(spectrum_out),
            meters,
            library: library.clone(),
            sample_rate: sample_rate.clone(),
        });

        Self {
            params,
            editor_state: EguiEditorState::from_size(
                LogicalSize::new(WINDOW_W as f32, WINDOW_H as f32),
                1.0,
            ),
            shared,
            dsp,
            sample_rate,
            generation,
            repaint: RepaintNotifier::new(),
        }
    }
}

impl Plugin for Roomcheck {
    const NAME: &'static str = "Roomcheck";
    const VENDOR: &'static str = "Roomcheck Audio";
    const URL: &'static str = "https://roomcheck.audio";
    const EMAIL: &'static str = "hello@roomcheck.audio";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type Editor = EguiEditor<RoomcheckEditor>;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Self::Editor> {
        let app = RoomcheckEditor::new(self.params.clone(), self.shared.clone());
        create_egui_editor(
            self.editor_state.clone(),
            self.repaint.clone(),
            EguiNiceSettings::new().with_tile("Roomcheck"),
            app,
        )
    }

    fn activate(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl ActivateContext<Self>,
    ) -> bool {
        self.sample_rate
            .store(buffer_config.sample_rate as u32, Ordering::Relaxed);
        self.dsp.set_sample_rate(buffer_config.sample_rate);
        self.generation.fetch_add(1, Ordering::Relaxed);
        context.set_latency_samples(BLOCK as u32);
        true
    }

    fn reset(&mut self) {
        self.dsp.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let metering = self.editor_state.is_open();
        let slices = buffer.as_slice();
        if slices.len() >= 2 {
            let (left, right) = slices.split_at_mut(1);
            self.dsp
                .process(left[0], right[0], &self.params, metering);
        }
        if metering {
            self.repaint.request_repaint();
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for Roomcheck {
    const CLAP_ID: &'static str = "audio.roomcheck.reference";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Speaker and environment reference plugin with convolution");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Utility,
        ClapFeature::Mastering,
    ];
}

impl Vst3Plugin for Roomcheck {
    const VST3_CLASS_ID: [u8; 16] = *b"RoomcheckRef2026";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

nice_export_clap!(Roomcheck);
nice_export_vst3!(Roomcheck);
