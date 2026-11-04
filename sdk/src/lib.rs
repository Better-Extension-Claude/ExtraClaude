//! Contrat entre BetterExt et ses extensions.
//!
//! Une extension est un dossier `%LOCALAPPDATA%\BetterExt\extensions\<id>\` qui contient
//! `extension.json` (le [`Manifest`]) et son exe. BetterExt est le seul hook de Claude :
//! - à chaque début de session, il lance l'exe des extensions activées avec `start`
//!   (sauf si elles tournent déjà) ;
//! - pour chaque événement de Claude listé dans `events`, il envoie le JSON du hook, tel
//!   que Claude le lui donne, sur le pipe de l'extension ([`events::listen`]).
//!
//! Une extension désactivée ou absente du dossier ne reçoit rien et n'est pas lancée ;
//! la désactiver ferme ses processus.

pub mod claude;
pub mod events;
pub mod process;
pub mod theme;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Événements de Claude Code que BetterExt écoute (un hook chacun dans settings.json).
pub const EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "Stop",
    "SubagentStop",
    "PreCompact",
];

/// `extension.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    /// Identifiant stable : nom du dossier et du pipe (`[a-z0-9-]`).
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Exe de l'extension : chemin relatif au dossier du manifeste, ou absolu.
    pub exe: String,
    /// Arguments du lancement en début de session. Absent : l'extension n'est jamais lancée
    /// par BetterExt (elle ne fait qu'écouter, lancée autrement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Vec<String>>,
    /// Événements de Claude envoyés sur son pipe ; `"*"` pour tous.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
}

/// Extension installée : son manifeste et son dossier.
#[derive(Clone, Debug)]
pub struct Extension {
    pub manifest: Manifest,
    pub dir: PathBuf,
    /// Trouvée dans un dossier surveillé (utilisée sur place) plutôt que copiée dans
    /// `extensions_dir()`.
    pub linked: bool,
}

impl Extension {
    pub fn exe_path(&self) -> PathBuf {
        let exe = Path::new(&self.manifest.exe);
        if exe.is_absolute() {
            exe.to_path_buf()
        } else {
            self.dir.join(exe)
        }
    }

    pub fn wants(&self, event: &str) -> bool {
        self.manifest.events.iter().any(|e| e == "*" || e == event)
    }
}

fn local_app_data() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into()))
}

/// `%LOCALAPPDATA%\BetterExt` : état et extensions installées.
pub fn data_dir() -> PathBuf {
    local_app_data().join("BetterExt")
}

/// BetterExt s'appelait ClaudeExt : reprend une seule fois son état, ses extensions et les
/// réglages des extensions (`%LOCALAPPDATA%\ClaudeExt`).
pub fn migrate_from_claudeext() {
    let old = local_app_data().join("ClaudeExt");
    let new = data_dir();
    for name in ["state.json", "extensions", "data"] {
        let (from, to) = (old.join(name), new.join(name));
        if from.exists() && !to.exists() {
            let _ = std::fs::create_dir_all(&new);
            let _ = std::fs::rename(&from, &to);
        }
    }
}

/// `%USERPROFILE%\BetterClaude` : là où BetterClaude installe ce qu'on télécharge. Hors
/// d'`AppData` exprès : Claude desktop (app du Microsoft Store) redirige ce que ses programmes y
/// créent vers sa copie privée, invisible dans l'Explorateur et effacée avec l'app.
pub fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into())).join("BetterClaude")
}

/// Dossier des extensions installées, une par sous-dossier à son nom (`extension.json`, son exe
/// et ce dont elle a besoin, rien d'autre). Choisi dans les réglages ; par défaut
/// `%USERPROFILE%\BetterClaude\Extensions`.
pub fn extensions_dir() -> PathBuf {
    State::load()
        .extensions_dir
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join("Extensions"))
}

/// Nom de dossier d'une extension : son nom affiché, sans les caractères interdits par Windows.
pub fn folder_name(manifest: &Manifest) -> String {
    let clean: String = manifest.name.chars().filter(|c| !"<>:\"/\\|?*".contains(*c) && !c.is_control()).collect();
    let clean = clean.trim().trim_end_matches('.').trim().to_string();
    if clean.is_empty() {
        manifest.id.clone()
    } else {
        clean
    }
}

/// Les extensions installées étaient dans `%LOCALAPPDATA%\BetterExt\extensions\<id>` : elles
/// passent dans [`extensions_dir`], sous leur nom. Une seule fois, au démarrage.
pub fn migrate_extensions() {
    let old = data_dir().join("extensions");
    let Ok(entries) = std::fs::read_dir(&old) else { return };
    for entry in entries.flatten() {
        let from = entry.path();
        let Some(manifest) = read_json::<Manifest>(&from.join("extension.json")) else { continue };
        let to = extensions_dir().join(folder_name(&manifest));
        if to.exists() {
            continue;
        }
        let _ = std::fs::create_dir_all(extensions_dir());
        if std::fs::rename(&from, &to).is_err() {
            // Autre volume ou fichiers ouverts : copie, l'ancien dossier reste.
            continue;
        }
    }
    let _ = std::fs::remove_dir(&old);
}

/// Dossier conseillé pour les réglages d'une extension (position de sa fenêtre…).
pub fn extension_data_dir(id: &str) -> PathBuf {
    data_dir().join("data").join(id)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// Profondeur de recherche dans un dossier surveillé : `<dossier>\GitPush\dist\extension.json`.
const SCAN_DEPTH: usize = 3;
const SCAN_SKIP: &[&str] = &["target", "node_modules", "build", "src"];

/// Ajoute à `out` les extensions trouvées sous `dir` : un dossier qui a un `extension.json`
/// et son exe est une extension (on ne descend pas dedans). Un manifeste sans son exe (la
/// racine d'un projet, l'exe étant compilé dans `dist\`) ne compte que si aucun sous-dossier
/// n'a la même extension complète.
fn scan(dir: &Path, depth: usize, out: &mut Vec<Extension>) {
    let here = read_json::<Manifest>(&dir.join("extension.json"))
        .map(|manifest| Extension { manifest, dir: dir.to_path_buf(), linked: true });
    if here.as_ref().is_some_and(|e| e.exe_path().exists() || depth == 0) {
        out.extend(here);
        return;
    }
    let before = out.len();
    if depth > 0 {
        scan_children(dir, depth, out);
    }
    if let Some(ext) = here {
        if !out[before..].iter().any(|e| e.manifest.id == ext.manifest.id) {
            out.push(ext);
        }
    }
}

fn scan_children(dir: &Path, depth: usize, out: &mut Vec<Extension>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
            && !name.starts_with('.')
            && !SCAN_SKIP.contains(&name.as_str())
        {
            scan(&entry.path(), depth - 1, out);
        }
    }
}

/// Extensions installées, triées par nom : celles copiées dans `extensions_dir()`, puis
/// celles des dossiers surveillés. Pour un même id, la copie installée l'emporte. Un
/// dossier sans manifeste lisible est ignoré.
pub fn installed() -> Vec<Extension> {
    let mut found: Vec<Extension> = std::fs::read_dir(extensions_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let dir = entry.path();
            let manifest: Manifest = read_json(&dir.join("extension.json"))?;
            Some(Extension { manifest, dir, linked: false })
        })
        .collect();
    let state = State::load();
    let installed = found.len();
    for folder in &state.folders {
        scan(Path::new(folder), SCAN_DEPTH, &mut found);
    }
    // Retirées de la liste : seulement celles des dossiers surveillés.
    let mut i = installed;
    while i < found.len() {
        if state.hidden.contains(&found[i].manifest.id) {
            found.remove(i);
        } else {
            i += 1;
        }
    }
    let mut list: Vec<Extension> = Vec::new();
    for ext in found {
        if !ext.manifest.id.is_empty() && !list.iter().any(|e| e.manifest.id == ext.manifest.id) {
            list.push(ext);
        }
    }
    list.sort_by_key(|e| e.manifest.name.to_lowercase());
    list
}

/// `state.json` : extensions activées (une extension absente de la liste est activée) et
/// dossiers surveillés, où les extensions sont trouvées et utilisées sur place.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub enabled: BTreeMap<String, bool>,
    #[serde(default)]
    pub folders: Vec<String>,
    /// Extensions des dossiers surveillés retirées de la liste (le dossier reste sur le disque).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
    /// Mises à jour automatiques des extensions installées (activées si absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_update: Option<bool>,
    /// Dossier des extensions installées ([`extensions_dir`]) ; absent : celui par défaut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions_dir: Option<String>,
}

impl State {
    fn path() -> PathBuf {
        data_dir().join("state.json")
    }

    pub fn load() -> State {
        read_json(&Self::path()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(data_dir())?;
        std::fs::write(Self::path(), serde_json::to_string_pretty(self).unwrap_or_default() + "\n")
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.enabled.get(id).copied().unwrap_or(true)
    }

    pub fn auto_update(&self) -> bool {
        self.auto_update.unwrap_or(true)
    }
}

/// Extensions installées et activées.
pub fn active() -> Vec<Extension> {
    let state = State::load();
    installed().into_iter().filter(|e| state.is_enabled(&e.manifest.id)).collect()
}

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
