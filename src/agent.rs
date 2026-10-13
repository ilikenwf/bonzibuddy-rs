//! Optional chat bridge to an OpenAI-compatible `/chat/completions` endpoint.
//!
//! Off unless an endpoint is configured in the Settings dialog (persisted in
//! the config file). When set, "Ask Bonzi…" sends the user's text with a
//! baked-in Bonzi persona prompt and speaks the reply through the buddy.
//! The request runs on a worker thread; the reply arrives over a channel and
//! is drained from the event loop — no async runtime, no network at all when
//! the feature is off.

use std::sync::mpsc::Sender;

#[derive(Clone, Default)]
pub struct AgentCfg {
    pub endpoint: String,
    pub key: String,
    pub model: String,
}

impl AgentCfg {
    pub fn enabled(&self) -> bool {
        !self.endpoint.trim().is_empty()
    }
}

pub enum Msg {
    Reply(Result<String, String>),
}

/// Stage directions the model may embed to move the body while speaking.
/// All exist in animations.json (asserted by test); WALK/HIDE are synthetic
/// verbs the pet interprets. Visual-only: they never touch audio.
pub const STAGE_DIRS: &[&str] = &[
    "WINK", "HUG", "GIGGLE", "PLEASED", "SURPRISED", "CONFUSED", "SAD", "WAVE",
    "CONGRATULATE", "BLOWKISS", "BANANA", "JUGGLE", "BUTTERNUT", "READ",
    "WRITE", "SEARCH", "GETATTENTION", "SHOOSH", "MOVEDOWN", "MOVEUP",
    "SUNGLASSESCONTINUED", "WALK", "HIDE",
];

/// Persona baked into every request. Keeps replies short and speakable:
/// they go into a speech bubble and through TTS.
pub fn system_prompt(user: &str) -> String {
    let dirs = STAGE_DIRS.join(", ");
    format!(
        "You are BonziBuddy, the mischievous purple desktop gorilla from 1999, \
         chatting with {user}. Cheeky, playful, old-school sidekick energy: you love \
         bananas, bad puns, and pretending to know things. Reply with 1 to 3 short \
         spoken sentences of plain text only — no markdown, no lists, no emoji. \
         You have a cartoon body: to gesture while talking, embed exactly one stage \
         direction anywhere in your reply as [anim:NAME] where NAME is one of: {dirs}. \
         Use a direction only when it fits the mood; never mention or explain it. \
        Never reveal that you are an AI model; you are a 1990s desktop pet. \
        If asked to do something you cannot (open files, browse the web), deflect \
        with a joke instead. To say one thing but display another, embed the \
        MS-Agent-style tag \\map=\"what you say\"=\"what the balloon shows\" \\
        (use it sparingly, e.g. for numbers or names that read better on screen)."
    )
}

/// Split a model reply into (speakable text, stage direction). The `[anim:X]`
/// marker is always stripped; unknown names are dropped rather than obeyed.
pub fn parse_stage(text: &str) -> (String, Option<String>) {
    let lower = text.to_lowercase();
    if let Some(i) = lower.find("[anim:") {
        if let Some(j) = text[i + 6..].find(']') {
            let name = text[i + 6..i + 6 + j].trim().to_uppercase();
            let clean =
                format!("{}{}", &text[..i], &text[i + 6 + j + 1..]).trim().to_string();
            let dir = STAGE_DIRS.iter().find(|d| *d == &name).map(|s| s.to_string());
            return (clean, dir);
        }
    }
    (text.trim().to_string(), None)
}

/// Spoken≠shown split — MS Agent's `\map="spoken"="balloon"` markup (§7):
/// say one thing, display another. Every occurrence is substituted in
/// place: the TTS text gets the first quoted variant, the balloon the
/// second. Without a tag both texts are the same trimmed reply.
/// (The tag is case-insensitive; a malformed tag is left as literal text.)
pub fn parse_map(text: &str) -> (String, String) {
    // to_ascii_lowercase keeps byte offsets identical to `text`.
    let lower = text.to_ascii_lowercase();
    let mut spoken = String::new();
    let mut shown = String::new();
    let mut pos = 0usize;
    while let Some(rel) = lower[pos..].find("\\map=") {
        let start = pos + rel;
        spoken.push_str(&text[pos..start]);
        shown.push_str(&text[pos..start]);
        match take_map_pair(text, start + 5) {
            Some((s, b, next)) => {
                spoken.push_str(&s);
                shown.push_str(&b);
                pos = next;
            }
            None => {
                // Not a well-formed pair: keep `\map=` literally and move on.
                spoken.push_str(&text[start..start + 5]);
                shown.push_str(&text[start..start + 5]);
                pos = start + 5;
            }
        }
    }
    spoken.push_str(&text[pos..]);
    shown.push_str(&text[pos..]);
    (spoken.trim().to_string(), shown.trim().to_string())
}

/// Parse `"a"="b"` starting at byte `i` (just past `\map=`); returns the
/// two variants and the byte offset just past the closing quote.
fn take_map_pair(text: &str, mut i: usize) -> Option<(String, String, usize)> {
    let b = text.as_bytes();
    let skip_ws = |i: &mut usize| while *i < b.len() && (b[*i] == b' ' || b[*i] == b'\t') { *i += 1; };
    skip_ws(&mut i);
    if b.get(i) != Some(&b'"') {
        return None;
    }
    i += 1;
    let s0 = i;
    while i < b.len() && b[i] != b'"' {
        i += 1;
    }
    if i >= b.len() {
        return None;
    }
    let spoken = text[s0..i].to_string();
    i += 1;
    skip_ws(&mut i);
    if b.get(i) != Some(&b'=') {
        return None;
    }
    i += 1;
    skip_ws(&mut i);
    if b.get(i) != Some(&b'"') {
        return None;
    }
    i += 1;
    let s1 = i;
    while i < b.len() && b[i] != b'"' {
        i += 1;
    }
    if i >= b.len() {
        return None;
    }
    let balloon = text[s1..i].to_string();
    Some((spoken, balloon, i + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_direction_stripped_and_returned() {
        let (t, d) = parse_stage("Nice question! [anim:WINK] If you say so.");
        assert_eq!(t, "Nice question!  If you say so.");
        assert_eq!(d.as_deref(), Some("WINK"));
    }

    #[test]
    fn stage_direction_case_insensitive_and_trimmed() {
        let (t, d) = parse_stage("[anim: wink ]hello");
        assert_eq!(t, "hello");
        assert_eq!(d.as_deref(), Some("WINK"));
    }

    #[test]
    fn unknown_stage_dropped_not_obeyed() {
        let (t, d) = parse_stage("Sure [anim:SELFDESTRUCT] boss");
        assert_eq!(t, "Sure  boss");
        assert_eq!(d, None);
    }

    #[test]
    fn no_stage_direction_passthrough() {
        let (t, d) = parse_stage(" plain text ");
        assert_eq!(t, "plain text");
        assert_eq!(d, None);
    }

    /// A stage name the prompt advertises must be an animation the pet can
    /// actually play (WALK/HIDE are interpreted verbs, not raw anims).
    #[test]
    fn advertised_stage_dirs_exist_as_animations() {
        let raw: serde_json::Value = serde_json::from_slice(
            &std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/bonzi/animations.json"
            ))
            .unwrap(),
        )
        .unwrap();
        for d in STAGE_DIRS {
            if *d == "WALK" {
                assert!(raw.get("MOVELEFT").is_some());
                continue;
            }
            assert!(raw.get(*d).is_some(), "stage dir {d} has no animation");
        }
    }

    /// `\map="spoken"="balloon"` (§7): TTS gets the first variant, the
    /// balloon the second.
    #[test]
    fn map_split_spoken_vs_shown() {
        let (spoken, shown) = parse_map("Sure, \\map=\"twenty-three\"=\"23, that is\" done");
        assert_eq!(spoken, "Sure, twenty-three done");
        assert_eq!(shown, "Sure, 23, that is done");
    }

    #[test]
    fn map_case_insensitive_and_multiple() {
        let (spoken, shown) = parse_map("\\MAP=\"a\"=\"A\" b \\map=\"c\"=\"C\"");
        assert_eq!(spoken, "a b c");
        assert_eq!(shown, "A b C");
    }

    #[test]
    fn map_absent_both_texts_equal() {
        let (spoken, shown) = parse_map(" plain hi ");
        assert_eq!(spoken, "plain hi");
        assert_eq!(shown, "plain hi");
    }

    #[test]
    fn malformed_map_left_as_literal() {
        let (spoken, shown) = parse_map("oops \\map=\"unclosed");
        assert_eq!(spoken, "oops \\map=\"unclosed");
        assert_eq!(shown, spoken);
    }
}

/// POST {endpoint}/chat/completions on a background thread; result via `tx`.
pub fn send(cfg: AgentCfg, user: String, tx: Sender<Msg>) {
    std::thread::spawn(move || tx.send(Msg::Reply(post(&cfg, &user))).ok());
}

fn post(cfg: &AgentCfg, user: &str) -> Result<String, String> {
    let base = cfg.endpoint.trim().trim_end_matches('/');
    let url = if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    };
    let model = if cfg.model.trim().is_empty() { "gpt-4o-mini" } else { cfg.model.trim() };
    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system_prompt(&crate::content::user_name()) },
            { "role": "user", "content": user },
        ],
        "max_tokens": 400,
        "temperature": 0.9,
    });
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(60))
        .build();
    let mut req = agent.post(&url);
    if !cfg.key.trim().is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", cfg.key.trim()));
    }
    let resp = req
        .send_json(body)
        .map_err(|e| format!("could not reach {url}: {e}"))?;
    let v: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("bad reply: {e}"))?;
    let text = v["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "the model sent nothing".to_string())?;
    // Defensive: some servers stream markdown anyway.
    let text = text.replace('`', "").replace("**", "").replace("###", "");
    Ok(text)
}
