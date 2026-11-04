//! Extension minimale : écrit chaque événement de Claude reçu dans
//! `%LOCALAPPDATA%\BetterExt\data\echo\events.log`.
//! extension.json : { "id": "echo", "name": "Echo", "exe": "echo.exe", "start": [], "events": ["*"] }

use std::io::Write;

fn main() {
    if !betterext::process::single_instance(r"Local\BetterExt-echo") {
        return;
    }
    let dir = betterext::extension_data_dir("echo");
    let _ = std::fs::create_dir_all(&dir);
    let log = dir.join("events.log");
    betterext::events::listen("echo", move |event| {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            let _ = writeln!(f, "{event}");
        }
    });
    // Une extension se ferme avec Claude.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        if !betterext::claude::is_open() {
            return;
        }
    }
}
