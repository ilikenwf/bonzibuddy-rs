//! Asset resolution and lazy loading.
//!
//! Assets live in a directory (`assets/`) shipped next to the binary.
//! Search order: `$BONZI_ASSETS`, `./assets`, `../assets`, `<exe_dir>/assets`,
//! `<exe_dir>/../assets`, `/usr/share/bonzibuddy-rs/assets`.

use ab_glyph::FontArc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub struct Assets {
    root: PathBuf,
    pub anims: HashMap<String, Animation>,
    pub char_w: u32,
    pub char_h: u32,
    pub font: FontArc,
    frame_cache: HashMap<String, Arc<Vec<Frame>>>,
    mouth_cache: HashMap<String, Arc<Frame>>,
    #[cfg(feature = "audio")]
    sfx_cache: HashMap<String, Arc<Vec<u8>>>,
}

pub struct Animation {
    pub name: String,
    pub return_anim: String,
    pub frames: Vec<FrameDef>,
}

pub struct FrameDef {
    pub file: String,
    pub duration_ms: u64,
    pub mouth: Vec<(u8, String)>,
    /// Original voice clip index (audio_NNN.wav); u16::MAX = silent.
    pub audio: u16,
}

pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Assets {
    pub fn load() -> Result<Assets, String> {
        let root = find_root().ok_or_else(|| "assets directory not found; set BONZI_ASSETS".to_string())?;
        let raw: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("bonzi/animations.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let ch: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("bonzi/character.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let mut anims = HashMap::new();
        if let Some(map) = raw.as_object() {
            for (name, v) in map {
                let mut frames = Vec::new();
                if let Some(fs) = v["frames"].as_array() {
                    for f in fs {
                        let mut mouth = Vec::new();
                        if let Some(m) = f["mouth"].as_object() {
                            let mut entries: Vec<(u8, String)> = m
                                .iter()
                                .filter_map(|(k, v)| {
                                    let k: u8 = k.strip_prefix("mouth")?.parse().ok()?;
                                    Some((k, v.as_str()?.to_string()))
                                })
                                .collect();
                            entries.sort();
                            mouth = entries;
                        }
                        frames.push(FrameDef {
                            file: f["file"].as_str().unwrap_or("").to_string(),
                            duration_ms: (f["duration_10ms"].as_u64().unwrap_or(10).max(4)) * 10,
                            mouth,
                            audio: f["audio"].as_u64().unwrap_or(65535) as u16,
                        });
                    }
                }
                anims.insert(
                    name.clone(),
                    Animation {
                        name: name.clone(),
                        return_anim: v["return_anim"].as_str().unwrap_or("").to_string(),
                        frames,
                    },
                );
            }
        }
        let font = FontArc::try_from_vec(
            std::fs::read(root.join("fonts/LiberationSans-Regular.ttf")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| format!("font: {e}"))?;
        Ok(Assets {
            root,
            anims,
            char_w: ch["width"].as_u64().unwrap_or(200) as u32,
            char_h: ch["height"].as_u64().unwrap_or(160) as u32,
            font,
            frame_cache: HashMap::new(),
            mouth_cache: HashMap::new(),
            #[cfg(feature = "audio")]
            sfx_cache: HashMap::new(),
        })
    }

    pub fn frames(&mut self, anim: &str) -> Option<Arc<Vec<Frame>>> {
        if let Some(c) = self.frame_cache.get(anim) {
            return Some(c.clone());
        }
        let def = self.anims.get(anim)?;
        if def.frames.is_empty() {
            return None;
        }
        let dir = self.root.join("bonzi/sprites").join(&def.name);
        let mut out = Vec::new();
        for f in &def.frames {
            let path = dir.join(&f.file);
            let img = image::open(&path)
                .map_err(|e| eprintln!("frame load {}: {e}", path.display()))
                .ok()?
                .to_rgba8();
            out.push(Frame { w: img.width(), h: img.height(), rgba: img.into_raw() });
        }
        let arc = Arc::new(out);
        self.frame_cache.insert(anim.to_string(), arc.clone());
        Some(arc)
    }

    pub fn mouth_frame(&mut self, anim: &str, frame_idx: usize, mouth: u8) -> Option<Arc<Frame>> {
        let def = self.anims.get(anim)?;
        let fd = def.frames.get(frame_idx)?;
        let file = fd.mouth.iter().find(|(k, _)| *k == mouth).map(|(_, f)| f.clone())?;
        let key = format!("{anim}/{file}");
        if let Some(c) = self.mouth_cache.get(&key) {
            return Some(c.clone());
        }
        let path = self.root.join("bonzi/sprites").join(&def.name).join(&file);
        let img = image::open(path).ok()?.to_rgba8();
        let arc = Arc::new(Frame { w: img.width(), h: img.height(), rgba: img.into_raw() });
        self.mouth_cache.insert(key, arc.clone());
        Some(arc)
    }

    pub fn frame_duration(&self, anim: &str, idx: usize) -> u64 {
        self.anims.get(anim).and_then(|a| a.frames.get(idx)).map(|f| f.duration_ms).unwrap_or(100)
    }

    /// The original voice clip (`audio_NNN.wav`) the character plays when
    /// this frame is shown, if the frame carries one.
    pub fn frame_audio(&self, anim: &str, idx: usize) -> Option<String> {
        let f = self.anims.get(anim)?.frames.get(idx)?;
        if f.audio == u16::MAX {
            None
        } else {
            Some(format!("audio_{:03}.wav", f.audio))
        }
    }

    pub fn return_anim(&self, anim: &str) -> Option<&str> {
        self.anims.get(anim).map(|a| a.return_anim.as_str()).filter(|s| !s.is_empty())
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Sound effect bytes: audio/<name> (extracted originals), then
    /// sfx/<name> and bonzi/audio/<name> for user-supplied packs.
    #[cfg(feature = "audio")]
    pub fn sound(&mut self, name: &str) -> Option<Arc<Vec<u8>>> {
        if let Some(c) = self.sfx_cache.get(name) {
            return Some(c.clone());
        }
        for sub in ["audio", "sfx", "bonzi/audio"] {
            let p = self.root.join(sub).join(name);
            if let Ok(b) = std::fs::read(&p) {
                let arc = Arc::new(b);
                self.sfx_cache.insert(name.to_string(), arc.clone());
                return Some(arc);
            }
        }
        None
    }

    /// True if any frame of the animation carries a mouth overlay, i.e. it
    /// is "speakable" — MS Agent §7: Speak re-uses the last played animation
    /// only when it has mouth overlays, else the Speaking-state fallback.
    pub fn has_mouth(&self, anim: &str) -> bool {
        self.anims
            .get(anim)
            .is_some_and(|a| a.frames.iter().any(|f| !f.mouth.is_empty()))
    }

    pub fn has(&self, anim: &str) -> bool {
        self.anims.contains_key(anim)
    }
}

fn find_root() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("BONZI_ASSETS") {
        cands.push(PathBuf::from(p));
    }
    cands.push(PathBuf::from("assets"));
    cands.push(PathBuf::from("../assets"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            cands.push(dir.join("assets"));
            cands.push(dir.join("../assets"));
        }
    }
    cands.push(PathBuf::from("/usr/share/bonzibuddy-rs/assets"));
    cands.into_iter().find(|p| p.join("bonzi/animations.json").exists())
}
