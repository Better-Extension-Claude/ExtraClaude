//! Réglages d'une extension, ceux qu'elle déclare dans `extension.json` (`settings`) : Vince les
//! change dans ExtraClaude (Mes extensions → Paramètres), l'extension peut aussi les changer
//! elle-même ([`set`]), et ExtraClaude affiche alors la nouvelle valeur.
//!
//! Enregistrés dans `extension_data_dir(id)\settings.json` : un objet `{ "clé": valeur }`. À relire
//! toutes les 1 à 2 s, comme le thème.
//!
//! ```no_run
//! let a_droite = betterext::settings::text("salle-des-agents", "cote", "À gauche") == "À droite";
//! betterext::settings::set("salle-des-agents", "cote", "À droite").ok();
//! ```

use serde_json::{Map, Value};
use std::path::PathBuf;

fn path(id: &str) -> PathBuf {
    crate::extension_data_dir(id).join("settings.json")
}

/// Toutes les valeurs enregistrées (vide si rien n'a encore été changé).
pub fn all(id: &str) -> Map<String, Value> {
    std::fs::read_to_string(path(id))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

/// Valeur enregistrée de `key`, s'il y en a une.
pub fn get(id: &str, key: &str) -> Option<Value> {
    all(id).remove(key)
}

/// Réglage à choix (texte), ou `default`.
pub fn text(id: &str, key: &str, default: &str) -> String {
    get(id, key).and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| default.to_string())
}

/// Interrupteur oui/non, ou `default`.
pub fn flag(id: &str, key: &str, default: bool) -> bool {
    get(id, key).and_then(|v| v.as_bool()).unwrap_or(default)
}

/// Enregistre `key` (les autres valeurs sont gardées). ExtraClaude affiche la nouvelle valeur.
pub fn set(id: &str, key: &str, value: impl Into<Value>) -> std::io::Result<()> {
    let mut values = all(id);
    values.insert(key.to_string(), value.into());
    let file = path(id);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Écrit à côté puis remplace : un lecteur ne voit jamais un fichier à moitié écrit.
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&Value::Object(values)).unwrap_or_default() + "\n")?;
    std::fs::rename(&tmp, &file)
}
