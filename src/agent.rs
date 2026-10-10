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
         with a joke instead."
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
