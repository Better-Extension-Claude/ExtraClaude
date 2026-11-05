//! Mise à jour d'ExtraClaude lui-même (dernière release du dépôt ExtraClaude).
//!
//! ExtraClaude se remplace sur place : la nouvelle version est téléchargée à côté
//! (`ExtraClaude.new.exe`), l'exe en marche est renommé `ExtraClaude.old.exe` (Windows
//! permet de renommer un exe en marche, pas de l'écraser), puis la nouvelle prend sa place.
//! L'ancienne copie est effacée au lancement suivant.
//!
//! Automatique : à l'ouverture de Claude (bouton puzzle) et à l'ouverture de la fenêtre ExtraClaude.

use crate::{discover, github, setup};
use betterext::process;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const REPO: &str = "Better-Extension-Claude/ExtraClaude";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Version de cet exe.
pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Dernière version publiée, lue dans la redirection de `releases/latest` (sans l'API
/// GitHub : pas de limite de requêtes).
pub fn latest() -> Result<String, String> {
    let url = format!("https://github.com/{REPO}/releases/latest");
    let out = Command::new("curl.exe")
        .args(["-s", "-o", "NUL", "--max-time", "30", "-w", "%{redirect_url}", &url])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("curl.exe introuvable : {e}"))?;
    let location = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let tag = location.rsplit_once("/tag/").map(|(_, t)| t.to_string()).filter(|t| !t.is_empty());
    tag.map(|t| t.trim_start_matches('v').to_string()).ok_or_else(|| "GitHub injoignable : impossible de vérifier les mises à jour.".into())
}

/// Nouvelle version disponible, s'il y en a une.
pub fn available() -> Result<Option<String>, String> {
    let latest = latest()?;
    Ok(discover::is_newer(&latest, current()).then_some(latest))
}

/// L'exe installé, si c'est bien lui qui tourne : une copie de travail (compilée, ou lancée
/// depuis Téléchargements avant l'installation) ne se remplace pas elle-même.
pub fn installed_self() -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    let hooked = setup::hooked_exe()?;
    setup::same_path(&me, &hooked).then_some(me)
}

fn old_copy(exe: &Path) -> PathBuf {
    exe.with_file_name("ExtraClaude.old.exe")
}

/// Efface l'ancienne copie laissée par la dernière mise à jour (au lancement).
pub fn cleanup() {
    if let Ok(me) = std::env::current_exe() {
        let old = old_copy(&me);
        if old.exists() && !setup::same_path(&old, &me) {
            let _ = std::fs::remove_file(old);
        }
    }
}

/// Télécharge `version` et remplace l'exe installé. Renvoie son chemin.
pub fn apply(version: &str) -> Result<PathBuf, String> {
    let exe = installed_self().ok_or("Seul ExtraClaude installé peut se mettre à jour.")?;
    let url = format!("https://github.com/{REPO}/releases/download/v{version}/ExtraClaude.exe");
    let bytes = github::request("GET", &url, None, None, "application/octet-stream")?;
    if bytes.len() < 1_000_000 || !bytes.starts_with(b"MZ") {
        return Err("Le fichier téléchargé n'est pas valide : rien n'a été changé.".into());
    }
    let new = exe.with_file_name("ExtraClaude.new.exe");
    std::fs::write(&new, &bytes).map_err(|e| format!("Écriture impossible : {e}"))?;
    let old = old_copy(&exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| {
        let _ = std::fs::remove_file(&new);
        format!("Remplacement impossible : {e}")
    })?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("Remplacement impossible : {e}"));
    }
    Ok(exe)
}

/// Après [`apply`] : relance le bouton puzzle avec la nouvelle version (l'ancien tourne
/// encore sous le nom `ExtraClaude.old.exe`).
pub fn restart_button(exe: &Path) {
    process::kill_all(&old_copy(exe));
    if betterext::claude::is_open() {
        let _ = process::spawn_detached(exe, &["--button".into(), "--relaunch".into()]);
    }
}

/// Mise à jour automatique d'ExtraClaude, depuis le bouton puzzle, une fois à l'ouverture
/// de Claude. Les extensions, elles, ne se mettent à jour que sur demande.
pub fn watch() {
    std::thread::spawn(|| {
        // Laisse Claude et les extensions démarrer d'abord.
        std::thread::sleep(Duration::from_secs(20));
        if betterext::State::load().self_update() && installed_self().is_some() {
            if let Ok(Some(version)) = available() {
                if let Ok(exe) = apply(&version) {
                    // Ce bouton est l'ancienne version : la nouvelle prend sa place.
                    let _ = process::spawn_detached(&exe, &["--button".into(), "--relaunch".into()]);
                    std::process::exit(0);
                }
            }
        }
    });
}
