//! Processus des extensions : savoir si un exe tourne, le lancer détaché, le fermer.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
};

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Une seule instance par nom (`Local\…`) : vrai pour la première.
pub fn single_instance(name: &str) -> bool {
    let name = crate::wide(name);
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if h.is_null() {
            return false;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // Pas gardé : sinon le verrou survit à son premier propriétaire, et une nouvelle
            // tentative (relance après une mise à jour) échouerait toujours.
            CloseHandle(h);
            return false;
        }
        true
    }
}

/// [`single_instance`], en réessayant jusqu'à `wait` : le temps que l'instance précédente
/// (relancée après une mise à jour) se ferme.
pub fn single_instance_wait(name: &str, wait: std::time::Duration) -> bool {
    let end = std::time::Instant::now() + wait;
    loop {
        if single_instance(name) {
            return true;
        }
        if std::time::Instant::now() >= end {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Chemin de l'exe du processus `pid`, en minuscules (vide si inaccessible).
pub fn exe_of(pid: u32) -> String {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return String::new();
        }
        let mut buf = [0u16; 520];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        if ok == 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..len as usize]).to_lowercase()
    }
}

fn normalized(path: &Path) -> String {
    let p = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    p.to_string_lossy().trim_start_matches(r"\\?\").to_lowercase()
}

/// Processus dont l'exe est `exe`.
pub fn pids_of(exe: &Path) -> Vec<u32> {
    let target = normalized(exe);
    let me = std::process::id();
    let mut pids = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return pids;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            let pid = entry.th32ProcessID;
            if pid != me && exe_of(pid) == target {
                pids.push(pid);
            }
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
    }
    pids
}

pub fn is_running(exe: &Path) -> bool {
    !pids_of(exe).is_empty()
}

/// Lance `exe` détaché de l'appelant (le hook de Claude rend la main tout de suite).
pub fn spawn_detached(exe: &Path, args: &[String]) -> std::io::Result<()> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
    cmd.spawn().map(|_| ())
}

/// Ferme tous les processus de `exe`.
pub fn kill_all(exe: &Path) {
    for pid in pids_of(exe) {
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !h.is_null() {
                TerminateProcess(h, 0);
                CloseHandle(h);
            }
        }
    }
}
