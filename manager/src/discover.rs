//! Découverte : les extensions publiées sur GitHub.
//!
//! La liste vient du **catalogue** (`catalogue.json` du dépôt BetterClaude) : chaque extension
//! y est donnée par son dépôt. Une seule requête, hors de l'API GitHub (pas de limite), puis
//! chaque dépôt est lu **en même temps** que les autres :
//! - public : `extension.json` de la dernière release par le lien direct, sans compte ;
//! - privé : par l'API, avec le compte GitHub connecté (ou GitHub CLI). Sans accès,
//!   l'extension reste affichée (nom et description du catalogue) mais ne s'installe pas.
//!
//! Avec un compte connecté, ses dépôts dans l'organisation s'ajoutent à la liste (une
//! extension pas encore au catalogue). Installer copie tous les fichiers de la release.

use crate::github::{self, Session};
use betterext::{extensions_dir, process, Manifest};
use serde::Deserialize;
use serde_json::Value;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Catalogue des extensions, lu sans passer par l'API GitHub.
const CATALOGUE: &str = "https://raw.githubusercontent.com/Better-Extension-Claude/BetterClaude/main/catalogue.json";

#[derive(Clone)]
pub struct Asset {
    pub id: u64,
    pub name: String,
    /// Lien de téléchargement direct (github.com) : pas compté dans la limite de l'API.
    pub url: String,
}

/// Extension publiée : manifeste de sa dernière release, et le jeton qui l'ouvre (privée).
#[derive(Clone)]
pub struct Remote {
    pub repo: String,
    pub private: bool,
    /// Faux : dépôt privé auquel le compte n'a pas accès (manifeste tiré du catalogue).
    pub access: bool,
    pub manifest: Manifest,
    pub version: String,
    /// Vide pour un dépôt public lu sans l'API : la liste est lue à l'installation.
    pub assets: Vec<Asset>,
    token: Option<String>,
}

/// Entrée du catalogue.
#[derive(Clone, Deserialize)]
struct Entry {
    repo: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
}

/// Jeton de GitHub CLI, s'il est installé et connecté (facultatif).
fn gh_token() -> Option<String> {
    let known = Path::new(&std::env::var("ProgramFiles").unwrap_or_default()).join("GitHub CLI").join("gh.exe");
    let gh = if known.exists() { known.to_string_lossy().to_string() } else { "gh".to_string() };
    let out = Command::new(gh).args(["auth", "token"]).creation_flags(CREATE_NO_WINDOW).output().ok()?;
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !token.is_empty()).then_some(token)
}

fn tokens() -> Vec<String> {
    let mut list: Vec<String> = Session::load().map(|s| s.token).into_iter().chain(gh_token()).collect();
    list.dedup();
    list
}

fn get(path: &str, token: Option<&str>) -> Result<Value, String> {
    github::get(path, token)
}

/// Fichier d'une release. Sans compte (dépôt public) : par le lien direct, que la limite de
/// 60 requêtes par heure de l'API sans compte ne bloque pas.
fn asset(repo: &str, file: &Asset, token: Option<&str>) -> Result<Vec<u8>, String> {
    if token.is_none() && !file.url.is_empty() {
        return github::request("GET", &file.url, None, None, "application/octet-stream");
    }
    let url = format!("https://api.github.com/repos/{repo}/releases/assets/{}", file.id);
    github::request("GET", &url, None, token, "application/octet-stream")
}

fn parse_manifest(bytes: &[u8]) -> Option<Manifest> {
    serde_json::from_str(String::from_utf8_lossy(bytes).trim_start_matches('\u{feff}')).ok()
}

/// Fichiers de la dernière release, par l'API, et sa version (tag sans « v »).
fn release_assets(repo: &str, token: Option<&str>) -> Result<(Vec<Asset>, String), String> {
    let release = get(&format!("repos/{repo}/releases/latest"), token)?;
    let assets = release
        .get("assets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| {
            Some(Asset {
                id: a.get("id")?.as_u64()?,
                name: a.get("name")?.as_str()?.to_string(),
                url: a.get("browser_download_url").and_then(Value::as_str).unwrap_or_default().to_string(),
            })
        })
        .collect();
    let tag = release.get("tag_name").and_then(Value::as_str).unwrap_or_default().trim_start_matches('v').to_string();
    Ok((assets, tag))
}

/// Lit une extension : d'abord comme dépôt public (lien direct), sinon avec chaque jeton.
fn read(entry: &Entry, tokens: &[String]) -> Option<Remote> {
    let repo = entry.repo.clone();
    let direct = format!("https://github.com/{repo}/releases/latest/download/extension.json");
    if let Some(manifest) = github::request("GET", &direct, None, None, "application/octet-stream").ok().and_then(|b| parse_manifest(&b)) {
        let version = manifest.version.clone().unwrap_or_default();
        return Some(Remote { repo, private: false, access: true, manifest, version, assets: Vec::new(), token: None });
    }
    for token in tokens {
        let Ok((assets, tag)) = release_assets(&repo, Some(token)) else { continue };
        let json = assets.iter().find(|a| a.name == "extension.json")?;
        let manifest = asset(&repo, json, Some(token)).ok().and_then(|b| parse_manifest(&b))?;
        let version = manifest.version.clone().unwrap_or(tag);
        return Some(Remote { repo, private: true, access: true, manifest, version, assets, token: Some(token.clone()) });
    }
    // Privée sans accès : ce que dit le catalogue.
    if entry.name.is_empty() {
        return None;
    }
    let id = if entry.id.is_empty() { repo.rsplit('/').next().unwrap_or_default().to_lowercase() } else { entry.id.clone() };
    let manifest = Manifest { id, name: entry.name.clone(), description: entry.description.clone(), ..Default::default() };
    Some(Remote { repo, private: true, access: false, manifest, version: String::new(), assets: Vec::new(), token: None })
}

/// Dépôts de l'organisation que le compte connecté voit (privés compris), en plus du catalogue.
fn account_repos(tokens: &[String]) -> Vec<String> {
    for token in tokens {
        let Ok(list) = get("user/repos?per_page=100&affiliation=owner,collaborator,organization_member", Some(token)) else { continue };
        return list
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| {
                let owner = r.pointer("/owner/login").and_then(Value::as_str).unwrap_or_default();
                github::owners().iter().any(|o| o.eq_ignore_ascii_case(owner)) && !r.get("archived").and_then(Value::as_bool).unwrap_or(false)
            })
            .filter_map(|r| Some(r.get("full_name")?.as_str()?.to_string()))
            .collect();
    }
    Vec::new()
}

/// Extensions publiées, triées par nom.
pub fn fetch() -> Result<Vec<Remote>, String> {
    let (catalogue, (tokens, extra)) = std::thread::scope(|s| {
        let accounts = s.spawn(|| {
            let tokens = tokens();
            let extra = account_repos(&tokens);
            (tokens, extra)
        });
        let catalogue = github::request("GET", CATALOGUE, None, None, "application/json")
            .and_then(|b| serde_json::from_slice::<Vec<Entry>>(&b).map_err(|e| format!("Catalogue illisible : {e}")));
        (catalogue, accounts.join().unwrap_or_default())
    });
    let mut entries = match catalogue {
        Ok(list) => list,
        Err(e) if extra.is_empty() => return Err(e),
        Err(_) => Vec::new(),
    };
    for repo in extra {
        if !entries.iter().any(|e| e.repo.eq_ignore_ascii_case(&repo)) {
            entries.push(Entry { repo, id: String::new(), name: String::new(), description: None });
        }
    }
    // Tous les dépôts en même temps.
    let mut list: Vec<Remote> = std::thread::scope(|s| {
        let handles: Vec<_> = entries.iter().map(|entry| s.spawn(|| read(entry, &tokens))).collect();
        handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
    });
    list.sort_by_key(|r| (r.manifest.name.to_lowercase(), !r.access));
    list.dedup_by(|a, b| a.manifest.id == b.manifest.id);
    Ok(list)
}

/// Fichiers à télécharger : ceux connus, sinon la liste de la release (avec un compte si
/// possible), sinon au moins le manifeste et l'exe par leur lien direct.
fn files_of(remote: &Remote) -> Vec<Asset> {
    if !remote.assets.is_empty() {
        return remote.assets.clone();
    }
    let attempts = tokens().into_iter().map(Some).chain(std::iter::once(None));
    for token in attempts {
        if let Ok((assets, _)) = release_assets(&remote.repo, token.as_deref()) {
            if !assets.is_empty() {
                return assets;
            }
        }
    }
    ["extension.json", remote.manifest.exe.as_str()]
        .iter()
        .filter(|n| !n.is_empty())
        .map(|name| Asset {
            id: 0,
            name: name.to_string(),
            url: format!("https://github.com/{}/releases/latest/download/{name}", remote.repo),
        })
        .collect()
}

/// Installe ou met à jour l'extension : les fichiers de la release sont téléchargés à
/// part, puis l'ancienne version est fermée et remplacée. Les réglages (activée…) restent.
pub fn install(remote: &Remote) -> Result<Manifest, String> {
    if !remote.access {
        return Err(format!("Vous n'avez pas accès à {}.", remote.manifest.name));
    }
    let id = &remote.manifest.id;
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(format!("Identifiant « {id} » invalide dans {}.", remote.repo));
    }
    // Préparé à côté du dossier des extensions (même volume, pas dans la liste).
    let staging = betterext::home_dir().join(".telechargement").join(id);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    for file in files_of(remote) {
        if file.name.contains('/') || file.name.contains('\\') || file.name.starts_with('.') {
            continue;
        }
        // Public : lien direct ; privé : par l'API avec le jeton.
        let bytes = asset(&remote.repo, &file, remote.token.as_deref())?;
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
