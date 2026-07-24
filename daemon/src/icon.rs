// Tray icon rendering: the Claude Code mascot silhouette (ported from
// spec-fedora's daemon/specd/icons/spec-symbolic.svg — same path data,
// same evenodd fill rule for the ear notches), plus a pair of solid black
// eyes in the head band that swap between "open" (small dots) and
// "closed" (a squinting "><" chevron pair) for a slow idle blink — see
// main.rs's IDLE_BLINK_INTERVAL. Not tied to pending requests at all;
// that's the toast notification's job now.
//
// No usage bars here anymore -- Windows renders tray icons at ~16px, and
// two 2px-tall bars at that scale were unreadable (the user's own words:
// "impossível enxergar"). Usage now lives as text/block-character progress
// bars in the right-click menu header and the hover tooltip instead (see
// main.rs's usage_bar_line()), where there's actually room to read a
// percentage.
//
// Windows tray icons are a plain bitmap with no GNOME-style "symbolic,
// theme recolors it" convention, so the mascot's color is fixed here
// instead (Claude's brand orange) rather than inherited from a taskbar
// theme.

use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform,
};
use tray_icon::Icon;

const CANVAS: u32 = 64;
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

fn mascot_path() -> Path {
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
    // The source SVG (spec-fedora) cuts a triangular notch out of each side
    // of the ears band here for a pointier "ear" silhouette. Dropped for
    // the tray bitmap: at ~16px that transparent wedge (right next to the
    // eyes) doesn't read as an ear, it reads as a hole torn in the face.
    // The wider ears band alone still gives a slight flared-shoulder step.
    pb.finish().expect("mascot path is well-formed")
}

/// Eye vertical center, sized/positioned by eye against the actual mascot
/// reference art (Downloads/HBSjoHSaEAA44Dk.jpg -- the "Welcome, Claw'd"
/// image): big bold squares taking up a real chunk of the head band
/// (10..28), not small dots.
const EYE_CY: f32 = 20.0;
const EYE_HALF: f32 = 6.5;

/// Open eyes: two big solid black squares, filled *on top of* the mascot
/// silhouette rather than cut out of it -- a transparent cutout let the
/// (dark) taskbar bleed through with an antialiasing halo that read as
/// pale/white at tray size instead of a clean black square.
fn eyes_open_path() -> Path {
    let mut pb = PathBuilder::new();
    for cx in [30.0, 70.0] {
        add_rect(
            &mut pb,
            cx - EYE_HALF,
            EYE_CY - EYE_HALF,
            cx + EYE_HALF,
            EYE_CY + EYE_HALF,
        );
    }
    pb.finish().expect("eyes path is well-formed")
}

/// Closed eyes: a content/squinting "><" -- left eye a ">" chevron
/// (pointing right, toward the nose), right eye a "<" (pointing left) --
/// stroked rather than filled, matching the reference sticker
/// (Downloads/st,small,507x507-pad,600x600,f8f8f8.jpg). Same bounding box
/// as the open-eye squares so the blink doesn't change the apparent eye
/// size, just the shape.
fn eyes_closed_path() -> Path {
    let mut pb = PathBuilder::new();
    // Left eye: ">"
    pb.move_to(30.0 - EYE_HALF, EYE_CY - EYE_HALF);
    pb.line_to(30.0 + EYE_HALF, EYE_CY);
    pb.line_to(30.0 - EYE_HALF, EYE_CY + EYE_HALF);
    // Right eye: "<"
    pb.move_to(70.0 + EYE_HALF, EYE_CY - EYE_HALF);
    pb.line_to(70.0 - EYE_HALF, EYE_CY);
    pb.line_to(70.0 + EYE_HALF, EYE_CY + EYE_HALF);
    pb.finish().expect("eyes path is well-formed")
}

fn solid_paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    paint
}

/// Claude's brand orange — the mascot is always this color now; there's no
/// more pending-request tint (the toast notification covers that signal).
fn mascot_color() -> Color {
    Color::from_rgba8(0xd9, 0x77, 0x57, 0xff)
}

fn eye_color() -> Color {
    Color::BLACK
}

pub fn render_icon(eyes_open: bool) -> Icon {
    let mut pixmap = Pixmap::new(CANVAS, CANVAS).expect("nonzero canvas size");
    pixmap.fill_path(
        &mascot_path(),
        &solid_paint(mascot_color()),
        FillRule::EvenOdd,
        Transform::from_scale(SCALE, SCALE),
        None,
    );
    if eyes_open {
        pixmap.fill_path(
            &eyes_open_path(),
            &solid_paint(eye_color()),
            FillRule::EvenOdd,
            Transform::from_scale(SCALE, SCALE),
            None,
        );
    } else {
        let stroke = Stroke {
            width: 5.5,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        pixmap.stroke_path(
            &eyes_closed_path(),
            &solid_paint(eye_color()),
            &stroke,
            Transform::from_scale(SCALE, SCALE),
            None,
        );
    }

    Icon::from_rgba(pixmap.data().to_vec(), CANVAS, CANVAS).expect("valid icon buffer")
}
