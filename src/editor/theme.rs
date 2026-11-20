//! Visual identity for Roomcheck, lifted 1:1 from the exported design.

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
};

pub const BG_APP: Color32 = Color32::from_rgb(0x0f, 0x0f, 0x0e);
pub const BG_BAR: Color32 = Color32::from_rgb(0x14, 0x13, 0x11);
pub const BG_CARD: Color32 = Color32::from_rgb(0x18, 0x17, 0x15);
pub const BG_INPUT: Color32 = Color32::from_rgb(0x1c, 0x1b, 0x18);
pub const BG_PILL: Color32 = Color32::from_rgb(0x10, 0x0f, 0x0e);
pub const BG_DIAL: Color32 = Color32::from_rgb(0x20, 0x1f, 0x1c);
pub const BG_DIAL_INNER: Color32 = Color32::from_rgb(0x26, 0x24, 0x20);

pub const BORDER: Color32 = Color32::from_rgb(0x26, 0x24, 0x1f);
pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(0x2b, 0x29, 0x25);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x3a, 0x37, 0x30);
pub const GRID: Color32 = Color32::from_rgb(0x2a, 0x28, 0x24);
pub const TRACK: Color32 = Color32::from_rgb(0x2c, 0x2a, 0x26);

pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xec, 0xe8, 0xdf);
pub const TEXT_BODY: Color32 = Color32::from_rgb(0xcf, 0xca, 0xc0);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0xa0, 0x9b, 0x90);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x6d, 0x69, 0x60);

pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0xb3, 0x47);
pub const ACCENT_INK: Color32 = Color32::from_rgb(0x1a, 0x12, 0x08);
pub const ORIGINAL_TEAL: Color32 = Color32::from_rgb(0x7f, 0xd8, 0xc8);

pub const SG400: &str = "sg-400";
pub const SG500: &str = "sg-500";
pub const SG600: &str = "sg-600";
pub const MONO400: &str = "mono-400";
pub const MONO500: &str = "mono-500";

pub fn sg(size: f32, weight: u16) -> FontId {
    let family = match weight {
        600.. => SG600,
        500.. => SG500,
        _ => SG400,
    };
    FontId::new(size, FontFamily::Name(family.into()))
}

pub fn mono(size: f32, weight: u16) -> FontId {
    let family = if weight >= 500 { MONO500 } else { MONO400 };
    FontId::new(size, FontFamily::Name(family.into()))
}

pub fn hairline() -> Stroke {
    Stroke::new(1.0, BORDER)
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |key: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(key.to_owned(), FontData::from_static(bytes).into());
        fonts
            .families
            .entry(FontFamily::Name(key.into()))
            .or_default()
            .insert(0, key.to_owned());
    };
    add(SG400, include_bytes!("../../assets/fonts/SpaceGrotesk-400.ttf"));
    add(SG500, include_bytes!("../../assets/fonts/SpaceGrotesk-500.ttf"));
    add(SG600, include_bytes!("../../assets/fonts/SpaceGrotesk-600.ttf"));
    add(
        MONO400,
        include_bytes!("../../assets/fonts/JetBrainsMono-400.ttf"),
    );
    add(
        MONO500,
        include_bytes!("../../assets/fonts/JetBrainsMono-500.ttf"),
    );

    // Proportional fallback for anything that does not ask for a family explicitly.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, SG400.to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, MONO400.to_owned());

    ctx.set_fonts(fonts);

    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = BG_APP;
    style.visuals.window_fill = BG_CARD;
    style.visuals.extreme_bg_color = BG_INPUT;
    style.visuals.override_text_color = Some(TEXT_PRIMARY);
    style.visuals.widgets.noninteractive.bg_fill = BG_CARD;
    style.visuals.widgets.noninteractive.bg_stroke = hairline();
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    style.visuals.widgets.inactive.bg_fill = BG_INPUT;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER_SUBTLE);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_BODY);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x24, 0x22, 0x1e);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(0x2a, 0x27, 0x22);
    style.visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(0xff, 0xb3, 0x47, 40);
    style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    style.visuals.window_corner_radius = CornerRadius::same(12);
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);

    style.text_styles.insert(TextStyle::Body, sg(13.0, 400));
    style.text_styles.insert(TextStyle::Button, sg(14.0, 500));
    style.text_styles.insert(TextStyle::Small, sg(11.0, 500));
    style.text_styles.insert(TextStyle::Monospace, mono(12.0, 400));

    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
}

