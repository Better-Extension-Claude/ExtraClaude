//! Découverte : les extensions publiées sur GitHub (organisation des extensions et compte
//! du propriétaire). Un dépôt est une extension quand sa dernière release contient
//! `extension.json` ; installer copie tous les fichiers de la release dans `extensions\<id>\`.
//!
//! Les dépôts publics se voient sans rien. Les privés se voient avec le compte GitHub
//! connecté dans BetterClaude, s'il y a accès (GitHub CLI connecté compte aussi).

use crate::github::{self, Session};
use betterext::{extensions_dir, process, Manifest};
use serde_json::Value;
use std::collections::BTreeMap;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone)]
pub struct Asset {
    pub id: u64,
    pub name: String,
}

/// Extension publiée : manifeste de sa dernière release, et le jeton qui l'ouvre (privée).
#[derive(Clone)]
pub struct Remote {
    pub repo: String,
    pub private: bool,
    pub manifest: Manifest,
    pub version: String,
    pub assets: Vec<Asset>,
    token: Option<String>,
}

/// Jeton de GitHub CLI, s'il est installé et connecté (facultatif).
fn gh_token() -> Option<String> {
    let known = Path::new(&std::env::var("ProgramFiles").unwrap_or_default()).join("GitHub CLI").join("gh.exe");
    let gh = if known.exists() { known.to_string_lossy().to_string() } else { "gh".to_string() };
    let out = Command::new(gh).args(["auth", "token"]).creation_flags(CREATE_NO_WINDOW).output().ok()?;
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !token.is_empty()).then_some(token)
}

fn get(path: &str, token: Option<&str>) -> Result<Value, String> {
    github::get(path, token)
}

fn asset(repo: &str, id: u64, token: Option<&str>) -> Result<Vec<u8>, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/assets/{id}");
    github::request("GET", &url, None, token, "application/octet-stream")
}

/// (dépôt, privé) d'une liste de dépôts GitHub, limitée aux comptes lus par Découverte.
fn ours(list: &Value) -> Vec<(String, bool)> {
    let owners = github::owners();
    list.as_array()
        .into_iter()
        .flatten()
        .filter(|r| {
            let owner = r.pointer("/owner/login").and_then(Value::as_str).unwrap_or_default();
            owners.iter().any(|o| o.eq_ignore_ascii_case(owner)) && !r.get("archived").and_then(Value::as_bool).unwrap_or(false)
        })
        .filter_map(|r| Some((r.get("full_name")?.as_str()?.to_string(), r.get("private").and_then(Value::as_bool).unwrap_or(false))))
        .collect()
}

/// Dépôts visibles : publics sans compte, puis ceux du compte connecté (privés compris),
/// chacun avec le jeton qui l'ouvre.
fn repos() -> Result<BTreeMap<String, (bool, Option<String>)>, String> {
    let mut repos = BTreeMap::new();
    let mut any = false;
    let mut last_error = None;
    for owner in github::owners() {
        match get(&format!("users/{owner}/repos?per_page=100"), None) {
            Ok(list) => {
                any = true;
                for (name, private) in ours(&list) {
                    repos.insert(name, (private, None));
                }
            }
            Err(e) => last_error = Some(e),
        }
    }
    let tokens = Session::load().map(|s| s.token).into_iter().chain(gh_token());
    for token in tokens {
        if let Ok(list) = get("user/repos?per_page=100&affiliation=owner,collaborator,organization_member", Some(&token)) {
            any = true;
            for (name, private) in ours(&list) {
                let entry = repos.entry(name).or_insert((private, None));
                if entry.1.is_none() && private {
                    entry.1 = Some(token.clone());
                }
            }
        }
    }
    if !any {
        return Err(last_error.unwrap_or_else(|| "GitHub injoignable.".into()));
    }
    Ok(repos)
}

/// Extensions publiées, triées par nom.
pub fn fetch() -> Result<Vec<Remote>, String> {
    let mut list = Vec::new();
    for (repo, (private, token)) in repos()? {
        let Ok(release) = get(&format!("repos/{repo}/releases/latest"), token.as_deref()) else { continue };
        let assets: Vec<Asset> = release
            .get("assets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| Some(Asset { id: a.get("id")?.as_u64()?, name: a.get("name")?.as_str()?.to_string() }))
            .collect();
        let Some(json) = assets.iter().find(|a| a.name == "extension.json") else { continue };
        let Ok(bytes) = asset(&repo, json.id, token.as_deref()) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let Ok(manifest) = serde_json::from_str::<Manifest>(text.trim_start_matches('\u{feff}')) else { continue };
        let tag = release.get("tag_name").and_then(Value::as_str).unwrap_or_default();
        let version = manifest.version.clone().unwrap_or_else(|| tag.trim_start_matches('v').to_string());
        list.push(Remote { repo, private, manifest, version, assets, token });
    }
    list.sort_by_key(|r| r.manifest.name.to_lowercase());
    Ok(list)
}

/// Installe ou met à jour l'extension : les fichiers de la release sont téléchargés à
/// part, puis l'ancienne version est fermée et remplacée. Les réglages (activée…) restent.
pub fn install(remote: &Remote) -> Result<Manifest, String> {
    let id = &remote.manifest.id;
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(format!("Identifiant « {id} » invalide dans {}.", remote.repo));
    }
    // Préparé à côté du dossier des extensions (même volume, pas dans la liste).
    let staging = betterext::home_dir().join(".telechargement").join(id);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    for file in &remote.assets {
        if file.name.contains('/') || file.name.contains('\\') || file.name.starts_with('.') {
            continue;
        }
        let bytes = asset(&remote.repo, file.id, remote.token.as_deref())?;
        std::fs::write(staging.join(&file.name), bytes).map_err(|e| e.to_string())?;
    }
    let dest = extensions_dir().join(betterext::folder_name(&remote.manifest));
    let old = betterext::installed().into_iter().find(|e| &e.manifest.id == id && !e.linked);
    if let Some(old) = &old {
        // Un exe en marche est verrouillé : on attend que ses processus soient fermés.
        let exe = old.exe_path();
        process::kill_all(&exe);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while process::is_running(&exe) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    // L'ancienne version (son dossier peut porter un autre nom), puis la place de la nouvelle.
    if let Some(old) = old.filter(|o| o.dir.starts_with(extensions_dir()) || o.dir.starts_with(betterext::data_dir())) {
        std::fs::remove_dir_all(&old.dir).map_err(|e| format!("Remplacement impossible : {e}"))?;
    }
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| format!("Remplacement impossible : {e}"))?;
    }
    std::fs::create_dir_all(extensions_dir()).map_err(|e| e.to_string())?;
    std::fs::rename(&staging, &dest).map_err(|e| format!("Installation impossible : {e}"))?;
    crate::hook::restart_installed(id);
    Ok(remote.manifest.clone())
}

/// Vrai si `remote` est plus récente que `installed` (versions « 1.2.3 », « v1.2 »…).
pub fn is_newer(remote: &str, installed: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v').split(['.', '-']).map(|p| p.parse().unwrap_or(0)).collect()
    };
    parts(remote) > parts(installed)
}
