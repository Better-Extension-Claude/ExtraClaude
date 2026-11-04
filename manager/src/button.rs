//! Le bouton puzzle sur Claude : une petite fenêtre Win32 gardée juste au-dessus de Claude, collée à
//! gauche de son bouton « Paramètres » (en bas de la barre latérale). Un clic ouvre la
//! fenêtre BetterClaude. Le bouton « Paramètres » est trouvé par UI Automation : le puzzle suit
//! la barre latérale quand on l'élargit, et se cache quand elle est repliée.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, TreeScope_Descendants, UIA_ButtonControlTypeId,
    UIA_ControlTypePropertyId,
};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, DIB_RGB_COLORS,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_LBUTTON};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, IsIconic, IsWindow,
    GetAncestor, GetWindow, GetWindowLongPtrW, GetWindowRect, IsWindowVisible, LoadCursorW, RegisterClassW, SetTimer,
    SetWindowPos, ShowWindow, TranslateMessage, UpdateLayeredWindow, ASFW_ANY, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE,
    EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZESTART, GA_ROOT,
    GWL_EXSTYLE, GW_HWNDPREV, HWND_NOTOPMOST, HWND_TOPMOST, IDC_HAND, MA_NOACTIVATE, MSG, OBJID_WINDOW, SWP_NOACTIVATE,
    SWP_ASYNCWINDOWPOS, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WM_CLOSE, WM_DESTROY, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_TIMER, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

pub const BUTTON_TITLE: &str = "BetterClaude-bouton";
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Journal détaillé du bouton (`%LOCALAPPDATA%\BetterExt\button.log`), actif tant que le
/// fichier `%LOCALAPPDATA%\BetterExt\debug.on` existe (ou avec BETTEREXT_DEBUG=1) : l'outil
/// `.outils/diagnostic.ps1` le crée et le lit, sans relancer le bouton.
fn debug(msg: impl AsRef<str>) {
    use std::io::Write;
    let on = std::env::var_os("BETTEREXT_DEBUG").is_some() || betterext::data_dir().join("debug.on").exists();
    if !on {
        return;
    }
    let path = betterext::data_dir().join("button.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let _ = writeln!(f, "{}.{:03}  {}", t.as_secs(), t.subsec_millis(), msg.as_ref());
    }
}

/// État du bouton en une ligne (`%LOCALAPPDATA%\BetterExt\button.status`), toujours tenu à jour
/// (écrit seulement quand il change) : pourquoi le puzzle est caché, ou où il est affiché.
fn status(text: &str) {
    thread_local! {
        static LAST: RefCell<String> = const { RefCell::new(String::new()) };
    }
    let changed = LAST.with(|l| {
        let mut l = l.borrow_mut();
        (l.as_str() != text).then(|| *l = text.to_string()).is_some()
    });
    if changed {
        debug(text);
        let _ = std::fs::write(betterext::data_dir().join("button.status"), format!("{text}\n"));
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Retrouve le bouton « Paramètres » de Claude et garde l'élément trouvé : relire sa
/// position ensuite est rapide.
struct Locator {
    uia: Option<IUIAutomation>,
    found: Option<IUIAutomationElement>,
    last_search: Option<Instant>,
}

impl Locator {
    fn new() -> Locator {
        let uia = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()
        };
        Locator { uia, found: None, last_search: None }
    }

    fn rect_of(e: &IUIAutomationElement) -> Option<RECT> {
        let r = unsafe { e.CurrentBoundingRectangle() }.ok()?;
        (r.right > r.left && r.bottom > r.top).then_some(RECT { left: r.left, top: r.top, right: r.right, bottom: r.bottom })
    }

    /// Position du bouton « Paramètres » à l'écran, en pixels.
    fn settings(&mut self, claude: HWND) -> Option<RECT> {
        if let Some(r) = self.found.as_ref().and_then(Self::rect_of) {
            return Some(r);
        }
        self.found = None;
        // Recherche complète (tout l'arbre de Claude) : au plus toutes les 2 s.
        if self.last_search.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
            return None;
        }
        self.last_search = Some(Instant::now());
        let Some(uia) = self.uia.as_ref() else {
            debug("UI Automation indisponible");
            return None;
        };
        unsafe {
            let root = match uia.ElementFromHandle(windows::Win32::Foundation::HWND(claude as *mut c_void)) {
                Ok(r) => r,
                Err(e) => {
                    debug(format!("ElementFromHandle : {e}"));
                    return None;
                }
            };
            let win = root.CurrentBoundingRectangle().ok()?;
            let cond = uia
                .CreatePropertyCondition(UIA_ControlTypePropertyId, &windows::core::VARIANT::from(UIA_ButtonControlTypeId.0))
                .ok()?;
            let all = match root.FindAll(TreeScope_Descendants, &cond) {
                Ok(a) => a,
                Err(e) => {
                    debug(format!("FindAll : {e}"));
                    return None;
                }
            };
            debug(format!("recherche : fenêtre {:?}, {} boutons", (win.left, win.top, win.right, win.bottom), all.Length().unwrap_or(0)));
            for i in 0..all.Length().unwrap_or(0) {
                let Ok(e) = all.GetElement(i) else { continue };
                let name = e.CurrentName().map(|n| n.to_string().to_lowercase()).unwrap_or_default();
                let Some(r) = Self::rect_of(&e) else { continue };
                if name.contains("param") || name.contains("setting") {
                    debug(format!("  candidat « {name} » {:?}", (r.left, r.top, r.right, r.bottom)));
                }
                let (w, h) = (r.right - r.left, r.bottom - r.top);
                // En bas de la moitié gauche (barre latérale), carré, nommé « Paramètres… ».
                let is_settings = (name.starts_with("paramètres") || name.starts_with("settings"))
                    && (w - h).abs() <= 6
                    && r.bottom > win.bottom - 90
                    && r.left < win.left + (win.right - win.left) / 2;
                if is_settings {
                    self.found = Some(e);
                    return Some(r);
                }
            }
        }
        None
    }
}

// ------------------------------------------------------------------ dessin

/// Distance signée à une pièce de puzzle pleine (corps carré, tenon à cou étroit en haut et
/// à droite), dans la grille 24×24 des icônes de Claude.
pub fn puzzle_sdf(x: f32, y: f32) -> f32 {
    let rounded_box = |cx: f32, cy: f32, hx: f32, hy: f32, r: f32| {
        let (qx, qy) = ((x - cx).abs() - (hx - r), (y - cy).abs() - (hy - r));
        (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
    };
    let circle = |cx: f32, cy: f32, r: f32| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r;
    let body = rounded_box(8.45, 15.55, 5.5, 5.5, 1.5);
    let top = circle(8.45, 6.3, 3.4).min(rounded_box(8.45, 9.6, 1.9, 0.9, 0.0));
    let right = circle(17.7, 15.55, 3.4).min(rounded_box(14.4, 15.55, 0.9, 1.9, 0.0));
    body.min(top).min(right)
}

/// Dessine le bouton (`size` × `size` pixels) : pièce de puzzle, fond arrondi au survol.
fn render(hwnd: HWND, size: i32, hover: bool) {
    let size = size.max(8);
    let s = size as f32;
    // Taille de l'icône : 18 px pour un bouton de 28 px, comme l'engrenage de Claude.
    let icon = s * 18.0 / 28.0;
    let unit = icon / 24.0;
    let origin = (s - icon) / 2.0;
    // Couleurs des icônes du thème (Réglages → Apparence) ; fond du survol blanc (sombre) ou noir (clair).
    let palette = betterext::theme::current();
    let light = !betterext::theme::is_dark();
    let betterext::theme::Rgb(r, g, b) = if hover { palette.icone_survol } else { palette.icone };
    let ink: [u8; 3] = [r, g, b];
    let bg_alpha = if hover { if light { 0.06f32 } else { 0.08 } } else { 0.0 };
    let tint = if light { 0.0f32 } else { 255.0 };
    let radius = 6.0 * s / 28.0;
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let dc = CreateCompatibleDC(screen);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = size;
        bi.bmiHeader.biHeight = -size;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if bmp.is_null() || bits.is_null() {
            DeleteDC(dc);
            ReleaseDC(std::ptr::null_mut(), screen);
            return;
        }
        let old = SelectObject(dc, bmp);
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (size * size) as usize);
        for y in 0..size {
            for x in 0..size {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                // Fond : carré arrondi, presque invisible hors survol (il capte quand même la souris).
                let (qx, qy) = ((fx - s / 2.0).abs() - (s / 2.0 - radius), (fy - s / 2.0).abs() - (s / 2.0 - radius));
                let d_bg = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius;
                let bg = (0.5 - d_bg).clamp(0.0, 1.0) * bg_alpha.max(1.0 / 255.0);
                // La pièce de puzzle.
                let d = puzzle_sdf((fx - origin) / unit, (fy - origin) / unit) * unit;
                let line = (0.5 - d).clamp(0.0, 1.0);
                // Encre par-dessus le fond, en alpha prémultiplié.
                let a = line + bg * (1.0 - line);
                let ch = |c: u8| ((c as f32 * line + tint * bg * (1.0 - line)).round() as u32).min(255);
                px[(y * size + x) as usize] =
                    ((a * 255.0).round() as u32) << 24 | ch(ink[0]) << 16 | ch(ink[1]) << 8 | ch(ink[2]);
            }
        }
        let sz = SIZE { cx: size, cy: size };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        UpdateLayeredWindow(hwnd, screen, std::ptr::null(), &sz, dc, &src, 0, &blend, ULW_ALPHA);
        SelectObject(dc, old);
        DeleteObject(bmp);
        DeleteDC(dc);
        ReleaseDC(std::ptr::null_mut(), screen);
    }
}

// ------------------------------------------------------------------ fenêtre

struct Button {
    hwnd: HWND,
    claude: HWND,
    locator: Locator,
    size: i32,
    is_hover: bool,
    is_pressed: bool,
    is_shown: bool,
    /// Dernière position donnée (x, y, taille) : on ne replace que si elle change.
    placed: (i32, i32, i32),
    started: Instant,
    last_seen: Option<Instant>,
    last_window_search: Instant,
    /// Couleurs avec lesquelles le puzzle est dessiné (redessiné quand elles changent).
    theme: Option<(betterext::theme::Palette, bool)>,
}

thread_local! {
    static BUTTON: RefCell<Option<Button>> = const { RefCell::new(None) };
}

impl Button {
    fn hide(&mut self, reason: &str) {
        status(&format!("caché : {reason}"));
        SHOULD_SHOW.store(false, Ordering::SeqCst);
        if unsafe { IsWindowVisible(self.hwnd) } != 0 {
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }
        self.is_shown = false;
    }

    /// Toutes les 100 ms : retrouve Claude et son bouton « Paramètres », et se place à côté.
    fn tick(&mut self) {
        // Thème changé (réglage ou Claude) : le puzzle prend ses nouvelles couleurs.
        let now = (betterext::theme::current(), betterext::theme::is_dark());
        if self.theme.as_ref() != Some(&now) {
            self.theme = Some(now);
            render(self.hwnd, self.size, self.is_hover);
        }
        let alive = !self.claude.is_null() && unsafe { IsWindow(self.claude) != 0 && IsWindowVisible(self.claude) != 0 };
        if !alive || self.last_window_search.elapsed() > Duration::from_secs(2) {
            self.last_window_search = Instant::now();
            let c = betterext::claude::window();
            if c != self.claude {
                debug(format!("fenêtre de Claude : {c:?}"));
                self.claude = c;
                WATCH_CLAUDE.store(c as isize, Ordering::Relaxed);
                self.locator.found = None;
                self.locator.last_search = None;
            }
        }
        if self.claude.is_null() {
            self.hide("fenêtre de Claude introuvable");
            let gone = match self.last_seen {
                Some(t) => t.elapsed() > Duration::from_secs(10),
                None => self.started.elapsed() > Duration::from_secs(180),
            };
            if gone {
                std::process::exit(0);
            }
            return;
        }
        self.last_seen = Some(Instant::now());
        if unsafe { IsIconic(self.claude) } != 0 {
            self.hide("Claude réduit");
            return;
        }
        // Filet : un « lâcher » de Claude manqué laisserait le puzzle caché pour de bon. Bouton
        // gauche relâché : le déplacement est fini.
        if MOVING.load(Ordering::SeqCst) && unsafe { GetAsyncKeyState(VK_LBUTTON as i32) } as u16 & 0x8000 == 0 {
            debug("fin de déplacement manquée : rattrapée");
            MOVING.store(false, Ordering::SeqCst);
        }
        let Some(gear) = self.locator.settings(self.claude) else {
            self.hide("bouton Paramètres de Claude introuvable (barre latérale repliée ?)");
            return;
        };
        let size = gear.bottom - gear.top;
        if size != self.size {
            self.size = size;
            render(self.hwnd, size, self.is_hover);
        }
        // Claude déplacé à la souris : caché jusqu'au lâcher (`on_event`).
        if MOVING.load(Ordering::SeqCst) {
            self.hide("Claude en cours de déplacement");
            return;
        }
        let gap = (size as f32 / 7.0).round() as i32;
        let (x, y) = (gear.left - size - gap, gear.top);
        // Position seulement si elle change ; l'ordre des fenêtres, à part (`order`).
        if (x, y, size) != self.placed {
            debug(format!("position ({x}, {y}) taille {size}"));
            unsafe { SetWindowPos(self.hwnd, std::ptr::null_mut(), x, y, size, size, SWP_NOZORDER | SWP_NOACTIVATE) };
            self.placed = (x, y, size);
        }
        // Affiché par `order`, sauf si une fenêtre recouvre sa place.
        SHOULD_SHOW.store(true, Ordering::SeqCst);
        self.is_shown = true;
        order(self.hwnd, self.claude);
        let visible = unsafe { IsWindowVisible(self.hwnd) } != 0;
        status(&if visible { format!("affiché en ({x}, {y})") } else { "caché : une autre fenêtre recouvre sa place".to_string() });
    }

    /// Le premier plan a changé : Claude peut passer devant juste avant ou juste après
    /// l'événement. Contrôle immédiat, puis chaque milliseconde pendant 500 ms dans un fil à
    /// part (un minuteur, ≈ 15 ms, laisserait voir Claude par-dessus le temps d'une image).
    fn watch_order(&mut self) {
        order(self.hwnd, self.claude);
        let until = epoch().elapsed().as_millis() as u64 + 500;
        // Un guet en cours est seulement prolongé.
        if WATCH_UNTIL.swap(until, Ordering::SeqCst) != 0 {
            return;
        }
        std::thread::spawn(|| loop {
            order(WATCH_BUTTON.load(Ordering::Relaxed) as HWND, WATCH_CLAUDE.load(Ordering::Relaxed) as HWND);
            std::thread::sleep(Duration::from_millis(1));
            let until = WATCH_UNTIL.load(Ordering::SeqCst);
            if epoch().elapsed().as_millis() as u64 >= until
                && WATCH_UNTIL.compare_exchange(until, 0, Ordering::SeqCst, Ordering::SeqCst).is_ok()
            {
                return;
            }
        });
    }

    fn event(&mut self, event: u32, hwnd: HWND) {
        if event == EVENT_SYSTEM_FOREGROUND {
            self.watch_order();
            return;
        }
        if hwnd != self.claude || self.claude.is_null() {
            // Une autre fenêtre : ne compte que si elle est de premier niveau (ses contrôles
            // bougent souvent) ; une fenêtre détruite ou cachée compte toujours.
            let top_level = event != EVENT_OBJECT_LOCATIONCHANGE || unsafe { GetAncestor(hwnd, GA_ROOT) } == hwnd;
            if top_level && !MOVING.load(Ordering::SeqCst) {
                order(self.hwnd, self.claude);
            }
            return;
        }
        match event {
            // Claude saisi à la souris : le puzzle disparaît, et revient au lâcher.
            // (MOVING est déjà mis à jour par `on_event`, même si le bouton était occupé.)
            EVENT_SYSTEM_MOVESIZESTART => self.hide("Claude en cours de déplacement"),
            EVENT_SYSTEM_MOVESIZEEND => self.tick(),
            // Agrandi, ancré, redimensionné sans la souris : le puzzle suit.
            EVENT_OBJECT_LOCATIONCHANGE if !MOVING.load(Ordering::SeqCst) => self.tick(),
            _ => {}
        }
    }

    fn set_hover(&mut self, hover: bool) {
        if hover != self.is_hover {
            self.is_hover = hover;
            render(self.hwnd, self.size, hover);
        }
    }

    fn message(&mut self, msg: u32, lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_TIMER => self.tick(),
            // Un WM_CLOSE venu d'ailleurs détruirait la fenêtre et laisserait un processus sans
            // bouton (qui garde le verrou d'instance unique) : on l'ignore.
            WM_CLOSE => {}
            WM_MOUSEACTIVATE => return Some(MA_NOACTIVATE as LRESULT),
            WM_MOUSEMOVE => {
                if !self.is_hover {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe { TrackMouseEvent(&mut tme) };
                    self.set_hover(true);
                }
            }
            WM_MOUSELEAVE => {
                self.is_pressed = false;
                self.set_hover(false);
            }
            WM_LBUTTONDOWN => self.is_pressed = true,
            WM_LBUTTONUP => {
                let (x, y) = ((lparam & 0xffff) as i16 as i32, ((lparam >> 16) & 0xffff) as i16 as i32);
                if std::mem::take(&mut self.is_pressed) && x >= 0 && y >= 0 && x < self.size && y < self.size {
                    open_window();
                }
            }
            _ => return None,
        }
        Some(0)
    }
}

// ------------------------------------------------------------------ ordre des fenêtres

/// Fenêtres surveillées par le guet (lues hors du fil de la fenêtre).
static WATCH_BUTTON: AtomicIsize = AtomicIsize::new(0);
static WATCH_CLAUDE: AtomicIsize = AtomicIsize::new(0);
/// Fin du guet en cours (ms depuis `epoch()`), 0 : aucun guet.
static WATCH_UNTIL: AtomicU64 = AtomicU64::new(0);
/// Claude est déplacé ou redimensionné à la souris.
static MOVING: AtomicBool = AtomicBool::new(false);

fn epoch() -> &'static Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now)
}

fn is_topmost(hwnd: HWND) -> bool {
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0 }
}

/// Une fenêtre au-dessus de Claude recouvre-t-elle le puzzle ? On ignore les fenêtres
/// « toujours au-dessus », traversables par la souris, invisibles ou masquées par Windows.
fn covered(button: HWND, claude: HWND) -> bool {
    let mut b = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if unsafe { GetWindowRect(button, &mut b) } == 0 {
        return false;
    }
    let mut w = unsafe { GetWindow(claude, GW_HWNDPREV) };
    for _ in 0..4096 {
        if w.is_null() {
            return false;
        }
        if w != button && unsafe { IsWindowVisible(w) } != 0 {
            let ex = unsafe { GetWindowLongPtrW(w, GWL_EXSTYLE) } as u32;
            let mut cloaked = 0u32;
            unsafe { DwmGetWindowAttribute(w, DWMWA_CLOAKED as _, &mut cloaked as *mut u32 as *mut c_void, 4) };
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            let overlaps = unsafe { GetWindowRect(w, &mut r) } != 0
                && r.left < b.right
                && b.left < r.right
                && r.top < b.bottom
                && b.top < r.bottom;
            if overlaps && ex & (WS_EX_TOPMOST | WS_EX_TRANSPARENT) == 0 && cloaked == 0 {
                return true;
            }
        }
        w = unsafe { GetWindow(w, GW_HWNDPREV) };
    }
    false
}

/// Ordre du puzzle (comme le bouton GitPush). Tant que rien ne recouvre sa place sur Claude, il
/// est « toujours au-dessus » : Claude qui repasse au premier plan ne peut plus le cacher, même
/// un instant. Sinon, il passe juste au-dessus de Claude, sous la fenêtre qui le recouvre, et ne
/// remonte que si Claude est passé devant (se doubler sans fin avec les autres extensions les
/// ferait clignoter). Jamais « possédé » par Claude : entre deux processus, ce lien relie leurs
/// files d'entrée et peut figer Claude.
fn order(button: HWND, claude: HWND) {
    if button.is_null() || claude.is_null() || MOVING.load(Ordering::SeqCst) || !SHOULD_SHOW.load(Ordering::SeqCst) {
        return;
    }
    if unsafe { IsIconic(claude) } != 0 {
        return;
    }
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
    // Toujours « toujours au-dessus » ; une fenêtre posée sur sa place (Chrome sur Claude…) le
    // CACHE au lieu de le faire descendre. Descendre puis remonter au-dessus de Claude au premier
    // plan, Windows le refuse ; et glisser Claude sous nous le renverrait derrière les autres
    // fenêtres (Claude ne pouvait plus revenir devant). Claude n'est donc jamais déplacé.
    let visible = unsafe { IsWindowVisible(button) } != 0;
    if covered(button, claude) {
        if visible {
            unsafe { ShowWindow(button, SW_HIDE) };
        }
        return;
    }
    if !is_topmost(button) {
        unsafe { SetWindowPos(button, HWND_TOPMOST, 0, 0, 0, 0, flags) };
    }
    if !visible {
        unsafe { ShowWindow(button, SW_SHOWNOACTIVATE) };
    }
    // Claude ne doit jamais être « toujours au-dessus » (laissé ainsi par une ancienne version) :
    // remis normal, sans changer sa place parmi les fenêtres normales.
    if is_topmost(claude) {
        debug("Claude « toujours au-dessus » : remis normal");
        unsafe { SetWindowPos(claude, HWND_NOTOPMOST, 0, 0, 0, 0, flags | SWP_ASYNCWINDOWPOS) };
    }
}

/// Le puzzle a sa place (Claude trouvé, bouton Paramètres trouvé, pas de déplacement) : `order`
/// l'affiche si rien ne la recouvre.
static SHOULD_SHOW: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: windows::Win32::Foundation::HWND,
    object: i32,
    child: i32,
    _thread: u32,
    _time: u32,
) {
    if object != OBJID_WINDOW || child != 0 {
        return;
    }
    // Début et fin de déplacement de Claude : notés tout de suite, même si le bouton est occupé
    // (sinon un « lâcher » perdu le laisserait caché).
    if hwnd.0 as isize == WATCH_CLAUDE.load(Ordering::Relaxed) {
        match event {
            EVENT_SYSTEM_MOVESIZESTART => {
                debug("Claude saisi");
                MOVING.store(true, Ordering::SeqCst);
            }
            EVENT_SYSTEM_MOVESIZEEND => {
                debug("Claude lâché");
                MOVING.store(false, Ordering::SeqCst);
            }
            _ => {}
        }
    }
    // Occupé (SetWindowPos ou UI Automation renvoient des messages en cours de traitement) :
    // le minuteur rattrapera.
    BUTTON.with(|b| {
        if let Ok(mut b) = b.try_borrow_mut() {
            if let Some(b) = b.as_mut() {
                b.event(event, hwnd.0 as HWND);
            }
        }
    });
}


/// Ouvre la fenêtre BetterClaude (ou la ramène devant si elle est déjà ouverte).
fn open_window() {
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Fenêtre détruite malgré tout : le processus s'arrête, le prochain lancement la recrée.
    if msg == WM_DESTROY {
        std::process::exit(0);
    }
    let handled = BUTTON.with(|b| match b.try_borrow_mut() {
        Ok(mut b) => b.as_mut().filter(|b| b.hwnd == hwnd).and_then(|b| b.message(msg, lparam)),
        Err(_) => None,
    });
    handled.unwrap_or_else(|| DefWindowProcW(hwnd, msg, wparam, lparam))
}

/// Le bouton : tourne jusqu'à la fermeture de Claude.
pub fn run() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(std::ptr::null());
        let class = wide("BetterClaudeButton");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_HAND),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let title = wide(BUTTON_TITLE);
        let hwnd = CreateWindowExW(
            // « Toujours au-dessus » dès la création : il ne quitte jamais cette couche.
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            -4000,
            -4000,
            28,
            28,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return;
        }
        render(hwnd, 28, false);
        BUTTON.with(|b| {
            *b.borrow_mut() = Some(Button {
                hwnd,
                claude: std::ptr::null_mut(),
                locator: Locator::new(),
                size: 28,
                is_hover: false,
                is_pressed: false,
                is_shown: false,
                placed: (0, 0, 0),
                started: Instant::now(),
                last_seen: None,
                last_window_search: Instant::now() - Duration::from_secs(60),
                theme: None,
            })
        });
        SetTimer(hwnd, 1, 100, None);
        WATCH_BUTTON.store(hwnd as isize, Ordering::Relaxed);
        epoch();
        // Fenêtres de tous les processus qui changent de premier plan, bougent, apparaissent ou
        // disparaissent : le puzzle revoit son ordre ; Claude saisi à la souris : il se cache.
        let ranges = [
            (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
            (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
        ];
        for (from, to) in ranges {
            let _ = SetWinEventHook(from, to, None, Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
