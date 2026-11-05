//! La fenêtre BetterClaude, ouverte par le bouton puzzle sur Claude : extensions installées,
//! Découverte (extensions publiées, clés d'accès aux privées) et réglages. Même allure que
//! la fenêtre GitPush : sans barre de Windows, déplacée par son en-tête, fermée par sa croix.

use crate::discover::{self, Remote};
use crate::github::{self, ManagedRepo, Person, Session};
use std::collections::HashMap;
use crate::{hook, setup};
use betterext::{claude, process, Extension, Manifest, State};
use eframe::egui::{
    self, vec2, Align, Color32, FontFamily, FontId, Key, Layout, Margin, Rect, RichText, Rounding, Sense, Stroke, Ui,
};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

const TITLE: &str = "BetterClaude";
use betterext::theme::{self, Palette, Rgb};
use std::cell::RefCell;

thread_local! {
    /// Thème en cours (`betterext::theme`, réglable dans Réglages → Apparence), relu à chaque image.
    static THEME: RefCell<(Palette, bool)> = RefCell::new((Palette::sombre(), true));
}

fn is_light() -> bool {
    THEME.with(|t| !t.borrow().1)
}

fn color(pick: impl Fn(&Palette) -> Rgb) -> Color32 {
    THEME.with(|t| {
        let Rgb(r, g, b) = pick(&t.borrow().0);
        Color32::from_rgb(r, g, b)
    })
}

/// Couleurs du thème, par rôle (les nuances de survol et d'appui en sont tirées).
macro_rules! palette {
    ($($name:ident = $pick:expr;)*) => {$(
        #[allow(non_snake_case)]
        fn $name() -> Color32 {
            color($pick)
        }
    )*};
}

palette! {
    BG = |p| p.fond;
    CARD = |p| p.carte;
    FIELD = |p| p.champ;
    HOVER = |p| p.survol;
    HOVER_STRONG = |p| p.survol.mix(p.texte, 0.06);
    PRESSED = |p| p.survol.mix(p.texte, 0.12);
    TOGGLE_OFF = |p| p.bordure.mix(p.texte, 0.1);
    BORDER = |p| p.bordure;
    TEXT = |p| p.texte;
    SUBTLE = |p| p.texte_secondaire;
    ACCENT = |p| p.accent;
    OK = |p| p.succes;
    ERR = |p| p.erreur;
}

/// Relit le thème ; vrai s'il a changé (le style d'egui est alors à refaire).
fn reload_theme() -> bool {
    let now = (theme::current(), theme::is_dark());
    THEME.with(|t| {
        let mut t = t.borrow_mut();
        let changed = *t != now;
        *t = now;
        changed
    })
}

/// Emplacement réel d'un dossier pour l'Explorateur. Claude desktop est une app du Microsoft
/// Store : ce qu'il lance (hooks, BetterClaude, extensions) et qui écrit dans `AppData\Local`
/// écrit en fait dans sa copie privée, `AppData\Local\Packages\Claude_…\LocalCache\Local\`.
/// L'Explorateur, hors de l'app, ne voit que cette copie.
fn real_path(path: &std::path::Path) -> std::path::PathBuf {
    let Ok(local) = std::env::var("LOCALAPPDATA").map(std::path::PathBuf::from) else {
        return path.to_path_buf();
    };
    let Ok(rest) = path.strip_prefix(&local) else {
        return path.to_path_buf();
    };
    let packages = std::fs::read_dir(local.join("Packages")).into_iter().flatten().flatten();
    packages
        .filter(|e| e.file_name().to_string_lossy().to_lowercase().starts_with("claude_"))
        .map(|e| e.path().join("LocalCache").join("Local").join(rest))
        .find(|p| p.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

// ---------------------------------------------------------------- style et petits widgets

/// Polices Segoe UI de Windows et couleurs de Claude, comme GitPush.
fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    let mut load = |key: &str, file: &str| -> bool {
        match std::fs::read(format!("C:\\Windows\\Fonts\\{file}")) {
            Ok(bytes) => {
                fonts.font_data.insert(key.into(), egui::FontData::from_owned(bytes));
                true
            }
            Err(_) => false,
        }
    };
    let has_regular = load("segoe", "segoeui.ttf");
    let has_bold = load("segoe-bold", "segoeuib.ttf");
    let has_symbols = load("segoe-symbol", "seguisym.ttf");
    let prop = fonts.families.get_mut(&FontFamily::Proportional).unwrap();
    if has_regular {
        prop.insert(0, "segoe".into());
    }
    if has_symbols {
        prop.push("segoe-symbol".into());
    }
    let mut bold_family = if has_bold { vec!["segoe-bold".to_string()] } else { Vec::new() };
    bold_family.extend(fonts.families[&FontFamily::Proportional].clone());
    bold_family.extend(fallback);
    fonts.families.insert(FontFamily::Name("bold".into()), bold_family);
    ctx.set_fonts(fonts);
    reload_theme();
    apply_theme(ctx);
    // Les deux thèmes d'egui (il suit le réglage « applications » de Windows) : sinon, en mode
    // clair, boutons et textes reprennent la taille par défaut.
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.button_padding = vec2(12.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(14.5));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(14.5));
        s.text_styles.insert(egui::TextStyle::Small, FontId::proportional(12.5));
    });
}

/// Couleurs du thème de Claude (clair ou sombre) dans le style d'egui.
fn apply_theme(ctx: &egui::Context) {
    let mut v = if is_light() { egui::Visuals::light() } else { egui::Visuals::dark() };
    v.panel_fill = BG();
    v.window_fill = BG();
    v.extreme_bg_color = FIELD();
    v.override_text_color = Some(TEXT());
    v.selection.bg_fill = ACCENT().linear_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT());
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.rounding = Rounding::same(7.0);
    }
    v.widgets.inactive.weak_bg_fill = HOVER();
    v.widgets.inactive.bg_fill = FIELD();
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER());
    v.widgets.hovered.weak_bg_fill = HOVER_STRONG();
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, SUBTLE());
    v.widgets.active.weak_bg_fill = PRESSED();
    v.window_stroke = Stroke::new(1.0_f32, BORDER());
    v.window_rounding = Rounding::same(10.0);
    v.menu_rounding = Rounding::same(8.0);
    // egui suit le thème de Windows : sous Windows en mode clair, il prendrait ses propres
    // couleurs claires (boutons gris « Windows »). Nos couleurs valent pour ses deux thèmes.
    ctx.set_visuals_of(egui::Theme::Dark, v.clone());
    ctx.set_visuals_of(egui::Theme::Light, v);
}

/// Croix de fermeture : fond rouge et croix blanche au survol, comme celle de Windows.
fn close_button(ui: &mut Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 28.0), Sense::click());
    let hovered = resp.hovered();
    if hovered {
        let red = if resp.is_pointer_button_down_on() { Color32::from_rgb(0x94, 0x1e, 0x13) } else { Color32::from_rgb(0xc4, 0x2b, 0x1c) };
        ui.painter().rect_filled(rect, Rounding::same(6.0), red);
    }
    let (c, d) = (rect.center(), 5.5);
    let stroke = Stroke::new(1.6_f32, if hovered { Color32::WHITE } else { TEXT() });
    ui.painter().line_segment([c + vec2(-d, -d), c + vec2(d, d)], stroke);
    ui.painter().line_segment([c + vec2(-d, d), c + vec2(d, -d)], stroke);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Interrupteur activé / désactivé.
fn toggle(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let bg = if *on { ACCENT() } else { TOGGLE_OFF() };
    ui.painter().rect_filled(rect, Rounding::same(10.0), bg);
    let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 7.0, Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Onglet de l'en-tête : texte, souligné à la couleur d'accent quand il est choisi.
fn tab(ui: &mut Ui, text: &str, selected: bool) -> egui::Response {
    let color = if selected { TEXT() } else { SUBTLE() };
    let galley = ui.painter().layout_no_wrap(text.to_string(), bold(15.0), color);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + vec2(4.0, 10.0), Sense::click());
    let color = if resp.hovered() && !selected { TEXT() } else { color };
    ui.painter().galley_with_override_text_color(rect.min + vec2(2.0, 2.0), galley, color);
    if selected {
        ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(2.0_f32, ACCENT()));
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn primary(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.into()).color(Color32::WHITE)).fill(ACCENT())
}

fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let r = egui::Frame::none()
        .fill(CARD())
        .stroke(Stroke::new(1.0_f32, BORDER()))
        .rounding(8.0)
        .inner_margin(Margin::symmetric(14.0, 12.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner;
    ui.add_space(8.0);
    r
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).font(bold(15.0)));
}

// ---------------------------------------------------------------- application

struct Row {
    ext: Extension,
    running: bool,
}

#[derive(PartialEq)]
enum Tab {
    Mine,
    Discover,
    Settings,
    Dev,
}

/// Résultats des travaux faits hors du fil de la fenêtre (GitHub).
enum Job {
    Fetched(Result<Vec<Remote>, String>),
    Installed(Result<Manifest, String>),
    /// « Tout mettre à jour » : le résultat de chaque extension.
    UpdatedAll(Vec<Result<Manifest, String>>),
    /// Connexion : le code à taper sur GitHub, puis la session (ou l'erreur).
    LoginCode(Result<(String, String), String>),
    LoggedIn(Result<Session, String>),
    DevLoaded(Result<Vec<ManagedRepo>, String>),
    DevDone(Result<String, String>),
}

/// Invitation en cours de saisie, pour un dépôt du panneau dev.
#[derive(Default)]
struct Invite {
    login: String,
    read: bool,
}

struct App {
    rows: Vec<Row>,
    state: State,
    hooked: Option<std::path::PathBuf>,
    folder: String,
    message: Option<(String, bool)>,
    /// Thème en cours de réglage (Réglages → Apparence), enregistré à chaque changement.
    theme: theme::Theme,
    confirm_remove: Option<String>,
    refreshed: Instant,
    tab: Tab,
    /// Extensions publiées : None pendant le premier chargement.
    remotes: Option<Result<Vec<Remote>, String>>,
    fetching: bool,
    /// Id de l'extension en cours d'installation.
    installing: Option<String>,
    session: Option<Session>,
    /// Connexion en cours : le code et la page GitHub où le taper (None : en attente du code).
    login: Option<Option<(String, String)>>,
    /// Panneau dev : dépôts privés et leurs accès.
    dev: Option<Result<Vec<ManagedRepo>, String>>,
    dev_busy: bool,
    invites: HashMap<String, Invite>,
    jobs: (Sender<Job>, Receiver<Job>),
}

impl App {
    fn new(ctx: egui::Context) -> App {
        let mut app = App {
            rows: Vec::new(),
            state: State::load(),
            hooked: None,
            folder: setup::default_folder().to_string_lossy().into(),
            message: None,
            theme: theme::Theme::load(),
            confirm_remove: None,
            refreshed: Instant::now(),
            tab: Tab::Mine,
            remotes: None,
            fetching: false,
            installing: None,
            session: Session::load(),
            login: None,
            dev: None,
            dev_busy: false,
            invites: HashMap::new(),
            jobs: channel(),
        };
        app.fetch(ctx);
        app.refresh();
        if let Some(exe) = &app.hooked {
            // Les anciens dossiers (ClaudeExt, BetterExt) ne sont pas proposés : l'installation les remplace.
            if let Some(dir) = exe.parent().filter(|d| !d.ends_with("ClaudeExt") && !d.ends_with("BetterExt")) {
                app.folder = dir.to_string_lossy().into();
            }
        }
        if app.hooked.is_none() {
            app.tab = Tab::Settings;
        }
        app
    }

    fn refresh(&mut self) {
        self.state = State::load();
        self.hooked = setup::hooked_exe();
        self.rows = betterext::installed()
            .into_iter()
            .map(|ext| Row { running: process::is_running(&ext.exe_path()), ext })
            .collect();
        self.refreshed = Instant::now();
    }

    fn report(&mut self, result: Result<String, String>) {
        self.message = Some(match result {
            Ok(m) => (m, true),
            Err(m) => (m, false),
        });
        self.refresh();
    }

    fn save_state(&mut self) -> bool {
        match self.state.save() {
            Ok(()) => true,
            Err(e) => {
                self.report(Err(format!("Le réglage n'a pas pu être enregistré ({e}).")));
                false
            }
        }
    }

    fn set_enabled(&mut self, ext: &Extension, on: bool) {
        self.state.enabled.insert(ext.manifest.id.clone(), on);
        if !self.save_state() {
            return;
        }
        if on {
            if claude::is_open() {
                hook::start(ext);
            }
        } else {
            process::kill_all(&ext.exe_path());
        }
        std::thread::sleep(Duration::from_millis(150));
        self.refresh();
    }

    /// Lance les extensions activées qui ne tournent pas, si Claude est ouvert.
    fn start_active(&self) {
        if claude::is_open() {
            for ext in betterext::active() {
                hook::start(&ext);
            }
        }
    }

    // ------------------------------------------------------------ GitHub, en arrière-plan

    fn spawn(&self, ctx: &egui::Context, work: impl FnOnce() -> Job + Send + 'static) {
        let (tx, ctx) = (self.jobs.0.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
    }

    fn fetch(&mut self, ctx: egui::Context) {
        if !self.fetching {
            self.fetching = true;
            self.spawn(&ctx, || Job::Fetched(discover::fetch()));
        }
    }

    fn install_remote(&mut self, remote: Remote, ctx: &egui::Context) {
        if self.installing.is_none() {
            self.installing = Some(remote.manifest.id.clone());
            self.spawn(ctx, move || Job::Installed(discover::install(&remote)));
        }
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        while let Ok(job) = self.jobs.1.try_recv() {
            match job {
                Job::Fetched(result) => {
                    self.fetching = false;
                    self.remotes = Some(result);
                }
                Job::Installed(result) => {
                    self.installing = None;
                    let result = result.map(|m| {
                        self.start_active();
                        let version = m.version.as_deref().map(|v| format!(" v{v}")).unwrap_or_default();
                        format!("{}{version} est installée.", m.name)
                    });
                    self.report(result);
                }
                Job::UpdatedAll(results) => {
                    self.installing = None;
                    self.start_active();
                    let done: Vec<String> = results.iter().filter_map(|r| r.as_ref().ok().map(|m| m.name.clone())).collect();
                    let failed: Vec<String> = results.into_iter().filter_map(Result::err).collect();
                    let result = if failed.is_empty() { Ok(format!("À jour : {}.", done.join(", "))) } else { Err(failed.join(" ")) };
                    self.report(result);
                }
                Job::LoginCode(Ok((code, url))) => {
                    // Le code est déjà copié : il n'y a qu'à le coller sur la page GitHub.
                    ctx.copy_text(code.clone());
                    let _ = std::process::Command::new("explorer").arg(&url).spawn();
                    self.login = Some(Some((code, url)));
                }
                Job::LoginCode(Err(e)) => {
                    self.login = None;
                    self.report(Err(e));
                }
                Job::LoggedIn(result) => {
                    self.login = None;
                    match result {
                        Ok(session) => {
                            let text = format!("Connecté à GitHub : {}.", session.login);
                            self.session = Some(session);
                            self.report(Ok(text));
                            self.fetch(ctx.clone());
                        }
                        Err(e) => self.report(Err(e)),
                    }
                }
                Job::DevLoaded(result) => {
                    self.dev_busy = false;
                    self.dev = Some(result);
                }
                Job::DevDone(result) => {
                    self.dev_busy = false;
                    self.report(result);
                    self.load_dev(ctx);
                }
            }
        }
    }

    fn start_login(&mut self, ctx: &egui::Context) {
        if self.login.is_some() {
            return;
        }
        self.login = Some(None);
        let (tx, ctx) = (self.jobs.0.clone(), ctx.clone());
        std::thread::spawn(move || {
            let code = github::start_login();
            let shown = code.as_ref().map(|c| (c.user_code.clone(), c.verification_uri.clone())).map_err(Clone::clone);
            let _ = tx.send(Job::LoginCode(shown));
            ctx.request_repaint();
            if let Ok(code) = code {
                let _ = tx.send(Job::LoggedIn(github::finish_login(&code)));
                ctx.request_repaint();
            }
        });
    }

    fn logout(&mut self, ctx: &egui::Context) {
        let result = github::logout().map(|_| "Déconnecté de GitHub.".to_string());
        self.session = None;
        self.dev = None;
        if self.tab == Tab::Dev {
            self.tab = Tab::Mine;
        }
        self.report(result);
        self.fetch(ctx.clone());
    }

    fn load_dev(&mut self, ctx: &egui::Context) {
        let Some(session) = self.session.clone().filter(Session::is_owner) else { return };
        if !self.dev_busy {
            self.dev_busy = true;
            self.spawn(ctx, move || Job::DevLoaded(github::managed_repos(&session)));
        }
    }

    /// Action du panneau dev sur GitHub, puis rechargement des accès.
    fn dev_action(&mut self, ctx: &egui::Context, work: impl FnOnce(&Session) -> Result<String, String> + Send + 'static) {
        let Some(session) = self.session.clone() else { return };
        if !self.dev_busy {
            self.dev_busy = true;
            self.spawn(ctx, move || Job::DevDone(work(&session)));
        }
    }

    /// Version publiée plus récente que celle installée, pour l'extension `id`.
    fn update_for(&self, id: &str, installed: Option<&str>) -> Option<Remote> {
        let remote = self.remotes.as_ref()?.as_ref().ok()?.iter().find(|r| r.manifest.id == id)?;
        discover::is_newer(&remote.version, installed.unwrap_or("0")).then(|| remote.clone())
    }

    // ------------------------------------------------------------ en-tête

    fn header(&mut self, ui: &mut Ui) -> bool {
        let mut close = false;
        // L'en-tête sert de poignée pour déplacer la fenêtre (elle n'a pas de barre de titre).
        let top = ui.max_rect();
        let handle = Rect::from_min_size(top.min - vec2(20.0, 20.0), vec2(top.width() + 40.0, 58.0));
        let drag = ui.interact(handle, ui.id().with("deplacer"), Sense::click_and_drag());
        if drag.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("BetterClaude").font(bold(22.0)).color(ACCENT()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if close_button(ui).on_hover_text("Fermer (Échap)").clicked() {
                    close = true;
                }
                ui.add_space(8.0);
                match self.session.clone() {
                    Some(session) => {
                        ui.menu_button(RichText::new(format!("@{}", session.login)).color(TEXT()), |ui| {
                            if ui.button("Se déconnecter de GitHub").clicked() {
                                ui.close_menu();
                                self.logout(&ui.ctx().clone());
                            }
                        });
                    }
                    None => {
                        self.login_button(ui);
                    }
                }
                ui.add_space(8.0);
                let (text, color) = if self.hooked.is_some() { ("● Actif", OK()) } else { ("● Pas installé", ERR()) };
                ui.label(RichText::new(text).color(color).small());
            });
        });
        if let Some(Some((code, url))) = self.login.clone() {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Collez ce code sur la page GitHub (il est déjà copié) :");
                    ui.label(RichText::new(&code).font(bold(18.0)).color(ACCENT()));
                    if ui.small_button("Copier").clicked() {
                        ui.ctx().copy_text(code.clone());
                    }
                    if ui.small_button("Rouvrir la page").clicked() {
                        let _ = std::process::Command::new("explorer").arg(&url).spawn();
                    }
                });
                ui.label(RichText::new("Validez la connexion sur GitHub, puis revenez ici.").color(SUBTLE()).small());
            });
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            let updates = self
                .rows
                .iter()
                .filter(|r| !r.ext.linked && self.update_for(&r.ext.manifest.id, r.ext.manifest.version.as_deref()).is_some())
                .count();
            let mine = if updates > 0 { format!("Mes extensions ({updates})") } else { "Mes extensions".to_string() };
            if tab(ui, &mine, self.tab == Tab::Mine).clicked() {
                self.tab = Tab::Mine;
            }
            if tab(ui, "Découverte", self.tab == Tab::Discover).clicked() {
                self.tab = Tab::Discover;
            }
            if tab(ui, "Réglages", self.tab == Tab::Settings).clicked() {
                self.tab = Tab::Settings;
            }
            // Panneau dev : seulement pour le propriétaire connecté.
            if self.session.as_ref().is_some_and(Session::is_owner) && tab(ui, "Dev", self.tab == Tab::Dev).clicked() {
                self.tab = Tab::Dev;
                if self.dev.is_none() {
                    self.load_dev(&ui.ctx().clone());
                }
            }
        });
        let y = ui.cursor().top();
        ui.painter().hline(ui.max_rect().x_range(), y, Stroke::new(1.0_f32, BORDER()));
        ui.add_space(10.0);
        close
    }

    // ------------------------------------------------------------ Mes extensions

    fn mine(&mut self, ui: &mut Ui) {
        // Vérifié à l'ouverture de la fenêtre : les mises à jour publiées, d'un clic.
        let updates: Vec<Remote> = self
            .rows
            .iter()
            .filter(|r| !r.ext.linked)
            .filter_map(|r| self.update_for(&r.ext.manifest.id, r.ext.manifest.version.as_deref()))
            .collect();
        if !updates.is_empty() {
            ui.horizontal(|ui| {
                let n = updates.len();
                ui.label(format!("{n} mise{} à jour disponible{}.", if n > 1 { "s" } else { "" }, if n > 1 { "s" } else { "" }));
                let label = if self.installing.is_some() { "Mise à jour…" } else { "Tout mettre à jour" };
                if ui.add_enabled(self.installing.is_none(), primary(label)).clicked() {
                    self.installing = Some(String::new());
                    self.spawn(&ui.ctx().clone(), move || Job::UpdatedAll(updates.iter().map(discover::install).collect()));
                }
            });
            ui.add_space(4.0);
        }
        if self.rows.is_empty() {
            ui.label(RichText::new("Aucune extension pour l'instant. Trouvez-en dans l'onglet Découverte.").color(SUBTLE()));
        }
        let mut toggle_to: Option<(usize, bool)> = None;
        let mut remove: Option<usize> = None;
        let mut update: Option<Remote> = None;
        for (i, row) in self.rows.iter().enumerate() {
            let m = &row.ext.manifest;
            let enabled = self.state.is_enabled(&m.id);
            let newer = (!row.ext.linked).then(|| self.update_for(&m.id, m.version.as_deref())).flatten();
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut on = enabled;
                    if toggle(ui, &mut on).on_hover_text(if enabled { "Désactiver" } else { "Activer" }).changed() {
                        toggle_to = Some((i, on));
                    }
                    ui.add_space(4.0);
                    ui.label(RichText::new(&m.name).font(bold(15.5)));
                    if let Some(v) = &m.version {
                        ui.label(RichText::new(format!("v{v}")).color(SUBTLE()).small());
                    }
                    let (text, color) = if !row.ext.exe_path().exists() {
                        ("fichier manquant, réinstallez-la", ERR())
                    } else if row.running {
                        ("en marche", OK())
                    } else if enabled {
                        ("activée", SUBTLE())
                    } else {
                        ("désactivée", SUBTLE())
                    };
                    ui.label(RichText::new(format!("· {text}")).color(color).small());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.confirm_remove.as_deref() == Some(m.id.as_str()) {
                            if ui.button("Annuler").clicked() {
                                self.confirm_remove = None;
                            }
                            if ui.add(egui::Button::new(RichText::new("Désinstaller").color(Color32::WHITE)).fill(ERR())).clicked() {
                                remove = Some(i);
                            }
                            return;
                        }
                        if !row.ext.linked && ui.button("Désinstaller").clicked() {
                            self.confirm_remove = Some(m.id.clone());
                        }
                        if ui.button("Dossier").clicked() {
                            // Le dossier de l'exe (GitPush : son exe est ailleurs que son
                            // manifeste), sinon celui du manifeste, s'il existe encore.
                            let exe = row.ext.exe_path();
                            let dir = exe
                                .parent()
                                .filter(|d| d.is_dir())
                                .map(|d| d.to_path_buf())
                                .or_else(|| row.ext.dir.is_dir().then(|| row.ext.dir.clone()));
                            match dir {
                                // L'Explorateur s'ouvre sur le dossier, l'exe sélectionné, à son
                                // emplacement réel (copie privée de Claude ou non).
                                Some(_) if exe.is_file() => {
                                    use std::os::windows::process::CommandExt;
                                    let select = format!("/select,\"{}\"", real_path(&exe).display());
                                    let _ = std::process::Command::new("explorer").raw_arg(select).spawn();
                                }
                                Some(dir) => {
                                    let _ = std::process::Command::new("explorer").arg(real_path(&dir)).spawn();
                                }
                                None => self.message = Some((format!("Le dossier de {} est introuvable : réinstallez-la.", m.name), false)),
                            }
                        }
                        if let Some(remote) = &newer {
                            let busy = self.installing.as_deref() == Some(m.id.as_str());
                            let label = if busy { "Mise à jour…".to_string() } else { format!("Mettre à jour (v{})", remote.version) };
                            if ui.add_enabled(self.installing.is_none(), primary(label)).clicked() {
                                update = Some(remote.clone());
                            }
                        }
                    });
                });
                if let Some(d) = &m.description {
                    ui.label(RichText::new(d).color(SUBTLE()));
                }
                if row.ext.linked {
                    ui.label(RichText::new("Fichiers source").color(SUBTLE()).small()).on_hover_text(row.ext.dir.display().to_string());
                }
            });
        }
        if let Some((i, on)) = toggle_to {
            let ext = self.rows[i].ext.clone();
            self.set_enabled(&ext, on);
        }
        if let Some(i) = remove {
            let ext = self.rows[i].ext.clone();
            self.confirm_remove = None;
            let result = setup::remove_extension(&ext).map(|_| format!("{} a été désinstallée.", ext.manifest.name));
            self.report(result);
        }
        if let Some(remote) = update {
            self.install_remote(remote, &ui.ctx().clone());
        }
    }

    // ------------------------------------------------------------ Découverte

    fn discover(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Des extensions à ajouter à Claude, en un clic.").color(SUBTLE()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if self.fetching { "Chargement…" } else { "Rafraîchir" };
                if ui.add_enabled(!self.fetching, egui::Button::new(label)).clicked() {
                    self.fetch(ui.ctx().clone());
                }
            });
        });
        ui.add_space(4.0);
        let remotes = match &self.remotes {
            None => Vec::new(),
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(ERR()));
                Vec::new()
            }
            Some(Ok(list)) => list.clone(),
        };
        if matches!(self.remotes, Some(Ok(_))) && remotes.is_empty() {
            ui.label(RichText::new("Aucune extension disponible pour l'instant.").color(SUBTLE()));
        }
        let mut install: Option<Remote> = None;
        for remote in remotes {
            let m = &remote.manifest;
            let local = self.rows.iter().find(|r| r.ext.manifest.id == m.id && !r.ext.linked);
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&m.name).font(bold(15.5)));
                    if !remote.version.is_empty() {
                        ui.label(RichText::new(format!("v{}", remote.version)).color(SUBTLE()).small());
                    }
                    if remote.private {
                        ui.label(RichText::new("· privée").color(SUBTLE()).small());
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let busy = self.installing.as_deref() == Some(m.id.as_str());
                        let installed = local.map(|r| r.ext.manifest.version.clone().unwrap_or_default());
                        let label = match &installed {
                            _ if busy => Some("Installation…".to_string()),
                            None => Some("Installer".to_string()),
                            Some(v) if discover::is_newer(&remote.version, v) => Some(format!("Mettre à jour (v{})", remote.version)),
                            Some(_) => None,
                        };
                        match label {
                            Some(label) => {
                                if ui.add_enabled(self.installing.is_none(), primary(label)).clicked() {
                                    install = Some(remote.clone());
                                }
                            }
                            None => {
                                ui.label(RichText::new("✓ installée").color(OK()));
                            }
                        }
                    });
                });
                if let Some(d) = &m.description {
                    ui.label(RichText::new(d).color(SUBTLE()));
                }
            });
        }
        if let Some(remote) = install {
            self.install_remote(remote, &ui.ctx().clone());
        }

        if self.session.is_none() {
            section(ui, "Extensions privées");
            card(ui, |ui| {
                ui.label(RichText::new("Connectez-vous à GitHub pour voir aussi les extensions privées qu'on vous a partagées.").color(SUBTLE()));
                self.login_button(ui);
            });
        }
    }

    fn login_button(&mut self, ui: &mut Ui) {
        let label = if self.login.is_some() { "Connexion…" } else { "Se connecter à GitHub" };
        if ui.add_enabled(self.login.is_none(), primary(label)).clicked() {
            self.start_login(&ui.ctx().clone());
        }
    }

    // ------------------------------------------------------------ Dev

    fn dev(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Qui a accès à tes dépôts privés. Visible seulement pour toi.").color(SUBTLE()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if self.dev_busy { "Chargement…" } else { "Rafraîchir" };
                if ui.add_enabled(!self.dev_busy, egui::Button::new(label)).clicked() {
                    self.load_dev(&ui.ctx().clone());
                }
            });
        });
        ui.add_space(4.0);
        let repos = match &self.dev {
            None => return,
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(ERR()));
                return;
            }
            Some(Ok(repos)) => repos.clone(),
        };
        if repos.is_empty() {
            ui.label(RichText::new("Aucun dépôt privé.").color(SUBTLE()));
        }
        let ctx = ui.ctx().clone();
        // Action choisie dans la liste : (dépôt, personne, Some(lecture) ou None pour retirer).
        let mut action: Option<(String, Person, Option<bool>)> = None;
        let mut invite: Option<(String, String, bool)> = None;
        for repo in &repos {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&repo.full_name).font(bold(15.5)));
                    if !repo.in_org {
                        ui.label(RichText::new("· compte perso : pas de lecture seule").color(SUBTLE()).small())
                            .on_hover_text("Sur un compte personnel, GitHub donne toujours le droit de modifier. Transfère le dépôt dans l'organisation (Settings → Transfer sur GitHub) pour la lecture seule.");
                    }
                });
                if repo.people.is_empty() {
                    ui.label(RichText::new("Personne d'autre que toi.").color(SUBTLE()));
                }
                for person in &repo.people {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&person.login).strong());
                        if person.invitation.is_some() {
                            ui.label(RichText::new("· invitation envoyée").color(SUBTLE()).small());
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add_enabled(!self.dev_busy, egui::Button::new("Retirer")).clicked() {
                                action = Some((repo.full_name.clone(), person.clone(), None));
                            }
                            let read = matches!(person.role.as_str(), "read" | "pull");
                            let known = read || matches!(person.role.as_str(), "write" | "push");
                            ui.add_enabled_ui(repo.in_org && !self.dev_busy, |ui| {
                                if ui.selectable_label(known && !read, "Modification").clicked() && read {
                                    action = Some((repo.full_name.clone(), person.clone(), Some(false)));
                                }
                                if ui.selectable_label(read, "Lecture").clicked() && !read {
                                    action = Some((repo.full_name.clone(), person.clone(), Some(true)));
                                }
                            });
                            if !known {
                                ui.label(RichText::new(&person.role).color(SUBTLE()).small());
                            }
                        });
                    });
                }
                ui.horizontal(|ui| {
                    let entry = self.invites.entry(repo.full_name.clone()).or_default();
                    ui.add(egui::TextEdit::singleline(&mut entry.login).hint_text("Nom GitHub à inviter").desired_width(200.0));
                    if repo.in_org {
                        ui.selectable_value(&mut entry.read, true, "Lecture");
                        ui.selectable_value(&mut entry.read, false, "Modification");
                    }
                    let can = !self.dev_busy && !entry.login.trim().is_empty();
                    if ui.add_enabled(can, primary("Inviter")).clicked() {
                        invite = Some((repo.full_name.clone(), entry.login.trim().to_string(), entry.read && repo.in_org));
                        entry.login.clear();
                    }
                });
            });
        }
        if let Some((repo, person, change)) = action {
            self.dev_action(&ctx, move |s| match change {
                None => github::remove_access(s, &repo, &person).map(|_| format!("{} n'a plus accès à {repo}.", person.login)),
                Some(read) => {
                    let result = match person.invitation {
                        Some(id) => github::set_invitation(s, &repo, id, read),
                        None => github::set_access(s, &repo, &person.login, read),
                    };
                    let role = if read { "lecture seule" } else { "modification" };
                    result.map(|_| format!("{} : {role} sur {repo}.", person.login))
                }
            });
        }
        if let Some((repo, login, read)) = invite {
            self.dev_action(&ctx, move |s| {
                github::set_access(s, &repo, &login, read).map(|_| format!("{login} est invité sur {repo} : il doit accepter l'invitation (mail ou github.com)."))
            });
        }
    }

    // ------------------------------------------------------------ Réglages

    fn settings(&mut self, ui: &mut Ui) {
        section(ui, "BetterClaude");
        card(ui, |ui| {
            let about = if self.hooked.is_some() {
                "BetterClaude est actif : vos extensions démarrent en même temps que Claude."
            } else {
                "Choisissez où installer BetterClaude. Vos extensions seront rangées au même endroit."
            };
            ui.label(RichText::new(about).color(SUBTLE()));
            ui.horizontal(|ui| {
                ui.label("Emplacement");
                ui.add(egui::TextEdit::singleline(&mut self.folder).desired_width(ui.available_width() - 100.0));
                if ui.button("Parcourir…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().set_directory(&self.folder).pick_folder() {
                        self.folder = dir.to_string_lossy().into();
                    }
                }
            });
            ui.horizontal(|ui| {
                let label = if self.hooked.is_some() { "Réinstaller" } else { "Installer" };
                if ui.add(primary(label)).clicked() {
                    let folder = self.folder.clone();
                    let result = setup::install(&folder).map(|_| "BetterClaude est installé. Vos extensions démarreront avec Claude.".to_string());
                    self.report(result);
                }
                if self.hooked.is_some()
                    && ui.button("Désactiver").on_hover_text("Plus aucune extension ne démarrera avec Claude.").clicked()
                {
                    let result = setup::uninstall().map(|_| "BetterClaude est désactivé : les extensions ne démarreront plus avec Claude.".to_string());
                    self.report(result);
                }
            });
        });

        section(ui, "Apparence");
        card(ui, |ui| {
            ui.label(RichText::new("Les couleurs de BetterClaude et de vos extensions.").color(SUBTLE()));
            let before = self.theme.clone();
            ui.horizontal(|ui| {
                ui.label("Thème");
                ui.selectable_value(&mut self.theme.mode, theme::Mode::Claude, "Comme Claude");
                ui.selectable_value(&mut self.theme.mode, theme::Mode::Light, "Clair");
                ui.selectable_value(&mut self.theme.mode, theme::Mode::Dark, "Sombre");
            });
            for (title, dark) in [("Couleurs du thème sombre", true), ("Couleurs du thème clair", false)] {
                egui::CollapsingHeader::new(title).id_salt(title).show(ui, |ui| {
                    let palette = if dark { &mut self.theme.dark } else { &mut self.theme.light };
                    egui::Grid::new(title).num_columns(4).spacing(vec2(14.0, 6.0)).show(ui, |ui| {
                        for (i, (name, rgb)) in palette.roles_mut().into_iter().enumerate() {
                            let mut c = [rgb.0, rgb.1, rgb.2];
                            if egui::color_picker::color_edit_button_srgb(ui, &mut c).changed() {
                                *rgb = Rgb(c[0], c[1], c[2]);
                            }
                            ui.label(name);
                            if i % 2 == 1 {
                                ui.end_row();
                            }
                        }
                    });
                    let default = if dark { Palette::sombre() } else { Palette::clair() };
                    if *palette != default && ui.small_button("Revenir aux couleurs de Claude").clicked() {
                        *palette = default;
                    }
                });
            }
            if self.theme != before {
                if let Err(e) = self.theme.save() {
                    self.report(Err(format!("Le thème n'a pas pu être enregistré ({e}).")));
                }
            }
        });

        section(ui, "Dossier des extensions");
        card(ui, |ui| {
            ui.label(RichText::new("Vos extensions sont installées ici, chacune dans son dossier.").color(SUBTLE()));
            let current = betterext::extensions_dir();
            ui.label(current.display().to_string());
            ui.horizontal(|ui| {
                if ui.button("Ouvrir").clicked() {
                    let _ = std::fs::create_dir_all(&current);
                    let _ = std::process::Command::new("explorer").arg(real_path(&current)).spawn();
                }
                let change = ui.button("Changer…").on_hover_text("Vos extensions seront déplacées dans le nouveau dossier.");
                if change.clicked() {
                    if let Some(dir) = rfd::FileDialog::new().set_title("Nouveau dossier pour vos extensions").pick_folder() {
                        let result = setup::move_extensions(&dir).map(|n| match n {
                            0 => format!("Vos extensions seront installées dans {}.", dir.display()),
                            1 => format!("Votre extension a été déplacée dans {}.", dir.display()),
                            n => format!("Vos {n} extensions ont été déplacées dans {}.", dir.display()),
                        });
                        self.report(result);
                    }
                }
                let add = ui
                    .button("Ajouter une extension…")
                    .on_hover_text("Pour une extension reçue en fichier : choisissez son fichier extension.json.");
                if add.clicked() {
                    let picked = rfd::FileDialog::new()
                        .set_title("Fichier extension.json de l'extension")
                        .add_filter("extension.json", &["json"])
                        .pick_file();
                    if let Some(file) = picked {
                        let result = setup::add_extension(&file).map(|m| {
                            self.start_active();
                            format!("{} est installée.", m.name)
                        });
                        self.report(result);
                    }
                }
            });
        });
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BG().to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_jobs(ctx);
        // Thème changé (réglage, ou Claude passé en clair / sombre) : la fenêtre suit.
        if reload_theme() {
            apply_theme(ctx);
        }
        if self.refreshed.elapsed() > Duration::from_secs(2) {
            self.refresh();
        }
        ctx.request_repaint_after(Duration::from_secs(2));
        let mut close = ctx.input(|i| i.key_pressed(Key::Escape));
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG()).stroke(Stroke::new(1.0_f32, BORDER())).inner_margin(Margin::same(20.0)))
            .show(ctx, |ui| {
                close |= self.header(ui);
                let bottom = 34.0;
                egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(ui.available_height() - bottom).show(ui, |ui| {
                    match self.tab {
                        Tab::Mine => self.mine(ui),
                        Tab::Discover => self.discover(ui),
                        Tab::Settings => self.settings(ui),
                        Tab::Dev => self.dev(ui),
                    }
                });
                if let Some((text, ok)) = &self.message {
                    ui.add_space(6.0);
                    ui.label(RichText::new(text).color(if *ok { OK() } else { ERR() }));
                }
            });
        if close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// Coins arrondis de Windows 11 pour une fenêtre sans barre de titre.
fn round_corners(hwnd: isize) {
    use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
    let pref = DWMWCP_ROUND;
    unsafe {
        DwmSetWindowAttribute(
            hwnd as _,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &pref as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&pref) as u32,
        );
    }
}

/// Centre la fenêtre sur Claude, sur l'écran où il se trouve, en pixels réels (à l'échelle de
/// cet écran), et la garde entière dans sa zone de travail.
fn center_on_claude(hwnd: isize, size: [f32; 2]) {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};
    let Some((rect, scale)) = claude::rect() else { return };
    let (w, h) = ((size[0] * scale).round() as i32, (size[1] * scale).round() as i32);
    let mut x = rect.x + (rect.w - w) / 2;
    let mut y = rect.y + (rect.h - h) / 2;
    unsafe {
        let monitor = MonitorFromWindow(claude::window(), MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) != 0 {
            let work = info.rcWork;
            x = x.min(work.right - w).max(work.left);
            y = y.min(work.bottom - h).max(work.top);
        }
        SetWindowPos(hwnd as _, std::ptr::null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Ramène la fenêtre BetterClaude déjà ouverte (même réduite, même sur un autre écran) au
/// centre de Claude, devant : un clic sur le puzzle la retrouve toujours là.
pub fn focus_existing() {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowRect, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE};
    let title: Vec<u16> = TITLE.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let w = FindWindowW(std::ptr::null(), title.as_ptr());
        if !w.is_null() {
            if IsIconic(w) != 0 {
                ShowWindow(w, SW_RESTORE);
            }
            // Sa taille actuelle, ramenée en points (l'écran de Claude peut avoir une autre échelle).
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetWindowRect(w, &mut r);
            let scale = (GetDpiForWindow(w) as f32 / 96.0).max(1.0);
            let size = [(r.right - r.left) as f32 / scale, (r.bottom - r.top) as f32 / scale];
            center_on_claude(w as isize, size);
            SetForegroundWindow(w);
        }
    }
}

/// La fenêtre, centrée sur Claude à l'ouverture.
pub fn run() {
    let size = [680.0_f32, 640.0];
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(TITLE)
        .with_inner_size(size)
        .with_min_inner_size([520.0, 420.0])
        .with_decorations(false)
        .with_icon(egui::IconData { rgba: crate::icon::rgba(64), width: 64, height: 64 })
        .with_active(true);
    // Position de départ approximative (sans la borner à 0 : un écran à gauche de l'écran
    // principal a des coordonnées négatives) ; `center_on_claude` la corrige au pixel près.
    if let Some((rect, scale)) = claude::rect() {
        let x = (rect.x as f32 + (rect.w as f32 - size[0] * scale) / 2.0) / scale;
        let y = (rect.y as f32 + (rect.h as f32 - size[1] * scale) / 2.0) / scale;
        viewport = viewport.with_position([x, y]);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let _ = eframe::run_native(
        TITLE,
        options,
        Box::new(|cc| {
            setup_style(&cc.egui_ctx);
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(RawWindowHandle::Win32(h)) = cc.window_handle().map(|h| h.as_raw()) {
                round_corners(h.hwnd.get());
                center_on_claude(h.hwnd.get(), size);
            }
            Ok(Box::new(App::new(cc.egui_ctx.clone())))
        }),
    );
}
