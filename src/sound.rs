//! Notification sound.
//!
//! Plays a short sound when a new unread report arrives, using the platform's
//! system audio player - afplay on macOS, paplay/pw-play on Linux, and
//! PowerShell's MediaPlayer on Windows. No Rust audio dependencies: the player
//! runs in a background thread so the reader never blocks on it.
//!
//! Disable with `--no-sound` or the `AGENT_INBOX_DISABLE_SOUND` environment
//! variable.

use std::process::Command;

/// Play the notification sound, returning immediately.
///
/// The player runs detached in a background thread. A missing player or a
/// failed spawn is not an error: a silent inbox is better than a broken one.
pub fn play() {
    if disabled() {
        return;
    }
    std::thread::spawn(|| {
        let _ = play_blocking();
    });
}

/// True when the user has opted out via the environment.
pub fn disabled() -> bool {
    std::env::var_os("AGENT_INBOX_DISABLE_SOUND").is_some()
}

fn play_blocking() -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // A built-in system sound, so nothing extra ships with the binary.
        Command::new("afplay")
            .arg("/System/Library/Sounds/Glass.aiff")
            .status()?;
    }

    #[cfg(target_os = "linux")]
    {
        // Best effort: try each player until one succeeds. The freedesktop
        // bell is the closest thing to a universal system sound on Linux.
        let sound = "/usr/share/sounds/freedesktop/stereo/bell.oga";
        for (player, args) in [
            ("paplay", &[sound][..]),
            ("pw-play", &[sound][..]),
            (
                "ffplay",
                &["-nodisp", "-autoexit", "-loglevel", "quiet", sound][..],
            ),
        ] {
            if Command::new(player).args(args).status().is_ok() {
                return Ok(());
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // PowerShell MediaPlayer, mirroring herdr's approach.
        let script = concat!(
            "$p = [System.Windows.Media.MediaPlayer]::new();",
            "$p.Open([uri]'C:\\Windows\\Media\\notify.wav');",
            "$p.Play();",
            "Start-Sleep -Milliseconds 500;"
        );
        Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ])
            .status()?;
    }

    Ok(())
}
