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
    /// Play a wav from assets (sfx/ or bonzi/audio/).
    Sound(String),
    /// Walk: -1 left, +1 right, 0 stop.
    Walk(i8),
    Hide,
    Quit,
}

#[derive(Clone, Debug)]
pub enum Effect {
    PlaySound(String),
    SpeakTts(String),
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
    "BANANA", "JUGGLE", "BUTTERNUT", "GIGGLE", "DOMAGIC1", "DOMAGIC2", "WINK", "BLOWKISS",
    "HUG", "SURPRISED", "UNBELIEVABLE", "CONFUSED", "SAD", "WAVE", "CONGRATULATE", "SHOOSH",
];
const PETTED_POOL: &[&str] = &["PLEASED", "PLEASEDSOFT", "PLEASEDHARD", "GIGGLE", "WINK", "HUG"];
const TALK_POOL: &[&str] = &["EXPLAIN", "EXPLAIN2", "EXPLAIN3", "EXPLAIN4", "GREET", "SUGGEST"];

pub struct Pet {
    pub anim: String,
    pub frame: usize,
    pub looping: bool,
    loop_until: Option<Instant>,
    pub bubble: Option<String>,
    bubble_until: Option<Instant>,
    pub speaking: bool,
    pub mouth: u8,
    mouth_at: Instant,
    pub walk_dir: i8,
    walk_until: Option<Instant>,
    pub frame_at: Instant,
    pub frame_dur: Duration,
    hide_until: Option<Instant>,
    next_idle: Instant,
    pub menu_open: bool,
    hidden: bool,
    queue: VecDeque<(Action, Instant)>,
    pub pending: Vec<Effect>,
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
            next_idle: now + Duration::from_secs(12),
            menu_open: false,
            hidden: false,
            hide_until: None,
            queue: VecDeque::new(),
            pending: Vec::new(),
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
        self.push(Action::AnimateLoop("HEADPHONESCONTINUED".into(), 9_000));
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

    /// Advance animation/bubble/personality clocks.
    /// Returns (window_dx_from_walk, effects).
    pub fn tick(&mut self, has_anim: impl Fn(&str) -> bool, content: &Content) -> (i32, Vec<Effect>) {
        let now = Instant::now();
        let mut dx = 0i32;

        // drain queue (one action per tick keeps pacing natural)
        if let Some((action, _)) = self.queue.pop_front() {
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
                    self.pending.push(Effect::SpeakTts(self.bubble.clone().unwrap_or_default()));
                }
                Action::Tell(anim, t) => {
                    if has_anim(&anim) { self.set_anim(&anim, false); }
                    let d = 2.5 + t.len() as f64 * 0.05;
                    self.bubble_now(t, d);
                }
                Action::Sound(s) => self.pending.push(Effect::PlaySound(s)),
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
            dx = self.walk_dir as i32 * 2; // px per tick (~60fps) ≈ 120 px/s
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
            self.frame_at = now;
            // duration set by caller via frame_dur hook; default handled in main
            let n = self.frame;
            if let Some(until) = self.loop_until {
                if now >= until {
                    self.loop_until = None;
                    self.looping = false;
                    let ret = return_of(&self.anim);
                    self.frame = 0;
                    if !ret.is_empty() && has_anim(&ret) {
                        self.anim = ret;
                    } else {
                        self.anim = "IDLE1_1".into();
                    }
                }
            }
            let _ = n;
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
    // fallback naming convention from the original character.
    for suffix in ["CONTINUED", "LEFT", "RIGHT", "UP", "DOWN"] {
        if let Some(base) = anim.strip_suffix(suffix) {
            let cand = format!("{base}{suffix}RETURN");
            return cand;
        }
    }
    String::new()
}

