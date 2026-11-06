//! Contrat entre ExtraClaude et ses extensions.
//!
//! Une extension est un dossier de [`extensions_dir`] qui contient `extension.json` (le
//! [`Manifest`]) et son exe. ExtraClaude est le seul hook de Claude :
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
pub mod settings;
pub mod theme;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Événements de Claude Code qu'ExtraClaude écoute (un hook chacun dans settings.json).
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
    /// Identifiant stable : nom du pipe et des réglages (`[a-z0-9-]`).
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Exe de l'extension : chemin relatif au dossier du manifeste, ou absolu.
    pub exe: String,
    /// Arguments du lancement en début de session. Absent : l'extension n'est jamais lancée
    /// par ExtraClaude (elle ne fait qu'écouter, lancée autrement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Vec<String>>,
    /// Événements de Claude envoyés sur son pipe ; `"*"` pour tous.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
    /// Réglages proposés dans ExtraClaude (bouton « Paramètres » de Mes extensions), lus par
    /// l'extension avec [`settings`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<Setting>,
}

/// Un réglage d'extension, déclaré dans `extension.json` :
/// `{ "key": "cote", "label": "Côté du panneau", "choices": ["À gauche", "À droite"], "default": "À gauche" }`
/// (sans `choices` : un interrupteur, `"default": true` ou `false` ; avec `"button": "Déplacer"` :
/// un bouton d'action).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Setting {
    /// Nom de la valeur dans `settings.json`.
    pub key: String,
    /// Texte affiché, pour l'utilisateur.
    pub label: String,
    /// Choix proposés ; absent : interrupteur oui/non.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// Texte d'un bouton d'action (« Déplacer »…) à la place de l'interrupteur : un clic écrit
    /// `true`, ExtraClaude se réduit et affiche « En cours… », puis l'extension remet `false`
    /// ([`settings::set`]) quand c'est fini.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button: Option<String>,
    /// Valeur par défaut : un des `choices`, ou `true` / `false`.
    #[serde(default)]
    pub default: serde_json::Value,
}

impl Setting {
    /// Valeur actuelle de ce réglage dans `values` (celles de [`settings::all`]), ou sa valeur
    /// par défaut si elle manque ou n'est plus valable.
    pub fn value(&self, values: &serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
        let stored = values.get(&self.key);
        if self.choices.is_empty() {
            let on = stored.and_then(serde_json::Value::as_bool).or(self.default.as_bool()).unwrap_or(false);
            return serde_json::Value::Bool(on);
        }
        let valid = |v: Option<&serde_json::Value>| {
            v.and_then(serde_json::Value::as_str).filter(|s| self.choices.iter().any(|c| c == s)).map(String::from)
        };
        let choice = valid(stored).or_else(|| valid(Some(&self.default))).unwrap_or_else(|| self.choices[0].clone());
        serde_json::Value::String(choice)
    }
}

/// Extension installée : son manifeste et son dossier.
#[derive(Clone, Debug)]
pub struct Extension {
    pub manifest: Manifest,
    pub dir: PathBuf,
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

/// `%LOCALAPPDATA%\BetterExt` : état, thème, réglages des extensions.
pub fn data_dir() -> PathBuf {
    local_app_data().join("BetterExt")
}

/// `%USERPROFILE%\ExtraClaude` : là où ExtraClaude installe ce qu'on télécharge. Hors
/// d'`AppData` exprès : Claude desktop (app du Microsoft Store) redirige ce que ses programmes y
/// créent vers sa copie privée, invisible dans l'Explorateur et effacée avec l'app.
pub fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into())).join("ExtraClaude")
}

/// Dossier des extensions installées, une par sous-dossier à son nom (`extension.json`, son exe
/// et ce dont elle a besoin, rien d'autre). Choisi dans les réglages ; par défaut
/// `%USERPROFILE%\ExtraClaude\Extensions`.
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

/// Dossier conseillé pour les réglages d'une extension (position de sa fenêtre…).
pub fn extension_data_dir(id: &str) -> PathBuf {
    data_dir().join("data").join(id)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// Extensions installées dans [`extensions_dir`], triées par nom. Un dossier sans manifeste
/// lisible est ignoré.
pub fn installed() -> Vec<Extension> {
    let mut list: Vec<Extension> = Vec::new();
    for entry in std::fs::read_dir(extensions_dir()).into_iter().flatten().flatten() {
        let dir = entry.path();
        let Some(manifest) = read_json::<Manifest>(&dir.join("extension.json")) else { continue };
        if !manifest.id.is_empty() && !list.iter().any(|e| e.manifest.id == manifest.id) {
            list.push(Extension { manifest, dir });
        }
    }
    list.sort_by_key(|e| e.manifest.name.to_lowercase());
    list
}

/// `state.json` : extensions activées (une extension absente de la liste est activée),
/// mise à jour automatique d'ExtraClaude et dossier des extensions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub enabled: BTreeMap<String, bool>,
    /// Mises à jour automatiques d'ExtraClaude lui-même (activées si absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_update: Option<bool>,
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

    pub fn self_update(&self) -> bool {
        self.self_update.unwrap_or(true)
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
