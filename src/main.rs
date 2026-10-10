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

mod agent;
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
use winit::event::{ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent};
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
pub struct Config {
    pub scale: f64,
    /// Physical window position; restored at startup when on-screen.
    pub x: Option<i32>,
    pub y: Option<i32>,
    /// Optional OpenAI-compatible chat endpoint ("Ask Bonzi…"). Empty = off;
    /// with it empty the binary never touches the network.
    pub agent_endpoint: String,
    pub agent_key: String,
    pub agent_model: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            scale: 1.0,
            x: None,
            y: None,
            agent_endpoint: String::new(),
            agent_key: String::new(),
            agent_model: String::new(),
        }
    }
}

/// A scale of 0 (or a corrupt value) would pin the buddy at the minimum
/// forever: the wheel can only multiply, and 0 × anything stays 0.
fn sanitize_scale(cfg: &mut Config) {
    if !(cfg.scale.is_finite() && (0.5..=4.0).contains(&cfg.scale)) {
        cfg.scale = 1.0;
    }
}

/// The wheel only multiplies, so a 0 (or corrupt) live scale can never move.
/// Treat such a value as 1.0 so scrolling always has an effect.
fn live_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

fn config_path() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let key = if cfg!(windows) { "APPDATA" } else { "HOME" };
            std::env::var_os(key).map(|h| {
                let mut p = std::path::PathBuf::from(h);
                // APPDATA is already the config dir; HOME needs .config appended.
                if !cfg!(windows) {
                    p.push(".config");
                }
                p
            })
        })?;
    Some(dir.join("bonzibuddy-rs.json"))
}

pub fn load_config() -> Config {
    let mut cfg: Config = config_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    sanitize_scale(&mut cfg);
    cfg
}

pub fn save_config(cfg: &Config) {
    if let Some(p) = config_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(b) = serde_json::to_vec_pretty(cfg) {
            let _ = std::fs::write(p, b);
        }
    }
}

/// Persist scale + current window position together.
fn save_state(window: Option<&Window>, scale: f64) {
    let pos = window.and_then(|w| w.outer_position().ok());
    // Load first so agent settings survive every position/scale save.
    let mut cfg = load_config();
    cfg.scale = scale.clamp(0.5, 4.0);
    cfg.x = pos.map(|p| p.x);
    cfg.y = pos.map(|p| p.y);
    save_config(&cfg);
}

/// System tray icon: left-click toggles visibility; the menu has
/// "Show / Hide" and "Quit". On Linux this needs libappindicator at runtime;
/// returns `None` when the desktop offers no tray (Hide then auto-returns).
fn make_tray(assets: &assets::Assets) -> Option<tray_icon::TrayIcon> {
    use tray_icon::menu::{Menu, MenuItem};
    use tray_icon::TrayIconBuilder;
    #[cfg(target_os = "linux")]
    {
        // muda/tray-icon use GTK on Linux and never initialize it themselves.
        if gtk::init().is_err() {
            return None;
        }
    }
    let img = image::open(assets.root().join("favicon.ico")).ok()?.to_rgba8();
    let img = image::imageops::resize(&img, 32, 32, image::imageops::FilterType::Lanczos3);
    let (tw, th) = (img.width(), img.height());
    let icon = tray_icon::Icon::from_rgba(img.into_raw(), tw, th).ok()?;
    let menu = Menu::new();
    let toggle = MenuItem::with_id("toggle", "Show / Hide", true, None);
    let quit = MenuItem::with_id("quit", "Quit", true, None);
    menu.append_items(&[&toggle, &quit]).ok()?;
    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("BonziBuddy")
        .with_icon(icon)
        .build()
        .ok()
}

fn toggle_visibility(app: &mut App) {
    let Some(w) = &app.window else { return };
    if w.is_visible().unwrap_or(true) {
        w.set_visible(false);
    } else {
        w.set_visible(true);
        // Some WMs drop the above-level hint when a window is remapped.
        w.set_window_level(WindowLevel::AlwaysOnTop);
        app.pet.reveal();
        w.request_redraw();
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

/// Open an output stream. Tries the platform default, then every host in
/// `cpal::ALL_HOSTS` (JACK before ALSA when built with the `jack` feature) and
/// every device of each host. On jack/PipeWire desktops the ALSA "default" is
/// often busy (dmix) while the JACK host connects cleanly — the old code only
/// probed the default host and silently ran silent.
#[cfg(feature = "audio")]
fn open_audio() -> Option<(rodio::OutputStream, rodio::OutputStreamHandle)> {
    let verbose = std::env::var("BONZI_AUDIO_DEBUG").is_ok();
    #[cfg(unix)]
    // ALSA/JACK print probe noise to stderr; keep it only when debugging.
    let _quiet = if verbose { None } else { Some(QuietStderr::new()) };
    #[cfg(not(unix))]
    let _ = &verbose;
    use cpal::traits::{DeviceTrait, HostTrait};
    if verbose {
        eprintln!("[audio] hosts: {:?}", cpal::platform::ALL_HOSTS);
    }
    match rodio::OutputStream::try_default() {
        Ok(x) => {
            if verbose {
                eprintln!("[audio] default device opened OK");
            }
            return Some(x);
        }
        Err(e) => {
            if verbose {
                eprintln!("[audio] default device failed: {e}");
            }
        }
    }
    for id in cpal::platform::ALL_HOSTS {
        let Ok(host) = cpal::platform::host_from_id(*id) else {
            if verbose {
                eprintln!("[audio] host {id:?}: unavailable");
            }
            continue;
        };
        if let Some(dev) = host.default_output_device() {
            let name = dev.name().unwrap_or_else(|_| "?".into());
            match rodio::OutputStream::try_from_device(&dev) {
                Ok(x) => {
                    if verbose {
                        eprintln!("[audio] opened default of host {id:?}: {name}");
                    }
                    return Some(x);
                }
                Err(e) if verbose => eprintln!("[audio] host {id:?} default {name}: {e}"),
                Err(_) => {}
            }
        }
        match host.devices() {
            Ok(devs) => {
                for d in devs {
                    let name = d.name().unwrap_or_else(|_| "?".into());
                    match rodio::OutputStream::try_from_device(&d) {
                        Ok(x) => {
                            if verbose {
                                eprintln!("[audio] opened {name} on host {id:?}");
                            }
                            return Some(x);
                        }
                        Err(e) if verbose => eprintln!("[audio] {name} on {id:?}: {e}"),
                        Err(_) => {}
                    }
                }
            }
            Err(e) if verbose => eprintln!("[audio] host {id:?} enumerate: {e}"),
            Err(_) => {}
        }
    }
    None
}
const ABOUT: &str = "BonziBuddy-rs v1.0\nA safe reimagining of the 1999 original.\nNo phone-home. No ads. No spyware.\n100% offline. Original character art.\nRight-click me for the menu!";

mod settings_ui;
/// Settings live in a forked child process running a *managed* window, so
/// the WM focuses it and native text inputs work (paste, IME, dead keys).
/// The child re-executes this binary with `--settings-ui`; it writes the
/// config file and exits — the pet reloads config when it changes (mtime).
fn fork_settings() -> Result<(), std::io::Error> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .arg("--settings-ui")
        .stdin(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

/// True when the config file's mtime changed since `last` (child saved).
fn config_mtime() -> Option<std::time::SystemTime> {
    config_path().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok())
}




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
    MenuItem { label: "Ask Bonzi…", action: |_, _| {} },
    MenuItem { label: "Settings…", action: |_, _| {} },
    MenuItem { label: "Hide", action: |p, _c| p.hide() },
    MenuItem { label: "About", action: |p, _c| p.say(ABOUT, 12.0) },
    MenuItem { label: "Quit", action: |p, _c| p.push(Action::Quit) },
];

/// In-app text-entry dialogs (rendered with our own font, no widgets crate).
#[derive(Clone)]
enum Dialog {
    None,
    /// Ask Bonzi: `caret` is the char index (not byte!) of the text cursor.
    Ask { input: String, caret: usize },
}

impl Dialog {
    fn active(&self) -> bool {
        !matches!(self, Dialog::None)
    }
}

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
    sink: Option<rodio::Sink>,
    /// Last spawned fallback player / TTS child; killed when new audio
    /// starts so outputs never overlap.
    child: Option<std::process::Child>,
    mouse: (f64, f64),
    press: Option<((f64, f64), (i32, i32))>,
    dragging: bool,
    menu_rects: Vec<(usize, (i32, i32, i32, i32))>,
    sprite_rect: (i32, i32, i32, i32),
    startup_done: bool,
    tray: Option<tray_icon::TrayIcon>,
    /// When set (tray active), the window hides once this deadline passes,
    /// after the HIDE animation has played.
    hide_at: Option<Instant>,
    agent: agent::AgentCfg,
    agent_tx: std::sync::mpsc::Sender<agent::Msg>,
    agent_rx: std::sync::mpsc::Receiver<agent::Msg>,
    dialog: Dialog,
    /// X11 window that held keyboard focus before a dialog grabbed it.
    prev_focus: Option<u32>,
    /// Config file mtime; a newer mtime means the Settings child saved.
    cfg_mtime: Option<std::time::SystemTime>,
    /// Our own X11 window id (override-redirect ⇒ the WM never focuses it,
    /// so dialogs must XSetInputFocus it themselves).
    x11_id: Option<u32>,
}

impl App {
    /// Resolve a sound name to its on-disk file (assets/audio first).
    fn sound_path(&self, name: &str) -> Option<std::path::PathBuf> {
        for sub in ["audio", "sfx", "bonzi/audio"] {
            let p = self.assets.root().join(sub).join(name);
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    /// Kill whatever audio is currently playing (rodio sink and/or a
    /// spawned player/TTS child). Called before every new output: one
    /// sound at a time, newer actions interrupt older ones.
    fn stop_audio(&mut self) {
        #[cfg(feature = "audio")]
        if let Some(s) = self.sink.take() {
            s.stop();
        }
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// Last-resort playback when cpal finds no openable device (common when
    /// the session has no Pulse/PipeWire socket): shell out to whatever the
    /// distro provides, same spirit as the TTS fallback.
    #[cfg(unix)]
    fn play_sound_subprocess(&mut self, path: &std::path::Path) {
        let p = path.to_str().unwrap_or_default();
        let cands: [(&str, Vec<&str>); 4] = [
            ("paplay", vec![p]),
            ("aplay", vec!["-q", p]),
            ("afplay", vec![p]),
            ("ffplay", vec!["-nodisp", "-autoexit", "-loglevel", "quiet", p]),
        ];
        for (prog, args) in cands {
            if let Ok(c) = std::process::Command::new(prog)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                self.child = Some(c);
                return;
            }
        }
    }
    #[cfg(not(unix))]
    fn play_sound_subprocess(&mut self, _path: &std::path::Path) {}

    #[cfg(feature = "audio")]
    fn play_sound(&mut self, name: &str) {
        if !self.opts.audio {
            return;
        }
        self.stop_audio();
        match &self.audio {
            Some((_s, handle)) => {
                let Some(bytes) = self.assets.sound(name) else { return };
                if let Ok(sink) = rodio::Sink::try_new(handle) {
                    match rodio::Decoder::new(std::io::Cursor::new(bytes.to_vec())) {
                        Ok(src) => {
                            sink.append(src);
                            self.sink = Some(sink);
                        }
                        Err(e) => {
                            if std::env::var("BONZI_AUDIO_DEBUG").is_ok() {
                                eprintln!("[audio] decode {name} failed: {e}");
                            }
                        }
                    }
                }
            }
            None => {
                if let Some(p) = self.sound_path(name) {
                    self.play_sound_subprocess(&p);
                }
            }
        }
    }

    #[cfg(not(feature = "audio"))]
    fn play_sound(&mut self, name: &str) {
        self.stop_audio();
        if let Some(p) = self.sound_path(name) {
            self.play_sound_subprocess(&p);
        }
    }

    fn dispatch_effects(&mut self, effects: Vec<Effect>, el: &ActiveEventLoop) {
        // Any batch of effects is a new "action": whatever was speaking or
        // playing before gets cut off first.
        if !effects.is_empty() {
            self.stop_audio();
        }
        for e in effects {
            match e {
                Effect::PlaySound(name) => self.play_sound(&name),
                Effect::SpeakTts(t) => {
                    if self.opts.tts {
                        self.child = tts::speak(&t);
                    }
                }
                Effect::AgentReply(t) => {
                    // Long model answers must not be truncated by the 14 s
                    // bubble cap: show for the full reading time.
                    let secs = (4.0 + t.chars().count() as f64 * 0.09).min(60.0);
                    self.pet.bubble = Some(t.clone());
                    self.pet.bubble_until =
                        Some(Instant::now() + Duration::from_secs_f64(secs));
                    self.pet.speaking = true;
                    if self.opts.tts {
                        self.child = tts::speak(&t);
                    }
                }
                Effect::Quit => {
                    save_state(self.window.as_deref(), self.opts.scale);
                    el.exit();
                }
            }
        }
    }

    fn click(&mut self, x: f64, y: f64) {
        if self.pet.menu_open {
            for (i, (mx, my, mw, mh)) in self.menu_rects.clone() {
                if x >= mx as f64 && x < (mx + mw) as f64 && y >= my as f64 && y < (my + mh) as f64 {
                    self.pet.menu_open = false;
                    match MENU[i].label {
                        "Ask Bonzi…" => self.open_ask(),
                        "Settings…" => self.open_settings(),
                        label => {
                            (MENU[i].action)(&mut self.pet, &self.content);
                            // With a tray, "Hide" minimizes to it after the HIDE
                            // animation; without one the pet auto-returns in 8 s.
                            if label == "Hide" && self.tray.is_some() {
                                self.hide_at = Some(Instant::now() + Duration::from_millis(1400));
                            }
                        }
                    }
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

    /// Override-redirect windows are invisible to the WM, so nothing ever
    /// gives them keyboard focus. Grab it explicitly for the duration of a
    /// dialog (the rofi trick) and hand it back afterwards.
    fn grab_keyboard(&mut self) {
        #[cfg(target_os = "linux")]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            use x11rb::connection::Connection;
            use x11rb::protocol::xproto::{ConnectionExt, InputFocus};
            // winit 0.30's WindowExtX11 is a marker trait; the real xid comes
            // through raw-window-handle.
            let id: Option<u32> = self
                .window
                .as_ref()
                .and_then(|w| w.window_handle().ok())
                .and_then(|h| match h.as_raw() {
                    RawWindowHandle::Xlib(h) => Some(h.window as u32),
                    RawWindowHandle::Xcb(h) => Some(h.window.get()),
                    _ => None,
                });
            if let Some(id) = id {
                self.x11_id = Some(id);
                if let Ok((conn, _)) = x11rb::connect(None) {
                    let focus = x11rb::protocol::xproto::get_input_focus(&conn)
                        .ok()
                        .and_then(|c| c.reply().ok());
                    if let Some(f) = focus {
                        // Focus ids 0/1 are the NONE/POINTER_ROOT sentinels.
                        self.prev_focus =
                            if f.focus > 1 && f.focus != id { Some(f.focus) } else { None };
                    }
                    let _ = conn.set_input_focus(InputFocus::PARENT, id, 0u32);
                    let _ = conn.flush();
                }
            }
        }
        if let Some(w) = &self.window {
            w.focus_window();
        }
    }

    fn release_keyboard(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(id) = self.prev_focus.take() {
            use x11rb::connection::Connection;
            use x11rb::protocol::xproto::{ConnectionExt, InputFocus};
            if let Ok((conn, _)) = x11rb::connect(None) {
                let _ = conn.set_input_focus(InputFocus::PARENT, id, 0u32);
                let _ = conn.flush();
            }
        }
    }

    /// Settings open in a forked child process with a normal, WM-managed
    /// window — real keyboard focus, real text inputs, paste works. The pet
    /// picks up the saved config via file mtime polling.
    fn open_settings(&mut self) {
        match fork_settings() {
            Ok(()) => self
                .pet
                .say("A settings window should have popped up — mind my banana!", 5.0),
            Err(e) => self.pet.say(&format!("Settings won't start: {e}"), 6.0),
        }
    }

    fn open_ask(&mut self) {
        if !self.agent.enabled() {
            self.pet.say(
                "I can't chat without a brain server! Set an API endpoint in Settings first, heh.",
                5.0,
            );
            return;
        }
        self.dialog = Dialog::Ask { input: String::new(), caret: 0 };
        self.grab_keyboard();
    }

    /// Route a key press to the open Ask dialog; returns true when consumed.
    fn dialog_key(&mut self, key: &Key, text: Option<&str>) -> bool {
        let (input, caret) = match &mut self.dialog {
            Dialog::Ask { input, caret } => (input, caret),
            Dialog::None => return false,
        };
        let chars: Vec<char> = input.chars().collect();
        match key {
            Key::Named(NamedKey::Escape) => {
                self.dialog = Dialog::None;
                self.release_keyboard();
                return true;
            }
            Key::Named(NamedKey::Backspace) => {
                if *caret > 0 {
                    let mut c = chars;
                    c.remove(*caret - 1);
                    *input = c.into_iter().collect();
                    *caret -= 1;
                }
                return true;
            }
            Key::Named(NamedKey::Delete) => {
                if *caret < chars.len() {
                    let mut c = chars;
                    c.remove(*caret);
                    *input = c.into_iter().collect();
                }
                return true;
            }
            Key::Named(NamedKey::ArrowLeft) => {
                *caret = caret.saturating_sub(1);
                return true;
            }
            Key::Named(NamedKey::ArrowRight) => {
                *caret = (*caret + 1).min(chars.len());
                return true;
            }
            Key::Named(NamedKey::Home) => {
                *caret = 0;
                return true;
            }
            Key::Named(NamedKey::End) => {
                *caret = chars.len();
                return true;
            }
            Key::Named(NamedKey::Enter) => {
                drop(chars);
                self.dialog_commit();
                return true;
            }
            _ => {}
        }
        if let Some(c) =
            text.and_then(|t| t.chars().next()).filter(|c| c.is_ascii_graphic() || *c == ' ')
        {
            if input.chars().count() < 400 {
                let mut ch: Vec<char> = input.chars().collect();
                ch.insert(*caret, c);
                *input = ch.into_iter().collect();
                *caret += 1;
            }
            return true;
        }
        false
    }

    fn dialog_commit(&mut self) {
        self.release_keyboard();
        match std::mem::replace(&mut self.dialog, Dialog::None) {
            Dialog::Ask { input, .. } => {
                let q = input.trim().to_string();
                if q.is_empty() {
                    return;
                }
                self.pet.say("Hmm, let me think…", 8.0);
                agent::send(self.agent.clone(), q, self.agent_tx.clone());
            }
            Dialog::None => {}
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
            let gap = (2.0 * s) as i32;
            let bx = ((w as i32) - bw) / 2;
            // Anchor the menu to the sprite and clamp it into the window:
            // at high zoom a top-anchored menu runs off-screen (the window can
            // be taller than the monitor), so clicks land on invisible rows.
            let total = MENU.len() as i32 * (bh + gap);
            let mut by = (sy - total - (8.0 * s) as i32).clamp(4, (h as i32 - total - 4).max(4));
            for (i, item) in MENU.iter().enumerate() {
                canvas.round_rect(bx, by, bw, bh, 6, [0xF0, 0xF0, 0xE0, 255], [0, 0, 0, 255]);
                render::draw_line(&mut canvas, &self.assets.font, item.label, bx as f32 + (8.0 * s) as f32, by as f32 + bh as f32 * 0.68, scale, [0, 0, 0, 255]);
                self.menu_rects.push((i, (bx, by, bw, bh)));
                by += bh + gap;
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

        // The Ask dialog draws over everything, anchored to the window top.
        if let Dialog::Ask { input, caret } = self.dialog.clone() {
            let pad = (10.0 * s) as i32;
            let bw = (w as i32) - 16;
            let bh = (lh * 3.6) as i32 + pad;
            let bx = 8;
            let by = 4;
            canvas.round_rect(bx, by, bw, bh, 10, [0xE8, 0xE8, 0xF4, 255], [0, 0, 0, 255]);
            let tx = bx as f32 + (10.0 * s) as f32;
            let ty = by as f32 + lh;
            render::draw_line(&mut canvas, &self.assets.font, "Ask Bonzi", tx, ty, scale, [0x40, 0x00, 0x80, 255]);
            // Input line: white field, text ending at the caret, blinking bar.
            let iy = ty + lh;
            canvas.round_rect(bx + 6, (iy - lh) as i32, bw - 12, (lh * 1.3) as i32, 4, [0xFF, 0xFF, 0xFF, 255], [0x80, 0x80, 0x80, 255]);
            let chars: Vec<char> = input.chars().collect();
            let shown: String = chars[..caret.min(chars.len())].iter().collect();
            render::draw_line(&mut canvas, &self.assets.font, &shown, tx, iy, scale, [0, 0, 0, 255]);
            if self.pet.frame % 6 < 3 {
                let cx = tx + render::text_width(&self.assets.font, &shown, scale);
                canvas.round_rect(cx as i32, (iy - lh * 0.85) as i32, (2.0 * s).max(1.0) as i32, (lh * 1.05) as i32, 0, [0, 0, 0, 255], [0, 0, 0, 255]);
            }
            render::draw_line(&mut canvas, &self.assets.font, "Enter: ask  Esc: cancel", tx, iy + lh * 0.9, scale, [0x60, 0x60, 0x60, 255]);
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
    // Child mode: the forked Settings window (managed ⇒ real keyboard focus).
    if std::env::args().any(|a| a == "--settings-ui") {
        if let Err(e) = settings_ui::run() {
            eprintln!("settings window failed: {e}");
            return Err(e);
        }
        return Ok(());
    }
    // A desktop pet needs override-redirect semantics (no taskbar, always
    // above, free programmatic movement). Native Wayland offers none of that
    // to normal clients — walking would freeze and the compositor decides
    // stacking. XWayland gives us all three under KWin/GNOME; opt out with
    // BONZI_WAYLAND=1.
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("BONZI_WAYLAND").as_deref() != Ok("1")
    {
        std::env::remove_var("WAYLAND_DISPLAY");
        eprintln!("note: Wayland session — using XWayland for always-on-top and walking (BONZI_WAYLAND=1 to override)");
    }
    let cfg = load_config();
    let (agent_tx, agent_rx) = std::sync::mpsc::channel();
    let saved_pos = (cfg.x, cfg.y);
    let mut opts = Opts { scale: cfg.scale, matte: std::env::var("BONZI_MATTE").is_ok(), audio: true, tts: false };
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--scale" => { opts.scale = args.get(i + 1).and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.0).clamp(0.5, 4.0); i += 1; }
            "--matte" => opts.matte = true,
            "--no-audio" => opts.audio = false,
            "--tts" => opts.tts = true,
            "--assets" => { if let Some(p) = args.get(i + 1) { std::env::set_var("BONZI_ASSETS", p); } i += 1; }
            "--name" => {
                if let Some(n) = args.get(i + 1).filter(|v| !v.trim().is_empty()) {
                    std::env::set_var("BONZI_NAME", n);
                }
                i += 1;
            }
            "-h" | "--help" => {
                println!("bonzibuddy-rs — safe offline BonziBuddy. Flags: --scale N --name NAME --matte --no-audio --tts --assets PATH\nScroll wheel over the window resizes the buddy; position, size and zoom are persisted.\nTray icon: left-click toggles visibility; menu has Show / Hide and Quit.");
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
        eprintln!(
            "note: no audio device via cpal; effects fall back to paplay/aplay/afplay/ffplay.\n\
             (set BONZI_AUDIO_DEBUG=1 to see every device probe; run under PipeWire/Pulse for sound)"
        );
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
        sink: None,
        child: None,
        mouse: (0.0, 0.0),
        press: None,
        dragging: false,
        menu_rects: Vec::new(),
        sprite_rect: (0, 0, 0, 0),
        startup_done: false,
        tray: None,
        hide_at: None,
        agent: agent::AgentCfg {
            endpoint: cfg.agent_endpoint.clone(),
            key: cfg.agent_key.clone(),
            model: cfg.agent_model.clone(),
        },
        agent_tx: agent_tx,
        agent_rx: agent_rx,
        dialog: Dialog::None,
        prev_focus: None,
        cfg_mtime: config_mtime(),
        x11_id: None,
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
            // X11: an override-redirect window bypasses the WM entirely —
            // no taskbar entry, no decorations, and nothing can un-float it.
            // The pet positions and moves itself programmatically anyway.
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            {
                use winit::platform::x11::WindowAttributesExtX11;
                // Only when we actually land on X11 (XWayland-forced or plain
                // X11 session); native Wayland (BONZI_WAYLAND=1) rejects it.
                if std::env::var_os("WAYLAND_DISPLAY").is_none() {
                    attrs = attrs.with_override_redirect(true);
                }
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
            // Restore the saved position when it is still on-screen.
            let pos = match saved_pos {
                (Some(x), Some(y)) if x >= ax - ww && x <= ax + aw && y >= ay && y <= ay + ah => {
                    (x, y)
                }
                _ => (ax + aw - ww - 40, ay + ah - wh - 20),
            };
            window.set_outer_position(Position::Physical(PhysicalPosition::new(pos.0, pos.1)));
            let window = Arc::new(window);
            if let Ok(ico) = image::open(app.assets.root().join("favicon.ico")) {
                let img = ico.to_rgba8();
                let (iw, ih) = img.dimensions();
                let _ = window.set_window_icon(Icon::from_rgba(img.into_raw(), iw, ih).ok());
            }
            if app.tray.is_none() {
                app.tray = make_tray(&app.assets);
                if app.tray.is_none() {
                    eprintln!(
                        "note: system tray unavailable (Linux: install libappindicator); \
                         Hide auto-returns after 8s"
                    );
                }
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
                    let base = live_scale(app.opts.scale);
                    let next = (base * (1.0 + 0.1 * dy.signum())).clamp(0.5, 4.0);
                    if (next - base).abs() > f64::EPSILON {
                        app.opts.scale = next;
                        save_state(app.window.as_deref(), next);
                        if let Some(w) = &app.window {
                            let _ = w.request_inner_size(LogicalSize::new(WIN_W as f64 * next, WIN_H as f64 * next));
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                app.mouse = (position.x, position.y);
                if let Some(((pmx, pmy), (wpx, wpy))) = app.press {
                    let dx = position.x - pmx;
                    let dy = position.y - pmy;
                    if dx.abs() + dy.abs() > 6.0 {
                        // Any drag moves the window — including drags that
                        // start on the buddy himself (that's how you grab a
                        // desktop pet). A press without motion stays a click
                        // (pet reaction / menu) via the release handler.
                        // drag_window() is a WM operation and cannot work on
                        // an unmanaged override-redirect window: move manually.
                        app.dragging = true;
                        if let Some(window) = &app.window {
                            let (ax, ay, aw, ah) = window
                                .primary_monitor()
                                .map(|m| {
                                    (
                                        m.position().x,
                                        m.position().y,
                                        m.size().width as i32,
                                        m.size().height as i32,
                                    )
                                })
                                .unwrap_or((0, 0, 1920, 1080));
                            let nx = (wpx as f64 + dx).clamp(ax as f64, (ax + aw) as f64);
                            let ny = (wpy as f64 + dy).clamp(ay as f64, (ay + ah) as f64);
                            window.set_outer_position(Position::Physical(PhysicalPosition::new(
                                nx as i32, ny as i32,
                            )));
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
                    } else if app.dragging {
                        save_state(app.window.as_deref(), app.opts.scale);
                    }
                    app.dragging = false;
                }
                (ElementState::Pressed, MouseButton::Right) => {
                    app.pet.menu_open = !app.pet.menu_open;
                }
                _ => {}
            },
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed =>
            {
                if app.dialog.active() {
                    app.dialog_key(&event.logical_key, event.text.as_deref());
                } else if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                    app.pet.menu_open = false;
                }
            }
            WindowEvent::CloseRequested => {
                save_state(app.window.as_deref(), app.opts.scale);
                el.exit();
            }
            WindowEvent::RedrawRequested => {
                if app.window.is_some() {
                    let effects = {
                        let App { pet, assets, content, window, size, opts, .. } = &mut app;
                        let prev_walk = pet.walk_dir;
                        let (dx, effects) = pet.tick(|a| assets.has(a), content);
                        if dx == 0 && prev_walk != 0 {
                            // walk just finished: remember where we ended up
                            save_state(window.as_deref(), opts.scale);
                        }
                        if dx != 0 {
                            if let Some(window) = window {
                                let (ax, aw) = window
                                    .primary_monitor()
                                    .map(|m| (m.position().x, m.size().width as i32))
                                    .unwrap_or((0, 1920));
                                let mut pos = window.outer_position().unwrap_or(PhysicalPosition::new(0, 0));
                                pos.x += (dx as f64 * window.scale_factor() * opts.scale) as i32;
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
            // Agent replies land here (worker thread → channel → bubble+TTS).
            while let Ok(agent::Msg::Reply(r)) = app.agent_rx.try_recv() {
                match r {
                    Ok(text) => {
                        let mut fx = vec![pet::Effect::AgentReply(text)];
                        if app.pet.is_idle() {
                            app.pet.set_anim("EXPLAIN", false);
                        }
                        fx.append(&mut app.pet.pending);
                        app.dispatch_effects(fx, el);
                    }
                    Err(e) => app.pet.say(&format!("My brain went fuzzy: {e}"), 6.0),
                }
            }
            // Settings child saved the config? Reload agent settings.
            if let Some(mt) = config_mtime() {
                if app.cfg_mtime.is_some_and(|m| mt > m) {
                    app.cfg_mtime = Some(mt);
                    let cfg = load_config();
                    app.agent = agent::AgentCfg {
                        endpoint: cfg.agent_endpoint,
                        key: cfg.agent_key,
                        model: cfg.agent_model,
                    };
                    app.pet.say("Ooh, a brain upgrade! Try Ask Bonzi now.", 4.0);
                }
            }
            #[cfg(target_os = "linux")]
            if app.tray.is_some() {
                // libappindicator delivers registration + menu events on the
                // default GMainContext; winit doesn't run it, so pump it.
                while gtk::glib::MainContext::default().iteration(false) {}
            }
            // Hide-to-tray deadline (checked here, not on redraw: a hidden
            // window stops emitting redraw events entirely).
            if let Some(at) = app.hide_at {
                if Instant::now() >= at {
                    app.hide_at = None;
                    if let Some(w) = &app.window {
                        w.set_visible(false);
                    }
                }
            }
            while let Ok(ev) = tray_icon::menu::MenuEvent::receiver().try_recv() {
                match ev.id.0.as_str() {
                    "toggle" => toggle_visibility(&mut app),
                    "quit" => {
                        save_state(app.window.as_deref(), app.opts.scale);
                        el.exit();
                    }
                    _ => {}
                }
            }
            while let Ok(ev) = tray_icon::TrayIconEvent::receiver().try_recv() {
                match ev {
                    tray_icon::TrayIconEvent::Click {
                        button: tray_icon::MouseButton::Left,
                        button_state: tray_icon::MouseButtonState::Up,
                        ..
                    }
                    | tray_icon::TrayIconEvent::DoubleClick { .. } => {
                        toggle_visibility(&mut app)
                    }
                    _ => {}
                }
            }
            if let Some(w) = &app.window {
                w.request_redraw();
            }
            el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(16)));
        }
        _ => {}
    })?;
    Ok(())
}
