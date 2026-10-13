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
    /// Choreographed one-shot sequence: each animation plays to its end,
    /// then the next starts; the last one returns to idle normally.
    DanceSeq(Vec<String>),
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
    Quit,
}

// Progressive idle pools — MS Agent §3.1: IdlingLevel1 → 2 → 3, each level
// a random pick from its own list, BLINK present in L1+L2 like Merlin's
// STATEINFO. Design guidance: L1 = blink/glance, L2 = reading/curious,
// L3 = sleeping. Asset limitation: Bonzi's extracted .acs has no IDLE2_*
// animations (Merlin's L2 = Idle2_* + Blink + Idle1_*), so L2 reuses the
// calm L1 subset plus the quiet thinking animations as the "reading" tier.
const IDLE1_POOL: &[&str] = &[
    "IDLE1_1", "IDLE1_3", "IDLE1_4", "IDLE1_5", "IDLE1_6", "IDLE1_7", "IDLE1_8", "IDLE1_9",
    "IDLE1_11", "IDLE1_12", "IDLE1_13", "IDLE1_14", "IDLE1_15", "IDLE1_21", "IDLE1_24",
    "IDLE1_25", "IDLE1_26", "BLINK", "BLINK", "LOOKLEFT", "LOOKRIGHT", "LOOKUP", "LOOKDOWN",
    "HEARING1", "HEARING2", "HEARING3", "RESTPOSE",
];
const IDLE2_POOL: &[&str] = &[
    "BLINK", "BLINK", "IDLE1_21", "IDLE1_24", "IDLE1_25", "IDLE1_26", "THINKING",
    "PROCESSING", "HANDSBEHIND", "SUGGEST",
];
const IDLE3_POOL: &[&str] = &["IDLE3_1", "IDLE3_2", "IDLE3_3", "BLINK"];
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
    /// Debug flag: --scene NAME suppresses the idle personality engine so
    /// the locked animation keeps looping for inspection.
    pub scene_lock: bool,
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
    /// Progressive idle level (MS Agent IdlingLevel1→2→3, §3.1): advanced
    /// per idle fire, reset by any interaction.
    pub idle_level: u8,
    /// Original `tmrCharacterAction_Timer` fired actions only while the
    /// user was idle (time since last mouse/keyboard input, BONZI432_RE.md
    /// §2). Maintained by main from the X11 MIT-SCREEN-SAVER extension;
    /// platforms without a query default to true.
    user_idle: bool,
    pub menu_open: bool,
    hidden: bool,
    queue: VecDeque<(Action, Instant)>,
    pub pending: Vec<Effect>,
    /// Last (anim, frame) shown, so frame-carried voice clips fire exactly
    /// once per frame entry — this is how the original .acs audio refs work.
    seen: (String, usize),
    /// True while a Music action's background loop is running.
    pub music_on: bool,
    /// Remaining animations of a DanceSeq, played one per anim_finished.
    dance_q: VecDeque<String>,
    /// voice clips and idle gags are suppressed so nothing cuts off the
    /// answer being spoken. User menu actions still interrupt (user wins).
    chat_lock: Option<Instant>,
}

fn idle_pool(level: u8) -> &'static [&'static str] {
    match level {
        1 => IDLE1_POOL,
        2 => IDLE2_POOL,
        _ => IDLE3_POOL,
    }
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
            scene_lock: false,
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
            idle_level: 1,
            user_idle: true,
            menu_open: false,
            hidden: false,
            hide_until: None,
            queue: VecDeque::new(),
            pending: Vec::new(),
            seen: ("IDLE1_1".into(), 0),
            music_on: false,
            dance_q: VecDeque::new(),
            chat_lock: None,
        }
    }

    pub fn push(&mut self, action: Action) {
        // Any new request cancels the idle cycle (MS Agent §3.1: the idle
        // levels persist "until canceled by any new animation request").
        self.idle_level = 1;
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

    /// An agent answer: highest-priority speech. Drops any queued gag so a
    /// half-queued joke can't cut the reply off, shows the full answer (no
    /// 14 s bubble cap), and locks out idle chatter until it has been read
    /// and spoken.
    pub fn chat_reply(&mut self, text: &str) {
        // An agent answer is activity: the idle cycle resets to level 1.
        self.idle_level = 1;
        self.queue.clear();
        self.dance_q.clear();
        self.walk_dir = 0;
        let secs = (4.0 + text.chars().count() as f64 * 0.09).min(60.0);
        self.bubble = Some(text.to_string());
        self.bubble_until = Some(Instant::now() + Duration::from_secs_f64(secs));
        self.speaking = true;
        self.pending.push(Effect::SpeakTts(text.to_string()));
        // Lock a little past the bubble: TTS usually outlives the text timer.
        self.chat_lock = Some(Instant::now() + Duration::from_secs_f64(secs + 2.0));
    }

    /// Apply a model-requested stage direction (`[anim:NAME]`). Visual only
    /// — never touches audio, so it cannot cut off the reply being spoken.
    pub fn stage(&mut self, name: &str) {
        match name {
            "WALK" => {
                self.walk_dir = if rand::rng().random_bool(0.5) { -1 } else { 1 };
                self.walk_until = Some(Instant::now() + Duration::from_secs(6));
                let a = if self.walk_dir < 0 { "MOVELEFT" } else { "MOVERIGHT" };
                self.set_anim(a, true);
            }
            "HIDE" => self.hide_now(),
            other => {
                self.walk_dir = 0;
                self.set_anim(other, false);
            }
        }
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

    /// Idle-gate input from the host (see `user_idle`). A user becoming
    /// active again is an interaction: the progressive idle level resets so
    /// the next idle period starts back at IdlingLevel1 (MS Agent §3.1).
    pub fn set_user_idle(&mut self, idle: bool) {
        if self.user_idle && !idle {
            self.idle_level = 1;
        }
        self.user_idle = idle;
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
        // No dedicated dance animation exists in any free Bonzi payload
        // (checked 4.3.2 cab, MSAgent .acs, surfing variant — the paid
        // "dance to MIDI" content never shipped). So choreograph one from
        // the original moves: jumps and crouches (whistle on every beat),
        // giggles, bounces, waves — over the intro jingle, offline & free.
        let seq = [
            "MOVEUP", "MOVEDOWN", "GIGGLE", "MOVEUP", "PLEASEDHARD", "MOVEDOWN",
            "WAVE", "GIGGLE", "MOVEUP", "PLEASEDHARD", "MOVEDOWN", "WINK", "GIGGLE",
        ];
        self.push(Action::DanceSeq(
            seq.iter().map(|s| s.to_string()).collect(),
        ));
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
                Action::DanceSeq(seq) => {
                    self.walk_dir = 0;
                    self.loop_until = None;
                    self.dance_q = seq.into_iter().filter(|a| has_anim(a)).collect();
                    if let Some(first) = self.dance_q.pop_front() {
                        self.set_anim(&first, false);
                    }
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

        // Chat reply priority: expire the lock, then stay silent while held.
        if self.chat_lock.is_some_and(|u| now >= u) {
            self.chat_lock = None;
        }
        let chat_locked = self.chat_lock.is_some();

        // Frame-carried voice clips: the original .acs stores a sound index
        // on specific frames (Bonzi says "Oh!" mid-HUG, whistles in idles…).
        // Fire once per (anim, frame) entry; `seen` still advances while
        // chat-locked so clips never fire retroactively afterwards.
        if (self.anim.as_str(), self.frame) != (self.seen.0.as_str(), self.seen.1) {
            self.seen = (self.anim.clone(), self.frame);
            if !chat_locked {
                if let Some(w) = audio_of(&self.anim, self.frame) {
                    self.pending.push(Effect::PlaySound(w));
                }
            }
        }

        // Idle personality engine — port of `tmrCharacterAction_Timer` +
        // `RandomActionEvent` (BONZI432_RE.md §2): the countdown re-arms to
        // 5-10 ticks (~1 s each) after each action; at zero an action fires
        // only while the USER is idle, otherwise it re-checks next tick.
        // The draw is `Int(8*Rnd+1)`: six cases land on silent random-idle
        // playback (slot 0x960 arg2), case 3 is the "Search" gag (Play
        // Search + Speak a fact), case 5 the "Explain" gag (Play Explain +
        // Speak a joke). Silent while a chat reply owns the voice.
        if !chat_locked && !self.scene_lock && self.is_idle() && now >= self.next_idle {
            if self.user_idle {
                self.next_idle =
                    now + Duration::from_secs(5 + rand::rng().random_range(0..6) as u64);
                let lvl = self.idle_level;
                match rand::rng().random_range(1..=8u32) {
                    3 => self.tell_fact(content),
                    5 => self.tell_joke(content),
                    _ => {
                        // Random idle playback: mostly calm idles, sometimes
                        // a fun gag; the clone adds the walk among these
                        // draws (the original's walking lived in MS Agent).
                        if rand::rng().random_bool(0.15) {
                            self.start_walk();
                        } else {
                            let pool: &[&str] = if rand::rng().random_bool(0.75) {
                                idle_pool(self.idle_level)
                            } else {
                                FUN_POOL
                            };
                            let a = pick(pool, "IDLE1_1");
                            if has_anim(a) {
                                self.set_anim(a, false);
                            }
                        }
                    }
                }
                // Progressive idle (MS Agent §3.1): this period played at the
                // current level; the next period escalates L1→L2→L3 and stays
                // at L3 until an *external* interaction resets the counter.
                // Captured `lvl` so the draw's own push() (joke/fact gags)
                // doesn't count as that interaction.
                self.idle_level = (lvl + 1).min(3);
            } else {
                self.next_idle = now + Duration::from_secs(1);
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
        // Choreographed dance: next move wins over the normal return.
        if let Some(next) = self.dance_q.pop_front() {
            self.set_anim(&next, false);
            return;
        }
        if self.music_on {
            // A dance sequence just ended: take the music off with it.
            self.music_on = false;
            self.pending.push(Effect::StopMusic);
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
            let a = pick(idle_pool(self.idle_level), "IDLE1_1");
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

    /// A chat reply owns the voice: queued gags are dropped, frame-carried
    /// clips stay silent, and the reply bubble survives.
    #[test]
    fn chat_reply_owns_the_voice() {
        let c = content();
        let mut p = Pet::new();
        p.pending.clear();
        p.frame_dur = Duration::ZERO;
        p.push(Action::Animate("HUG".into())); // would cut the reply off
        p.chat_reply("the model's fine answer");
        p.pending.clear(); // main dispatches the reply's own SpeakTts here
        assert!(p.queue.is_empty(), "queued gag must be dropped");
        assert_eq!(p.bubble.as_deref(), Some("the model's fine answer"));
        let audio = |_a: &str, f: usize| -> Option<String> {
            if f == 5 {
                Some("audio_008.wav".into())
            } else {
                None
            }
        };
        // Many frames past the clip's trigger frame: nothing may fire.
        let mut noisy = Vec::new();
        for _ in 0..40 {
            let (_, e) = p.tick(|_| true, audio, &c);
            noisy.extend(e.into_iter().filter(|e| {
                matches!(e, Effect::PlaySound(_) | Effect::SpeakTts(_) | Effect::StopAll)
            }));
        }
        assert!(noisy.is_empty(), "chat lock must silence automatic audio: {noisy:?}");
        assert_eq!(p.bubble.as_deref(), Some("the model's fine answer"), "bubble intact");
    }

    #[test]
    fn stage_directions_are_visual_only() {
        let mut p = Pet::new();
        p.pending.clear();
        p.stage("WINK");
        assert_eq!(p.anim, "WINK");
        p.stage("WALK");
        assert_ne!(p.walk_dir, 0);
        assert!(p.pending.is_empty(), "stage must not queue audio effects");
        p.stage("HIDE");
        assert!(p.hidden);
    }

    /// The choreographed dance must chain every move and stop the music
    /// when the last one finishes.
    #[test]
    fn dance_sequence_chains_and_stops_music() {
        let c = content();
        let mut p = Pet::new();
        p.dance();
        // Drain the three queued actions (DanceSeq, Music, Speak).
        for _ in 0..3 {
            p.tick(|_| true, |_, _| None, &c);
        }
        assert_eq!(p.anim, "MOVEUP", "first move starts");
        assert!(p.music_on);
        let n = p.dance_q.len();
        // One finish per move: the last one (empty queue) ends the music.
        for _ in 0..n + 1 {
            p.anim_finished(|_| true, None);
        }
        assert_eq!(p.dance_q.len(), 0);
        assert!(!p.music_on, "music stops with the last move");
        assert!(p.pending.iter().any(|e| matches!(e, Effect::StopMusic)));
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
        let mut reach: std::collections::HashSet<String> = IDLE1_POOL
            .iter()
            .copied()
            .chain(IDLE2_POOL.iter().copied())
            .chain(IDLE3_POOL.iter().copied())
            .chain(FUN_POOL.iter().copied())
            .chain(PETTED_POOL.iter().copied())
            .chain(TALK_POOL.iter().copied())
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

    #[test]
    fn scheduler_respects_user_idle_gate() {
        let c = content();
        let mut p = Pet::new();
        p.pending.clear();
        // User is active: the expiring countdown must NOT fire an action;
        // it re-arms a ~1 s re-check (tmrCharacterAction idle gate).
        p.set_user_idle(false);
        p.next_idle = Instant::now() - Duration::from_millis(1);
        let (_, e) = p.tick(|_| true, |_, _| None, &c);
        assert!(e.is_empty() && p.queue.is_empty(), "no action while the user is active");
        let recheck = p.next_idle.saturating_duration_since(Instant::now());
        assert!(recheck < Duration::from_secs(3), "active gate re-checks quickly: {recheck:?}");
        // User idle: every expired countdown performs a draw, proven by the
        // 5-10 tick re-arm (a blocked gate would re-arm ~1 s instead).
        p.set_user_idle(true);
        for i in 0..20 {
            p.next_idle = Instant::now() - Duration::from_millis(1);
            p.queue.clear();
            p.walk_dir = 0;
            p.bubble = None;
            p.speaking = false;
            p.looping = false;
            p.tick(|_| true, |_, _| None, &c);
            let rearm = p.next_idle.saturating_duration_since(Instant::now());
            assert!(
                rearm > Duration::from_secs(4) && rearm < Duration::from_secs(11),
                "draw {i} must re-arm 5-10 ticks, got {rearm:?}"
            );
        }
    }

    /// Progressive idle levels (MS Agent §3.1): each idle period escalates
    /// L1 → L2 → L3, L3 persists until canceled, and any interaction —
    /// a queued request or the user becoming active again — resets to L1.
    #[test]
    fn idle_levels_escalate_and_reset_on_interaction() {
        let c = content();
        let mut p = Pet::new();
        assert_eq!(p.idle_level, 1);
        let fire = |p: &mut Pet| {
            p.next_idle = Instant::now() - Duration::from_millis(1);
            p.queue.clear();
            p.walk_dir = 0;
            p.bubble = None;
            p.speaking = false;
            p.looping = false;
            p.tick(|_| true, |_, _| None, &c);
        };
        fire(&mut p);
        assert_eq!(p.idle_level, 2, "second idle period escalates to L2");
        fire(&mut p);
        assert_eq!(p.idle_level, 3, "third period escalates to L3");
        fire(&mut p);
        fire(&mut p);
        assert_eq!(p.idle_level, 3, "L3 persists until canceled");
        // Any new request cancels the cycle (reset to L1).
        p.push(Action::Animate("WINK".into()));
        assert_eq!(p.idle_level, 1, "push resets the idle level");
        fire(&mut p);
        assert_eq!(p.idle_level, 2);
        // User becoming active is an interaction too.
        p.set_user_idle(false);
        assert_eq!(p.idle_level, 1, "user activity resets the idle level");
        // Returning to idle starts the cycle again from L1; staying idle
        // (true→true) must not reset an escalating cycle.
        p.set_user_idle(true);
        fire(&mut p);
        assert_eq!(p.idle_level, 2);
        p.set_user_idle(true);
        assert_eq!(p.idle_level, 2, "no spurious reset while idle state holds");
    }

    /// Pool shape: BLINK in L1+L2 like Merlin's STATEINFO, L3 is the sleepy
    /// tier only, and the level lookup saturates at L3.
    #[test]
    fn idle_pools_are_progressive() {
        assert!(IDLE1_POOL.contains(&"BLINK"), "L1 has blink");
        assert!(IDLE2_POOL.contains(&"BLINK"), "L2 has blink (Merlin)");
        assert!(
            IDLE3_POOL.iter().all(|a| a.starts_with("IDLE3") || *a == "BLINK"),
            "L3 is the sleepy tier"
        );
        assert!(
            IDLE1_POOL.iter().all(|a| a.starts_with("IDLE1")
                || matches!(
                    *a,
                    "BLINK" | "LOOKLEFT" | "LOOKRIGHT" | "LOOKUP" | "LOOKDOWN"
                        | "HEARING1" | "HEARING2" | "HEARING3" | "RESTPOSE"
                )),
            "L1 is blink/glance tier"
        );
        assert_eq!(idle_pool(1), IDLE1_POOL);
        assert_eq!(idle_pool(2), IDLE2_POOL);
        assert_eq!(idle_pool(3), IDLE3_POOL);
        assert_eq!(idle_pool(9), IDLE3_POOL, "level lookup saturates at L3");
    }
}
