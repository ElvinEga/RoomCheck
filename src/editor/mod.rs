//! The Roomcheck editor: a single 1160×740 fixed-size canvas, painted the way
//! the design was drawn.

pub mod theme;
pub mod widgets;

use crate::analysis::{IrAnalysis, format_rt60};
use crate::dsp::{BLOCK, Meters, SPECTRUM_BANDS};
use crate::params::RoomcheckParams;
use crate::presets::{self, Category, Icon, Preset};
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2,
};
use nice_plug::context::gui::GuiContext;
use nice_plug::prelude::*;
use nice_plug_egui::NiceEguiApp;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use theme::*;
use widgets as w;

const WIDTH: f32 = 1160.0;
const HEIGHT: f32 = 740.0;
const HEADER: f32 = 56.0;
const SIDEBAR: f32 = 248.0;

pub struct UiShared {
    pub analysis: Mutex<triple_buffer::Output<Arc<IrAnalysis>>>,
    pub spectrum: Mutex<triple_buffer::Output<[f32; SPECTRUM_BANDS]>>,
    pub meters: Arc<Meters>,
    pub library: Arc<RwLock<Vec<Preset>>>,
    pub sample_rate: Arc<AtomicU32>,
}

impl UiShared {
    pub fn analysis(&self) -> Arc<IrAnalysis> {
        self.analysis
            .lock()
            .map(|mut a| a.read().clone())
            .unwrap_or_default()
    }

    pub fn spectrum(&self) -> [f32; SPECTRUM_BANDS] {
        self.spectrum
            .lock()
            .map(|mut s| *s.read())
            .unwrap_or([0.0; SPECTRUM_BANDS])
    }
}

struct OpenState {
    gui: GuiContext,
    _ctx: egui::Context,
}

/// Where interactions send parameter changes. The hosted editor talks to the
/// host through `ParamSetter`; layout tests use the inert variant.
pub struct Sink<'a>(Option<&'a ParamSetter<'a>>);

impl<'a> Sink<'a> {
    pub fn host(setter: &'a ParamSetter<'a>) -> Self {
        Self(Some(setter))
    }

    pub fn headless() -> Self {
        Self(None)
    }

    fn begin(&self, param: &impl Param) {
        if let Some(setter) = self.0 {
            setter.begin_set_parameter(param);
        }
    }

    fn end(&self, param: &impl Param) {
        if let Some(setter) = self.0 {
            setter.end_set_parameter(param);
        }
    }

    fn set_norm(&self, param: &FloatParam, value: f32) {
        if let Some(setter) = self.0 {
            setter.set_parameter_normalized(param, value);
        }
    }

    fn set_int(&self, param: &IntParam, value: i32) {
        if let Some(setter) = self.0 {
            setter.begin_set_parameter(param);
            setter.set_parameter(param, value);
            setter.end_set_parameter(param);
        }
    }

    fn set_bool(&self, param: &BoolParam, value: bool) {
        if let Some(setter) = self.0 {
            setter.begin_set_parameter(param);
            setter.set_parameter(param, value);
            setter.end_set_parameter(param);
        }
    }
}

pub struct RoomcheckEditor {
    params: Arc<RoomcheckParams>,
    shared: Arc<UiShared>,
    open: Option<OpenState>,

    analysis: Arc<IrAnalysis>,
    spectrum: [f32; SPECTRUM_BANDS],
    search: String,
    last_preset: i32,
    bypass_ui: bool,
    dirty_library: bool,
}

impl RoomcheckEditor {
    pub fn new(params: Arc<RoomcheckParams>, shared: Arc<UiShared>) -> Self {
        Self {
            params,
            shared,
            open: None,
            analysis: Arc::new(IrAnalysis::default()),
            spectrum: [0.0; SPECTRUM_BANDS],
            search: String::new(),
            last_preset: -1,
            bypass_ui: false,
            dirty_library: false,
        }
    }
}

impl NiceEguiApp for RoomcheckEditor {
    fn build(
        &mut self,
        egui_ctx: egui::Context,
        nice_gui_ctx: GuiContext,
        _frame: &mut nice_plug_egui::Frame,
    ) -> Result<(), nice_plug_egui::baseview::HandlerError> {
        theme::install(&egui_ctx);
        self.open = Some(OpenState {
            gui: nice_gui_ctx,
            _ctx: egui_ctx,
        });
        Ok(())
    }

    fn editor_closed(&mut self) {
        self.open = None;
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut nice_plug_egui::Frame) {
        let Some(gui) = self.open.as_ref().map(|o| o.gui.clone()) else {
            return;
        };
        let setter = gui.param_setter();
        let sink = Sink::host(&setter);
        self.render(ui, &sink);
    }
}

impl RoomcheckEditor {
    /// Paints one frame of the whole editor. Kept separate from the `NiceEguiApp`
    /// plumbing so it can be rendered offscreen in tests.
    pub fn render(&mut self, ui: &mut Ui, sink: &Sink<'_>) {
        // Pull fresh data once per frame.
        self.analysis = self.shared.analysis();
        self.spectrum = self.shared.spectrum();

        let library = self
            .shared
            .library
            .read()
            .map(|l| l.clone())
            .unwrap_or_default();
        let selected =
            (self.params.preset.value().max(0) as usize).min(library.len().saturating_sub(1));
        if selected != self.last_preset as usize {
            self.last_preset = selected as i32;
        }
        let preset = library.get(selected).cloned();

        let rect = Rect::from_min_size(ui.min_rect().min, Vec2::new(WIDTH, HEIGHT));
        ui.set_clip_rect(rect);
        let painter = ui.painter().clone();

        painter.rect_filled(rect, CornerRadius::same(16), BG_APP);
        painter.rect(
            rect,
            CornerRadius::same(16),
            Color32::TRANSPARENT,
            Stroke::new(1.0, Color32::from_rgb(0x2a, 0x28, 0x24)),
            StrokeKind::Inside,
        );

        self.paint_header(ui, &painter, sink, &library, selected, preset.as_ref());
        self.paint_sidebar(ui, &painter, sink, &library, selected);
        self.paint_main(ui, &painter, sink, preset.as_ref());

        // Space bar = listen to the original while held.
        let space = ui.input(|i| i.key_down(egui::Key::Space));
        let typing = ui.ctx().egui_wants_keyboard_input();
        let held = self.bypass_ui || (space && !typing);
        if held != self.params.bypass.value() {
            sink.set_bool(&self.params.bypass, held);
        }

        // Meters and the live spectrum both need a continuous repaint.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
}

impl RoomcheckEditor {
    fn paint_header(
        &self,
        ui: &mut Ui,
        painter: &egui::Painter,
        sink: &Sink<'_>,
        library: &[Preset],
        selected: usize,
        preset: Option<&Preset>,
    ) {
        let header = Rect::from_min_size(Pos2::ZERO, Vec2::new(WIDTH, HEADER));
        painter.rect_filled(header, CornerRadius::ZERO, BG_BAR);
        painter.line_segment(
            [Pos2::new(0.0, HEADER - 0.5), Pos2::new(WIDTH, HEADER - 0.5)],
            Stroke::new(1.0, BORDER),
        );

        w::logo(
            painter,
            Rect::from_min_size(Pos2::new(20.0, 14.0), Vec2::splat(28.0)),
        );
        painter.text(
            Pos2::new(58.0, 20.0),
            Align2::LEFT_CENTER,
            "Roomcheck",
            sg(17.0, 600),
            TEXT_PRIMARY,
        );
        painter.text(
            Pos2::new(58.0, 37.0),
            Align2::LEFT_CENTER,
            "REFERENCE",
            FontId::new(10.5, egui::FontFamily::Name(SG500.into())),
            TEXT_MUTED,
        );

        // Centre navigation.
        let prev = Rect::from_min_size(Pos2::new(372.0, 10.0), Vec2::splat(36.0));
        let next = Rect::from_min_size(Pos2::new(732.0, 10.0), Vec2::splat(36.0));
        let pill = Rect::from_min_size(Pos2::new(420.0, 10.0), Vec2::new(300.0, 36.0));

        for (rect, is_prev) in [(prev, true), (next, false)] {
            w::rounded(painter, rect, 10, BG_INPUT, Stroke::new(1.0, BORDER_SUBTLE));
            w::arrow(painter, rect, is_prev, TEXT_PRIMARY);
        }
        if w::pressable(ui, ui.scope_id().with("prev_ref"), prev).clicked() && !library.is_empty() {
            let idx = if selected == 0 {
                library.len() - 1
            } else {
                selected - 1
            };
            sink.set_int(&self.params.preset, idx as i32);
        }
        if w::pressable(ui, ui.scope_id().with("next_ref"), next).clicked() && !library.is_empty() {
            let idx = (selected + 1) % library.len();
            sink.set_int(&self.params.preset, idx as i32);
        }

        w::rounded(painter, pill, 10, BG_INPUT, Stroke::new(1.0, BORDER_SUBTLE));
        let name = preset
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "No reference loaded".to_owned());
        let font = sg(15.0, 600);
        let max_w = pill.width() - 28.0;
        let trimmed_name = truncate_to_width(painter, &name, &font, max_w);
        painter.text(
            pill.center(),
            Align2::CENTER_CENTER,
            trimmed_name,
            font,
            TEXT_PRIMARY,
        );

        // Right side status.
        let sr = self.shared.sample_rate.load(Ordering::Relaxed);
        let status = if sr >= 1000 {
            format!("{:.1} kHz · {} smp", sr as f32 / 1000.0, BLOCK)
        } else {
            format!("— · {BLOCK} smp")
        };
        let settings = Rect::from_min_size(Pos2::new(1104.0, 10.0), Vec2::splat(36.0));
        painter.text(
            Pos2::new(settings.left() - 10.0, HEADER / 2.0),
            Align2::RIGHT_CENTER,
            status,
            mono(12.0, 400),
            TEXT_MUTED,
        );

        w::rounded(
            painter,
            settings,
            10,
            Color32::TRANSPARENT,
            Stroke::new(1.0, BORDER_SUBTLE),
        );
        w::sliders_icon(painter, settings.shrink(9.0), TEXT_MUTED);
        if w::pressable(ui, ui.scope_id().with("settings"), settings)
            .on_hover_text("Open the Roomcheck IR folder")
            .clicked()
        {
            let dir = presets::user_ir_dir();
            let _ = std::fs::create_dir_all(&dir);
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open").arg(&dir).spawn();
            #[cfg(not(target_os = "macos"))]
            let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
        }
    }

    fn paint_sidebar(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        sink: &Sink<'_>,
        library: &[Preset],
        selected: usize,
    ) {
        let sidebar =
            Rect::from_min_size(Pos2::new(0.0, HEADER), Vec2::new(SIDEBAR, HEIGHT - HEADER));
        painter.rect_filled(sidebar, CornerRadius::ZERO, BG_BAR);
        painter.line_segment(
            [
                Pos2::new(SIDEBAR - 0.5, HEADER),
                Pos2::new(SIDEBAR - 0.5, HEIGHT),
            ],
            Stroke::new(1.0, BORDER),
        );

        // Search.
        let search = Rect::from_min_size(Pos2::new(14.0, HEADER + 14.0), Vec2::new(220.0, 36.0));
        w::rounded(
            painter,
            search,
            8,
            BG_INPUT,
            Stroke::new(1.0, BORDER_SUBTLE),
        );
        painter.circle_stroke(
            Pos2::new(search.left() + 18.0, search.center().y - 1.0),
            5.0,
            Stroke::new(1.8, TEXT_MUTED),
        );
        painter.line_segment(
            [
                Pos2::new(search.left() + 22.0, search.center().y + 3.0),
                Pos2::new(search.left() + 26.0, search.center().y + 7.0),
            ],
            Stroke::new(1.8, TEXT_MUTED),
        );
        let edit_rect = Rect::from_min_max(
            Pos2::new(search.left() + 32.0, search.top() + 7.0),
            Pos2::new(search.right() - 8.0, search.bottom() - 7.0),
        );
        let mut search_text = self.search.clone();
        let response = ui.put(
            edit_rect,
            egui::TextEdit::singleline(&mut search_text)
                .frame(egui::Frame::NONE)
                .hint_text(egui::RichText::new("Search references").color(TEXT_FAINT))
                .font(sg(13.0, 400))
                .text_color(TEXT_PRIMARY)
                .desired_width(edit_rect.width()),
        );
        self.search = search_text;
        let _ = response;

        let query = self.search.trim().to_ascii_lowercase();
        let matches = |p: &Preset| {
            query.is_empty()
                || p.name.to_ascii_lowercase().contains(&query)
                || p.tag().to_ascii_lowercase().contains(&query)
        };

        let speakers: Vec<usize> = library
            .iter()
            .enumerate()
            .filter(|(_, p)| p.category == Category::Speaker && matches(p))
            .map(|(i, _)| i)
            .collect();
        let spaces: Vec<usize> = library
            .iter()
            .enumerate()
            .filter(|(_, p)| p.category == Category::Space && matches(p))
            .map(|(i, _)| i)
            .collect();

        // Import.
        let import = Rect::from_min_size(
            Pos2::new(14.0, HEIGHT - 14.0 - 56.0),
            Vec2::new(220.0, 56.0),
        );
        w::dashed_outline(painter, import, 10, Stroke::new(1.5, BORDER_STRONG));

        let import_label = "Import your own IR (.wav)";
        let font_id = sg(12.5, 500);
        let text_galley = painter.layout_no_wrap(import_label.to_owned(), font_id, TEXT_BODY);
        let icon_w = 12.0;
        let gap = 8.0;
        let total_w = icon_w + gap + text_galley.size().x;
        let start_x = import.center().x - total_w * 0.5;
        let plus_cx = start_x + icon_w * 0.5;
        let plus_cy = import.center().y;

        painter.line_segment(
            [
                Pos2::new(plus_cx - 6.0, plus_cy),
                Pos2::new(plus_cx + 6.0, plus_cy),
            ],
            Stroke::new(2.0, TEXT_BODY),
        );
        painter.line_segment(
            [
                Pos2::new(plus_cx, plus_cy - 6.0),
                Pos2::new(plus_cx, plus_cy + 6.0),
            ],
            Stroke::new(2.0, TEXT_BODY),
        );
        painter.galley(
            Pos2::new(
                start_x + icon_w + gap,
                import.center().y - text_galley.size().y * 0.5,
            ),
            text_galley,
            TEXT_BODY,
        );

        if w::pressable(ui, ui.scope_id().with("import_ir"), import).clicked() {
            self.import_ir(sink);
        }

        let scroll_rect = Rect::from_min_max(
            Pos2::new(0.0, search.bottom() + 6.0),
            Pos2::new(SIDEBAR - 1.0, import.top() - 6.0),
        );
        let mut list_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(scroll_rect)
                .layout(egui::Layout::top_down(egui::Align::LEFT)),
        );
        egui::ScrollArea::vertical()
            .id_salt("sidebar_presets_scroll")
            .auto_shrink([false, false])
            .show(&mut list_ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                ui.add_space(4.0);
                self.paint_group(ui, sink, "SPEAKERS", &speakers, library, selected);
                ui.add_space(6.0);
                self.paint_group(ui, sink, "SPACES", &spaces, library, selected);
            });
    }

    fn paint_group(
        &self,
        ui: &mut Ui,
        sink: &Sink<'_>,
        title: &str,
        indices: &[usize],
        library: &[Preset],
        selected: usize,
    ) {
        let (title_rect, _) = ui.allocate_exact_size(Vec2::new(220.0, 20.0), Sense::hover());
        ui.painter().text(
            Pos2::new(24.0, title_rect.center().y),
            Align2::LEFT_CENTER,
            title,
            FontId::new(11.0, egui::FontFamily::Name(SG500.into())),
            TEXT_MUTED,
        );

        if indices.is_empty() {
            let (empty_rect, _) = ui.allocate_exact_size(Vec2::new(220.0, 30.0), Sense::hover());
            ui.painter().text(
                Pos2::new(24.0, empty_rect.center().y),
                Align2::LEFT_CENTER,
                if title == "SPACES" {
                    "Import a room or car IR"
                } else {
                    "No matches"
                },
                sg(11.5, 400),
                TEXT_FAINT,
            );
            return;
        }

        for &index in indices {
            let Some(preset) = library.get(index) else {
                continue;
            };
            let (alloc_rect, response) =
                ui.allocate_exact_size(Vec2::new(220.0, 38.0), Sense::click());
            let row =
                Rect::from_min_size(Pos2::new(14.0, alloc_rect.top()), Vec2::new(220.0, 38.0));
            let is_selected = index == selected;
            let painter = ui.painter();
            if is_selected {
                let bg = mix(BG_CARD, ACCENT, 0.13);
                w::rounded(painter, row, 8, bg, Stroke::NONE);
                painter.rect(
                    row,
                    CornerRadius::same(8),
                    Color32::TRANSPARENT,
                    Stroke::new(1.0, mix(BG_CARD, ACCENT, 0.45)),
                    StrokeKind::Inside,
                );
            }
            let icon_colour = if is_selected { ACCENT } else { TEXT_MUTED };
            w::draw_icon(
                ui,
                preset.icon,
                Rect::from_min_size(
                    Pos2::new(row.left() + 10.0, row.center().y - 10.0),
                    Vec2::splat(20.0),
                ),
                icon_colour,
            );

            let is_speaker = preset.category == Category::Speaker;
            let (max_name_w, badge) = if is_speaker {
                let badge_w = 18.0;
                let badge_h = 16.0;
                let b = Rect::from_center_size(
                    Pos2::new(row.right() - 10.0 - badge_w * 0.5, row.center().y),
                    Vec2::new(badge_w, badge_h),
                );
                let label = if preset.channels <= 1 { "M" } else { "S" };
                let max_w = (b.left() - 8.0) - (row.left() + 40.0);
                (max_w, Some((b, label)))
            } else {
                let max_w = (row.right() - 10.0) - (row.left() + 40.0);
                (max_w, None)
            };

            if let Some((b, lbl)) = badge {
                let kbd_bg = Color32::from_rgb(0x1e, 0x1d, 0x1a);
                let kbd_stroke = Stroke::new(1.0, Color32::from_rgb(0x35, 0x32, 0x2c));
                painter.rect(b, CornerRadius::same(4), kbd_bg, kbd_stroke, StrokeKind::Inside);
                painter.text(
                    b.center(),
                    Align2::CENTER_CENTER,
                    lbl,
                    mono(10.0, 500),
                    TEXT_MUTED,
                );
            }

            let name_font = sg(14.0, 500);
            let name_color = if is_selected {
                TEXT_PRIMARY
            } else {
                TEXT_BODY
            };
            let name = truncate_to_width(painter, &preset.name, &name_font, max_name_w);
            painter.text(
                Pos2::new(row.left() + 40.0, row.center().y),
                Align2::LEFT_CENTER,
                name,
                name_font,
                name_color,
            );

            if response.clicked() {
                sink.set_int(&self.params.preset, index as i32);
            }
        }
    }

    fn import_ir(&mut self, sink: &Sink<'_>) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Impulse response", &["wav", "WAV"])
            .set_title("Import an impulse response")
            .pick_file()
        else {
            return;
        };
        match presets::import(&path) {
            Ok(preset) => {
                if let Ok(mut lib) = self.shared.library.write() {
                    if let Some(existing) = lib.iter().position(|p| p.id == preset.id) {
                        lib.remove(existing);
                    }
                    lib.push(preset);
                    let index = lib.len() - 1;
                    sink.set_int(&self.params.preset, index as i32);
                }
                self.dirty_library = true;
            }
            Err(err) => {
                nice_error!("Roomcheck: {err}");
            }
        }
    }

    fn paint_main(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        sink: &Sink<'_>,
        preset: Option<&Preset>,
    ) {
        let left = SIDEBAR + 16.0;
        let width = WIDTH - left - 16.0;
        let hero = Rect::from_min_size(Pos2::new(left, HEADER + 16.0), Vec2::new(width, 280.0));
        let mid_y = hero.bottom() + 12.0;
        let mid_h = 148.0;
        let rack_y = mid_y + mid_h + 12.0;
        let rack_h = HEIGHT - rack_y - 16.0;
        let map_w = (width - 12.0) * (1.25 / 2.25);
        let map = Rect::from_min_size(Pos2::new(left, mid_y), Vec2::new(map_w, mid_h));
        let decay = Rect::from_min_size(
            Pos2::new(left + map_w + 12.0, mid_y),
            Vec2::new(width - map_w - 12.0, mid_h),
        );
        let rack =
            Rect::from_min_size(Pos2::new(left, rack_y), Vec2::new(width, rack_h.max(180.0)));

        self.paint_hero(ui, painter, preset, hero);
        self.paint_map(ui, painter, preset, map);
        self.paint_decay(painter, preset, decay);
        self.paint_rack(ui, painter, sink, preset, rack);
    }

    fn paint_hero(
        &self,
        ui: &mut Ui,
        painter: &egui::Painter,
        _preset: Option<&Preset>,
        rect: Rect,
    ) {
        w::card(painter, rect);

        painter.text(
            Pos2::new(rect.left() + 18.0, rect.top() + 25.0),
            Align2::LEFT_CENTER,
            "Frequency response",
            sg(14.0, 600),
            TEXT_PRIMARY,
        );

        let mut lx = rect.left() + 174.0;
        // dashed original
        for i in 0..4 {
            let x = lx + i as f32 * 6.0;
            painter.line_segment(
                [
                    Pos2::new(x, rect.top() + 25.0),
                    Pos2::new(x + 4.0, rect.top() + 25.0),
                ],
                Stroke::new(1.5, Color32::from_rgb(0x8d, 0x89, 0x7f)),
            );
        }
        painter.text(
            Pos2::new(lx + 30.0, rect.top() + 25.0),
            Align2::LEFT_CENTER,
            "Original, flat",
            sg(12.0, 400),
            TEXT_MUTED,
        );
        lx += 122.0;
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(lx, rect.top() + 23.0), Vec2::new(20.0, 3.0)),
            CornerRadius::same(2),
            ACCENT,
        );
        painter.text(
            Pos2::new(lx + 28.0, rect.top() + 25.0),
            Align2::LEFT_CENTER,
            "Through the environment",
            sg(12.0, 400),
            TEXT_MUTED,
        );
        lx += 190.0;
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(lx, rect.top() + 20.0), Vec2::splat(10.0)),
            CornerRadius::same(2),
            Color32::from_rgb(0x2d, 0x2b, 0x27),
        );
        painter.text(
            Pos2::new(lx + 18.0, rect.top() + 25.0),
            Align2::LEFT_CENTER,
            "Your mix",
            sg(12.0, 400),
            TEXT_MUTED,
        );
        painter.text(
            Pos2::new(rect.right() - 18.0, rect.top() + 25.0),
            Align2::RIGHT_CENTER,
            "20 Hz to 20 kHz · 1/6 oct",
            mono(11.0, 400),
            TEXT_MUTED,
        );

        let plot = Rect::from_min_max(
            Pos2::new(rect.left() + 18.0, rect.top() + 46.0),
            Pos2::new(rect.right() - 18.0, rect.bottom() - 14.0),
        );
        w::freq_plot(
            ui,
            plot,
            &self.analysis.freq_response_db,
            &self.spectrum,
            ACCENT,
        );
    }

    fn paint_map(&self, ui: &mut Ui, painter: &egui::Painter, preset: Option<&Preset>, rect: Rect) {
        w::card(painter, rect);
        let icon = preset.map(|p| p.icon).unwrap_or(Icon::Speaker);
        let is_space = preset
            .map(|p| p.category == Category::Space)
            .unwrap_or(false);
        let title = if is_space {
            "Listening position"
        } else {
            "Playback position"
        };
        painter.text(
            Pos2::new(rect.left() + 14.0, rect.top() + 22.0),
            Align2::LEFT_CENTER,
            title,
            sg(13.0, 600),
            TEXT_PRIMARY,
        );
        let map_right = rect.left() + 14.0 + 270.0;
        painter.circle_filled(
            Pos2::new(map_right - 88.0, rect.top() + 22.0),
            3.5,
            Color32::from_rgb(0x8a, 0x85, 0x7a),
        );
        painter.text(
            Pos2::new(map_right - 81.0, rect.top() + 22.0),
            Align2::LEFT_CENTER,
            "Speaker",
            sg(11.0, 400),
            TEXT_MUTED,
        );
        painter.circle_filled(Pos2::new(map_right - 26.0, rect.top() + 22.0), 3.5, ACCENT);
        painter.text(
            Pos2::new(map_right - 4.0, rect.top() + 22.0),
            Align2::RIGHT_CENTER,
            "You",
            sg(11.0, 400),
            TEXT_MUTED,
        );

        let sketch = Rect::from_min_size(
            Pos2::new(rect.left() + 14.0, rect.top() + 40.0),
            Vec2::new(270.0, 98.0),
        );
        match icon {
            Icon::Phone | Icon::Tablet | Icon::Bluetooth | Icon::Laptop => {
                w::phone_sketch(ui, sketch, self.params.distance.value())
            }
            Icon::Car | Icon::Bus => w::car_sketch(ui, sketch, self.params.distance.value()),
            _ => w::room_sketch(ui, sketch, self.params.distance.value()),
        }

        let meta_x = rect.left() + 14.0 + 270.0 + 14.0;
        let meta_right = rect.right() - 14.0;
        painter.line_segment(
            [
                Pos2::new(meta_x - 14.0, rect.top() + 36.0),
                Pos2::new(meta_x - 14.0, rect.bottom() - 14.0),
            ],
            Stroke::new(1.0, BORDER),
        );
        let mut y = rect.top() + 40.0;
        let rows = [
            (
                "Source",
                if preset.map(|p| p.factory).unwrap_or(false) {
                    "Measured".to_owned()
                } else {
                    "Imported".to_owned()
                },
            ),
            ("IR length", format_duration(self.analysis.ir_length_sec)),
            ("RT60", format_rt60(self.analysis.rt60_sec)),
            (
                "Channels",
                match self.analysis.channels {
                    0 | 1 => "Mono",
                    2 => "Stereo",
                    3 | 4 => "True stereo",
                    _ => "Multi",
                }
                .to_owned(),
            ),
        ];
        for (k, v) in rows {
            painter.text(
                Pos2::new(meta_x, y),
                Align2::LEFT_CENTER,
                k,
                sg(12.0, 400),
                TEXT_MUTED,
            );
            painter.text(
                Pos2::new(meta_right, y),
                Align2::RIGHT_CENTER,
                v,
                mono(12.0, 500),
                TEXT_PRIMARY,
            );
            y += 24.0;
        }
    }

    fn paint_decay(&self, painter: &egui::Painter, _preset: Option<&Preset>, rect: Rect) {
        w::card(painter, rect);
        painter.text(
            Pos2::new(rect.left() + 14.0, rect.top() + 21.0),
            Align2::LEFT_CENTER,
            "Decay",
            sg(13.0, 600),
            TEXT_PRIMARY,
        );
        painter.text(
            Pos2::new(rect.right() - 14.0, rect.top() + 21.0),
            Align2::RIGHT_CENTER,
            format!("RT60 {}", format_rt60(self.analysis.rt60_sec)),
            mono(12.0, 500),
            ACCENT,
        );

        let plot = Rect::from_min_max(
            Pos2::new(rect.left() + 14.0, rect.top() + 42.0),
            Pos2::new(rect.right() - 14.0, rect.bottom() - 26.0),
        );
        let early: Vec<(f32, f32)> = self.analysis.early.iter().map(|e| (e.t, e.db)).collect();
        w::decay_plot(
            painter,
            plot,
            &self.analysis.decay_db,
            self.analysis.decay_span_sec,
            &early,
        );

        let span = self.analysis.decay_span_sec;
        let ticks: [String; 4] = if span < 0.15 {
            [
                "0".to_owned(),
                format!("{:.0}", span * 1000.0 / 3.0),
                format!("{:.0}", span * 2000.0 / 3.0),
                format!("{:.0} ms", span * 1000.0),
            ]
        } else {
            [
                "0".to_owned(),
                format!("{:.2}", span / 3.0),
                format!("{:.2}", span * 2.0 / 3.0),
                format!("{span:.2} s"),
            ]
        };
        for (i, label) in ticks.iter().enumerate() {
            let x = plot.left() + plot.width() * i as f32 / 3.0;
            painter.text(
                Pos2::new(x, plot.bottom() + 10.0),
                if i == 0 {
                    Align2::LEFT_CENTER
                } else if i == 3 {
                    Align2::RIGHT_CENTER
                } else {
                    Align2::CENTER_CENTER
                },
                label,
                mono(11.0, 400),
                TEXT_MUTED,
            );
        }
    }

    fn paint_rack(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        sink: &Sink<'_>,
        preset: Option<&Preset>,
        rect: Rect,
    ) {
        w::card(painter, rect);
        let inner = rect.shrink2(Vec2::new(18.0, 14.0));

        // --- Listen through -------------------------------------------------
        let listen_w = 176.0;
        let listen = Rect::from_min_size(inner.min, Vec2::new(listen_w, inner.height()));
        painter.text(
            Pos2::new(listen.left(), listen.top() + 7.0),
            Align2::LEFT_CENTER,
            "LISTEN THROUGH",
            FontId::new(11.0, egui::FontFamily::Name(SG500.into())),
            TEXT_MUTED,
        );

        let original = Rect::from_min_size(
            Pos2::new(listen.left(), listen.top() + 24.0),
            Vec2::new(listen_w, 46.0),
        );
        let pressed = self.params.bypass.value();
        w::rounded(
            painter,
            original,
            10,
            if pressed {
                mix(BG_CARD, ACCENT, 0.10)
            } else {
                Color32::TRANSPARENT
            },
            Stroke::new(1.0, if pressed { ACCENT } else { BORDER_STRONG }),
        );
        painter.text(
            Pos2::new(original.left() + 14.0, original.center().y),
            Align2::LEFT_CENTER,
            "Original",
            sg(14.0, 500),
            TEXT_PRIMARY,
        );
        painter.text(
            Pos2::new(original.right() - 14.0, original.center().y),
            Align2::RIGHT_CENTER,
            "hold Space",
            mono(11.0, 400),
            TEXT_MUTED,
        );
        let original_response = w::holdable(ui, ui.scope_id().with("listen_original"), original);
        self.bypass_ui = original_response.is_pointer_button_down_on();

        let reference = Rect::from_min_size(
            Pos2::new(listen.left(), listen.top() + 80.0),
            Vec2::new(listen_w, 46.0),
        );
        w::glow(painter, reference, 10);
        painter.rect_filled(reference, CornerRadius::same(10), ACCENT);
        painter.circle_filled(
            Pos2::new(reference.left() + 18.0, reference.center().y),
            4.0,
            ACCENT_INK,
        );
        let name = preset
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Reference".to_owned());
        let font = sg(14.0, 600);
        let max_w = reference.width() - 38.0;
        let label = truncate_to_width(painter, &name, &font, max_w);
        painter.text(
            Pos2::new(reference.left() + 30.0, reference.center().y),
            Align2::LEFT_CENTER,
            label,
            font,
            ACCENT_INK,
        );

        painter.line_segment(
            [
                Pos2::new(listen.right() + 18.0, inner.top()),
                Pos2::new(listen.right() + 18.0, inner.bottom()),
            ],
            Stroke::new(1.0, BORDER),
        );

        // --- Knobs ----------------------------------------------------------
        let right_w = 218.0;
        let right_divider_x = inner.right() - right_w - 16.0;
        painter.line_segment(
            [
                Pos2::new(right_divider_x, inner.top()),
                Pos2::new(right_divider_x, inner.bottom()),
            ],
            Stroke::new(1.0, BORDER),
        );

        let knob_left = listen.right() + 32.0;
        let knob_right = right_divider_x - 16.0;

        // Header & Reset button
        painter.text(
            Pos2::new(knob_left, inner.top() + 7.0),
            Align2::LEFT_CENTER,
            "CONTROLS",
            FontId::new(11.0, egui::FontFamily::Name(SG500.into())),
            TEXT_MUTED,
        );

        let is_dirty = (self.params.intensity.modulated_normalized_value()
            - self.params.intensity.default_normalized_value())
        .abs()
            > 0.002
            || (self.params.distance.modulated_normalized_value()
                - self.params.distance.default_normalized_value())
            .abs()
                > 0.002
            || (self.params.tail.modulated_normalized_value()
                - self.params.tail.default_normalized_value())
            .abs()
                > 0.002
            || (self.params.width.modulated_normalized_value()
                - self.params.width.default_normalized_value())
            .abs()
                > 0.002
            || (self.params.mix.modulated_normalized_value()
                - self.params.mix.default_normalized_value())
            .abs()
                > 0.002
            || (self.params.trim_db.modulated_normalized_value()
                - self.params.trim_db.default_normalized_value())
            .abs()
                > 0.002
            || self.params.match_loudness.value() != true
            || self.params.mono_sum.value() != false;

        let reset_btn = Rect::from_min_size(
            Pos2::new(knob_right - 66.0, inner.top() - 2.0),
            Vec2::new(66.0, 20.0),
        );
        let reset_resp = ui.interact(
            reset_btn,
            ui.scope_id().with("reset_defaults"),
            Sense::click(),
        );
        let hovered = reset_resp.hovered() && is_dirty;

        let (btn_bg, btn_stroke, btn_fg) = if is_dirty {
            if hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                (
                    mix(BG_CARD, ACCENT, 0.20),
                    Stroke::new(1.0, ACCENT),
                    TEXT_PRIMARY,
                )
            } else {
                (
                    mix(BG_CARD, ACCENT, 0.08),
                    Stroke::new(1.0, mix(BORDER_SUBTLE, ACCENT, 0.40)),
                    ACCENT,
                )
            }
        } else {
            (
                Color32::TRANSPARENT,
                Stroke::new(1.0, Color32::from_rgb(0x2a, 0x28, 0x24)),
                TEXT_FAINT,
            )
        };

        w::rounded(painter, reset_btn, 6, btn_bg, btn_stroke);
        w::reset_icon(
            painter,
            Pos2::new(reset_btn.left() + 14.0, reset_btn.center().y),
            btn_fg,
        );
        painter.text(
            Pos2::new(reset_btn.left() + 24.0, reset_btn.center().y),
            Align2::LEFT_CENTER,
            "Reset",
            FontId::new(11.0, egui::FontFamily::Name(SG500.into())),
            btn_fg,
        );

        let clicked = reset_resp.clicked();
        if is_dirty {
            let _ = reset_resp.on_hover_text("Reset controls to default values");
            if clicked {
                sink.begin(&self.params.intensity);
                sink.set_norm(
                    &self.params.intensity,
                    self.params.intensity.default_normalized_value(),
                );
                sink.end(&self.params.intensity);

                sink.begin(&self.params.distance);
                sink.set_norm(
                    &self.params.distance,
                    self.params.distance.default_normalized_value(),
                );
                sink.end(&self.params.distance);

                sink.begin(&self.params.tail);
                sink.set_norm(
                    &self.params.tail,
                    self.params.tail.default_normalized_value(),
                );
                sink.end(&self.params.tail);

                sink.begin(&self.params.width);
                sink.set_norm(
                    &self.params.width,
                    self.params.width.default_normalized_value(),
                );
                sink.end(&self.params.width);

                sink.begin(&self.params.mix);
                sink.set_norm(&self.params.mix, self.params.mix.default_normalized_value());
                sink.end(&self.params.mix);

                sink.begin(&self.params.trim_db);
                sink.set_norm(
                    &self.params.trim_db,
                    self.params.trim_db.default_normalized_value(),
                );
                sink.end(&self.params.trim_db);

                sink.set_bool(&self.params.match_loudness, true);
                sink.set_bool(&self.params.mono_sum, false);
            }
        }

        let gap = 4.0;
        let cell_w = (knob_right - knob_left - gap * 5.0) / 6.0;
        let knob_top = inner.top() + 20.0;
        let knob_h = 118.0;

        let is_space = preset
            .map(|p| p.category == Category::Space)
            .unwrap_or(false);
        let distance_value = if is_space {
            format!("{:.0}%", self.params.distance.value() * 100.0)
        } else {
            format!("{:.0} cm", self.params.distance.value() * 100.0)
        };

        let specs: [(&FloatParam, String, &str); 6] = [
            (
                &self.params.intensity,
                format!("{:.0}%", self.params.intensity.value() * 100.0),
                "Intensity",
            ),
            (&self.params.distance, distance_value, "Distance"),
            (
                &self.params.tail,
                format!("{:.2}×", self.params.tail.value()),
                "Tail",
            ),
            (
                &self.params.width,
                format!("{:.0}%", self.params.width.value() * 100.0),
                "Width",
            ),
            (
                &self.params.mix,
                format!("{:.0}%", self.params.mix.value() * 100.0),
                "Mix",
            ),
            (
                &self.params.trim_db,
                if self.params.trim_db.value().abs() < 0.05 {
                    "0.0 dB".to_owned()
                } else {
                    format!("{:+.1} dB", self.params.trim_db.value())
                },
                "Trim",
            ),
        ];

        for (i, (param, value, label)) in specs.iter().enumerate() {
            let cell = Rect::from_min_size(
                Pos2::new(knob_left + i as f32 * (cell_w + gap), knob_top),
                Vec2::new(cell_w, knob_h),
            );
            let out = w::knob(
                ui,
                ui.scope_id().with(("knob", i)),
                cell,
                param.modulated_normalized_value(),
                label,
                value.as_str(),
                param.default_normalized_value(),
            );
            if out.drag_started {
                sink.begin(*param);
            }
            if out.changed {
                sink.set_norm(*param, out.norm);
            }
            if out.drag_stopped {
                sink.end(*param);
            }
        }

        // --- Meters & toggles ----------------------------------------------
        let right = Rect::from_min_size(
            Pos2::new(right_divider_x + 18.0, inner.top() + 4.0),
            Vec2::new(right_w, inner.height() - 8.0),
        );
        painter.text(
            Pos2::new(right.left(), right.top() + 7.0),
            Align2::LEFT_CENTER,
            "LOUDNESS · LUFS-M",
            FontId::new(11.0, egui::FontFamily::Name(SG500.into())),
            TEXT_MUTED,
        );

        let orig = self.shared.meters.original();
        let env = self.shared.meters.environment();
        for (i, (label, value, colour)) in [
            ("Original", orig, ORIGINAL_TEAL),
            ("Environment", env, ACCENT),
        ]
        .iter()
        .enumerate()
        {
            let y = right.top() + 28.0 + i as f32 * 20.0;
            painter.text(
                Pos2::new(right.left(), y),
                Align2::LEFT_CENTER,
                *label,
                sg(12.0, 400),
                TEXT_MUTED,
            );
            let bar = Rect::from_min_size(
                Pos2::new(right.left() + 82.0, y - 4.0),
                Vec2::new(right.width() - 82.0 - 42.0, 8.0),
            );
            w::meter(painter, bar, *value, *colour);
            painter.text(
                Pos2::new(right.right(), y),
                Align2::RIGHT_CENTER,
                format_lufs(*value),
                mono(12.0, 500),
                TEXT_PRIMARY,
            );
        }

        painter.line_segment(
            [
                Pos2::new(right.left(), right.top() + 66.0),
                Pos2::new(right.right(), right.top() + 66.0),
            ],
            Stroke::new(1.0, BORDER),
        );

        let toggles = [
            ("Match loudness", &self.params.match_loudness),
            ("Mono sum", &self.params.mono_sum),
        ];
        for (i, (label, param)) in toggles.iter().enumerate() {
            let y = right.top() + 86.0 + i as f32 * 30.0;
            painter.text(
                Pos2::new(right.left(), y),
                Align2::LEFT_CENTER,
                *label,
                sg(13.0, 500),
                TEXT_BODY,
            );
            let track = Rect::from_min_size(
                Pos2::new(right.right() - 38.0, y - 11.0),
                Vec2::new(38.0, 22.0),
            );
            let on = param.value();
            if w::toggle(ui, ui.scope_id().with(("toggle", i)), track, on).clicked() {
                sink.set_bool(*param, !on);
            }
        }
    }
}

fn truncate_to_width(painter: &egui::Painter, text: &str, font_id: &FontId, max_w: f32) -> String {
    let full_w = painter
        .layout_no_wrap(text.to_owned(), font_id.clone(), Color32::WHITE)
        .size()
        .x;
    if full_w <= max_w {
        return text.to_owned();
    }
    let ellipsis = "…";
    let ellipsis_w = painter
        .layout_no_wrap(ellipsis.to_owned(), font_id.clone(), Color32::WHITE)
        .size()
        .x;
    let target_w = (max_w - ellipsis_w).max(0.0);
    let mut truncated = String::new();
    for ch in text.chars() {
        truncated.push(ch);
        let w = painter
            .layout_no_wrap(truncated.clone(), font_id.clone(), Color32::WHITE)
            .size()
            .x;
        if w > target_w {
            truncated.pop();
            break;
        }
    }
    truncated.push_str(ellipsis);
    truncated
}


fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        lerp(a.r(), b.r()),
        lerp(a.g(), b.g()),
        lerp(a.b(), b.b()),
        lerp(a.a(), b.a()),
    )
}

fn format_duration(seconds: f32) -> String {
    if seconds <= 0.0 {
        "—".to_owned()
    } else if seconds < 0.1 {
        format!("{:.1} ms", seconds * 1000.0)
    } else {
        format!("{seconds:.2} s")
    }
}

fn format_lufs(value: f32) -> String {
    if value <= -69.0 || !value.is_finite() {
        "—".to_owned()
    } else {
        if value < 0.0 {
            format!("−{:.1}", -value)
        } else {
            format!("{value:.1}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use std::sync::RwLock;

    fn editor() -> RoomcheckEditor {
        let params = RoomcheckParams::arc();
        let library = Arc::new(RwLock::new(crate::presets::scan()));

        // Seed the analysis from a real factory IR so the plots show real curves.
        let mut analysis_tx = None;
        let mut analysis_rx = None;
        for preset in library.read().unwrap().iter().filter(|p| p.factory) {
            if let Ok(channels) = crate::presets::load_preset(preset, 48_000) {
                let analysis = crate::analysis::analyze_ir(&channels, 48_000.0);
                let (tx, rx) = triple_buffer::triple_buffer(&Arc::new(analysis));
                analysis_tx = Some(tx);
                analysis_rx = Some(rx);
                break;
            }
        }
        let analysis_rx = analysis_rx.unwrap_or_else(|| {
            triple_buffer::triple_buffer(&Arc::new(crate::analysis::IrAnalysis::default())).1
        });
        drop(analysis_tx);

        let (mut spectrum_tx, spectrum_rx) =
            triple_buffer::triple_buffer(&[0.0f32; SPECTRUM_BANDS]);
        let spectrum: [f32; SPECTRUM_BANDS] = std::array::from_fn(|i| {
            let base = 0.62 - 0.22 * (i as f32 / SPECTRUM_BANDS as f32);
            (base + 0.12 * (i as f32 * 1.7).sin()).clamp(0.08, 0.95)
        });
        let _ = spectrum_tx.write(spectrum);

        let meters = Arc::new(Meters::new());
        meters.set(-14.1, -14.9);

        let shared = Arc::new(UiShared {
            analysis: Mutex::new(analysis_rx),
            spectrum: Mutex::new(spectrum_rx),
            meters,
            library,
            sample_rate: Arc::new(AtomicU32::new(48_000)),
        });
        RoomcheckEditor::new(params, shared)
    }

    /// Paints the editor offscreen so layout regressions are visible in CI.
    #[test]
    fn renders_at_design_size() {
        let editor = editor();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(WIDTH, HEIGHT))
            .with_context_setup(|ctx| {
                theme::install(ctx);
            })
            .build_ui_state(
                move |ui, state: &mut RoomcheckEditor| {
                    state.render(ui, &Sink::headless());
                },
                editor,
            );

        harness.run();

        let image = harness.render().expect("offscreen render");
        assert_eq!(image.width(), WIDTH as u32);
        assert_eq!(image.height(), HEIGHT as u32);

        let path = std::env::temp_dir().join("roomcheck-editor.png");
        image.save(&path).expect("save snapshot");
        println!("wrote {}", path.display());

        // Make sure the canvas was actually painted, not left transparent.
        let painted = image.pixels().filter(|p| p[3] > 0).count() as f32 / (WIDTH * HEIGHT);
        assert!(
            painted > 0.95,
            "only {:.0}% of the canvas was painted",
            painted * 100.0
        );
    }

    #[test]
    fn test_truncate_to_width() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(WIDTH, HEIGHT))
            .with_context_setup(|ctx| {
                theme::install(ctx);
            })
            .build_ui(|ui| {
                let font = sg(14.0, 500);
                let painter = ui.painter();
                let short = truncate_to_width(painter, "Phone", &font, 100.0);
                assert_eq!(short, "Phone");

                let long = "Very Long Impulse Response Name That Exceeds Available Width";
                let truncated = truncate_to_width(painter, long, &font, 80.0);
                assert!(truncated.ends_with('…'));
                let w = painter
                    .layout_no_wrap(truncated, font, Color32::WHITE)
                    .size()
                    .x;
                assert!(w <= 80.0);
            });
        harness.run();
    }
}
