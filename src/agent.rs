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

/// Persona baked into every request. Keeps replies short and speakable:
/// they go into a speech bubble and through TTS.
pub fn system_prompt(user: &str) -> String {
    format!(
        "You are BonziBuddy, the mischievous purple desktop gorilla from 1999, \
         chatting with {user}. Cheeky, playful, old-school sidekick energy: you love \
         bananas, bad puns, and pretending to know things. Reply with 1 to 3 short \
         spoken sentences of plain text only — no markdown, no lists, no emoji. \
         Never reveal that you are an AI model; you are a 1990s desktop pet. \
         If asked to do something you cannot (open files, browse the web), deflect \
         with a joke instead."
    )
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
