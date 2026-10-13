//! Reusable Win95/BonziBuddy-era visual theme for the hand-drawn settings
//! window.
//!
//! NOTE on the `apply_retro_theme(&egui::Context)` shape: this repo has no
//! egui — the settings window is a hand-drawn winit + pixels UI on
//! [`crate::render::Canvas`]. `apply_retro_theme` here is the reusable entry
//! point: it paints the window base (button-face gray) and hands back the
//! [`Theme`] (display scale + MS-Sans-Serif-style font size) that every
//! widget primitive below consumes. A sibling repo that *does* use egui can
//! port this module by mapping the same palette/bevel constants onto
//! `egui::Visuals` (panel/window fill = FACE, extreme_bg = WHITE for
//! fields, 2px raised/sunken bevels via the widget strokes, 11px
//! "MS Sans Serif" font).
//!
//! The palette and bevel geometry follow the classic Win95 `DrawEdge`
//! styles: raised (button face) and sunken (client field) 2px bevels,
//! etched group boxes, chunky 13px checkboxes/radios, a property-sheet tab
//! strip and a sunken status bar. The original BonziBuddy options dialog
//! was a VB6 property sheet on a Win95/98 desktop — tab captions General /
//! Personality / Voice / Privacy / Registration / E-Mail Account were
//! recovered from the 2004 engine string table (`analysis/extracted2/6`,
//! see `analysis/BONZI432_RE.md`) — so this flat-gray bevel theme is
//! period-correct for it.
//!
//! Scrollbars are deliberately absent: the settings sheet fits without
//! scrolling, so there is nothing to scroll. The window title bar is drawn
//! by the window manager (the winit window is WM-managed), so title-bar
//! styling is out of our hands here; TITLE_A/TITLE_B are kept for sibling
//! ports that draw their own chrome.

use crate::render;
use crate::render::Canvas;
use ab_glyph::{FontArc, PxScale};

pub type Rgb = [u8; 4];

/// Classic Win95 system colors (COLOR_BTNFACE family).
pub const FACE: Rgb = [0xC0, 0xC0, 0xC0, 255];
pub const HILIGHT: Rgb = [0xFF, 0xFF, 0xFF, 255];
pub const LIGHT: Rgb = [0xDF, 0xDF, 0xDF, 255];
pub const SHADOW: Rgb = [0x80, 0x80, 0x80, 255];
pub const DARK: Rgb = [0x00, 0x00, 0x00, 255];
pub const WHITE: Rgb = [0xFF, 0xFF, 0xFF, 255];
/// COLOR_WINDOW: edit-field background.
pub const WINDOW: Rgb = [0xFF, 0xFF, 0xFF, 255];
pub const TEXT: Rgb = [0x00, 0x00, 0x00, 255];
/// Disabled label: gray glyph with a white emboss one pixel down-right.
pub const DISABLED: Rgb = [0x80, 0x80, 0x80, 255];
/// Active title-bar gradient (for sibling ports; our WM draws the bar).
pub const TITLE_A: Rgb = [0x00, 0x00, 0x80, 255];
#[allow(dead_code)] // gradient end: only consumed by ports that draw their own title bar
pub const TITLE_B: Rgb = [0x10, 0x84, 0xD0, 255];

/// Per-window theme state: display scale + MS-Sans-Serif-style font size
/// (8 pt ≈ 11 px at 96 dpi; Liberation Sans is the metric stand-in).
pub struct Theme {
    pub s: f32,
    pub font: f32,
}

impl Theme {
    pub fn scale(&self) -> PxScale {
        let px = self.font.max(10.0);
        PxScale { x: px, y: px }
    }
    /// Chunky control metric (checkbox/radio box size).
    pub fn box_px(&self) -> i32 {
        (13.0 * self.s).round() as i32
    }
}

/// Paint the window base in button-face gray and return the theme to draw
/// the widgets with. The reusable "install the retro look" entry point.
pub fn apply_retro_theme(canvas: &mut Canvas, s: f32) -> Theme {
    canvas.clear(FACE);
    Theme { s, font: 12.0 * s }
}

// ---------------------------------------------------------------------------
// low-level geometry
// ---------------------------------------------------------------------------

fn fill(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32, c: Rgb) {
    canvas.fill(x, y, w, h, c);
}

fn edge(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32, tl: Rgb, br: Rgb) {
    for xx in x..x + w {
        canvas.blend(xx, y, tl);
        canvas.blend(xx, y + h - 1, br);
    }
    for yy in y..y + h {
        canvas.blend(x, yy, tl);
        canvas.blend(x + w - 1, yy, br);
    }
}

/// Raised 2px bevel (button face): white/light top-left, black/shadow
/// bottom-right, face fill.
pub fn raised(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32) {
    fill(canvas, x, y, w, h, FACE);
    edge(canvas, x, y, w, h, HILIGHT, DARK);
    edge(canvas, x + 1, y + 1, w - 2, h - 2, LIGHT, SHADOW);
}

/// Sunken 2px bevel (client edge): shadow/black top-left, white/light
/// bottom-right.
pub fn sunken(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32, bg: Rgb) {
    fill(canvas, x, y, w, h, bg);
    edge(canvas, x, y, w, h, SHADOW, WHITE);
    edge(canvas, x + 1, y + 1, w - 2, h - 2, DARK, LIGHT);
}

// ---------------------------------------------------------------------------
// text
// ---------------------------------------------------------------------------

pub fn text(canvas: &mut Canvas, font: &FontArc, t: &Theme, txt: &str, x: f32, y: f32, color: Rgb) {
    render::draw_line(canvas, font, txt, x, y, t.scale(), color);
}

pub fn text_w(font: &FontArc, t: &Theme, txt: &str) -> f32 {
    render::text_width(font, txt, t.scale())
}

/// Draw text; when `disabled`, use the Win95 embossed-gray treatment.
pub fn label(canvas: &mut Canvas, font: &FontArc, t: &Theme, txt: &str, x: f32, y: f32, disabled: bool) {
    if disabled {
        text(canvas, font, t, txt, x + t.s, y + t.s, WHITE);
        text(canvas, font, t, txt, x, y, DISABLED);
    } else {
        text(canvas, font, t, txt, x, y, TEXT);
    }
}

// ---------------------------------------------------------------------------
// widgets
// ---------------------------------------------------------------------------

/// Push button. `default` adds the 1px black default-frame; `pressed`
/// inverts the bevel and shifts the caption like a held Win95 button.
/// Returns the button rect (the face, not the default frame).
pub fn button(
    canvas: &mut Canvas,
    font: &FontArc,
    t: &Theme,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    caption: &str,
    default: bool,
    pressed: bool,
    disabled: bool,
) -> (i32, i32, i32, i32) {
    if default && !pressed {
        // Black frame one pixel outside the button, face gap between.
        edge(canvas, x - 2, y - 2, w + 4, h + 4, DARK, DARK);
        fill(canvas, x - 1, y - 1, (w + 2).max(0), (h + 2).max(0), FACE);
    }
    if pressed {
        sunken(canvas, x, y, w, h, FACE);
    } else {
        raised(canvas, x, y, w, h);
    }
    let off = if pressed { t.s } else { 0.0 };
    let tw = text_w(font, t, caption);
    let lh = render::line_height(font, t.scale());
    label(
        canvas,
        font,
        t,
        caption,
        x as f32 + (w as f32 - tw) / 2.0 + off,
        y as f32 + (h as f32 - lh) / 2.0 + lh + off,
        disabled,
    );
    (x, y, w, h)
}

/// Etched group box with a caption punched out of the top line.
pub fn group(
    canvas: &mut Canvas,
    font: &FontArc,
    t: &Theme,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    caption: &str,
    disabled: bool,
) {
    edge(canvas, x, y, w, h, SHADOW, WHITE);
    edge(canvas, x + 1, y + 1, w - 2, h - 2, WHITE, SHADOW);
    if !caption.is_empty() {
        let tw = text_w(font, t, caption);
        fill(canvas, x + 6, y - (t.font * 0.45) as i32, (tw + 6.0) as i32, (t.font * 1.1) as i32, FACE);
        label(canvas, font, t, caption, x as f32 + 9.0, y as f32 + t.font * 0.35, disabled);
    }
}

/// Sunken edit field (COLOR_WINDOW background, or face gray when disabled).
pub fn field(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32, disabled: bool) {
    sunken(canvas, x, y, w, h, if disabled { FACE } else { WINDOW });
}

/// Dotted focus rectangle 2px inside a rect (Win95 keyboard focus).
pub fn focus_rect(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32) {
    let (x, y, w, h) = (x + 2, y + 2, w - 4, h - 4);
    for xx in x..x + w {
        if (xx - x) % 2 == 0 {
            canvas.blend(xx, y, DARK);
            canvas.blend(xx, y + h - 1, DARK);
        }
    }
    for yy in y..y + h {
        if (yy - y) % 2 == 0 {
            canvas.blend(x, yy, DARK);
            canvas.blend(x + w - 1, yy, DARK);
        }
    }
}

/// Chunky 13px checkbox with a black check mark.
pub fn checkbox(
    canvas: &mut Canvas,
    font: &FontArc,
    t: &Theme,
    x: i32,
    y: i32,
    caption: &str,
    checked: bool,
    disabled: bool,
) {
    let b = t.box_px();
    sunken(canvas, x, y, b, b, if disabled { FACE } else { WINDOW });
    if checked {
        let u = b as f32 / 13.0;
        let pts: [(f32, f32); 3] = [(3.0, 6.5), (5.5, 9.5), (10.0, 3.0)];
        for seg in pts.windows(2) {
            let (x0, y0) = (x as f32 + seg[0].0 * u, y as f32 + seg[0].1 * u);
            let (x1, y1) = (x as f32 + seg[1].0 * u, y as f32 + seg[1].1 * u);
            let steps = ((x1 - x0).abs() + (y1 - y0).abs()).max(1.0);
            for i in 0..=steps as i32 {
                let f = i as f32 / steps;
                let (px, py) = (x0 + (x1 - x0) * f, y0 + (y1 - y0) * f);
                let c = if disabled { DISABLED } else { DARK };
                canvas.blend(px as i32, py as i32, c);
                canvas.blend(px as i32 + 1, py as i32, c);
            }
        }
    }
    label(canvas, font, t, caption, (x + b + 5) as f32, y as f32 + t.font * 0.95, disabled);
}

/// Chunky radio button (sunken circle + dot).
pub fn radio(
    canvas: &mut Canvas,
    font: &FontArc,
    t: &Theme,
    x: i32,
    y: i32,
    caption: &str,
    checked: bool,
    disabled: bool,
) {
    let d = t.box_px();
    let (cx, cy) = (x as f32 + d as f32 / 2.0, y as f32 + d as f32 / 2.0);
    let r = d as f32 / 2.0;
    for yy in y..y + d {
        for xx in x..x + d {
            let dx = xx as f32 + 0.5 - cx;
            let dy = yy as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let c = if dist > r {
                FACE
            } else if dist > r - 1.0 {
                SHADOW
            } else if dist > r - 2.0 {
                DARK
            } else if checked && dist < r * 0.38 {
                if disabled { DISABLED } else { DARK }
            } else if disabled {
                FACE
            } else {
                WINDOW
            };
            canvas.blend(xx, yy, c);
        }
    }
    label(canvas, font, t, caption, (x + d + 5) as f32, y as f32 + t.font * 0.95, disabled);
}

/// Disabled combo-box look: sunken field + arrow button with a ▼ glyph.
pub fn combo(canvas: &mut Canvas, t: &Theme, x: i32, y: i32, w: i32, h: i32, disabled: bool) {
    let bw = (20.0 * t.s).round() as i32;
    field(canvas, x, y, w - bw - 2, h, disabled);
    raised(canvas, x + w - bw, y, bw, h);
    // ▼ arrow, 2px-fat pixel triangle.
    let (ax, ay) = (x + w - bw / 2, y + h / 2 - 2);
    for i in 0..5i32 {
        let hw = 4 - i;
        for xx in (ax - hw)..=(ax + hw) {
            canvas.blend(xx, ay + i, if disabled { DISABLED } else { DARK });
            canvas.blend(xx + 1, ay + i, if disabled { DISABLED } else { DARK });
        }
    }
}

/// Property-sheet tab strip. Draws the raised panel frame first, then the
/// tabs; the selected tab overlaps (and erases) the panel's top border.
/// Returns each tab's hit rect.
pub fn tab_strip(
    canvas: &mut Canvas,
    font: &FontArc,
    t: &Theme,
    x: i32,
    y: i32,
    panel_w: i32,
    panel_bottom: i32,
    labels: &[&str],
    sel: usize,
) -> Vec<(i32, i32, i32, i32)> {
    // Panel: raised frame from just under the tabs to panel_bottom.
    let th = (24.0 * t.s).round() as i32;
    raised(canvas, x, y + th - 1, panel_w, panel_bottom - (y + th) + 1);
    let mut rects = Vec::new();
    let mut tx = x + (4.0 * t.s) as i32;
    let pad = (8.0 * t.s) as i32;
    // Unselected tabs first (they sit *behind* the panel top border).
    for (i, lab) in labels.iter().enumerate() {
        let tw = text_w(font, t, lab) as i32 + 2 * pad;
        let ty = if i == sel { y } else { y + (2.0 * t.s) as i32 };
        let hh = if i == sel { th + 2 } else { th };
        if i != sel {
            raised(canvas, tx, ty, tw, hh);
        }
        rects.push((tx, ty, tw, hh));
        tx += tw + (2.0 * t.s) as i32;
    }
    // Selected tab last: overlaps the panel border and erases its seam.
    let (sx, sy, sw, sh) = rects[sel];
    raised(canvas, sx, sy, sw, sh);
    fill(canvas, sx + 1, sy + sh - 2, sw - 2, 2, FACE);
    let lh = render::line_height(font, t.scale());
    for (i, lab) in labels.iter().enumerate() {
        let (rx, ry, rw, rh) = rects[i];
        let tw = text_w(font, t, lab);
        label(
            canvas,
            font,
            t,
            lab,
            rx as f32 + (rw as f32 - tw) / 2.0,
            ry as f32 + (rh as f32 - lh) / 2.0 + lh - t.s,
            false,
        );
    }
    rects
}

/// Sunken status bar strip with left-aligned text.
pub fn statusbar(
    canvas: &mut Canvas,
    font: &FontArc,
    t: &Theme,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    txt: &str,
) {
    sunken(canvas, x, y, w, h, FACE);
    let lh = render::line_height(font, t.scale());
    text(canvas, font, t, txt, (x + 6) as f32, y as f32 + (h as f32 - lh) / 2.0 + lh, TEXT);
}
