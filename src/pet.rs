//! The pet behavior state machine.
//!
//! # Phase-2 integration point
//! Every trigger — menu click, idle personality roll, or (later) an HTTP
//! agent response — funnels through [`Action`]. An OpenAPI-compatible agent
//! client only needs to push `Action`s into `Pet::push()`; the offline engine
//! below works identically with no agent attached.

use crate::content::{clean_tts, Content};
use rand::Rng;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub enum Action {
    /// Play a one-shot animation (falls back to idle when unknown).
    Animate(String),
    /// Loop an animation for N ms, then its return animation if any.
    AnimateLoop(String, u64),
    /// Speech bubble (+ optional local TTS).
    Speak(String),
    /// Combined: gesture + talk.
    Tell(String, String),
    /// Play a wav from assets (audio/, sfx/ or bonzi/audio/).
    Sound(String),
    /// Loop background music (repeats the wav) until StopMusic or the
    /// next action replaces it.
    Music(String),
    /// Walk: -1 left, +1 right, 0 stop.
    Walk(i8),
    Hide,
    Quit,
}

#[derive(Clone, Debug)]
pub enum Effect {
    /// Voice/sfx: interrupts any previous voice or TTS, never the music.
    PlaySound(String),
    /// Background music: replaces previous music, survives voice clips.
    PlayMusic(String),
    StopMusic,
    /// A new user/idle action begins: silence everything (voice, music,
    /// TTS child) before its own sounds start.
    StopAll,
    SpeakTts(String),
    /// Ask Bonzi reply: shown verbatim (no 14 s bubble cap) and voiced.
    AgentReply(String),
    Quit,
}

const IDLE_POOL: &[&str] = &[
    "IDLE1_1", "IDLE1_3", "IDLE1_4", "IDLE1_5", "IDLE1_6", "IDLE1_7", "IDLE1_8", "IDLE1_9",
    "IDLE1_11", "IDLE1_12", "IDLE1_13", "IDLE1_14", "IDLE1_15", "IDLE1_21", "IDLE1_24",
    "IDLE1_25", "IDLE1_26", "IDLE3_1", "IDLE3_2", "IDLE3_3", "BLINK", "BLINK", "BLINK",
    "LOOKLEFT", "LOOKRIGHT", "LOOKUP", "LOOKDOWN", "THINKING", "PROCESSING", "HEARING1",
    "HEARING2", "HEARING3", "HANDSBEHIND", "RESTPOSE", "SUGGEST",
];
const FUN_POOL: &[&str] = &[
    "BANANA", "BANANAMISS", "JUGGLE", "BUTTERNUT", "GIGGLE", "DOMAGIC1", "DOMAGIC2", "WINK",
    "BLOWKISS", "HUG", "SURPRISED", "UNBELIEVABLE", "CONFUSED", "SAD", "WAVE", "CONGRATULATE",
    "SHOOSH", "GETATTENTION", "GETATTENTION2", "MAILCHECK", "MAILCHECKEMPTY", "MAILCHECKFULL",
    "MAILNEXT", "MAILREAD", "MOVEDOWN", "MOVEUP", "READ", "READCONTINUED", "READLOOKUP",
    "READLOOKUPCONTINUED", "READING", "WRITE", "WRITECONTINUED", "WRITEONCE", "WRITEONCEAGAIN",
    "WRITING", "SEARCH", "SUNGLASSESCONTINUED",
];
const PETTED_POOL: &[&str] = &["PLEASED", "PLEASEDSOFT", "PLEASEDHARD", "GIGGLE", "WINK", "HUG"];
const TALK_POOL: &[&str] = &["EXPLAIN", "EXPLAIN2", "EXPLAIN3", "EXPLAIN4", "GREET", "SUGGEST"];

pub struct Pet {
    pub anim: String,
    pub frame: usize,
    pub looping: bool,
    loop_until: Option<Instant>,
    pub bubble: Option<String>,
    pub bubble_until: Option<Instant>,
    pub speaking: bool,
    pub mouth: u8,
    mouth_at: Instant,
    pub walk_dir: i8,
    walk_until: Option<Instant>,
    /// Wall-clock dt bookkeeping: walk speed is px/second, not px/redraw,
    /// so ground speed stays glued to the 100 ms/frame vine-swing cycle
    /// regardless of monitor refresh or frame hitches.
    last_tick: Instant,
    walk_frac: f64,
    pub frame_at: Instant,
    pub frame_dur: Duration,
    hide_until: Option<Instant>,
    next_idle: Instant,
    pub menu_open: bool,
    hidden: bool,
    queue: VecDeque<(Action, Instant)>,
    pub pending: Vec<Effect>,
    /// Last (anim, frame) shown, so frame-carried voice clips fire exactly
    /// once per frame entry — this is how the original .acs audio refs work.
    seen: (String, usize),
    /// True while a Music action's background loop is running.
    pub music_on: bool,
}

fn pick<'a>(pool: &'a [&'a str], fallback: &'a str) -> &'a str {
    if pool.is_empty() { return fallback; }
    pool[rand::rng().random_range(0..pool.len())]
}

impl Pet {
    pub fn new() -> Pet {
        let now = Instant::now();
        Pet {
            anim: "IDLE1_1".into(),
            frame: 0,
            frame_at: now,
            frame_dur: Duration::from_millis(100),
            looping: false,
            loop_until: None,
            bubble: None,
            bubble_until: None,
            speaking: false,
            mouth: 0,
            mouth_at: now,
            walk_dir: 0,
            walk_until: None,
            last_tick: now,
            walk_frac: 0.0,
            next_idle: now + Duration::from_secs(12),
            menu_open: false,
            hidden: false,
            hide_until: None,
            queue: VecDeque::new(),
            pending: Vec::new(),
            seen: ("IDLE1_1".into(), 0),
            music_on: false,
        }
    }

    pub fn push(&mut self, action: Action) {
        self.queue.push_back((action, Instant::now()));
    }

    pub fn startup(&mut self, content: &Content) {
        self.pending.push(Effect::PlaySound("CHORD.WAV".into()));
        self.bubble_now(clean_tts(&Self::pick_text(&content.greetings)), 5.0);
        self.set_anim("GREET", false);
    }

    fn pick_text(pool: &Vec<String>) -> String {
        if pool.is_empty() {
            return "Hi! I'm Bonzi!".into();
        }
        pool[rand::rng().random_range(0..pool.len())].clone()
    }

    fn bubble_now(&mut self, text: String, secs: f64) {
        let secs = secs.min(14.0);
        self.bubble = Some(text);
        self.bubble_until = Some(Instant::now() + Duration::from_secs_f64(secs));
        self.speaking = true;
        // Every bubble is speech: TTS (when enabled) must voice all of them,
        // not just the Action::Speak path.
        self.pending
            .push(Effect::SpeakTts(self.bubble.clone().unwrap_or_default()));
    }

    pub fn say(&mut self, text: &str, secs: f64) {
        self.bubble_now(text.to_string(), secs);
    }

    pub fn trick(&mut self) {
        self.push(Action::Animate(pick(FUN_POOL, "GIGGLE").to_string()));
    }

    pub fn set_anim(&mut self, anim: &str, looping: bool) {
        self.anim = anim.to_string();
        self.frame = 0;
        self.frame_at = Instant::now();
        self.looping = looping;
    }

    pub fn is_idle(&self) -> bool {
        !self.speaking
            && self.queue.is_empty()
            && self.walk_dir == 0
            && !self.menu_open
            && !self.hidden
            && !self.looping
            && self.bubble.is_none()
    }

    pub fn click(&mut self, content: &Content) {
        self.menu_open = false;
        self.walk_dir = 0;
        let anim = pick(PETTED_POOL, "PLEASED");
        self.set_anim(anim, false);
        let line = Self::pick_text(&content.petting);
        self.pending.push(Effect::PlaySound("Snd1.wav".into()));
        self.bubble_now(line, 4.0);
    }

    pub fn tell_joke(&mut self, content: &Content) {
        let j = Self::pick_text(&content.jokes);
        self.push(Action::Sound("Snd2.wav".into()));
        self.push(Action::Tell(pick(TALK_POOL, "EXPLAIN").to_string(), j));
    }

    pub fn tell_fact(&mut self, content: &Content) {
        let f = Self::pick_text(&content.facts);
        self.push(Action::Sound("Snd2.wav".into()));
        self.push(Action::Tell(pick(TALK_POOL, "EXPLAIN2").to_string(), f));
    }

    pub fn sing(&mut self, content: &Content) {
        let s = Self::pick_text(&content.songs);
        self.push(Action::AnimateLoop("HEADPHONESCONTINUED".into(), 12_000));
        self.push(Action::Speak(s));
    }

    pub fn dance(&mut self) {
        // The real dance in the .acs is SUNGLASSESCONTINUED (shades on,
        // snapping, hip sways); HEADPHONESCONTINUED is just listening.
        // Music: the original dance MIDI was a paid "Bonzi Unlimited"
        // feature that never shipped in the payload — the intro jingle
        // loops in its place, offline and free.
        self.push(Action::AnimateLoop("SUNGLASSESCONTINUED".into(), 13_000));
        self.push(Action::Music("Intro2.wav".into()));
        self.push(Action::Speak("I just love to dance!".into()));
    }

    /// The original "What's the time?" — local clock, no network.
    pub fn tell_time(&mut self) {
        let (h, m) = local_hm();
        let ampm = if h >= 12 { "PM" } else { "AM" };
        let h12 = match h % 12 {
            0 => 12,
            x => x,
        };
        let t = format!("It's {}:{:02} {}!", h12, m, ampm);
        self.push(Action::Sound("Snd2.wav".into()));
        self.push(Action::Tell(pick(TALK_POOL, "EXPLAIN").to_string(), t));
    }

    pub fn start_walk(&mut self) {
        self.push(Action::Walk(if rand::rng().random_bool(0.5) { -1 } else { 1 }));
    }

    pub fn hide(&mut self) {
        self.push(Action::Hide);
    }

    fn hide_now(&mut self) {
        self.set_anim("HIDE", false);
        self.hidden = true;
        self.hide_until = Some(Instant::now() + Duration::from_secs(8));
    }

    /// Tray "Show": cancel the hidden state and play the reveal animation.
    pub fn reveal(&mut self) {
        self.hidden = false;
        self.hide_until = None;
        self.set_anim("HIDESHOW", false);
    }

    /// Advance animation/bubble/personality clocks.
    /// `audio_of(anim, frame)` maps a frame to its original voice clip.
    /// Returns (window_dx_from_walk, effects).
    pub fn tick(
        &mut self,
        has_anim: impl Fn(&str) -> bool,
        audio_of: impl Fn(&str, usize) -> Option<String>,
        content: &Content,
    ) -> (i32, Vec<Effect>) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64().clamp(0.0, 0.25);
        self.last_tick = now;
        let mut dx = 0i32;

        // drain queue (one action per tick keeps pacing natural)
        if let Some((action, _)) = self.queue.pop_front() {
            // A new action always cuts off whatever was playing before.
            self.pending.push(Effect::StopAll);
            match action {
                Action::Animate(a) if has_anim(&a) => { self.walk_dir = 0; self.set_anim(&a, false); }
                Action::AnimateLoop(a, ms) if has_anim(&a) => {
                    self.walk_dir = 0;
                    self.set_anim(&a, true);
                    self.loop_until = Some(now + Duration::from_millis(ms));
                }
                Action::Speak(t) => {
                    let d = 2.5 + t.len() as f64 * 0.05;
                    self.bubble_now(t, d);
                }
                Action::Tell(anim, t) => {
                    if has_anim(&anim) { self.set_anim(&anim, false); }
                    let d = 2.5 + t.len() as f64 * 0.05;
                    self.bubble_now(t, d);
                }
                Action::Sound(s) => self.pending.push(Effect::PlaySound(s)),
                Action::Music(name) => {
                    self.music_on = true;
                    self.pending.push(Effect::PlayMusic(name));
                }
                Action::Walk(d) => {
                    self.walk_dir = d.clamp(-1, 1);
                    if self.walk_dir != 0 {
                        self.walk_until = Some(now + Duration::from_secs(6));
                        let a = if self.walk_dir < 0 { "MOVELEFT" } else { "MOVERIGHT" };
                        if has_anim(a) { self.set_anim(a, true); }
                    }
                }
                Action::Hide => self.hide_now(),
                Action::Quit => self.pending.push(Effect::Quit),
                _ => {}
            }
        }

        // bubble expiry
        if let Some(until) = self.bubble_until {
            if now >= until {
                self.bubble = None;
                self.bubble_until = None;
                self.speaking = false;
                self.mouth = 0;
            }
        }
        // mouth flap cycle while speaking
        if self.speaking && now.duration_since(self.mouth_at).as_millis() > 110 {
            self.mouth_at = now;
            self.mouth = match self.mouth {
                1 => 3, 2 => 4, 3 => 2, _ => 1,
            };
        } else if !self.speaking {
            self.mouth = 0;
        }

        // walk movement
        if self.walk_dir != 0 {
            // 150 logical px/s at scale 1.0 ≈ one vine-swing cycle (1 s of
            // original 100 ms frames) per ~1.1 sprite widths.
            self.walk_frac += dt * 150.0;
            let whole = self.walk_frac.trunc() as i32;
            if whole != 0 {
                self.walk_frac -= whole as f64;
                dx = self.walk_dir as i32 * whole;
            }
            if let Some(until) = self.walk_until {
                if now >= until {
                    self.walk_dir = 0;
                    self.walk_until = None;
                    let ret = if self.anim.starts_with("MOVELEFT") { "MOVELEFTRETURN" } else { "MOVERIGHTRETURN" };
                    if has_anim(ret) { self.set_anim(ret, false); } else { self.set_anim("IDLE1_1", false); }
                }
            }
        }

        // hidden auto-return
        if self.hidden {
            if let Some(until) = self.hide_until {
                if now >= until {
                    self.hidden = false;
                    self.hide_until = None;
                    // the HIDESHOW animation is the reveal; window stays up
                    self.set_anim("HIDESHOW", false);
                }
            }
            return (0, std::mem::take(&mut self.pending));
        }

        // frame advance
        if now >= self.frame_at + self.frame_dur {
            self.frame += 1;
            // Advance by the scheduled duration (not `now`) so per-frame
            // timing tracks the original .acs schedule; resync only after a
            // long stall (hidden window, suspend).
            self.frame_at += self.frame_dur;
            if now.duration_since(self.frame_at) > 4 * self.frame_dur {
                self.frame_at = now;
            }
            if let Some(until) = self.loop_until {
                if now >= until {
                    self.loop_until = None;
                    self.looping = false;
                    if self.music_on {
                        self.music_on = false;
                        self.pending.push(Effect::StopMusic);
                    }
                    let ret = return_of(&self.anim);
                    self.frame = 0;
                    if !ret.is_empty() && has_anim(&ret) {
                        self.anim = ret;
                    } else {
                        self.anim = "IDLE1_1".into();
                    }
                }
            }
        }

        // Frame-carried voice clips: the original .acs stores a sound index
        // on specific frames (Bonzi says "Oh!" mid-HUG, whistles in idles…).
        // Fire once per (anim, frame) entry.
        if (self.anim.as_str(), self.frame) != (self.seen.0.as_str(), self.seen.1) {
            self.seen = (self.anim.clone(), self.frame);
            if let Some(w) = audio_of(&self.anim, self.frame) {
                self.pending.push(Effect::PlaySound(w));
            }
        }

        // idle personality engine
        if self.is_idle() && now >= self.next_idle {
            self.next_idle = now + Duration::from_secs(18 + rand::rng().random_range(0..30) as u64);
            let roll: u32 = rand::rng().random_range(0..100);
            match roll {
                0..=24 => self.tell_joke(content),
                25..=49 => self.tell_fact(content),
                50..=64 => { let a = pick(FUN_POOL, "GIGGLE"); if has_anim(a) { self.set_anim(a, false); } }
                65..=72 => self.dance(),
                73..=79 => self.sing(content),
                80..=89 => self.start_walk(),
                _ => {
                    let line = Self::pick_text(&content.greetings);
                    self.bubble_now(line, 4.5);
                }
            }
        }
        (dx, std::mem::take(&mut self.pending))
    }

    /// Called by main when the current animation's last frame ends.
    pub fn anim_finished(&mut self, has_anim: impl Fn(&str) -> bool, ret: Option<&str>) {
        self.frame = 0;
        if self.looping {
            return;
        }
        if let Some(r) = ret {
            if !r.is_empty() && has_anim(r) {
                self.anim = r.to_string();
                return;
            }
        }
        let r = return_of(&self.anim);
        if !r.is_empty() && has_anim(&r) {
            self.anim = r;
        } else {
            let a = pick(IDLE_POOL, "IDLE1_1");
            self.anim = a.to_string();
        }
    }

}

fn return_of(anim: &str) -> String {
    // ACS return animations are stored in animations.json; this is the
    // fallback naming convention from the original character. The
    // CONTINUED family returns to {base}RETURN (HEADPHONESCONTINUED →
    // HEADPHONESRETURN), not {base}CONTINUEDRETURN.
    if let Some(base) = anim.strip_suffix("CONTINUED") {
        return format!("{base}RETURN");
    }
    for suffix in ["LEFT", "RIGHT", "UP", "DOWN"] {
        if let Some(base) = anim.strip_suffix(suffix) {
            let cand = format!("{base}{suffix}RETURN");
            return cand;
        }
    }
    String::new()
}

fn local_hm() -> (u32, u32) {
    #[cfg(unix)]
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (tm.tm_hour as u32, tm.tm_min as u32)
    }
    #[cfg(windows)]
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_s(&mut tm, &t);
        (tm.tm_hour as u32, tm.tm_min as u32)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        ((secs / 3600 % 24) as u32, (secs / 60 % 60) as u32)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::Content;

    fn content() -> Content {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/content/nbd_decoded.json");
        Content::load(&path).expect("content loads")
    }

    #[test]
    fn every_bubble_voices_exactly_once() {
        let c = content();
        let mut p = Pet::new();
        p.pending.clear();
        p.tell_joke(&c);
        // drain the queue like the main loop does; tick() returns the effects
        let mut seen = Vec::new();
        while !p.queue.is_empty() {
            let (_, effects) = p.tick(|_| true, |_, _| None, &c);
            seen.extend(effects.into_iter().filter(|e| matches!(e, Effect::SpeakTts(_))));
        }
        assert_eq!(seen.len(), 1, "joke must voice exactly once");
        match &seen[0] {
            Effect::SpeakTts(t) => assert!(!t.is_empty() && !t.contains("spd="), "bad tts text: {t:?}"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn frame_voice_clips_fire_once_per_frame_entry() {
        let c = content();
        let mut p = Pet::new();
        p.pending.clear();
        p.frame_dur = Duration::ZERO; // advance one frame per tick
        p.push(Action::Animate("HUG".into()));
        let audio = |a: &str, f: usize| -> Option<String> {
            if a == "HUG" && f == 5 {
                Some("audio_008.wav".into())
            } else {
                None
            }
        };
        let mut fired: Vec<String> = Vec::new();
        let mut stop_all = 0;
        for _ in 0..10 {
            let (_, e) = p.tick(|_| true, audio, &c);
            for eff in e {
                match eff {
                    Effect::PlaySound(s) => fired.push(s),
                    Effect::StopAll => stop_all += 1,
                    _ => {}
                }
            }
        }
        assert_eq!(fired, vec!["audio_008.wav".to_string()], "clip fires exactly once");
        assert_eq!(stop_all, 1, "queued action cuts previous output once");
    }

    #[test]
    fn continued_loops_return_to_base_return() {
        assert_eq!(return_of("HEADPHONESCONTINUED"), "HEADPHONESRETURN");
        assert_eq!(return_of("READCONTINUED"), "READRETURN");
        assert_eq!(return_of("MOVELEFT"), "MOVELEFTRETURN");
        assert_eq!(return_of("IDLE1_1"), "");
    }

    /// The whole point of the voice-clip wiring is that no original sound
    /// is stranded: every animation carrying an audio ref must be reachable
    /// from a pool or a hardcoded trigger (walking, hide/show, sing),
    /// including via return-animation chains.
    #[test]
    fn every_audio_carrying_animation_is_reachable() {
        let raw: serde_json::Value = serde_json::from_slice(
            &std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/bonzi/animations.json"
            ))
            .expect("animations.json"),
        )
        .expect("valid json");
        let mut reach: std::collections::HashSet<String> = IDLE_POOL
            .iter()
            .chain(FUN_POOL.iter())
            .chain(PETTED_POOL.iter())
            .chain(TALK_POOL.iter())
            .map(|s| s.to_string())
            .collect();
        // Hardcoded triggers: hide/reveal, walking, sing's headphone loop.
        for a in [
            "HIDE",
            "HIDESHOW",
            "SHOW",
            "MOVELEFT",
            "MOVERIGHT",
            "MOVELEFTRETURN",
            "MOVERIGHTRETURN",
            "HEADPHONESCONTINUED",
        ] {
            reach.insert(a.to_string());
        }
        // Close over return animations (json-declared and naming convention).
        loop {
            let mut added = false;
            let mut add: Vec<String> = Vec::new();
            for a in &reach {
                if let Some(r) = raw.get(a).and_then(|v| v["return_anim"].as_str()) {
                    if !r.is_empty() && !reach.contains(r) {
                        add.push(r.to_string());
                    }
                }
                let r = return_of(a);
                if !r.is_empty() && !reach.contains(&r) {
                    add.push(r);
                }
            }
            for a in add {
                reach.insert(a);
                added = true;
            }
            if !added {
                break;
            }
        }
        let missing: Vec<String> = raw
            .as_object()
            .unwrap()
            .iter()
            .filter_map(|(k, v)| {
                let has_audio = v["frames"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|f| f["audio"].as_u64().unwrap_or(65535) != 65535);
                if has_audio && !reach.contains(k) {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .collect();
        assert!(missing.is_empty(), "audio-carrying animations never triggered: {missing:?}");
    }
}
