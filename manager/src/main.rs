#![windows_subsystem = "windows"]

mod button;
mod discover;
mod github;
mod hook;
mod icon;
mod keys;
mod setup;
mod ui;

/// Modes de BetterClaude.exe :
/// - `--hook` : appelé par Claude à chaque événement (JSON sur l'entrée standard) ;
/// - `--button` : le bouton puzzle sur Claude, lancé en début de session ;
/// - `--install-to <dossier>` : installation sans fenêtre (hooks compris) ;
/// - sans argument : la fenêtre (extensions, Découverte, installation).
fn main() {
    betterext::migrate_from_claudeext();
    betterext::migrate_extensions();
    let args: Vec<String> = std::env::args().collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if has("--hook") {
        hook::run();
        return;
    }
    if has("--button") {
        if betterext::process::single_instance(r"Local\BetterClaude-bouton") {
            button::run();
        }
        return;
    }
    // build.sh : écrit le logo comme icône de l'exe compilé.
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    if let Some(exe) = value("--embed-icon") {
        let result = icon::embed(std::path::Path::new(&exe));
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    if let Some(path) = value("--icon-preview") {
        let _ = icon::write_png(std::path::Path::new(&path), 256);
        return;
    }
    // Diagnostic : ce que le panneau dev voit (jeton GitHub lu sur l'entrée standard).
    if has("--dev-list") {
        let mut token = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut token);
        let user = github::get("user", Some(token.trim()));
        let login = user.ok().and_then(|u| u.get("login")?.as_str().map(String::from)).unwrap_or_default();
        let session = github::Session { login, token: token.trim().to_string() };
        match github::managed_repos(&session) {
            Ok(repos) => {
                for r in repos {
                    println!("{} (organisation : {})", r.full_name, r.in_org);
                    for p in r.people {
                        println!("  {} : {}{}", p.login, p.role, if p.invitation.is_some() { " (invitation)" } else { "" });
                    }
                }
            }
            Err(e) => println!("erreur : {e}"),
        }
        return;
    }
    // Diagnostic : ce que l'onglet Découverte voit.
    if has("--discover") {
        match discover::fetch() {
            Ok(list) => list.iter().for_each(|r| println!("{}\t{}\tv{}\tprivée={}", r.manifest.id, r.repo, r.version, r.private)),
            Err(e) => println!("erreur : {e}"),
        }
        return;
    }
    // Installe une extension depuis son extension.json (comme « Ajouter » dans la fenêtre).
    if let Some(manifest) = value("--add") {
        let ok = setup::add_extension(std::path::Path::new(&manifest)).is_ok();
        std::process::exit(if ok { 0 } else { 1 });
    }
    // Déplace les extensions installées (comme Réglages → Changer…).
    if let Some(dir) = value("--move-extensions") {
        let result = setup::move_extensions(std::path::Path::new(&dir));
        if let Err(e) = &result {
            eprintln!("{e}");
        }
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    if let Some(i) = args.iter().position(|a| a == "--install-to") {
        let ok = setup::install(args.get(i + 1).map(String::as_str).unwrap_or("")).is_ok();
        std::process::exit(if ok { 0 } else { 1 });
    }
    if betterext::process::single_instance(r"Local\BetterClaude-fenetre") {
        ui::run();
    } else {
        ui::focus_existing();
    }
}
