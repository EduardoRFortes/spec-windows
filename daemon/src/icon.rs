// Tray icon rendering: the Claude Code mascot silhouette (ported from
// spec-fedora's daemon/specd/icons/spec-symbolic.svg — same path data,
// same evenodd fill rule for the ear notches) plus two usage bars baked
// into the bottom strip the mascot's legs leave empty (viewBox y=90..100),
// plus a pair of eye cutouts in the head band that swap between "open"
// (small squares) and "closed" (thin bars) for a slow idle blink — see
// main.rs's IDLE_BLINK_INTERVAL. Not tied to pending requests at all
// anymore; that's the toast notification's job now.
//
// Windows tray icons are a plain bitmap with no GNOME-style "symbolic,
// theme recolors it" convention, so the mascot's color is fixed here
// instead (Claude's brand orange) rather than inherited from a taskbar
// theme.

use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Rect, Transform};
use tray_icon::Icon;

const CANVAS: u32 = 32;
/// The SVG path is authored in a 100x100 viewBox; everything here is in
/// those units and scaled down to CANVAS at fill time.
const SCALE: f32 = CANVAS as f32 / 100.0;

fn add_rect(pb: &mut PathBuilder, x0: f32, y0: f32, x1: f32, y1: f32) {
    pb.move_to(x0, y0);
    pb.line_to(x1, y0);
    pb.line_to(x1, y1);
    pb.line_to(x0, y1);
    pb.close();
}

fn mascot_path(eyes_open: bool) -> Path {
    let mut pb = PathBuilder::new();
    let rects: [(f32, f32, f32, f32); 7] = [
        (9.0, 10.0, 91.0, 28.0),  // head top band
        (1.0, 28.0, 99.0, 48.0),  // ears band, full width
        (9.0, 48.0, 91.0, 69.0),  // body band
        (9.0, 69.0, 18.0, 90.0),  // leg 1
        (27.0, 69.0, 36.0, 90.0), // leg 2
        (64.0, 69.0, 73.0, 90.0), // leg 3
        (82.0, 69.0, 91.0, 90.0), // leg 4
    ];
    for (x0, y0, x1, y1) in rects {
        add_rect(&mut pb, x0, y0, x1, y1);
    }
    // Ear notches: evenodd fill makes these subtract from the ears band
    // above rather than add to it, same as the source SVG.
    pb.move_to(17.0, 28.0);
    pb.line_to(17.0, 42.0);
    pb.line_to(32.0, 35.0);
    pb.close();
    pb.move_to(83.0, 28.0);
    pb.line_to(83.0, 42.0);
    pb.line_to(68.0, 35.0);
    pb.close();
    // Eyes, same evenodd-subtraction trick as the ear notches: small dot
    // squares when open, thin bars when closed, centered in the head band.
    for cx in [30.0, 70.0] {
        let cy = 19.0;
        if eyes_open {
            add_rect(&mut pb, cx - 3.5, cy - 3.5, cx + 3.5, cy + 3.5);
        } else {
            add_rect(&mut pb, cx - 5.0, cy - 1.25, cx + 5.0, cy + 1.25);
        }
    }
    pb.finish().expect("mascot path is well-formed")
}

fn solid_paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    paint
}

/// Green under 70%, orange 70-90%, red above — same thresholds a battery
/// or disk-usage indicator would use, nothing Spec-specific.
fn usage_color(pct: f64) -> Color {
    if pct >= 90.0 {
        Color::from_rgba8(0xe0, 0x3a, 0x3a, 0xff)
    } else if pct >= 70.0 {
        Color::from_rgba8(0xe6, 0x7e, 0x22, 0xff)
    } else {
        Color::from_rgba8(0x2b, 0x8a, 0x3e, 0xff)
    }
}

/// One bar's track (background) plus its fill, both in 100x100-viewBox
/// units, drawn into the bottom strip (y=90..100) the mascot's legs leave
/// empty.
fn draw_bar(pixmap: &mut Pixmap, x0: f32, x1: f32, pct: Option<f64>) {
    let y0 = 91.5;
    let y1 = 98.5;
    let track = Rect::from_ltrb(x0 * SCALE, y0 * SCALE, x1 * SCALE, y1 * SCALE);
    if let Some(track) = track {
        pixmap.fill_rect(
            track,
            &solid_paint(Color::from_rgba8(0x55, 0x55, 0x55, 0xff)),
            Transform::identity(),
            None,
        );
    }
    let Some(pct) = pct else { return };
    let pct = pct.clamp(0.0, 100.0);
    let fill_x1 = x0 + (x1 - x0) * (pct as f32 / 100.0);
    if let Some(fill) = Rect::from_ltrb(x0 * SCALE, y0 * SCALE, fill_x1 * SCALE, y1 * SCALE) {
        pixmap.fill_rect(
            fill,
            &solid_paint(usage_color(pct)),
            Transform::identity(),
            None,
        );
    }
}

pub struct UsageSnapshot {
    pub five_hour_pct: Option<f64>,
    pub seven_day_pct: Option<f64>,
}

/// Claude's brand orange — the mascot is always this color now; there's no
/// more pending-request tint (the toast notification covers that signal).
fn mascot_color() -> Color {
    Color::from_rgba8(0xd9, 0x77, 0x57, 0xff)
}

pub fn render_icon(eyes_open: bool, usage: &UsageSnapshot) -> Icon {
    let mut pixmap = Pixmap::new(CANVAS, CANVAS).expect("nonzero canvas size");
    pixmap.fill_path(
        &mascot_path(eyes_open),
        &solid_paint(mascot_color()),
        FillRule::EvenOdd,
        Transform::from_scale(SCALE, SCALE),
        None,
    );
    draw_bar(&mut pixmap, 9.0, 47.0, usage.five_hour_pct);
    draw_bar(&mut pixmap, 53.0, 91.0, usage.seven_day_pct);

    Icon::from_rgba(pixmap.data().to_vec(), CANVAS, CANVAS).expect("valid icon buffer")
}
