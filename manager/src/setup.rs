//! Installation de BetterClaude : copie de l'exe, ses hooks dans les réglages de Claude Code,
//! et reprise de GitPush (son propre hook devient une extension de BetterClaude).

use betterext::{extensions_dir, process, Extension, Manifest, EVENTS};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Réglages de Claude Code : ~/.claude/settings.json (ou $CLAUDE_CONFIG_DIR).
pub fn settings_path() -> PathBuf {
    let dir = std::env::var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(&std::env::var("USERPROFILE").unwrap_or_default()).join(".claude"));
    dir.join("settings.json")
}

/// Dossier proposé à l'installation : `%USERPROFILE%\BetterClaude` (BetterClaude.exe, et ses
/// extensions dans `Extensions\`). Hors d'`AppData`, que Claude desktop redirige vers sa copie privée.
pub fn default_folder() -> PathBuf {
    betterext::home_dir()
}

fn command_of(hook: &Value) -> &str {
    hook.get("command").and_then(Value::as_str).unwrap_or_default()
}

fn is_ours(hook: &Value) -> bool {
    let c = command_of(hook).to_lowercase();
    ["betterclaude.exe", "betterext.exe", "claudeext.exe"].iter().any(|name| c.contains(name))
}

fn is_gitpush(hook: &Value) -> bool {
    command_of(hook).to_lowercase().contains("gitpush.exe")
}

/// Exe d'une commande de hook : `"C:/…/x.exe" --arg` ou `C:/…/x.exe`.
fn exe_in(command: &str) -> PathBuf {
    let c = command.trim();
    let path = match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or_default(),
        None => c.find(".exe").map(|i| &c[..i + 4]).unwrap_or(c),
    };
    PathBuf::from(path.replace('/', "\\"))
}

fn read_settings() -> Result<Value, String> {
    let path = settings_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into());
    // Un éditeur Windows peut avoir ajouté une marque BOM, que serde_json refuse.
    let text = text.trim_start_matches('\u{feff}');
    let settings: Value = serde_json::from_str(if text.trim().is_empty() { "{}" } else { text })
        .map_err(|e| format!("{} n'est pas un JSON valide ({e}) : rien n'a été modifié.", path.display()))?;
    if !settings.is_object() {
        return Err(format!("{} n'est pas un objet JSON : rien n'a été modifié.", path.display()));
    }
    Ok(settings)
}

fn write_settings(settings: &Value) -> Result<(), String> {
    let path = settings_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if path.exists() {
        let _ = std::fs::copy(&path, path.with_extension("json.betterext.bak"));
    }
    let out = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, out + "\n").map_err(|e| format!("Écriture de {} impossible : {e}", path.display()))
}

/// Toutes les entrées de hook, tous événements confondus.
fn all_hooks(settings: &Value) -> impl Iterator<Item = &Value> {
    settings
        .get("hooks")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|h| h.values())
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|g| g.get("hooks")?.as_array())
        .flatten()
}

/// BetterClaude.exe que les hooks de Claude lancent, s'il est branché.
pub fn hooked_exe() -> Option<PathBuf> {
    let settings = read_settings().ok()?;
    let found = all_hooks(&settings).find(|h| is_ours(h)).map(|h| exe_in(command_of(h)));
    found
}

/// Retire les hooks pour lesquels `drop` est vrai, puis ajoute `add` (événement, commande).
fn edit_hooks(settings: &mut Value, drop: impl Fn(&Value) -> bool, add: &[(&str, String)]) -> Result<(), String> {
    let root = settings.as_object_mut().ok_or("réglages illisibles")?;
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or("« hooks » n'est pas un objet.")?;
    for groups in hooks.values_mut().filter_map(Value::as_array_mut) {
        for group in groups.iter_mut() {
            if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                list.retain(|h| !drop(h));
            }
        }
        groups.retain(|g| g.get("hooks").and_then(Value::as_array).map(|l| !l.is_empty()).unwrap_or(true));
    }
    for (event, command) in add {
        let groups = hooks.entry(*event).or_insert_with(|| json!([]));
        let groups = groups.as_array_mut().ok_or_else(|| format!("« {event} » n'est pas une liste."))?;
        let timeout = if *event == "SessionStart" { 10 } else { 5 };
        groups.push(json!({ "hooks": [ { "type": "command", "command": command, "timeout": timeout } ] }));
    }
    hooks.retain(|_, v| v.as_array().map(|a| !a.is_empty()).unwrap_or(true));
    if hooks.is_empty() {
        root.remove("hooks");
    }
    Ok(())
}

fn write_manifest(manifest: &Manifest) -> Result<(), String> {
    let dir = extensions_dir().join(betterext::folder_name(manifest));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(manifest).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("extension.json"), text + "\n").map_err(|e| e.to_string())
}

/// Copie BetterClaude.exe dans `folder`, branche ses hooks sur tous les événements de Claude
/// et reprend GitPush : son hook est retiré, il devient l'extension « gitpush ».
pub fn install(folder: &str) -> Result<PathBuf, String> {
    let folder = PathBuf::from(folder.trim());
    if folder.as_os_str().is_empty() {
        return Err("Choisis un dossier.".into());
    }
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let target = folder.join("BetterClaude.exe");
    std::fs::create_dir_all(&folder).map_err(|e| format!("Création de {} impossible : {e}", folder.display()))?;
    if !same_path(&me, &target) {
        std::fs::copy(&me, &target).map_err(|e| format!("Copie vers {} impossible : {e}", target.display()))?;
    }

    let mut settings = read_settings()?;
    // GitPush installé : par son hook, sinon à son emplacement par défaut.
    let gitpush = all_hooks(&settings)
        .find(|h| is_gitpush(h))
        .map(|h| exe_in(command_of(h)))
        .or_else(|| Some(Path::new(&std::env::var("LOCALAPPDATA").ok()?).join("GitPush").join("GitPush.exe")));
    if let Some(exe) = gitpush.filter(|p| p.exists()) {
        if !betterext::installed().iter().any(|e| e.manifest.id == "gitpush") {
            write_manifest(&Manifest {
                id: "gitpush".into(),
                name: "GitPush".into(),
                description: Some("Envoyez votre projet sur GitHub en un clic, sans passer par Claude.".into()),
                exe: exe.to_string_lossy().into(),
                start: Some(vec!["--button".into()]),
                ..Default::default()
            })?;
        }
    }
    // Première installation : les extensions vont à côté de BetterClaude, dans `Extensions\`.
    let mut state = betterext::State::load();
    if state.extensions_dir.is_none() && !betterext::extensions_dir().exists() {
        state.extensions_dir = Some(folder.join("Extensions").to_string_lossy().into());
        let _ = state.save();
    }
    let command = format!("\"{}\" --hook", target.to_string_lossy().replace('\\', "/"));
    let add: Vec<(&str, String)> = EVENTS.iter().map(|e| (*e, command.clone())).collect();
    edit_hooks(&mut settings, |h| is_ours(h) || is_gitpush(h), &add)?;
    write_settings(&settings)?;
    // BetterClaude s'est appelé ClaudeExt puis BetterExt : les anciennes copies ne servent plus.
    let programs = Path::new(&std::env::var("LOCALAPPDATA").unwrap_or_default()).join("Programs");
    for name in ["ClaudeExt", "BetterExt"] {
        let old = programs.join(name);
        if old.exists() && !same_path(&old, &folder) {
            for exe in ["ClaudeExt.exe", "BetterExt.exe", "BetterClaude.exe"] {
                process::kill_all(&old.join(exe));
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
            let _ = std::fs::remove_dir_all(&old);
        }
    }
    // Le bouton puzzle tout de suite, sans attendre la prochaine session.
    if betterext::claude::is_open() {
        let _ = process::spawn_detached(&target, &["--button".to_string()]);
    }
    Ok(target)
}

/// Débranche BetterClaude : plus aucune extension ne démarre avec Claude.
pub fn uninstall() -> Result<(), String> {
    let mut settings = read_settings()?;
    edit_hooks(&mut settings, is_ours, &[])?;
    write_settings(&settings)
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase();
    norm(a) == norm(b)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// Installe l'extension dont `manifest_path` est le `extension.json` : tout son dossier
/// est copié. Une version déjà installée est fermée puis remplacée.
pub fn add_extension(manifest_path: &Path) -> Result<Manifest, String> {
    let source = manifest_path.parent().ok_or("Dossier de l'extension introuvable.")?;
    let text = std::fs::read_to_string(manifest_path)
        .map_err(|_| format!("Lecture de {} impossible.", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("extension.json illisible : {e}"))?;
    let id_ok = !manifest.id.is_empty()
        && manifest.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !id_ok {
        return Err(format!("Identifiant « {} » invalide : lettres minuscules, chiffres et tirets.", manifest.id));
    }
    let dest = extensions_dir().join(betterext::folder_name(&manifest));
    if same_path(source, &dest) {
        return Ok(manifest);
    }
    if let Some(old) = betterext::installed().into_iter().find(|e| e.manifest.id == manifest.id && !e.linked) {
        // Un exe en marche est verrouillé : on attend que ses processus soient fermés.
        let exe = old.exe_path();
        process::kill_all(&exe);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while process::is_running(&exe) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        std::fs::remove_dir_all(&old.dir).map_err(|e| format!("Remplacement impossible : {e}"))?;
    }
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| format!("Remplacement impossible : {e}"))?;
    }
    copy_dir(source, &dest).map_err(|e| format!("Copie impossible : {e}"))?;
    crate::hook::restart_installed(&manifest.id);
    Ok(manifest)
}

/// Déplace toutes les extensions installées dans `new_dir`, qui devient le dossier des
/// extensions. Celles qui tournent sont fermées, puis relancées depuis leur nouvelle place.
pub fn move_extensions(new_dir: &Path) -> Result<usize, String> {
    let old_dir = extensions_dir();
    if same_path(&old_dir, new_dir) {
        return Ok(0);
    }
    if new_dir.starts_with(&old_dir) {
        return Err("Choisissez un dossier qui n'est pas à l'intérieur du dossier actuel.".into());
    }
    std::fs::create_dir_all(new_dir).map_err(|e| format!("Impossible de créer {} : {e}", new_dir.display()))?;
    let installed: Vec<Extension> = betterext::installed().into_iter().filter(|e| !e.linked).collect();
    // Fermées d'abord : un exe en marche ne peut pas être déplacé.
    let was_running: Vec<String> =
        installed.iter().filter(|e| process::is_running(&e.exe_path())).map(|e| e.manifest.id.clone()).collect();
    for ext in &installed {
        process::kill_all(&ext.exe_path());
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while installed.iter().any(|e| process::is_running(&e.exe_path())) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let mut moved = 0;
    for ext in &installed {
        let Some(name) = ext.dir.file_name() else { continue };
        let to = new_dir.join(name);
        if to.exists() {
            return Err(format!("{} existe déjà : rien n'a été déplacé de plus.", to.display()));
        }
        // Même disque : simple renommage ; sinon copie puis suppression.
        if std::fs::rename(&ext.dir, &to).is_err() {
            copy_dir(&ext.dir, &to).map_err(|e| format!("Copie de {} impossible : {e}", ext.manifest.name))?;
            let _ = std::fs::remove_dir_all(&ext.dir);
        }
        moved += 1;
    }
    let mut state = betterext::State::load();
    state.extensions_dir = Some(new_dir.to_string_lossy().into());
    state.save().map_err(|e| format!("Réglage non enregistré : {e}"))?;
    let _ = std::fs::remove_dir(&old_dir);
    for id in was_running {
        crate::hook::restart_installed(&id);
    }
    Ok(moved)
}

/// Ferme l'extension et retire son dossier (un exe hors du dossier, comme GitPush repris,
/// reste en place).
/// Une extension d'un dossier surveillé est seulement retirée de la liste : son dossier (un
/// projet en cours, souvent) reste sur le disque. Réglages → Dossiers surveillés la remet.
pub fn remove_extension(ext: &Extension) -> Result<(), String> {
    process::kill_all(&ext.exe_path());
    if ext.linked {
        let mut state = betterext::State::load();
        if !state.hidden.contains(&ext.manifest.id) {
            state.hidden.push(ext.manifest.id.clone());
        }
        return state.save().map_err(|e| format!("Réglage non enregistré : {e}"));
    }
    std::fs::remove_dir_all(&ext.dir).map_err(|e| format!("Suppression impossible : {e}"))
}
