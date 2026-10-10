//! Forked Settings window: a normal, WM-managed winit window with the pet's
//! own font renderer. The pet itself is override-redirect (unmanaged), so the
//! WM never gives it keyboard focus and in-app text entry is unreliable —
//! this child window is managed, gets focus normally, and implements full
//! text editing (caret, arrows, Home/End, Delete, Backspace).
//!
//! Run as: `bonzibuddy-rs --settings-ui`. On Save it writes the config file
//! and exits; the pet notices via mtime and reloads.

use crate::assets;
use crate::render;
use crate::Config;
use ab_glyph::PxScale;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Event, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes};

const W: u32 = 460;
const H: u32 = 300;
const FIELDS: [&str; 3] = ["API endpoint (blank = offline)", "API key", "Model"];

struct Ui {
    assets: assets::Assets,
    cfg: Config,
    fields: [String; 3],
    sel: usize,
    caret: usize,
    window: Option<Arc<Window>>,
    pixels: Option<pixels::Pixels<'static>>,
    size: (u32, u32),
    rects: Vec<(i32, i32, i32, i32)>,
    save_rect: (i32, i32, i32, i32),
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
        for (i, (rx, ry, rw, rh)) in self.rects.iter().enumerate() {
            if x >= *rx as f64 && x < (rx + rw) as f64 && y >= *ry as f64 && y < (ry + rh) as f64 {
                self.sel = i;
                self.caret = self.fields[i].chars().count();
                return;
            }
        }
        let (rx, ry, rw, rh) = self.save_rect;
        if x >= rx as f64 && x < (rx + rw) as f64 && y >= ry as f64 && y < (ry + rh) as f64 {
            self.save();
        }
    }

    fn build_pixels(
        w: u32,
        h: u32,
        window: &Arc<Window>,
    ) -> Option<pixels::Pixels<'static>> {
        let st = pixels::SurfaceTexture::new(w, h, window.clone());
        pixels::PixelsBuilder::new(w, h, st)
            .clear_color(pixels::wgpu::Color { r: 0.96, g: 0.96, b: 0.93, a: 1.0 })
            .build()
            .ok()
    }

    fn render(&mut self) {
        let Some(window) = &self.window else { return };
        let (w, h) = self.size;
        let mut canvas = render::Canvas::new(w, h);
        canvas.clear([0xF4, 0xF4, 0xEC, 255]);
        let sf = window.scale_factor() as f32;
        let s = sf;
        let scale = PxScale { x: (15.0 * s).max(11.0), y: (15.0 * s).max(11.0) };
        let lh = render::line_height(&self.assets.font, scale);
        let mut y = 14.0 * s + lh;
        render::draw_line(
            &mut canvas,
            &self.assets.font,
            if self.ask { "Ask Bonzi" } else { "BonziBuddy — agent settings" },
            14.0 * s,
            y,
            PxScale { x: scale.x * 1.15, y: scale.y * 1.15 },
            [0x40, 0x00, 0x80, 255],
        );
        self.rects.clear();
        y += lh * 1.4;
        for i in 0..if self.ask { 1 } else { 3 } {
            let label = if self.ask { "Your question" } else { FIELDS[i] };
            render::draw_line(&mut canvas, &self.assets.font, label, 14.0 * s, y, scale, [0x33, 0x33, 0x33, 255]);
            y += lh * 0.95;
            let (bx, by, bw, bh) = (
                (14.0 * s) as i32,
                (y - lh * 0.55) as i32,
                (w as f32 - 28.0 * s) as i32,
                (lh * 1.5) as i32,
            );
            let fill = if i == self.sel { [0xFF, 0xFF, 0xFF, 255] } else { [0xEF, 0xEF, 0xE7, 255] };
            let border = if i == self.sel { [0x40, 0x00, 0x80, 255] } else { [0x99, 0x99, 0x99, 255] };
            canvas.round_rect(bx, by, bw, bh, 5, fill, border);
            let shown = self.field_text(i);
            render::draw_line(&mut canvas, &self.assets.font, &shown, bx as f32 + 6.0 * s, y + lh * 0.35, scale, [0, 0, 0, 255]);
            if i == self.sel && self.window.as_ref().is_some_and(|w| w.has_focus()) && self.frame_blink() {
                let caret_shown = if i == 1 { "•".repeat(self.caret.min(8)) } else { self.fields[i].chars().take(self.caret).collect() };
                let cx = bx as f32 + 6.0 * s
                    + render::text_width(&self.assets.font, &caret_shown, scale);
                canvas.round_rect(cx as i32, (y - lh * 0.35) as i32, (2.0 * s).max(1.0) as i32, (lh * 1.2) as i32, 0, [0, 0, 0, 255], [0, 0, 0, 255]);
            }
            self.rects.push((bx, by, bw, bh));
            y += lh * 1.35;
        }
        // Save button.
        let (bx, by, bw, bh) = ((14.0 * s) as i32, y as i32, (110.0 * s) as i32, (lh * 1.7) as i32);
        canvas.round_rect(bx, by, bw, bh, 6, [0xE8, 0xE0, 0xF8, 255], [0x40, 0x00, 0x80, 255]);
        render::draw_line(&mut canvas, &self.assets.font, if self.ask { "Ask" } else { "Save" }, bx as f32 + bw as f32 / 2.0 - 16.0 * s, by as f32 + bh as f32 * 0.68, scale, [0x40, 0x00, 0x80, 255]);
        self.save_rect = (bx, by, bw, bh);
        render::draw_line(
            &mut canvas,
            &self.assets.font,
            &self.status,
            (bx + bw + 14) as f32,
            by as f32 + bh as f32 * 0.68,
            scale,
            [0x20, 0x60, 0x20, 255],
        );
        render::draw_line(
            &mut canvas,
            &self.assets.font,
            if self.ask { "Enter asks. Esc closes." } else { "Click a field to edit. Enter saves. Esc closes." },
            14.0 * s,
            (by + bh) as f32 + lh * 1.3,
            scale,
            [0x60, 0x60, 0x60, 255],
        );
        let _ = h;

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
        size: (W, if ask { 170 } else { H }),
        rects: Vec::new(),
        save_rect: (0, 0, 0, 0),
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
                    .with_inner_size(LogicalSize::new(W, if ui.ask { 170 } else { H }))
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
                ui.window.as_ref().map(|w| w.request_redraw());
            }
            WindowEvent::KeyboardInput { event: KeyEvent { state: ElementState::Pressed, logical_key, text, .. }, .. } => {
                match &logical_key {
                    Key::Named(NamedKey::Escape) => el.exit(),
                    Key::Named(NamedKey::Enter) => {
                        ui.save();
                        el.exit();
                    }
                    Key::Named(NamedKey::Tab) => {
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
        },
        Event::AboutToWait => {
            el.set_control_flow(ControlFlow::Wait);
        }
        _ => {}
    })?;
    Ok(())
}
