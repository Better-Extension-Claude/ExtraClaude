//! Le hook de Claude : lance les extensions activées en début de session et leur relaie
//! les événements qu'elles écoutent. Rend la main vite, n'écrit rien (Claude continue).

use betterext::{events, process, Extension};
use serde_json::Value;
use std::io::Read;
use std::time::{Duration, Instant};

pub fn run() {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let Ok(event) = serde_json::from_str::<Value>(input.trim_start_matches('\u{feff}')) else { return };
    let name = event.get("hook_event_name").and_then(Value::as_str).unwrap_or_default().to_string();
    let active = betterext::active();
    let mut started = Vec::new();
    if name == "SessionStart" {
        // Le bouton puzzle (une seule instance : il se ferme seul s'il tourne déjà).
        if let Ok(me) = std::env::current_exe() {
            let _ = process::spawn_detached(&me, &["--button".to_string()]);
        }
        started = active.iter().filter(|e| start(e)).map(|e| e.manifest.id.clone()).collect();
    }
    for ext in active.iter().filter(|e| e.wants(&name)) {
        let id = &ext.manifest.id;
        // Une extension tout juste lancée n'écoute pas encore : on lui laisse un instant.
        let deadline = Instant::now() + Duration::from_millis(1500);
        while !events::send(id, &event) && started.contains(id) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Après une installation ou une mise à jour : relance tout de suite l'extension `id` si elle
/// est activée et que Claude est ouvert (sinon elle attendrait la prochaine session).
pub fn restart_installed(id: &str) {
    if !betterext::claude::is_open() || !betterext::State::load().is_enabled(id) {
        return;
    }
    if let Some(ext) = betterext::installed().into_iter().find(|e| e.manifest.id == id) {
        start(&ext);
    }
}

/// Lance l'extension si elle a un lancement et ne tourne pas déjà. Vrai si lancée.
pub fn start(ext: &Extension) -> bool {
    let Some(args) = &ext.manifest.start else { return false };
    let exe = ext.exe_path();
    exe.exists() && !process::is_running(&exe) && process::spawn_detached(&exe, args).is_ok()
}
