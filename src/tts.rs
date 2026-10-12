//! Optional local text-to-speech. Shells out to platform voices only —
//! never the network. Disabled unless `--tts` is passed.
//!
//! On Linux with the `audio` feature, espeak-ng renders to stdout
//! (`--stdout`) and the WAV is decoded into the same rodio mixer as the
//! sound effects: speech and SFX then mix together instead of two
//! processes fighting over the ALSA device (the direct-spawn version cut
//! off or was cut off by whichever side grabbed the device first).
//! Without that path, the voice is a spawned child process the caller
//! kills when the next action starts (new sounds interrupt older speech).

/// A running voice output the caller can stop at any time.
pub enum Voice {
    /// Raw player/TTS child (fallback path: spd-say, flite, macOS, Windows).
    Child(std::process::Child),
    /// espeak-ng piped through the rodio mixer.
    #[cfg(all(feature = "audio", target_os = "linux"))]
    Mixed {
        child: std::process::Child,
        /// Set before killing so the pump thread never appends late.
        killed: std::sync::Arc<std::sync::atomic::AtomicBool>,
        sink: std::sync::Arc<std::sync::Mutex<Option<rodio::Sink>>>,
    },
}

impl Voice {
    /// Silence this voice now: kill the child and stop any started sink.
    pub fn stop(self) {
        match self {
            Voice::Child(mut c) => {
                let _ = c.kill();
                let _ = c.wait();
            }
            #[cfg(all(feature = "audio", target_os = "linux"))]
            Voice::Mixed { mut child, killed, sink } => {
                killed.store(true, std::sync::atomic::Ordering::Relaxed);
                if let Some(s) = sink.lock().unwrap().take() {
                    s.stop();
                }
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

/// Spawn a direct platform voice (no rodio mixer). Returns the child so
/// the caller can kill it when the next action starts.
pub fn speak(text: &str) -> Option<std::process::Child> {
    if text.is_empty() {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        for (prog, args) in [
            ("espeak-ng", vec!["-v", "en-uk-north", text]),
            ("spd-say", vec!["-l", "en-uk", text]),
            ("flite", vec!["-voice", "slt", text]),
        ] {
            if let Ok(c) = std::process::Command::new(prog)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                return Some(c);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("say")
            .arg(text)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let safe = text.replace('\'', "''");
        return std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &format!(
                    "Add-Type -AssemblyName System.Speech; (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak('{safe}')"
                ),
            ])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn()
            .ok();
    }
    #[allow(unreachable_code)]
    None
}

/// espeak-ng rendering to stdout, decoded into the rodio mixer so speech
/// mixes with sound effects on one device. `None` when espeak-ng is
/// unavailable — caller should fall back to [`speak`].
#[cfg(all(feature = "audio", target_os = "linux"))]
pub fn speak_mixed(text: &str, handle: &rodio::OutputStreamHandle) -> Option<Voice> {
    if text.is_empty() {
        return None;
    }
    let mut child = std::process::Command::new("espeak-ng")
        .args(["-v", "en-uk-north", "--stdout", text])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let killed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sink_slot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let (k2, s2, h2) = (killed.clone(), sink_slot.clone(), handle.clone());
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        if k2.load(std::sync::atomic::Ordering::Relaxed) || buf.len() < 100 {
            return;
        }
        let (Ok(src), Ok(sink)) = (
            rodio::Decoder::new(std::io::Cursor::new(buf)),
            rodio::Sink::try_new(&h2),
        ) else {
            return;
        };
        sink.append(src);
        let mut slot = s2.lock().unwrap();
        *slot = Some(sink);
        // stop() may have run between the read and the append.
        if k2.load(std::sync::atomic::Ordering::Relaxed) {
            if let Some(s) = slot.take() {
                s.stop();
            }
        }
    });
    Some(Voice::Mixed { child, killed, sink: sink_slot })
}
