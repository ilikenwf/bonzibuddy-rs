//! bonzibuddy-rs — a safe, offline, cross-platform reimplementation of the
//! 1999 BonziBuddy desktop companion, using the original MS Agent character
//! data extracted from the archived installers.
//!
//! What the original did that this does NOT:
//!   - phone home (im.bonziworld.com, daily content downloads)
//!   - log your browsing history and search queries (Browse.nbd / Search.nbd)
//!   - email modules (emsmtp/empop3) + referral spam
//!   - "Bonzi's Download Manager" (fetch & run arbitrary payloads, IE hook)
//!   - ad links (sites.nbd) and paid upsells (Bonzi Unlimited gating)
//!
//! This binary contains zero network code. All content is local.
//!
//! Phase 2 (planned): an OpenAPI-compatible agent client can push the same
//! `pet::Action` values the offline engine uses — see src/pet.rs.

mod assets;
mod content;
mod pet;
mod render;
mod tts;

use ab_glyph::PxScale;
use pet::{Action, Effect, Pet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::dpi::{LogicalSize, PhysicalPosition, Position};
use winit::event::{ElementState, Event, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Icon, Window, WindowAttributes, WindowLevel};

const WIN_W: u32 = 340;
const WIN_H: u32 = 460;

struct Opts {
    scale: f64,
    matte: bool,
    audio: bool,
    tts: bool,
}

/// Persisted user settings (`$XDG_CONFIG_HOME/bonzibuddy-rs.json`).
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Config {
    scale: f64,
}

impl Default for Config {
    fn default() -> Self {
        Config { scale: 1.0 }
    }
}

fn config_path() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let key = if cfg!(windows) { "APPDATA" } else { "HOME" };
            std::env::var_os(key).map(std::path::PathBuf::from)
        })?;
    Some(dir.join("bonzibuddy-rs.json"))
}

fn load_config() -> Config {
    config_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_config(cfg: &Config) {
    if let Some(p) = config_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(b) = serde_json::to_vec_pretty(cfg) {
            let _ = std::fs::write(p, b);
        }
    }
}

/// ALSA/JACK print connection-probe failures to stderr (C libraries) even when
/// another backend ultimately works; silence them while probing devices.
#[cfg(all(unix, feature = "audio"))]
struct QuietStderr(i32);
#[cfg(all(unix, feature = "audio"))]
impl QuietStderr {
    fn new() -> Self {
        use std::os::fd::AsRawFd;
        let saved = unsafe { libc::dup(2) };
        if let Ok(null) = std::fs::File::open("/dev/null") {
            unsafe { libc::dup2(null.as_raw_fd(), 2) };
        }
        QuietStderr(saved)
    }
}
#[cfg(all(unix, feature = "audio"))]
impl Drop for QuietStderr {
    fn drop(&mut self) {
        unsafe {
            libc::fflush(std::ptr::null_mut());
            libc::dup2(self.0, 2);
            libc::close(self.0);
        }
    }
}

/// Open an output stream: default device first, then every enumerated device
/// (covers JACK/PipeWire/ALSA ordering quirks where the "default" is busy).
#[cfg(feature = "audio")]
fn open_audio() -> Option<(rodio::OutputStream, rodio::OutputStreamHandle)> {
    #[cfg(unix)]
    let _quiet = QuietStderr::new();
    if let Ok(x) = rodio::OutputStream::try_default() {
        return Some(x);
    }
    use cpal::traits::HostTrait;
    if let Ok(devs) = cpal::default_host().devices() {
        for d in devs {
            if let Ok(x) = rodio::OutputStream::try_from_device(&d) {
                return Some(x);
            }
        }
    }
    None
}
const ABOUT: &str = "BonziBuddy-rs v1.0\nA safe reimagining of the 1999 original.\nNo phone-home. No ads. No spyware.\n100% offline. Original character art.\nRight-click me for the menu!";

struct MenuItem {
    label: &'static str,
    action: fn(&mut Pet, &content::Content),
}

const MENU: &[MenuItem] = &[
    MenuItem { label: "Tell me a joke", action: |p, c| p.tell_joke(c) },
    MenuItem { label: "Fun fact", action: |p, c| p.tell_fact(c) },
    MenuItem { label: "Sing a song", action: |p, c| p.sing(c) },
    MenuItem { label: "Do a trick", action: |p, _c| p.trick() },
    MenuItem { label: "Dance!", action: |p, _c| p.dance() },
    MenuItem { label: "Take a walk", action: |p, _c| p.start_walk() },
    MenuItem { label: "Hide", action: |p, _c| p.hide() },
    MenuItem { label: "About", action: |p, _c| p.say(ABOUT, 12.0) },
    MenuItem { label: "Quit", action: |p, _c| p.push(Action::Quit) },
];

struct App {
    assets: assets::Assets,
    content: content::Content,
    pet: Pet,
    opts: Opts,
    pixels: Option<pixels::Pixels<'static>>,
    window: Option<Arc<Window>>,
    size: (u32, u32),
    #[cfg(feature = "audio")]
    audio: Option<(rodio::OutputStream, rodio::OutputStreamHandle)>,
    #[cfg(feature = "audio")]
    sinks: Vec<rodio::Sink>,
    mouse: (f64, f64),
    press: Option<((f64, f64), (i32, i32))>,
    dragging: bool,
    menu_rects: Vec<(usize, (i32, i32, i32, i32))>,
    sprite_rect: (i32, i32, i32, i32),
    startup_done: bool,
}

impl App {
    #[cfg(feature = "audio")]
    fn play_sound(&mut self, name: &str) {
        if !self.opts.audio {
            return;
        }
        let Some(bytes) = self.assets.sound(name) else { return };
        let Some((_s, handle)) = &self.audio else { return };
        if let Ok(sink) = rodio::Sink::try_new(handle) {
            if let Ok(src) = rodio::Decoder::new(std::io::Cursor::new(bytes.to_vec())) {
                sink.append(src);
                self.sinks.retain(|s| !s.empty());
                self.sinks.push(sink);
            }
        }
    }

    #[cfg(not(feature = "audio"))]
    fn play_sound(&mut self, _name: &str) {}

    fn dispatch_effects(&mut self, effects: Vec<Effect>, el: &ActiveEventLoop) {
        for e in effects {
            match e {
                Effect::PlaySound(name) => self.play_sound(&name),
                Effect::SpeakTts(t) => {
                    if self.opts.tts {
                        tts::speak(&t);
                    }
                }
                Effect::Quit => el.exit(),
            }
        }
    }

    fn click(&mut self, x: f64, y: f64) {
        if self.pet.menu_open {
            for (i, (mx, my, mw, mh)) in self.menu_rects.clone() {
                if x >= mx as f64 && x < (mx + mw) as f64 && y >= my as f64 && y < (my + mh) as f64 {
                    self.pet.menu_open = false;
                    (MENU[i].action)(&mut self.pet, &self.content);
                    return;
                }
            }
            self.pet.menu_open = false;
            return;
        }
        let (sx, sy, sw, sh) = self.sprite_rect;
        if x >= sx as f64 && x < (sx + sw) as f64 && y >= sy as f64 && y < (sy + sh) as f64 {
            self.pet.click(&self.content);
        }
    }

    fn render(&mut self) {
        let Some(window) = &self.window else { return };
        let (w, h) = self.size;
        let mut canvas = Canvas::new(w, h);
        if self.opts.matte {
            canvas.clear([0x00, 0x80, 0x80, 255]); // classic Win95 teal
        } else {
            canvas.clear([0, 0, 0, 0]);
        }

        let sf = window.scale_factor();
        let s = self.opts.scale * sf;
        let cw = (self.assets.char_w as f64 * s) as i32;
        let chh = (self.assets.char_h as f64 * s) as i32;
        let sx = ((w as i32) - cw) / 2;
        let sy = (h as i32) - chh;
        self.sprite_rect = (sx, sy, cw, chh);

        let scale = PxScale { x: (15.0 * s).max(11.0) as f32, y: (15.0 * s).max(11.0) as f32 };
        let lh = render::line_height(&self.assets.font, scale);
        self.menu_rects.clear();
        if self.pet.menu_open {
            let bw = (190.0 * s) as i32;
            let bh = (26.0 * s) as i32;
            let bx = ((w as i32) - bw) / 2;
            let mut by = 6;
            for (i, item) in MENU.iter().enumerate() {
                canvas.round_rect(bx, by, bw, bh, 6, [0xF0, 0xF0, 0xE0, 255], [0, 0, 0, 255]);
                render::draw_line(&mut canvas, &self.assets.font, item.label, bx as f32 + (8.0 * s) as f32, by as f32 + bh as f32 * 0.68, scale, [0, 0, 0, 255]);
                self.menu_rects.push((i, (bx, by, bw, bh)));
                by += bh + 2;
            }
        } else if let Some(bubble) = self.pet.bubble.clone() {
            let max_w = (w as f32) - (24.0 * s) as f32;
            let lines = render::wrap(&self.assets.font, &bubble, max_w, scale);
            let th = lines.len() as f32 * lh;
            let bw = (w as i32) - 16;
            let bh = (th + (16.0 * s) as f32) as i32;
            let bx = 8;
            let by = (sy as f32 - bh as f32 - 6.0).max(4.0) as i32;
            canvas.round_rect(bx, by, bw, bh, 10, [0xFF, 0xFF, 0xCC, 255], [0, 0, 0, 255]);
            let mut ty = by as f32 + lh;
            for line in &lines {
                render::draw_line(&mut canvas, &self.assets.font, line, bx as f32 + (10.0 * s) as f32, ty, scale, [0, 0, 0, 255]);
                ty += lh;
            }
        }

        if let Some(frames) = self.assets.frames(&self.pet.anim) {
            if !frames.is_empty() {
                let idx = self.pet.frame % frames.len();
                let mouth_frame = if self.pet.speaking && self.pet.mouth > 0 {
                    self.assets.mouth_frame(&self.pet.anim, idx, self.pet.mouth)
                } else {
                    None
                };
                match &mouth_frame {
                    Some(f) => canvas.blit(f, sx, sy, s),
                    None => canvas.blit(&frames[idx], sx, sy, s),
                }
            }
        }

        // Upload as premultiplied BGRA (Wayland/X11 compositor convention).
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
}

use render::Canvas;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut opts = Opts { scale: load_config().scale, matte: std::env::var("BONZI_MATTE").is_ok(), audio: true, tts: false };
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--scale" => { opts.scale = args.get(i + 1).and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.0).clamp(0.5, 4.0); i += 1; }
            "--matte" => opts.matte = true,
            "--no-audio" => opts.audio = false,
            "--tts" => opts.tts = true,
            "--assets" => { if let Some(p) = args.get(i + 1) { std::env::set_var("BONZI_ASSETS", p); } i += 1; }
            "-h" | "--help" => {
                println!("bonzibuddy-rs — safe offline BonziBuddy. Flags: --scale N --matte --no-audio --tts --assets PATH\nScroll wheel over the window resizes the buddy (persisted).");
                return Ok(());
            }
            _ => {}
        }
        i += 1;
    }

    let assets = assets::Assets::load().map_err(|e| format!("failed to load assets: {e}"))?;
    let content = content::Content::load(&assets.root().join("content/nbd_decoded.json"))
        .unwrap_or_else(|e| {
            eprintln!("content: {e} — using empty pools");
            content::Content {
                jokes: vec![],
                facts: vec![],
                songs: vec![],
                greetings: vec!["Hi! I'm Bonzi!".into()],
                petting: vec!["Hehe!".into()],
            }
        });
    #[cfg(feature = "audio")]
    let audio = if opts.audio { open_audio() } else { None };
    #[cfg(feature = "audio")]
    if audio.is_none() && opts.audio {
        eprintln!("note: no audio device; running silent");
    }

    let mut app = App {
        assets,
        content,
        pet: Pet::new(),
        opts,
        window: None,
        pixels: None,
        size: (WIN_W, WIN_H),
        #[cfg(feature = "audio")]
        audio,
        #[cfg(feature = "audio")]
        sinks: Vec::new(),
        mouse: (0.0, 0.0),
        press: None,
        dragging: false,
        menu_rects: Vec::new(),
        sprite_rect: (0, 0, 0, 0),
        startup_done: false,
    };

    let el = EventLoop::new()?;
    el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(16)));
    // run_app migration is a pure rename of this closure into an ApplicationHandler impl;
    // EventLoop::run is still fully supported in winit 0.30.
    #[allow(deprecated)]
    el.run(move |event, el| match event {
        Event::Resumed if app.window.is_none() => {
            #[cfg_attr(not(windows), allow(unused_mut))]
            let mut attrs = WindowAttributes::default()
                .with_title("BonziBuddy")
                .with_decorations(false)
                .with_transparent(!app.opts.matte)
                .with_resizable(false)
                .with_inner_size(LogicalSize::new(WIN_W as f64 * app.opts.scale, WIN_H as f64 * app.opts.scale));
            #[cfg(windows)]
            {
                use winit::platform::windows::WindowAttributesExtWindows;
                attrs = attrs.with_skip_taskbar(true);
            }
            let Ok(window) = el.create_window(attrs) else {
                eprintln!("could not create window");
                el.exit();
                return;
            };
            window.set_window_level(WindowLevel::AlwaysOnTop);
            let (ax, ay, aw, ah) = window
                .current_monitor()
                .or_else(|| window.primary_monitor())
                .map(|m| {
                    let s = m.size();
                    let o = m.position();
                    (o.x, o.y, s.width as i32, s.height as i32)
                })
                .unwrap_or((0, 0, 1920, 1080));
            let sf0 = window.scale_factor();
            let ww = (WIN_W as f64 * app.opts.scale * sf0) as i32;
            let wh = (WIN_H as f64 * app.opts.scale * sf0) as i32;
            window.set_outer_position(Position::Physical(PhysicalPosition::new(
                ax + aw - ww - 40,
                ay + ah - wh - 20,
            )));
            let window = Arc::new(window);
            if let Ok(ico) = image::open(app.assets.root().join("favicon.ico")) {
                let img = ico.to_rgba8();
                let (iw, ih) = img.dimensions();
                let _ = window.set_window_icon(Icon::from_rgba(img.into_raw(), iw, ih).ok());
            }
            let size = window.inner_size();
            let (sw, sh) = (size.width.max(1), size.height.max(1));
            let build = |alpha: Option<pixels::wgpu::CompositeAlphaMode>| {
                let st = pixels::SurfaceTexture::new(sw, sh, window.clone());
                let mut b = pixels::PixelsBuilder::new(sw, sh, st)
                    .clear_color(pixels::wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 });
                if let Some(a) = alpha {
                    b = b.alpha_mode(a);
                }
                b.build()
            };
            // Premultiplied gives true per-pixel transparency on Wayland/X11/Windows;
            // fall back to the compositor default if the surface refuses it.
            let px = match build(Some(pixels::wgpu::CompositeAlphaMode::PreMultiplied)) {
                Ok(px) => px,
                Err(e1) => match build(None) {
                    Ok(px) => px,
                    Err(e2) => {
                        eprintln!("renderer: {e1} / {e2}");
                        el.exit();
                        return;
                    }
                },
            };
            app.size = (sw, sh);
            app.pixels = Some(px);
            app.window = Some(window);
            if !app.startup_done {
                app.startup_done = true;
                app.pet.startup(&app.content);
            }
        }
        Event::WindowEvent { event, .. } => match event {
            WindowEvent::Resized(size) => {
                let (nw, nh) = (size.width.max(1), size.height.max(1));
                if let Some(px) = &mut app.pixels {
                    let _ = px.resize_surface(nw, nh);
                    let _ = px.resize_buffer(nw, nh);
                }
                app.size = (nw, nh);
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(window) = &app.window {
                    let size = window.inner_size();
                    let (nw, nh) = (size.width.max(1), size.height.max(1));
                    if let Some(px) = &mut app.pixels {
                        let _ = px.resize_surface(nw, nh);
                        let _ = px.resize_buffer(nw, nh);
                    }
                    app.size = (nw, nh);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 60.0,
                };
                if dy != 0.0 {
                    let next = (app.opts.scale * (1.0 + 0.1 * dy.signum())).clamp(0.5, 4.0);
                    if (next - app.opts.scale).abs() > f64::EPSILON {
                        app.opts.scale = next;
                        save_config(&Config { scale: next });
                        if let Some(w) = &app.window {
                            let _ = w.request_inner_size(LogicalSize::new(WIN_W as f64 * next, WIN_H as f64 * next));
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                app.mouse = (position.x, position.y);
                let App { press, dragging, window, .. } = &mut app;
                if let (Some(((pmx, pmy), (wpx, wpy))), Some(window)) = (*press, &mut *window) {
                    let dx = position.x - pmx;
                    let dy = position.y - pmy;
                    if dx.abs() + dy.abs() > 6.0 {
                        *dragging = true;
                        // Wayland cannot move a window from the client: hand the drag
                        // to the compositor (also used on X11/macOS/Windows when
                        // supported); fall back to manual positioning otherwise.
                        if window.drag_window().is_err() {
                            window.set_outer_position(Position::Physical(PhysicalPosition::new(
                                (wpx as f64 + dx) as i32,
                                (wpy as f64 + dy) as i32,
                            )));
                        } else {
                            *press = None;
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => match (state, button) {
                (ElementState::Pressed, MouseButton::Left) => {
                    if let Some(window) = &app.window {
                        let pos = window.outer_position().unwrap_or(PhysicalPosition::new(0, 0));
                        app.press = Some((app.mouse, (pos.x, pos.y)));
                        app.dragging = false;
                    }
                }
                (ElementState::Released, MouseButton::Left) => {
                    if app.press.take().is_some() && !app.dragging {
                        app.click(app.mouse.0, app.mouse.1);
                    }
                    app.dragging = false;
                }
                (ElementState::Pressed, MouseButton::Right) => {
                    app.pet.menu_open = !app.pet.menu_open;
                }
                _ => {}
            },
            WindowEvent::KeyboardInput {
                event: KeyEvent { logical_key: Key::Named(NamedKey::Escape), state: ElementState::Pressed, .. },
                ..
            } => {
                app.pet.menu_open = false;
            }
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::RedrawRequested => {
                if app.window.is_some() {
                    let effects = {
                        let App { pet, assets, content, window, size, .. } = &mut app;
                        let (dx, effects) = pet.tick(|a| assets.has(a), content);
                        if dx != 0 {
                            if let Some(window) = window {
                                let (ax, aw) = window
                                    .primary_monitor()
                                    .map(|m| (m.position().x, m.size().width as i32))
                                    .unwrap_or((0, 1920));
                                let mut pos = window.outer_position().unwrap_or(PhysicalPosition::new(0, 0));
                                pos.x += (dx as f64 * window.scale_factor()) as i32;
                                if pos.x < ax {
                                    pos.x = ax;
                                    pet.walk_dir = 1;
                                    pet.set_anim("MOVERIGHT", true);
                                } else if pos.x > ax + aw - size.0 as i32 {
                                    pos.x = ax + aw - size.0 as i32;
                                    pet.walk_dir = -1;
                                    pet.set_anim("MOVELEFT", true);
                                }
                                window.set_outer_position(Position::Physical(pos));
                            }
                        }
                        if let Some(frames) = assets.frames(&pet.anim) {
                            if frames.is_empty() {
                                pet.anim = "IDLE1_1".into();
                            } else {
                                if pet.frame >= frames.len() {
                                    let ret = assets.return_anim(&pet.anim).map(|s| s.to_string());
                                    pet.anim_finished(|a| assets.has(a), ret.as_deref());
                                }
                                let idx = pet.frame.min(frames.len() - 1);
                                pet.frame_dur = Duration::from_millis(assets.frame_duration(&pet.anim, idx));
                            }
                        }
                        effects
                    };
                    app.dispatch_effects(effects, el);
                    app.render();
                }
            }
            _ => {}
        },
        Event::AboutToWait => {
            if let Some(w) = &app.window {
                w.request_redraw();
            }
            el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(16)));
        }
        _ => {}
    })?;
    Ok(())
}
