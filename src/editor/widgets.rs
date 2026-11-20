//! Custom-painted widgets: rotary knobs, pill toggles, the response and decay
//! plots, and the little line-art icons used by the sidebar and map cards.

use crate::editor::theme::*;
use egui::{
    Align2, Color32, CornerRadius, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};
use std::f32::consts::PI;

pub fn rounded(painter: &egui::Painter, rect: Rect, radius: u8, fill: Color32, stroke: Stroke) {
    painter.rect(
        rect,
        CornerRadius::same(radius),
        fill,
        stroke,
        StrokeKind::Inside,
    );
}

pub fn card(painter: &egui::Painter, rect: Rect) {
    rounded(painter, rect, 12, BG_CARD, Stroke::new(1.0, BORDER));
}

/// Filled area under a curve, fading from `top` to `bottom` like the CSS
/// gradients in the design export.
pub fn gradient_area(
    painter: &egui::Painter,
    points: &[Pos2],
    baseline: f32,
    top: Color32,
    bottom: Color32,
) {
    let mut mesh = egui::Mesh::default();
    for pair in points.windows(2) {
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(pair[0], top);
        mesh.colored_vertex(pair[1], top);
        mesh.colored_vertex(Pos2::new(pair[1].x, baseline), bottom);
        mesh.colored_vertex(Pos2::new(pair[0].x, baseline), bottom);
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    }
    painter.add(Shape::mesh(mesh));
}

/// A soft accent halo, standing in for the CSS box-shadow on the reference pill.
pub fn glow(painter: &egui::Painter, rect: Rect, radius: u8) {
    for (i, alpha) in [(0.0f32, 10u8), (3.0, 8), (6.0, 5)] {
        let expanded = Rect::from_min_max(
            Pos2::new(rect.left() - 4.0 - i, rect.top() - 4.0 - i),
            Pos2::new(rect.right() + 4.0 + i, rect.bottom() + 4.0 + i),
        );
        painter.rect_filled(
            expanded,
            CornerRadius::same(radius.saturating_add(6)),
            Color32::from_rgba_unmultiplied(0xff, 0xb3, 0x47, alpha),
        );
    }
}

pub fn dashed_line(painter: &egui::Painter, from: Pos2, to: Pos2, stroke: Stroke) {
    let delta = to - from;
    let len = delta.length();
    if len < 0.5 {
        return;
    }
    let dir = delta / len;
    let mut t = 0.0;
    while t < len {
        let end = (t + 4.0).min(len);
        painter.line_segment([from + dir * t, from + dir * end], stroke);
        t += 7.0;
    }
}

/// Dashed rounded outline, used by the import drop zone.
pub fn dashed_outline(painter: &egui::Painter, rect: Rect, radius: u8, stroke: Stroke) {
    let r = (radius as f32).min(rect.width() * 0.5).min(rect.height() * 0.5);
    let corners = [
        (Pos2::new(rect.right() - r, rect.top() + r), -90.0f32),
        (Pos2::new(rect.right() - r, rect.bottom() - r), 0.0),
        (Pos2::new(rect.left() + r, rect.bottom() - r), 90.0),
        (Pos2::new(rect.left() + r, rect.top() + r), 180.0),
    ];

    let mut points = Vec::with_capacity(40);
    for (centre, start) in corners {
        for i in 0..=8 {
            let a = (start + (90.0 / 8.0) * i as f32).to_radians();
            points.push(centre + Vec2::new(a.cos(), a.sin()) * r);
        }
    }
    if let Some(&first) = points.first() {
        points.push(first);
    }

    let dash_len = 4.0f32;
    let gap_len = 3.5f32;
    let period = dash_len + gap_len;
    let mut dist = 0.0f32;

    for pair in points.windows(2) {
        let p1 = pair[0];
        let p2 = pair[1];
        let seg_len = (p2 - p1).length();
        if seg_len < 0.001 {
            continue;
        }
        let dir = (p2 - p1) / seg_len;
        let mut cur = 0.0;
        while cur < seg_len {
            let phase = (dist + cur) % period;
            if phase < dash_len {
                let draw_end = (cur + (dash_len - phase)).min(seg_len);
                painter.line_segment([p1 + dir * cur, p1 + dir * draw_end], stroke);
                cur = draw_end;
            } else {
                let skip_end = (cur + (period - phase)).min(seg_len);
                cur = skip_end;
            }
        }
        dist += seg_len;
    }
}

// ---------------------------------------------------------------------------
// Knobs
// ---------------------------------------------------------------------------

pub struct KnobOut {
    pub norm: f32,
    pub changed: bool,
    pub drag_started: bool,
    pub drag_stopped: bool,
}

/// Draws the value pill, the 270° amber dial, the pointer and the label.
/// `rect` covers the whole cell; the knob is centred horizontally.
pub fn knob(
    ui: &mut Ui,
    id: egui::Id,
    rect: Rect,
    norm: f32,
    label: &str,
    value: &str,
    default_norm: f32,
) -> KnobOut {
    let response = ui.interact(rect, id, Sense::click_and_drag());

    let mut new_norm = norm;
    let mut changed = false;
    if response.double_clicked() {
        new_norm = default_norm;
        changed = true;
    } else if response.dragged() {
        let speed = if ui.input(|i| i.modifiers.shift) {
            0.0012
        } else {
            0.0045
        };
        new_norm = (norm - response.drag_delta().y * speed).clamp(0.0, 1.0);
        changed = true;
    }

    let painter = ui.painter();
    let centre_x = rect.center().x;

    // Value pill. The design lets it spill a few pixels past its grid cell.
    let pill = Rect::from_center_size(
        Pos2::new(centre_x, rect.top() + 12.0),
        Vec2::new(60.0, 24.0),
    );
    rounded(
        painter,
        pill,
        8,
        BG_PILL,
        Stroke::new(1.0, BORDER_SUBTLE),
    );
    painter.text(
        pill.center(),
        Align2::CENTER_CENTER,
        value,
        mono(12.0, 500),
        TEXT_PRIMARY,
    );

    // Dial geometry, scaled so adjacent knob arcs never collide.
    let dial_centre = Pos2::new(centre_x, pill.bottom() + 6.0 + 25.0);
    let radius = 25.0;
    let start = 135.0_f32.to_radians();
    let sweep = 270.0_f32.to_radians();

    draw_arc(painter, dial_centre, radius, start, sweep, 4.2, TRACK);
    if new_norm > 0.002 {
        draw_arc(
            painter,
            dial_centre,
            radius,
            start,
            sweep * new_norm,
            4.2,
            ACCENT,
        );
    }

    painter.circle(
        dial_centre,
        18.5,
        BG_DIAL,
        Stroke::new(1.5, Color32::from_rgb(0x35, 0x32, 0x2c)),
    );
    painter.circle_filled(dial_centre, 14.5, BG_DIAL_INNER);

    let angle = start + sweep * new_norm;
    let dir = Vec2::new(angle.cos(), angle.sin());
    painter.line_segment(
        [dial_centre + dir * 17.0, dial_centre + dir * 23.0],
        Stroke::new(2.4, ACCENT),
    );

    painter.text(
        Pos2::new(centre_x, dial_centre.y + radius + 11.0),
        Align2::CENTER_CENTER,
        label,
        sg(13.0, 500),
        TEXT_BODY,
    );

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }

    KnobOut {
        norm: new_norm,
        changed,
        drag_started: response.drag_started() || response.double_clicked(),
        drag_stopped: response.drag_stopped() || response.double_clicked(),
    }
}

fn draw_arc(
    painter: &egui::Painter,
    centre: Pos2,
    radius: f32,
    start: f32,
    sweep: f32,
    width: f32,
    colour: Color32,
) {
    let steps = ((sweep.abs() / (2.0 * PI)) * 96.0).ceil().max(6.0) as usize;
    let points: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let a = start + sweep * (i as f32 / steps as f32);
            centre + Vec2::new(a.cos(), a.sin()) * radius
        })
        .collect();
    painter.add(Shape::line(points, Stroke::new(width, colour)));
}

pub fn reset_icon(painter: &egui::Painter, centre: Pos2, colour: Color32) {
    let r = 4.0;
    let stroke = Stroke::new(1.3, colour);
    draw_arc(painter, centre, r, 0.7, 4.8, 1.3, colour);
    let tip = centre + Vec2::new(0.7f32.cos(), 0.7f32.sin()) * r;
    painter.line_segment([tip, Pos2::new(tip.x + 3.0, tip.y - 0.5)], stroke);
    painter.line_segment([tip, Pos2::new(tip.x + 0.5, tip.y + 3.0)], stroke);
}

// ---------------------------------------------------------------------------
// Toggles & buttons
// ---------------------------------------------------------------------------

pub fn toggle(ui: &mut Ui, id: egui::Id, rect: Rect, on: bool) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    let painter = ui.painter();
    let track = if on {
        ACCENT
    } else {
        Color32::from_rgb(0x34, 0x31, 0x2b)
    };
    painter.rect_filled(rect, CornerRadius::same(11), track);
    let thumb_x = if on {
        rect.right() - 11.0
    } else {
        rect.left() + 11.0
    };
    painter.circle_filled(
        Pos2::new(thumb_x, rect.center().y),
        8.0,
        if on { ACCENT_INK } else { TEXT_MUTED },
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

pub fn pressable(ui: &mut Ui, id: egui::Id, rect: Rect) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

pub fn holdable(ui: &mut Ui, id: egui::Id, rect: Rect) -> Response {
    let response = ui.interact(rect, id, Sense::click_and_drag());
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

// ---------------------------------------------------------------------------
// Frequency response plot
// ---------------------------------------------------------------------------

/// Paints the whole hero plot and returns its inner plotting rect.
#[allow(clippy::too_many_arguments)]
pub fn freq_plot(
    ui: &mut Ui,
    rect: Rect,
    curve: &[f32],
    bars: &[f32],
    accent: Color32,
) -> Rect {
    let plot = Rect::from_min_max(
        Pos2::new(rect.left() + 40.0, rect.top() + 4.0),
        Pos2::new(rect.right() - 8.0, rect.bottom() - 22.0),
    );
    let painter = ui.painter_at(rect);
    let l20 = 20.0f32.log10();
    let fx = |f: f32| plot.left() + ((f.log10() - l20) / 3.0) * plot.width();
    let fy = |db: f32| plot.top() + ((12.0 - db) / 36.0) * plot.height();

    // Live mix bars.
    if bars.len() > 1 {
        let n = bars.len();
        let bar_w = (plot.width() / n as f32 * 0.9).clamp(3.0, 8.0);
        for (i, v) in bars.iter().enumerate() {
            let f = 20.0 * 1000.0f32.powf(i as f32 / (n - 1) as f32);
            let x = fx(f);
            let h = v.clamp(0.0, 1.0) * plot.height() * 0.82;
            let bar = Rect::from_min_max(
                Pos2::new(x - bar_w * 0.5, plot.bottom() - h),
                Pos2::new(x + bar_w * 0.5, plot.bottom()),
            );
            painter.rect_filled(bar, CornerRadius::same(1), Color32::from_white_alpha(20));
        }
    }

    // Grid.
    let grid = Stroke::new(1.0, GRID);
    for (f, label) in [
        (20.0, "20"),
        (50.0, "50"),
        (100.0, "100"),
        (200.0, "200"),
        (500.0, "500"),
        (1000.0, "1k"),
        (2000.0, "2k"),
        (5000.0, "5k"),
        (10000.0, "10k"),
        (20000.0, "20k"),
    ] {
        let x = fx(f);
        if f > 20.0 && f < 20000.0 {
            painter.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], grid);
        }
        painter.text(
            Pos2::new(x, plot.bottom() + 5.0),
            Align2::CENTER_TOP,
            label,
            mono(10.5, 400),
            TEXT_MUTED,
        );
    }
    for (db, label) in [(6.0, "+6"), (0.0, "0"), (-6.0, "−6"), (-12.0, "−12"), (-18.0, "−18")] {
        let y = fy(db);
        if db == 0.0 {
            let mut x = plot.left();
            let dash = Stroke::new(1.2, Color32::from_rgb(0x8d, 0x89, 0x7f));
            while x < plot.right() {
                painter.line_segment(
                    [Pos2::new(x, y), Pos2::new((x + 4.0).min(plot.right()), y)],
                    dash,
                );
                x += 8.0;
            }
        } else {
            painter.line_segment([Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)], grid);
        }
        painter.text(
            Pos2::new(plot.left() - 8.0, y),
            Align2::RIGHT_CENTER,
            label,
            mono(10.5, 400),
            TEXT_MUTED,
        );
    }

    if curve.len() < 2 {
        return plot;
    }

    let n = curve.len();
    let points: Vec<Pos2> = curve
        .iter()
        .enumerate()
        .map(|(i, db)| {
            let f = 20.0 * 1000.0f32.powf(i as f32 / (n - 1) as f32);
            Pos2::new(fx(f), fy(db.clamp(-24.0, 12.0)))
        })
        .collect();

    gradient_area(
        &painter,
        &points,
        plot.bottom(),
        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 102),
        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 5),
    );
    painter.add(Shape::line(points.clone(), Stroke::new(2.25, accent)));

    // Hover readout.
    if let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) {
        if plot.contains(pointer) {
            let norm_x = ((pointer.x - plot.left()) / plot.width()).clamp(0.0, 1.0);
            let idx = ((norm_x * (n - 1) as f32).round() as usize).min(n - 1);
            let freq = 20.0 * 1000.0f32.powf(norm_x);
            let db = curve[idx];
            let pt = points[idx];

            painter.line_segment(
                [Pos2::new(pt.x, plot.top()), Pos2::new(pt.x, plot.bottom())],
                Stroke::new(1.0, Color32::from_white_alpha(90)),
            );
            painter.circle(pt, 5.0, BG_APP, Stroke::new(2.0, accent));

            let freq_text = if freq >= 1000.0 {
                format!("{:.1} kHz", freq / 1000.0)
            } else {
                format!("{freq:.0} Hz")
            };
            let tip = format!("{freq_text}  {:+.1} dB", db);
            let tip_w = 132.0;
            let tip_pos = Pos2::new(
                (pt.x + 12.0).min(plot.right() - tip_w),
                (pt.y - 34.0).max(plot.top() + 2.0),
            );
            let tip_rect = Rect::from_min_size(tip_pos, Vec2::new(tip_w, 24.0));
            painter.rect(
                tip_rect,
                CornerRadius::same(6),
                BG_APP,
                Stroke::new(1.0, Color32::from_rgb(0x34, 0x31, 0x2b)),
                StrokeKind::Inside,
            );
            painter.text(
                tip_rect.center(),
                Align2::CENTER_CENTER,
                tip,
                mono(11.0, 500),
                TEXT_PRIMARY,
            );
        }
    }

    plot
}

// ---------------------------------------------------------------------------
// Decay plot
// ---------------------------------------------------------------------------

pub fn decay_plot(
    painter: &egui::Painter,
    rect: Rect,
    decay_db: &[f32],
    span: f32,
    early: &[(f32, f32)],
) {
    let painter = painter.with_clip_rect(rect);
    let height = rect.height() - 16.0;
    let bottom = rect.top() + height;

    painter.line_segment(
        [
            Pos2::new(rect.left(), bottom + 0.5),
            Pos2::new(rect.right(), bottom + 0.5),
        ],
        Stroke::new(1.0, GRID),
    );

    if decay_db.len() < 2 {
        return;
    }
    let n = decay_db.len();
    let points: Vec<Pos2> = decay_db
        .iter()
        .enumerate()
        .map(|(i, db)| {
            let x = rect.left() + rect.width() * i as f32 / (n - 1) as f32;
            let y = rect.top() + (-db / 60.0).clamp(0.0, 1.0) * height;
            Pos2::new(x, y)
        })
        .collect();

    gradient_area(
        &painter,
        &points,
        bottom,
        Color32::from_rgba_unmultiplied(0xff, 0xb3, 0x47, 89),
        Color32::from_rgba_unmultiplied(0xff, 0xb3, 0x47, 5),
    );
    painter.add(Shape::line(
        points,
        Stroke::new(1.5, Color32::from_rgba_premultiplied(0xff, 0xb3, 0x47, 190)),
    ));

    if span > 0.0 {
        for (t, db) in early {
            let x = rect.left() + rect.width() * (t / span).clamp(0.0, 1.0);
            let y = rect.top() + (-db / 60.0).clamp(0.0, 1.0) * height;
            painter.line_segment(
                [Pos2::new(x, bottom), Pos2::new(x, y)],
                Stroke::new(2.0, ACCENT),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Icons
// ---------------------------------------------------------------------------

pub fn draw_icon(ui: &Ui, kind: crate::presets::Icon, rect: Rect, colour: Color32) {
    let painter = ui.painter();
    let s = Stroke::new(1.7, colour);
    let x = rect.left();
    let y = rect.top();
    let w = rect.width();
    let h = rect.height();
    let k = w.min(h) / 24.0;

    let line = |a: Pos2, b: Pos2| painter.line_segment([a, b], s);
    let p = |px: f32, py: f32| Pos2::new(x + px * k, y + py * k);
    let outline = |r: Rect, radius: u8| {
        painter.rect(
            r,
            CornerRadius::same(radius),
            Color32::TRANSPARENT,
            s,
            StrokeKind::Inside,
        )
    };

    use crate::presets::Icon as I;
    match kind {
        I::Phone => {
            outline(
                Rect::from_min_max(p(7.0, 3.0), p(17.0, 21.0)),
                3,
            );
            line(p(10.5, 18.0), p(13.5, 18.0));
        }
        I::Tablet => {
            outline(Rect::from_min_max(p(5.0, 3.5), p(19.0, 20.5)), 2);
            painter.circle_filled(p(12.0, 18.0), 0.6 * k, colour);
        }
        I::Bluetooth => {
            outline(Rect::from_min_max(p(7.0, 3.0), p(17.0, 21.0)), 3);
            painter.circle_stroke(p(12.0, 12.0), 2.6 * k, s);
        }
        I::Laptop => {
            outline(Rect::from_min_max(p(5.0, 6.0), p(19.0, 16.0)), 1);
            line(p(2.5, 19.0), p(21.5, 19.0));
        }
        I::Car => {
            line(p(4.0, 16.0), p(4.0, 12.0));
            line(p(4.0, 12.0), p(6.0, 8.0));
            line(p(6.0, 8.0), p(18.0, 8.0));
            line(p(18.0, 8.0), p(20.0, 12.0));
            line(p(20.0, 12.0), p(20.0, 16.0));
            line(p(4.0, 16.0), p(20.0, 16.0));
            line(p(4.0, 12.0), p(20.0, 12.0));
            painter.circle_filled(p(7.5, 17.0), 1.2 * k, colour);
            painter.circle_filled(p(16.5, 17.0), 1.2 * k, colour);
        }
        I::Bus => {
            outline(Rect::from_min_max(p(4.0, 4.0), p(20.0, 17.0)), 1);
            line(p(4.0, 10.0), p(20.0, 10.0));
            painter.circle_filled(p(7.5, 18.5), 1.1 * k, colour);
            painter.circle_filled(p(16.5, 18.5), 1.1 * k, colour);
        }
        I::Club => {
            for (i, top) in [10.0, 6.0, 8.5, 4.5, 10.0].iter().enumerate() {
                let px = 4.0 + i as f32 * 4.0;
                line(p(px, *top), p(px, 20.0 - top + 4.0));
            }
        }
        I::Hall => {
            line(p(3.0, 20.0), p(21.0, 20.0));
            line(p(5.0, 20.0), p(5.0, 10.0));
            line(p(5.0, 10.0), p(12.0, 4.0));
            line(p(12.0, 4.0), p(19.0, 10.0));
            line(p(19.0, 10.0), p(19.0, 20.0));
            line(p(9.5, 20.0), p(9.5, 15.0));
            line(p(9.5, 15.0), p(14.5, 15.0));
            line(p(14.5, 15.0), p(14.5, 20.0));
        }
        I::Bed => {
            line(p(3.0, 19.0), p(3.0, 11.0));
            line(p(3.0, 11.0), p(5.0, 11.0));
            line(p(5.0, 11.0), p(19.0, 11.0));
            line(p(19.0, 11.0), p(21.0, 13.0));
            line(p(21.0, 13.0), p(21.0, 19.0));
            line(p(3.0, 15.5), p(21.0, 15.5));
            outline(Rect::from_min_max(p(7.0, 6.5), p(11.0, 11.0)), 1);
        }
        I::Speaker => {
            outline(Rect::from_min_max(p(6.0, 3.0), p(18.0, 21.0)), 2);
            // Tweeter
            painter.circle_stroke(p(12.0, 7.5), 1.6 * k, s);
            painter.circle_filled(p(12.0, 7.5), 0.8 * k, colour);
            // Woofer
            painter.circle_stroke(p(12.0, 15.0), 3.2 * k, s);
            painter.circle_stroke(p(12.0, 15.0), 1.2 * k, Stroke::new(1.2, colour));
        }
    }
}

// ---------------------------------------------------------------------------
// Map sketches
// ---------------------------------------------------------------------------

fn sketch_transform(rect: Rect) -> impl Fn(f32, f32) -> Pos2 {
    let sx = rect.width() / 270.0;
    let sy = rect.height() / 98.0;
    move |x: f32, y: f32| Pos2::new(rect.left() + x * sx, rect.top() + y * sy)
}

pub fn phone_sketch(ui: &Ui, rect: Rect, distance: f32) {
    let painter = ui.painter();
    let t = sketch_transform(rect);
    let outline = |a: (f32, f32), b: (f32, f32), radius: u8| {
        painter.rect(
            Rect::from_min_max(t(a.0, a.1), t(b.0, b.1)),
            CornerRadius::same(radius),
            Color32::TRANSPARENT,
            Stroke::new(1.5, Color32::from_rgb(0x4a, 0x46, 0x3e)),
            StrokeKind::Inside,
        )
    };
    outline((30.0, 6.0), (76.0, 92.0), 6);
    outline((35.0, 14.0), (71.0, 76.0), 2);
    painter.line_segment(
        [t(44.0, 84.0), t(62.0, 84.0)],
        Stroke::new(2.0, Color32::from_rgb(0x8a, 0x85, 0x7a)),
    );
    // Sound arcs.
    let grey = Stroke::new(1.5, Color32::from_rgba_premultiplied(0x8a, 0x85, 0x7a, 220));
    for (dx, dy) in [(84.0, 74.0), (92.0, 68.0), (100.0, 62.0)] {
        let pts = vec![t(dx, dy), t(dx + 8.0, 82.0), t(dx, 91.0 + (dy - 74.0) * 0.2)];
        painter.add(Shape::line(pts, grey));
    }
    let d = distance.clamp(0.0, 1.0);
    let listener_x = 170.0 + d * 55.0;
    let dashed = Stroke::new(1.25, Color32::from_rgb(0x6d, 0x69, 0x60));
    dashed_line(
        &painter,
        t(124.0, 78.0),
        t(listener_x - 12.0, 62.0),
        dashed,
    );
    let cm = (20.0 + d * 80.0).round() as i32;
    let label_pos = t((124.0 + listener_x) * 0.5, 60.0);
    painter.rect_filled(
        Rect::from_center_size(label_pos, Vec2::new(44.0, 15.0)),
        CornerRadius::same(3),
        BG_CARD,
    );
    painter.text(
        label_pos,
        Align2::CENTER_CENTER,
        format!("{cm} cm"),
        mono(11.0, 400),
        TEXT_MUTED,
    );
    painter.circle_stroke(
        t(listener_x, 59.0),
        11.0 * rect.width() / 270.0,
        Stroke::new(1.5, Color32::from_rgba_premultiplied(0xff, 0xb3, 0x47, 100)),
    );
    painter.circle_filled(t(listener_x, 59.0), 5.0 * rect.width() / 270.0, ACCENT);
}

pub fn car_sketch(ui: &Ui, rect: Rect, _distance: f32) {
    let painter = ui.painter();
    let t = sketch_transform(rect);
    let dark = Color32::from_rgb(0x4a, 0x46, 0x3e);
    let mid = Color32::from_rgb(0x3a, 0x37, 0x2f);
    let grey = Color32::from_rgb(0x8a, 0x85, 0x7a);
    let stroke = Stroke::new(1.5, dark);
    painter.rect(
        Rect::from_min_max(t(32.0, 10.0), t(238.0, 88.0)),
        CornerRadius::same(12),
        Color32::TRANSPARENT,
        stroke,
        StrokeKind::Inside,
    );
    painter.add(Shape::line(
        vec![t(178.0, 22.0), t(196.0, 30.0), t(196.0, 68.0), t(178.0, 76.0)],
        stroke,
    ));
    painter.add(Shape::line(
        vec![t(78.0, 22.0), t(64.0, 31.0), t(64.0, 67.0), t(78.0, 76.0)],
        stroke,
    ));
    for (x, y) in [(140.0, 24.0), (140.0, 56.0)] {
        painter.rect(
            Rect::from_min_max(t(x, y), t(x + 16.0, y + 18.0)),
            CornerRadius::same(3),
            Color32::TRANSPARENT,
            Stroke::new(1.5, mid),
            StrokeKind::Inside,
        );
    }
    painter.rect(
        Rect::from_min_max(t(102.0, 22.0), t(116.0, 76.0)),
        CornerRadius::same(4),
        Color32::TRANSPARENT,
        Stroke::new(1.5, mid),
        StrokeKind::Inside,
    );
    for (x, y, r) in [
        (150.0, 15.0, 4.0),
        (150.0, 83.0, 4.0),
        (110.0, 15.0, 4.0),
        (110.0, 83.0, 4.0),
    ] {
        painter.circle_filled(t(x, y), r * rect.width() / 270.0, grey);
    }
    painter.circle_filled(t(52.0, 49.0), 6.0 * rect.width() / 270.0, grey);
    painter.circle_stroke(
        t(148.0, 33.0),
        11.0 * rect.width() / 270.0,
        Stroke::new(1.5, Color32::from_rgba_premultiplied(0xff, 0xb3, 0x47, 100)),
    );
    painter.circle_filled(t(148.0, 33.0), 5.0 * rect.width() / 270.0, ACCENT);
}

pub fn room_sketch(ui: &Ui, rect: Rect, distance: f32) {
    let painter = ui.painter();
    let t = sketch_transform(rect);
    let k = rect.width() / 270.0;
    let dark = Stroke::new(1.5, Color32::from_rgb(0x4a, 0x46, 0x3e));
    let faint = Color32::from_rgb(0x6d, 0x69, 0x60);
    let grey = Color32::from_rgb(0x8a, 0x85, 0x7a);

    painter.rect(
        Rect::from_min_max(t(30.0, 12.0), t(240.0, 86.0)),
        CornerRadius::same(3),
        Color32::TRANSPARENT,
        dark,
        StrokeKind::Inside,
    );

    // Speaker cabinet at the front of the room.
    painter.rect(
        Rect::from_min_max(t(196.0, 32.0), t(216.0, 64.0)),
        CornerRadius::same(3),
        Color32::TRANSPARENT,
        Stroke::new(1.5, grey),
        StrokeKind::Inside,
    );
    painter.circle_stroke(t(206.0, 56.0), 6.0 * k, Stroke::new(1.5, grey));
    painter.circle_stroke(t(206.0, 40.0), 2.6 * k, Stroke::new(1.5, grey));

    // Distance between the speaker and the listening position.
    let listener_x = 96.0 + distance.clamp(0.0, 1.0) * 72.0;
    dashed_line(
        &painter,
        t(listener_x + 14.0, 49.0),
        t(190.0, 49.0),
        Stroke::new(1.25, faint),
    );
    let label_pos = t((listener_x + 190.0) * 0.5, 41.0);
    painter.rect_filled(
        Rect::from_center_size(label_pos, Vec2::new(32.0, 14.0)),
        CornerRadius::same(3),
        BG_CARD,
    );
    painter.text(
        label_pos,
        Align2::CENTER_CENTER,
        "1 m",
        mono(10.0, 400),
        TEXT_MUTED,
    );

    painter.circle_stroke(
        t(listener_x, 49.0),
        11.0 * k,
        Stroke::new(1.5, Color32::from_rgba_premultiplied(0xff, 0xb3, 0x47, 100)),
    );
    painter.circle_filled(t(listener_x, 49.0), 5.0 * k, ACCENT);
}

// ---------------------------------------------------------------------------
// Small helpers used by the rack
// ---------------------------------------------------------------------------

pub fn meter(painter: &egui::Painter, rect: Rect, value: f32, colour: Color32) {
    painter.rect_filled(rect, CornerRadius::same(4), BORDER);
    let norm = ((value + 60.0) / 60.0).clamp(0.0, 1.0);
    if norm > 0.001 {
        let filled = Rect::from_min_size(rect.min, Vec2::new(rect.width() * norm, rect.height()));
        painter.rect_filled(filled, CornerRadius::same(4), colour);
    }
}

pub fn arrow(painter: &egui::Painter, rect: Rect, left: bool, colour: Color32) {
    let c = rect.center();
    let s = Stroke::new(2.0, colour);
    let d = 5.0;
    if left {
        painter.add(Shape::line(
            vec![
                Pos2::new(c.x + d * 0.5, c.y - d),
                Pos2::new(c.x - d * 0.5, c.y),
                Pos2::new(c.x + d * 0.5, c.y + d),
            ],
            s,
        ));
    } else {
        painter.add(Shape::line(
            vec![
                Pos2::new(c.x - d * 0.5, c.y - d),
                Pos2::new(c.x + d * 0.5, c.y),
                Pos2::new(c.x - d * 0.5, c.y + d),
            ],
            s,
        ));
    }
}

/// The little two-slider glyph used by the settings button.
pub fn sliders_icon(painter: &egui::Painter, rect: Rect, colour: Color32) {
    let stroke = Stroke::new(1.8, colour);
    let y1 = rect.top() + rect.height() * 0.32;
    let y2 = rect.top() + rect.height() * 0.68;
    painter.line_segment(
        [Pos2::new(rect.left() + 2.0, y1), Pos2::new(rect.left() + 7.0, y1)],
        stroke,
    );
    painter.line_segment(
        [Pos2::new(rect.left() + 13.0, y1), Pos2::new(rect.right() - 2.0, y1)],
        stroke,
    );
    painter.circle_stroke(Pos2::new(rect.left() + 10.0, y1), 2.2, stroke);
    painter.line_segment(
        [Pos2::new(rect.left() + 2.0, y2), Pos2::new(rect.left() + 11.0, y2)],
        stroke,
    );
    painter.line_segment(
        [Pos2::new(rect.left() + 17.0, y2), Pos2::new(rect.right() - 2.0, y2)],
        stroke,
    );
    painter.circle_stroke(Pos2::new(rect.left() + 14.0, y2), 2.2, stroke);
}

pub fn logo(painter: &egui::Painter, rect: Rect) {
    let scale_x = rect.width() / 28.0;
    let scale_y = rect.height() / 28.0;
    let pt = |x: f32, y: f32| Pos2::new(rect.left() + x * scale_x, rect.top() + y * scale_y);

    // Speaker dot.
    painter.circle_filled(pt(7.0, 14.0), 3.0 * scale_x, ACCENT);

    // Radiating sound wave 1: Q bezier from (13, 9) via (17.5, 14) to (13, 19).
    let stroke1 = Stroke::new(2.4 * scale_x, ACCENT);
    let pts1: Vec<Pos2> = (0..=16)
        .map(|i| {
            let t = i as f32 / 16.0;
            let mt = 1.0 - t;
            let x = mt * mt * 13.0 + 2.0 * mt * t * 17.5 + t * t * 13.0;
            let y = mt * mt * 9.0 + 2.0 * mt * t * 14.0 + t * t * 19.0;
            pt(x, y)
        })
        .collect();
    painter.add(Shape::line(pts1, stroke1));

    // Radiating sound wave 2: Q bezier from (18, 5.5) via (25.5, 14) to (18, 22.5) with 60% opacity.
    let stroke2 = Stroke::new(
        2.4 * scale_x,
        Color32::from_rgba_premultiplied(255, 179, 71, 153),
    );
    let pts2: Vec<Pos2> = (0..=16)
        .map(|i| {
            let t = i as f32 / 16.0;
            let mt = 1.0 - t;
            let x = mt * mt * 18.0 + 2.0 * mt * t * 25.5 + t * t * 18.0;
            let y = mt * mt * 5.5 + 2.0 * mt * t * 14.0 + t * t * 22.5;
            pt(x, y)
        })
        .collect();
    painter.add(Shape::line(pts2, stroke2));
}
