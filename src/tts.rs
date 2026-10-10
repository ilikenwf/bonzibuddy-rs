//! Optional local text-to-speech. Shells out to platform voices only —
//! never the network. Disabled unless `--tts` is passed.

pub fn speak(text: &str) {
    if text.is_empty() {
        return;
    }
    #[cfg(target_os = "linux")]
    {
        for (prog, args) in [
            ("espeak-ng", vec!["-v", "en-uk-north", text]),
            ("spd-say", vec!["-l", "en-uk", text]),
            ("flite", vec!["-voice", "slt", text]),
        ] {
            if std::process::Command::new(prog)
                .args(&args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .is_ok()
            {
                return;
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("say")
            .arg(text)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let safe = text.replace('\'', "''");
        let _ = std::process::Command::new("powershell")
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
            .spawn();
    }
}
