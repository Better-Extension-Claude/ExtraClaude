//! Événements de Claude, de BetterClaude vers une extension : un pipe nommé par extension,
//! `\\.\pipe\betterext-<id>`. Chaque connexion porte un événement, le JSON du hook de
//! Claude sur une ligne (champ `hook_event_name`, `session_id`, `cwd`, `tool_name`…).

use crate::wide;
use serde_json::Value;
use std::io::{Read, Write};
use std::os::windows::io::FromRawHandle;
use std::time::Duration;
use windows_sys::Win32::Foundation::{GetLastError, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_INBOUND;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

pub fn pipe_name(id: &str) -> String {
    format!(r"\\.\pipe\betterext-{id}")
}

/// Écoute les événements de Claude destinés à l'extension `id`, sur un fil à part.
/// `on_event` reçoit chaque JSON de hook, dans l'ordre d'arrivée.
pub fn listen(id: &str, mut on_event: impl FnMut(Value) + Send + 'static) {
    let name = wide(&pipe_name(id));
    std::thread::spawn(move || loop {
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                0,
                1 << 16,
                0,
                std::ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        // Le File ferme le pipe en fin de tour.
        let mut pipe = unsafe { std::fs::File::from_raw_handle(handle) };
        if !connected {
            continue;
        }
        let mut text = String::new();
        if pipe.read_to_string(&mut text).is_ok() || !text.is_empty() {
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                if let Ok(event) = serde_json::from_str(line) {
                    on_event(event);
                }
            }
        }
    });
}

/// Envoie un événement à l'extension `id`. Faux si elle n'écoute pas (pas lancée).
pub fn send(id: &str, event: &Value) -> bool {
    let line = event.to_string() + "\n";
    // Toutes les instances du pipe peuvent être occupées un court instant.
    for _ in 0..10 {
        match std::fs::OpenOptions::new().write(true).open(pipe_name(id)) {
            Ok(mut pipe) => return pipe.write_all(line.as_bytes()).is_ok(),
            // ERROR_PIPE_BUSY
            Err(e) if e.raw_os_error() == Some(231) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return false,
        }
    }
    false
}
