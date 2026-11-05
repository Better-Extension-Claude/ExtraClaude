//! GitHub sans GitHub CLI : appels HTTPS par `curl.exe` (livré avec Windows), connexion par
//! code (« device flow ») avec l'appli GitHub d'ExtraClaude, et gestion des accès aux
//! dépôts pour le propriétaire (panneau dev).
//!
//! ExtraClaude ne contient aucun secret : CLIENT_ID est public. Ce qu'un compte peut voir
//! ou modifier, c'est GitHub qui le décide pour ce compte.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Identifiant public de l'appli GitHub « ExtraClaude » (connexion par code). Vide tant
/// que l'appli n'est pas créée : la connexion est alors indisponible.
pub const CLIENT_ID: &str = "Iv23lirfagQxl4Xua7Ya";
/// Compte propriétaire d'ExtraClaude : son panneau dev n'apparaît que pour lui.
pub const OWNER: &str = "zomone";
/// Organisation où sont rangées les extensions (publiques et privées). Vide : pas encore.
pub const ORG: &str = "Better-Extension-Claude";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Comptes dont les dépôts sont lus par Découverte.
pub fn owners() -> Vec<&'static str> {
    [ORG, OWNER].into_iter().filter(|o| !o.is_empty()).collect()
}

/// Requête HTTPS : `method url`, corps JSON éventuel, jeton éventuel. Le jeton passe par
/// l'entrée standard de curl : il n'apparaît pas dans la liste des processus.
pub fn request(method: &str, url: &str, body: Option<&Value>, token: Option<&str>, accept: &str) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new("curl.exe");
    cmd.args(["-sSL", "--max-time", "120", "-X", method, "-H", &format!("Accept: {accept}"), "-H", "X-GitHub-Api-Version: 2022-11-28"])
        .args(["-w", "\n%{http_code}"]);
    if token.is_some() {
        cmd.args(["-H", "@-"]);
    }
    if let Some(body) = body {
        cmd.args(["-H", "Content-Type: application/json", "--data-binary", &body.to_string()]);
    }
    let mut child = cmd
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("curl.exe introuvable : {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Some(token) = token {
            let _ = stdin.write_all(format!("Authorization: Bearer {token}\r\n").as_bytes());
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("GitHub injoignable : {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    // Dernière ligne : le code HTTP ajouté par -w.
    let mut data = out.stdout;
    let split = data.iter().rposition(|b| *b == b'\n').unwrap_or(0);
    let code: u16 = String::from_utf8_lossy(&data[split..]).trim().parse().unwrap_or(0);
    data.truncate(split);
    if (200..300).contains(&code) {
        Ok(data)
    } else {
        let message = serde_json::from_slice::<Value>(&data)
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(String::from))
            .unwrap_or_default();
        Err(format!("GitHub a répondu {code} {message}").trim().to_string())
    }
}

pub fn api(method: &str, path: &str, body: Option<&Value>, token: Option<&str>) -> Result<Value, String> {
    let bytes = request(method, &format!("https://api.github.com/{path}"), body, token, "application/vnd.github+json")?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("Réponse de GitHub illisible : {e}"))
}

pub fn get(path: &str, token: Option<&str>) -> Result<Value, String> {
    api("GET", path, None, token)
}

// ---------------------------------------------------------------- session

/// Compte connecté : son nom GitHub et son jeton, gardés chiffrés par Windows.
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub login: String,
    pub token: String,
}

impl Session {
    pub fn load() -> Option<Session> {
        crate::keys::load_session()
    }

    pub fn is_owner(&self) -> bool {
        self.login.eq_ignore_ascii_case(OWNER)
    }
}

/// Début de connexion : le code à taper sur la page GitHub.
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    device_code: String,
    interval: u64,
    expires: Instant,
}

pub fn start_login() -> Result<DeviceCode, String> {
    if CLIENT_ID.is_empty() {
        return Err("La connexion GitHub n'est pas encore configurée (appli GitHub à créer).".into());
    }
    let body = json!({ "client_id": CLIENT_ID });
    let bytes = request("POST", "https://github.com/login/device/code", Some(&body), None, "application/json")?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let field = |k: &str| v.get(k).and_then(Value::as_str).map(String::from).ok_or(format!("Réponse de GitHub sans « {k} »."));
    Ok(DeviceCode {
        user_code: field("user_code")?,
        verification_uri: field("verification_uri")?,
        device_code: field("device_code")?,
        interval: v.get("interval").and_then(Value::as_u64).unwrap_or(5),
        expires: Instant::now() + Duration::from_secs(v.get("expires_in").and_then(Value::as_u64).unwrap_or(900)),
    })
}

/// Attend que la personne valide le code sur GitHub, puis enregistre la session.
pub fn finish_login(code: &DeviceCode) -> Result<Session, String> {
    let mut interval = code.interval;
    while Instant::now() < code.expires {
        std::thread::sleep(Duration::from_secs(interval));
        let body = json!({
            "client_id": CLIENT_ID,
            "device_code": code.device_code,
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
        });
        let bytes = request("POST", "https://github.com/login/oauth/access_token", Some(&body), None, "application/json")?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if let Some(token) = v.get("access_token").and_then(Value::as_str) {
            let user = get("user", Some(token))?;
            let login = user.get("login").and_then(Value::as_str).unwrap_or_default().to_string();
            let session = Session { login, token: token.to_string() };
            crate::keys::save_session(Some(&session))?;
            return Ok(session);
        }
        match v.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += 5,
            Some("access_denied") => return Err("Connexion refusée sur GitHub.".into()),
            Some("expired_token") => break,
            Some(other) => return Err(format!("Connexion impossible : {other}.")),
            None => return Err("Réponse de GitHub inattendue.".into()),
        }
    }
    Err("Le code a expiré : recommence la connexion.".into())
}

pub fn logout() -> Result<(), String> {
    crate::keys::save_session(None)
}

// ---------------------------------------------------------------- panneau dev

/// Dépôt privé géré par le propriétaire.
#[derive(Clone)]
pub struct ManagedRepo {
    pub full_name: String,
    /// Rôles « lecture seule » possibles (dépôt d'organisation) ; sinon GitHub ne
    /// connaît que « peut modifier ».
    pub in_org: bool,
    pub people: Vec<Person>,
}

#[derive(Clone)]
pub struct Person {
    pub login: String,
    /// « read » ou « write » (ou admin / maintain / triage, affichés tels quels).
    pub role: String,
    /// Invitation pas encore acceptée : son id, pour l'annuler.
    pub invitation: Option<u64>,
}

/// Dépôts privés que le compte administre (son compte et l'organisation), avec leurs
/// collaborateurs directs et les invitations en attente.
pub fn managed_repos(session: &Session) -> Result<Vec<ManagedRepo>, String> {
    let token = Some(session.token.as_str());
    let list = get("user/repos?per_page=100&affiliation=owner,organization_member", token)?;
    let mut repos = Vec::new();
    for r in list.as_array().into_iter().flatten() {
        let private = r.get("private").and_then(Value::as_bool).unwrap_or(false);
        let admin = r.pointer("/permissions/admin").and_then(Value::as_bool).unwrap_or(false);
        let owner = r.pointer("/owner/login").and_then(Value::as_str).unwrap_or_default();
        let Some(full_name) = r.get("full_name").and_then(Value::as_str) else { continue };
        if !private || !admin || !owners().iter().any(|o| o.eq_ignore_ascii_case(owner)) {
            continue;
        }
        let in_org = r.pointer("/owner/type").and_then(Value::as_str) == Some("Organization");
        let mut people = Vec::new();
        for c in get(&format!("repos/{full_name}/collaborators?affiliation=direct&per_page=100"), token)?.as_array().into_iter().flatten() {
            let login = c.get("login").and_then(Value::as_str).unwrap_or_default();
            if login.eq_ignore_ascii_case(&session.login) {
                continue;
            }
            let role = c.get("role_name").and_then(Value::as_str).unwrap_or("write").to_string();
            people.push(Person { login: login.to_string(), role, invitation: None });
        }
        for i in get(&format!("repos/{full_name}/invitations?per_page=100"), token)?.as_array().into_iter().flatten() {
            let login = i.pointer("/invitee/login").and_then(Value::as_str).unwrap_or_default();
            let role = i.get("permissions").and_then(Value::as_str).unwrap_or("write").to_string();
            people.push(Person { login: login.to_string(), role, invitation: i.get("id").and_then(Value::as_u64) });
        }
        repos.push(ManagedRepo { full_name: full_name.to_string(), in_org, people });
    }
    repos.sort_by_key(|r| r.full_name.to_lowercase());
    Ok(repos)
}

/// Donne (ou change) l'accès de `login` : lecture seule (`read == true`) ou modification.
/// Une personne pas encore collaboratrice reçoit une invitation à accepter.
pub fn set_access(session: &Session, repo: &str, login: &str, read: bool) -> Result<(), String> {
    let permission = if read { "pull" } else { "push" };
    api("PUT", &format!("repos/{repo}/collaborators/{login}"), Some(&json!({ "permission": permission })), Some(&session.token))?;
    Ok(())
}

/// Change le rôle d'une invitation en attente.
pub fn set_invitation(session: &Session, repo: &str, id: u64, read: bool) -> Result<(), String> {
    let permissions = if read { "read" } else { "write" };
    api("PATCH", &format!("repos/{repo}/invitations/{id}"), Some(&json!({ "permissions": permissions })), Some(&session.token))?;
    Ok(())
}

/// Retire l'accès de `person` (collaborateur ou invitation).
pub fn remove_access(session: &Session, repo: &str, person: &Person) -> Result<(), String> {
    let path = match person.invitation {
        Some(id) => format!("repos/{repo}/invitations/{id}"),
        None => format!("repos/{repo}/collaborators/{}", person.login),
    };
    api("DELETE", &path, None, Some(&session.token))?;
    Ok(())
}
