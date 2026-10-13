//! Forked Settings window: a normal, WM-managed winit window with the pet's
//! own font renderer. The pet itself is override-redirect (unmanaged), so the
//! WM never gives it keyboard focus and in-app text entry is unreliable —
//! this child window is managed, gets focus normally, and implements full
//! text editing (caret, arrows, Home/End, Delete, Backspace).
//!
//! Visual design: a Win95/Bonzi-era property sheet drawn with the reusable
//! [`crate::retro`] theme (button-face gray, 2px 3D bevels, chunky
//! checkboxes/radios, tab strip, OK/Cancel default-button row, sunken
//! status bar). The tab taxonomy mirrors the original BonziBuddy options
//! dialog recovered from the 2004 engine string table
//! (`analysis/extracted2/6`: General / Personality / Voice / Privacy /
//! Registration / E-Mail Account). All functional settings of this fork
//! live on **General**; the original tabs that were network- or
//! phone-home-shaped (Privacy telemetry opt-outs, Registration upload,
//! E-Mail account — see `analysis/BONZI432_RE.md` §3) are rendered as
//! visibly **disabled** period-plausible controls marked "(retired)" and
//! are NOT functional here: this build has a zero-network invariant, so
//! those controls must never come back to life. Personality/Voice controls
//! are likewise shown disabled — the fork has no personality/voice
//! settings to back them, and inventing fake ones would be worse than an
//! honest retired tab.
//!
//! The window is WM-managed, so the title bar is drawn by the window
//! manager; retro title-bar styling (retro::TITLE_A/TITLE_B gradient) is
//! only usable by repos that draw their own chrome.
//!
//! Run as: `bonzibuddy-rs --settings-ui`. On Save it writes the config file
//! and exits; the pet notices via mtime and reloads.

use crate::assets;
use crate::Config;
use crate::render::{self, Canvas};
use crate::retro;
use ab_glyph::FontArc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Event, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes};

const W: u32 = 460;
const H: u32 = 396;
const ASK_H: u32 = 220;
const FIELDS: [&str; 3] = ["API endpoint (blank = offline)", "API key", "Model"];
/// Original options-dialog tabs, recovered from the 2004 engine strings.
const TABS: [&str; 6] = ["General", "Personality", "Voice", "Privacy", "Registration", "E-Mail Account"];

type Rect = (i32, i32, i32, i32);

fn in_rect(r: &Rect, x: f64, y: f64) -> bool {
    x >= r.0 as f64 && x < (r.0 + r.2) as f64 && y >= r.1 as f64 && y < (r.1 + r.3) as f64
}

struct Ui {
    assets: assets::Assets,
    cfg: Config,
    fields: [String; 3],
    sel: usize,
    caret: usize,
    window: Option<Arc<Window>>,
    pixels: Option<pixels::Pixels<'static>>,
    size: (u32, u32),
    rects: Vec<Rect>,
    tab_rects: Vec<Rect>,
    ok_rect: Rect,
    cancel_rect: Rect,
    tab: usize,
    quit: bool,
    status: String,
    mouse_pos: (f64, f64),
    /// Ask mode: one question field + Ask button (writes the ask file).
    ask: bool,
}

impl Ui {

    fn field_text(&self, i: usize) -> String {
        let s = &self.fields[i];
        if !self.ask && i == 1 && !s.is_empty() {
            // Never echo the key back on screen.
            format!("•••••••• ({} chars)", s.chars().count())
        } else if s.is_empty() {
            "…".into()
        } else {
            s.clone()
        }
    }

    fn insert(&mut self, c: char) {
        let n = self.fields[self.sel].chars().count();
        if n < 300 {
            let mut ch: Vec<char> = self.fields[self.sel].chars().collect();
            ch.insert(self.caret.min(ch.len()), c);
            self.fields[self.sel] = ch.into_iter().collect();
            self.caret += 1;
        }
    }

    fn backspace(&mut self) {
        if self.caret > 0 {
            let mut ch: Vec<char> = self.fields[self.sel].chars().collect();
            ch.remove(self.caret - 1);
            self.fields[self.sel] = ch.into_iter().collect();
            self.caret -= 1;
        }
    }

    fn save(&mut self) {
        if self.ask {
            // Hand the question to the pet via the ask file (mtime-polled).
            let q = self.fields[0].trim().to_string();
            let path = crate::ask_path();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&path, &q);
            self.status = if q.is_empty() {
                "Nothing asked.".into()
            } else {
                "Question sent to Bonzi!".into()
            };
            return;
        }
        let mut cfg = crate::load_config();
        cfg.agent_endpoint = self.fields[0].trim().to_string();
        cfg.agent_key = self.fields[1].trim().to_string();
        cfg.agent_model = self.fields[2].trim().to_string();
        crate::save_config(&cfg);
        self.cfg = cfg;
        self.status = "Saved. Bonzi reloads this automatically.".to_string();
    }

    fn click(&mut self, x: f64, y: f64) {
        if !self.ask {
            for (i, r) in self.tab_rects.iter().enumerate() {
                if in_rect(r, x, y) {
                    self.tab = i;
                    return;
                }
            }
        }
        if self.tab == 0 {
            for (i, r) in self.rects.iter().enumerate() {
                if in_rect(r, x, y) {
                    self.sel = i;
                    self.caret = self.fields[i].chars().count();
                    return;
                }
            }
        }
        if in_rect(&self.ok_rect, x, y) {
            self.save();
            self.quit = true;
            return;
        }
        if in_rect(&self.cancel_rect, x, y) {
            self.quit = true;
        }
    }

    fn build_pixels(
        w: u32,
        h: u32,
        window: &Arc<Window>,
    ) -> Option<pixels::Pixels<'static>> {
        let st = pixels::SurfaceTexture::new(w, h, window.clone());
        pixels::PixelsBuilder::new(w, h, st)
            .clear_color(pixels::wgpu::Color { r: 0.75, g: 0.75, b: 0.75, a: 1.0 })
            .build()
            .ok()
    }

    /// Draw one editable field row (label + sunken edit + caret) and
    /// record its hit rect.
    fn edit_row(
        &mut self,
        canvas: &mut Canvas,
        font: &FontArc,
        t: &retro::Theme,
        label_txt: &str,
        i: usize,
        x: i32,
        y: f32,
        w: i32,
    ) -> f32 {
        let s = t.s;
        let scale = t.scale();
        let lh = render::line_height(font, scale);
        retro::label(canvas, font, t, label_txt, x as f32, y, false);
        let fy = (y + lh * 0.35) as i32;
        let fh = (24.0 * s) as i32;
        let selected = i == self.sel;
        retro::field(canvas, x, fy, w, fh, false);
        let shown = self.field_text(i);
        let mut clip = String::new();
        let mut cw = 0.0;
        for c in shown.chars() {
            let cw2 = cw + render::text_width(font, c.encode_utf8(&mut [0u8; 4]), scale);
            if cw2 > w as f32 - 14.0 * s {
                break;
            }
            cw = cw2;
            clip.push(c);
        }
        retro::text(canvas, font, t, &clip, x as f32 + 5.0 * s, fy as f32 + (fh as f32 + lh * 0.72) / 2.0, retro::TEXT);
        if selected {
            retro::focus_rect(canvas, x, fy, w, fh);
            if self.window.as_ref().is_some_and(|w| w.has_focus()) && self.frame_blink() {
                let caret_shown = if !self.ask && i == 1 {
                    "•".repeat(self.caret.min(8))
                } else {
                    self.fields[i].chars().take(self.caret).collect()
                };
                let cx = x as f32 + 5.0 * s
                    + render::text_width(font, &caret_shown, scale).min(w as f32 - 10.0 * s);
                canvas.fill(cx as i32, fy + 3, (1.5 * s).max(1.0) as i32, fh - 6, retro::DARK);
            }
        }
        self.rects.push((x, fy, w, fh));
        y + lh * 0.35 + fh as f32 + 9.0 * s
    }

    fn render(&mut self) {
        let Some(window) = &self.window else { return };
        let (w, h) = self.size;
        let mut canvas = render::Canvas::new(w, h);
        let (w, h) = (w as i32, h as i32);
        let s = window.scale_factor() as f32;
        let t = retro::apply_retro_theme(&mut canvas, s);
        let font = self.assets.font.clone();
        let font = &font;
        let scale = t.scale();
        let lh = render::line_height(font, scale);
        let m = (8.0 * s) as i32;
        self.rects.clear();

        // Bottom chrome: sunken status bar + OK/Cancel row (both modes).
        let sb_h = (20.0 * s) as i32;
        let sb_y = h as i32 - sb_h - m;
        let bh = (24.0 * s) as i32;
        let bw = (85.0 * s) as i32;
        let by = sb_y - bh - (8.0 * s) as i32;
        let cx = (w as i32 - (2 * bw + (10.0 * s) as i32)) / 2;
        self.ok_rect = retro::button(&mut canvas, font, &t, cx, by, bw, bh,
            if self.ask { "Ask" } else { "OK" }, true, false, false);
        self.cancel_rect = retro::button(&mut canvas, font, &t,
            cx + bw + (10.0 * s) as i32, by, bw, bh, "Cancel", false, false, false);

        if self.ask {
            // Ask mode: single group box with the question field.
            let gy = (10.0 * s) as i32;
            retro::group(&mut canvas, font, &t, m, gy + (t.font * 0.5) as i32,
                w - 2 * m, by - gy - (16.0 * s) as i32, "Ask Bonzi", false);
            let fx = m + (12.0 * s) as i32;
            let fy = gy + (t.font * 2.4) as i32;
            self.edit_row(&mut canvas, font, &t, "Your question", 0, fx, fy as f32, w - 2 * m - (24.0 * s) as i32);
        } else {
            // Property sheet: tab strip + panel + per-tab content.
            let ty = (6.0 * s) as i32;
            self.tab_rects = retro::tab_strip(&mut canvas, font, &t, m, ty,
                w - 2 * m, by - (8.0 * s) as i32, &TABS, self.tab);
            let th = (24.0 * s) as i32;
            let px = m + (12.0 * s) as i32;
            let pw = w - 2 * m - (24.0 * s) as i32;
            let py = ty + th + (14.0 * s) as i32;
            match self.tab {
                0 => {
                    // General: all functional settings of this fork.
                    let gh = (3.0 * (24.0 * s + lh * 0.35 + 9.0 * s) + 26.0 * s) as i32;
                    retro::group(&mut canvas, font, &t, m + (6.0 * s) as i32,
                        py - (4.0 * s) as i32, w - 2 * m - (12.0 * s) as i32, gh,
                        "BonziBuddy agent (Ask Bonzi)", false);
                    let mut y = py as f32 + t.font * 1.2;
                    for i in 0..3 {
                        y = self.edit_row(&mut canvas, font, &t, FIELDS[i], i, px, y, pw);
                    }
                    // Original General tab had Sleep Mode (RelaxWhenIdle
                    // registry keys, RE §2); no fork equivalent → retired.
                    let sy = py + (t.font * 0.6) as i32 + gh + (14.0 * s) as i32;
                    retro::group(&mut canvas, font, &t, m + (6.0 * s) as i32, sy,
                        w - 2 * m - (12.0 * s) as i32, (52.0 * s) as i32, "Sleep Mode", true);
                    retro::checkbox(&mut canvas, font, &t, px, sy + (20.0 * s) as i32,
                        "Relax when idle (retired)", false, true);
                }
                1 => {
                    self.retired_note(&mut canvas, font, &t, px, py,
                        "Personality options are retired in this build.");
                    retro::group(&mut canvas, font, &t, m + (6.0 * s) as i32,
                        py + (26.0 * s) as i32, w - 2 * m - (12.0 * s) as i32,
                        (150.0 * s) as i32, "About you", true);
                    let mut y = py as f32 + 26.0 * s + t.font * 1.6;
                    self.disabled_row(&mut canvas, font, &t, "Name:", px, &mut y, pw, "");
                    self.disabled_row(&mut canvas, font, &t, "Age:", px, &mut y, pw, "");
                    retro::label(&mut canvas, font, &t, "Gender:", px as f32, y, true);
                    retro::radio(&mut canvas, font, &t, px + (60.0 * s) as i32, (y - t.font) as i32, "Male", false, true);
                    retro::radio(&mut canvas, font, &t, px + (130.0 * s) as i32, (y - t.font) as i32, "Female", false, true);
                    y += 26.0 * s;
                    self.disabled_combo_row(&mut canvas, font, &t, "Country:", px, &mut y, pw, "");
                }
                2 => {
                    self.retired_note(&mut canvas, font, &t, px, py,
                        "Voice options are retired in this build.");
                    retro::group(&mut canvas, font, &t, m + (6.0 * s) as i32,
                        py + (26.0 * s) as i32, w - 2 * m - (12.0 * s) as i32,
                        (150.0 * s) as i32, "Voice", true);
                    let mut y = py as f32 + 26.0 * s + t.font * 1.6;
                    self.disabled_combo_row(&mut canvas, font, &t, "Voice:", px, &mut y, pw, "");
                    self.disabled_combo_row(&mut canvas, font, &t, "Speed:", px, &mut y, pw, "");
                    retro::checkbox(&mut canvas, font, &t, px, y as i32,
                        "Characters Listen for Input (retired)", false, true);
                    retro::button(&mut canvas, font, &t, px, (y + 26.0 * s) as i32,
                        (140.0 * s) as i32, (24.0 * s) as i32, "Test Voice (retired)", false, false, true);
                }
                3 => {
                    // The original Privacy tab drove the phone-home opt-outs
                    // (Browse.nbd / Search.nbd logs, daily/product fetches —
                    // RE §3). Zero-network invariant: rendered disabled,
                    // never functional.
                    self.retired_note(&mut canvas, font, &t, px, py,
                        "Privacy options are retired: this build never connects to the network.");
                    retro::group(&mut canvas, font, &t, m + (6.0 * s) as i32,
                        py + (26.0 * s) as i32, w - 2 * m - (12.0 * s) as i32,
                        (110.0 * s) as i32, "Privacy", true);
                    retro::checkbox(&mut canvas, font, &t, px, (py as f32 + 46.0 * s) as i32,
                        "Collect browsing history (retired)", false, true);
                    retro::checkbox(&mut canvas, font, &t, px, (py as f32 + 74.0 * s) as i32,
                        "Send anonymous usage information (retired)", false, true);
                }
                4 => {
                    self.retired_note(&mut canvas, font, &t, px, py,
                        "Registration is retired: there is nothing to register with.");
                    retro::group(&mut canvas, font, &t, m + (6.0 * s) as i32,
                        py + (26.0 * s) as i32, w - 2 * m - (12.0 * s) as i32,
                        (120.0 * s) as i32, "Registration", true);
                    let mut y = py as f32 + 26.0 * s + t.font * 1.6;
                    self.disabled_row(&mut canvas, font, &t, "Registration number:", px, &mut y, pw, "");
                    retro::button(&mut canvas, font, &t, px, y as i32,
                        (160.0 * s) as i32, (24.0 * s) as i32, "Register Now... (retired)", false, false, true);
                }
                _ => {
                    self.retired_note(&mut canvas, font, &t, px, py,
                        "E-Mail is retired: this build never connects to the network.");
                    retro::group(&mut canvas, font, &t, m + (6.0 * s) as i32,
                        py + (26.0 * s) as i32, w - 2 * m - (12.0 * s) as i32,
                        (150.0 * s) as i32, "Mail account", true);
                    let mut y = py as f32 + 26.0 * s + t.font * 1.6;
                    self.disabled_row(&mut canvas, font, &t, "Mail Server:", px, &mut y, pw, "");
                    self.disabled_row(&mut canvas, font, &t, "User Name:", px, &mut y, pw, "");
                    self.disabled_row(&mut canvas, font, &t, "Password:", px, &mut y, pw, "");
                }
            }
        }
        retro::statusbar(&mut canvas, font, &t, m, sb_y, w - 2 * m, sb_h, &self.status);
        let _ = (scale, lh);

        let Some(px) = &mut self.pixels else { return };
        let frame = px.frame_mut();
        for (dst, src) in frame.chunks_exact_mut(4).zip(canvas.px.chunks_exact(4)) {
            let a = src[3] as u16;
            dst[0] = (src[2] as u16 * a / 255) as u8;
            dst[1] = (src[1] as u16 * a / 255) as u8;
            dst[2] = (src[0] as u16 * a / 255) as u8;
            dst[3] = a as u8;
        }
        let _ = px.render();
    }

    fn retired_note(&self, canvas: &mut Canvas, font: &FontArc, t: &retro::Theme, x: i32, y: i32, txt: &str) {
        retro::text(canvas, font, t, txt, x as f32, y as f32 + t.font, retro::TITLE_A);
    }

    /// Disabled label + sunken gray field row (retired-tab filler).
    fn disabled_row(&self, canvas: &mut Canvas, font: &FontArc, t: &retro::Theme,
        label_txt: &str, x: i32, y: &mut f32, w: i32, val: &str) {
        retro::label(canvas, font, t, label_txt, x as f32, *y, true);
        let fy = (*y + t.font * 0.5) as i32;
        let fh = (24.0 * t.s) as i32;
        retro::field(canvas, x, fy, w, fh, true);
        if !val.is_empty() {
            retro::label(canvas, font, t, val, x as f32 + 5.0 * t.s, fy as f32 + fh as f32 * 0.7, true);
        }
        *y = fy as f32 + fh as f32 + 9.0 * t.s;
    }

    fn disabled_combo_row(&self, canvas: &mut Canvas, font: &FontArc, t: &retro::Theme,
        label_txt: &str, x: i32, y: &mut f32, w: i32, _val: &str) {
        retro::label(canvas, font, t, label_txt, x as f32, *y, true);
        let fy = (*y + t.font * 0.5) as i32;
        let fh = (24.0 * t.s) as i32;
        retro::combo(canvas, t, x, fy, w, fh, true);
        *y = fy as f32 + fh as f32 + 9.0 * t.s;
    }

    fn frame_blink(&self) -> bool {
        (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
            % 1000) < 550
    }
}

pub fn run(ask: bool) -> Result<(), Box<dyn std::error::Error>> {
    let el = EventLoop::new()?;
    let mut ui = Ui {
        assets: match assets::Assets::load() {
            Ok(a) => a,
            Err(e) => return Err(e.into()),
        },
        cfg: crate::load_config(),
        fields: {
            let c = crate::load_config();
            [c.agent_endpoint, c.agent_key, c.agent_model]
        },
        sel: 0,
        caret: 0,
        window: None,
        pixels: None,
        size: (W, if ask { ASK_H } else { H }),
        rects: Vec::new(),
        tab_rects: Vec::new(),
        ok_rect: (0, 0, 0, 0),
        cancel_rect: (0, 0, 0, 0),
        tab: 0,
        quit: false,
        status: String::new(),
        mouse_pos: (0.0, 0.0),
        ask,
    };
    if ask {
        ui.fields = [String::new(), String::new(), String::new()];
    }
    ui.caret = ui.fields[0].chars().count();

    #[allow(deprecated)] // run_app needs an AppHandler impl; closure form is fine here
    el.run(move |event, el| match event {
        Event::NewEvents(winit::event::StartCause::Init) => {
            let Ok(window) = el.create_window(
                WindowAttributes::default()
                    .with_title(if ui.ask { "Ask Bonzi" } else { "BonziBuddy settings" })
                    .with_inner_size(LogicalSize::new(W, if ui.ask { ASK_H } else { H }))
                    .with_resizable(false),
            ) else {
                el.exit();
                return;
            };
            if let Ok(ico) = image::open(ui.assets.root().join("favicon.ico")) {
                let img = ico.to_rgba8();
                let (iw, ih) = img.dimensions();
                let _ = window.set_window_icon(winit::window::Icon::from_rgba(img.into_raw(), iw, ih).ok());
            }
            let window = Arc::new(window);
            // The surface MUST be sized to the window's *physical* inner
            // size: on a scaled display (KDE fractional scaling, HiDPI)
            // LogicalSize(460,300) maps to a larger physical buffer, and a
            // 460x300 swapchain on a bigger surface presents as a black box.
            let (pw, ph): (u32, u32) = {
                let s = window.inner_size();
                (s.width.max(1), s.height.max(1))
            };
            ui.size = (pw, ph);
            match Ui::build_pixels(pw, ph, &window) {
                Some(p) => {
                    ui.pixels = Some(p);
                    ui.window = Some(window);
                }
                None => {
                    eprintln!("settings: renderer failed");
                    el.exit();
                }
            }
        }
        Event::WindowEvent { event, .. } => match event {
            WindowEvent::Resized(sz) => {
                if sz.width > 0 && sz.height > 0 && (sz.width, sz.height) != ui.size {
                    ui.size = (sz.width, sz.height);
                    if let Some(w) = ui.window.clone() {
                        // Rare (window is non-resizable), but a scale-factor
                        // change lands here: rebuild the swapchain at the
                        // new physical size instead of presenting garbage.
                        if let Some(p) = Ui::build_pixels(sz.width, sz.height, &w) {
                            ui.pixels = Some(p);
                        }
                    }
                }
            }
            WindowEvent::Focused(_) | WindowEvent::RedrawRequested => {
                ui.render();
                el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(250)));
            }
            WindowEvent::CursorMoved { position, .. } => {
                ui.mouse_pos = (position.x, position.y);
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                let (x, y) = ui.mouse_pos;
                ui.click(x, y);
                if ui.quit {
                    el.exit();
                } else {
                    ui.window.as_ref().map(|w| w.request_redraw());
                }
            }
            WindowEvent::KeyboardInput { event: KeyEvent { state: ElementState::Pressed, logical_key, text, .. }, .. } => {
                match &logical_key {
                    Key::Named(NamedKey::Escape) => el.exit(),
                    Key::Named(NamedKey::Enter) => {
                        ui.save();
                        el.exit();
                    }
                    Key::Named(NamedKey::Tab) if ui.ask || ui.tab == 0 => {
                        ui.sel = (ui.sel + 1) % if ui.ask { 1 } else { 3 };
                        ui.caret = ui.fields[ui.sel].chars().count();
                    }
                    Key::Named(NamedKey::Backspace) => ui.backspace(),
                    Key::Named(NamedKey::Delete) => {
                        let mut ch: Vec<char> = ui.fields[ui.sel].chars().collect();
                        if ui.caret < ch.len() {
                            ch.remove(ui.caret);
                            ui.fields[ui.sel] = ch.into_iter().collect();
                        }
                    }
                    Key::Named(NamedKey::ArrowLeft) => ui.caret = ui.caret.saturating_sub(1),
                    Key::Named(NamedKey::ArrowRight) => {
                        ui.caret = (ui.caret + 1).min(ui.fields[ui.sel].chars().count())
                    }
                    Key::Named(NamedKey::Home) => ui.caret = 0,
                    Key::Named(NamedKey::End) => ui.caret = ui.fields[ui.sel].chars().count(),
                    _ => {
                        if let Some(c) = text
                            .and_then(|t| t.chars().next())
                            .filter(|c| c.is_ascii_graphic() || *c == ' ')
                        {
                            ui.insert(c);
                        }
                    }
                }
                ui.window.as_ref().map(|w| w.request_redraw());
            }
            WindowEvent::CloseRequested => el.exit(),
            _ => {}
        }
        Event::AboutToWait => {
            el.set_control_flow(ControlFlow::Wait);
        }
        _ => {}
    })?;
    Ok(())
}
