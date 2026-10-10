//! Offline content: jokes, facts, songs and chatter decoded from the original
//! BonziBuddy `.nbd` files (analysis/nbd "encryption": odd bytes + 1).
//!
//! The original TTS markup (`<sp=...>`, `\pau=2000\`, `<pl=...>`, `<chr=...>`)
//! is stripped to plain text for the speech bubble.
//!
//! NOTE: `sites.nbd`, `Daily.nbd`, `Browse.nbd` and `Search.nbd` are
//! deliberately NOT loaded — they carried ad links, phone-home content feeds
//! and the user-browsing/search spy logs. See README.

use serde_json::Value;

pub struct Content {
    pub jokes: Vec<String>,
    pub facts: Vec<String>,
    pub songs: Vec<String>,
    pub greetings: Vec<String>,
    pub petting: Vec<String>,
}

/// Strip Bonzi TTS markup to plain text.
/// `<sp=TEXT>` keeps TEXT (the utterance); other `<tag..>` cues are dropped.
/// Backslashes are FIELD DELIMITERS, not paired quotes — the original data
/// contains odd backslash counts, so any pair-based parse desynchronizes and
/// leaks `spd=`/`pit=` noise into speech bubbles. A delimited field is dropped
/// when it is a control (`pau=1500`, `spd=120`, `map=".."=".."`), the `emp`
/// marker, or whitespace; anything else is a pronunciation/emphasis word
/// (`\the\`, `\long\`) and is kept as text.
pub fn clean_tts(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    let mut in_sp = false;
    while let Some(c) = it.next() {
        match c {
            '>' if in_sp => in_sp = false,
            '<' => {
                let mut tag = String::new();
                while let Some(&c2) = it.peek() {
                    if c2 == '=' || c2 == '>' {
                        break;
                    }
                    tag.push(c2);
                    it.next();
                }
                if tag == "sp" {
                    if it.peek() == Some(&'=') {
                        it.next(); // keep the utterance body in the stream
                    }
                    in_sp = true;
                } else {
                    while let Some(&c2) = it.peek() {
                        it.next();
                        if c2 == '>' {
                            break;
                        }
                    }
                }
            }
            '\\' => {
                let mut field = String::new();
                while let Some(&c2) = it.peek() {
                    if c2 == '\\' {
                        break;
                    }
                    if c2 == '>' && in_sp {
                        break;
                    }
                    field.push(c2);
                    it.next();
                }
                // map="X"="Y" and friends: cut at the first quote.
                if let Some(q) = field.find('"') {
                    field.truncate(q);
                }
                let t = field.trim();
                let keep = !t.is_empty() && !field.contains('=') && t != "emp";
                if keep {
                    out.push(' ');
                    out.push_str(t);
                    out.push(' ');
                } else {
                    out.push(' ');
                }
            }
            _ => out.push(c),
        }
    }
    // collapse whitespace
    let mut res = String::with_capacity(out.len());
    let mut space = false;
    for c in out.trim().chars() {
        if c.is_whitespace() { space = true; continue; }
        if space && !res.is_empty() { res.push(' '); }
        space = false;
        res.push(c);
    }
    res
}

/// Name Bonzi uses for `[username]` placeholders in the original scripts.
/// Priority: `--name` flag (exported to `BONZI_NAME` by main), then the login
/// account name (title-cased), then "Friend".
pub fn user_name() -> String {
    if let Some(n) = std::env::var("BONZI_NAME").ok().filter(|s| !s.trim().is_empty()) {
        return n.trim().to_string();
    }
    #[cfg(unix)]
    let login = std::env::var("USER").ok();
    #[cfg(windows)]
    let login = std::env::var("USERNAME").ok();
    #[cfg(not(any(unix, windows)))]
    let login: Option<String> = None;
    login
        .filter(|s| !s.trim().is_empty() && s != "root")
        .map(|s| {
            let s = s.trim();
            let mut c = s.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .unwrap_or_else(|| "Friend".into())
}

impl Content {
    pub fn load(path: &std::path::Path) -> Result<Content, String> {
        let raw: Value = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let grab = |names: &[&str]| -> Vec<String> {
            let mut v = Vec::new();
            for n in names {
                if let Some(arr) = raw.get(*n).and_then(|x| x.as_array()) {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            // Entries may carry a literal "Header" marker prefix.
                            let s = s.strip_prefix("Header").unwrap_or(s);
                            let mut c = clean_tts(s);
                            if c.contains("[username]") {
                                c = c.replace("[username]", &user_name());
                            }
                            if c.len() > 8 && !c.contains("Header") { v.push(c); }
                        }
                    }
                }
            }
            v
        };
        let jokes = grab(&["j001.nbd", "j2.nbd", "j3.nbd"]);
        let facts = grab(&["t001.nbd", "t2.nbd", "t3.nbd"]);
        let songs = grab(&["s1.nbd"]);
        let greetings = vec![
            "Hi there! My name is Bonzi. I'm your new best friend!".into(),
            "Welcome to the world of BonziBUDDY!".into(),
            "I can tell jokes, sing songs, and read you stories!".into(),
            "Did you know I remember everything you tell me? Just kidding. Or am I?".into(),
            "Let's have some fun!".into(),
        ];
        let mut petting = vec![
            "Hehehe, that tickles!".into(),
            "I like you. You're fun to be around!".into(),
            "Do I have bananas on my breath?".into(),
            "Wheee!".into(),
            "You are my favorite human!".into(),
            "Ooh, that's the good spot.".into(),
        ];
        // p001.nbd is the original petting/personality script (uses [username]).
        petting.extend(grab(&["p001.nbd"]));
        Ok(Content { jokes, facts, songs, greetings, petting })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> Content {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/content/nbd_decoded.json");
        Content::load(&path).expect("content loads")
    }

    #[test]
    fn pools_populated() {
        let c = content();
        assert!(c.jokes.len() > 100, "jokes: {}", c.jokes.len());
        assert!(c.facts.len() > 100, "facts: {}", c.facts.len());
        assert!(!c.songs.is_empty(), "songs empty");
    }

    #[test]
    fn no_markup_leaks() {
        let c = content();
        let all = c
            .jokes
            .iter()
            .chain(c.facts.iter())
            .chain(c.songs.iter())
            .chain(c.greetings.iter())
            .chain(c.petting.iter());
        for text in all {
            assert!(!text.contains('<') && !text.contains('\\'), "markup in: {text:?}");
            assert!(!text.contains("Header"), "header in: {text:?}");
            assert!(
                !text.contains("spd=") && !text.contains("pit=") && !text.contains("pau=") && !text.contains("map="),
                "tts control leaked: {text:?}"
            );
            assert!(!text.trim().is_empty());
        }
    }

    #[test]
    fn clean_tts_cases() {
        assert_eq!(
            clean_tts("<sp=Why did the chicken cross the road? \\pau=2000\\ To get to the other side!>"),
            "Why did the chicken cross the road? To get to the other side!"
        );
        assert_eq!(clean_tts("<pl=Blink>"), "");
        assert_eq!(
            clean_tts("<sp=The word \\emp\\set has many meanings>"),
            "The word set has many meanings"
        );
    }

    #[test]
    fn username_substituted() {
        std::env::set_var("BONZI_NAME", "Zoe");
        let c = content();
        let with = c
            .petting
            .iter()
            .chain(c.greetings.iter())
            .find(|s| s.contains("Zoe"));
        assert!(with.is_some(), "no line addressed to Zoe");
        let all = c
            .jokes
            .iter()
            .chain(c.facts.iter())
            .chain(c.songs.iter())
            .chain(c.greetings.iter())
            .chain(c.petting.iter());
        for text in all {
            assert!(!text.contains("[username]"), "placeholder left in: {text:?}");
        }
    }
}

