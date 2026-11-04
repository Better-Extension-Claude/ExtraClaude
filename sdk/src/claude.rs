//! La fenêtre de Claude desktop, pour coller une fenêtre d'extension dessus (même
//! détection que GitPush : titre « Claude », classe Chromium, exe claude.exe).

use crate::process::exe_of;
use std::ffi::c_void;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindow, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, GW_OWNER,
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

fn text_of(hwnd: HWND, class: bool) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe {
        if class {
            GetClassNameW(hwnd, buf.as_mut_ptr(), 256)
        } else {
            GetWindowTextW(hwnd, buf.as_mut_ptr(), 256)
        }
    };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let found = &mut *(lparam as *mut HWND);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid != GetCurrentProcessId()
        && IsWindowVisible(hwnd) != 0
        && GetWindow(hwnd, GW_OWNER).is_null()
        && text_of(hwnd, false) == "Claude"
        && text_of(hwnd, true) == "Chrome_WidgetWin_1"
        && exe_of(pid).ends_with("\\claude.exe")
    {
        *found = hwnd;
        return 0;
    }
    1
}

/// Fenêtre principale de Claude desktop, ou null.
pub fn window() -> HWND {
    let mut found: HWND = std::ptr::null_mut();
    unsafe { EnumWindows(Some(visit), &mut found as *mut HWND as LPARAM) };
    found
}

/// Position et taille de Claude (bord visible, sans l'ombre) et son échelle d'affichage.
pub fn rect() -> Option<(Rect, f32)> {
    let hwnd = window();
    if hwnd.is_null() {
        return None;
    }
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as _,
            &mut r as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        );
    }
    let scale = (unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0).max(1.0);
    Some((Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }, scale))
}

pub fn is_open() -> bool {
    !window().is_null()
}

/// `config.json` de Claude desktop (le plus récent) : `%LOCALAPPDATA%\Claude-Data\<paquet>\`
/// pour la version du Microsoft Store, `%APPDATA%\Claude\` sinon.
fn config_path() -> Option<std::path::PathBuf> {
    let env = |k: &str| std::env::var(k).map(std::path::PathBuf::from).ok();
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(local) = env("LOCALAPPDATA") {
        for entry in std::fs::read_dir(local.join("Claude-Data")).into_iter().flatten().flatten() {
            candidates.push(entry.path().join("config.json"));
        }
    }
    if let Some(roaming) = env("APPDATA") {
        candidates.push(roaming.join("Claude").join("config.json"));
    }
    candidates
        .into_iter()
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

/// Windows en mode sombre pour les applications (`AppsUseLightTheme` = 0).
fn windows_is_dark() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let key = crate::wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = crate::wide("AppsUseLightTheme");
    let mut value = 1u32;
    let mut size = 4u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut value as *mut u32 as *mut c_void,
            &mut size,
        )
    } == 0;
    ok && value == 0
}

/// Thème de Claude : `userThemeMode` de son `config.json` (« light », « dark »), ou celui de
/// Windows quand il suit le système. Sombre par défaut.
pub fn is_dark() -> bool {
    let mode = config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(t.trim_start_matches('\u{feff}')).ok())
        .and_then(|v| v.get("userThemeMode")?.as_str().map(str::to_lowercase));
    match mode.as_deref() {
        Some("light") => false,
        Some("dark") => true,
        Some(_) => windows_is_dark(),
        None => true,
    }
}
