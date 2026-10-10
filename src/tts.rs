//! Optional local text-to-speech. Shells out to platform voices only —
//! never the network. Disabled unless `--tts` is passed.
//!
//! `speak` returns the spawned child so the caller can kill it when the
//! next action starts (new sounds interrupt older speech).

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
