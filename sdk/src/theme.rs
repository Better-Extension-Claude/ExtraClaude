//! Thème partagé par BetterClaude et toutes ses extensions : clair ou sombre (ou comme Claude),
//! et les couleurs de chacun, réglables dans BetterClaude → Réglages → Apparence.
//! Enregistré dans `%LOCALAPPDATA%\BetterExt\theme.json` ; les extensions le relisent seules.
//!
//! Une extension qui dessine : `let p = betterext::theme::current();` puis `p.fond`, `p.texte`…
//! (relu au plus toutes les secondes, redessiner quand il change).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Couleur RVB, enregistrée en `"#RRGGBB"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const fn hex(v: u32) -> Rgb {
        Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// Mélange avec `other` : 0 = soi, 1 = `other`.
    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8;
        Rgb(m(self.0, other.0), m(self.1, other.1), m(self.2, other.2))
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2))
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Rgb, D::Error> {
        let s = String::deserialize(d)?;
        let h = s.trim().trim_start_matches('#');
        u32::from_str_radix(h, 16)
            .ok()
            .filter(|_| h.len() == 6)
            .map(Rgb::hex)
            .ok_or_else(|| serde::de::Error::custom(format!("couleur invalide : {s}")))
    }
}

/// Les couleurs d'un thème, par rôle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Palette {
    pub fond: Rgb,
    pub carte: Rgb,
    pub champ: Rgb,
    pub survol: Rgb,
    pub bordure: Rgb,
    pub texte: Rgb,
    pub texte_secondaire: Rgb,
    pub accent: Rgb,
    pub icone: Rgb,
    pub icone_survol: Rgb,
    pub succes: Rgb,
    pub erreur: Rgb,
}

impl Palette {
    /// Couleurs de Claude, thème sombre.
    pub fn sombre() -> Palette {
        Palette {
            fond: Rgb::hex(0x262624),
            carte: Rgb::hex(0x2E2E2B),
            champ: Rgb::hex(0x1F1E1D),
            survol: Rgb::hex(0x34332F),
            bordure: Rgb::hex(0x45443F),
            texte: Rgb::hex(0xECEBE6),
            texte_secondaire: Rgb::hex(0x9A9893),
            accent: Rgb::hex(0xD97757),
            icone: Rgb::hex(0xB8B5A9),
            icone_survol: Rgb::hex(0xECEAE2),
            succes: Rgb::hex(0x8CC076),
            erreur: Rgb::hex(0xE56B5D),
        }
    }

    /// Couleurs de Claude, thème clair.
    pub fn clair() -> Palette {
        Palette {
            fond: Rgb::hex(0xFAF9F5),
            carte: Rgb::hex(0xF0EEE6),
            champ: Rgb::hex(0xFFFFFF),
            survol: Rgb::hex(0xE8E6DC),
            bordure: Rgb::hex(0xD9D6CC),
            texte: Rgb::hex(0x1F1E1D),
            texte_secondaire: Rgb::hex(0x73726C),
            accent: Rgb::hex(0xC96442),
            icone: Rgb::hex(0x3D3D3A),
            icone_survol: Rgb::hex(0x141413),
            succes: Rgb::hex(0x4E8A3A),
            erreur: Rgb::hex(0xC4473A),
        }
    }

    /// Les rôles, avec leur nom affiché, dans l'ordre des réglages.
    pub fn roles_mut(&mut self) -> [(&'static str, &mut Rgb); 12] {
        [
            ("Fond", &mut self.fond),
            ("Cartes", &mut self.carte),
            ("Champs de saisie", &mut self.champ),
            ("Survol", &mut self.survol),
            ("Bordures", &mut self.bordure),
            ("Texte", &mut self.texte),
            ("Texte secondaire", &mut self.texte_secondaire),
            ("Couleur principale", &mut self.accent),
            ("Icônes", &mut self.icone),
            ("Icônes survolées", &mut self.icone_survol),
            ("Succès", &mut self.succes),
            ("Erreur", &mut self.erreur),
        ]
    }
}

impl Default for Palette {
    fn default() -> Palette {
        Palette::sombre()
    }
}

/// Clair ou sombre : comme Claude, ou choisi.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Claude,
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    #[serde(default)]
    pub mode: Mode,
    #[serde(default = "Palette::sombre")]
    pub dark: Palette,
    #[serde(default = "Palette::clair")]
    pub light: Palette,
}

impl Default for Theme {
    fn default() -> Theme {
        Theme { mode: Mode::Claude, dark: Palette::sombre(), light: Palette::clair() }
    }
}

fn path() -> std::path::PathBuf {
    crate::data_dir().join("theme.json")
}

static CACHE: Mutex<Option<(Instant, Theme, bool)>> = Mutex::new(None);

impl Theme {
    /// Le thème enregistré (celui par défaut s'il n'y en a pas).
    pub fn load() -> Theme {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default()
    }

    /// Enregistre le thème : BetterClaude et les extensions le prennent aussitôt (≤ 1 s).
    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(crate::data_dir())?;
        std::fs::write(path(), serde_json::to_string_pretty(self).unwrap_or_default() + "\n")?;
        *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(())
    }

    fn resolve_dark(&self) -> bool {
        match self.mode {
            Mode::Claude => crate::claude::is_dark(),
            Mode::Light => false,
            Mode::Dark => true,
        }
    }
}

/// Thème et sombre/clair actuels, relus au plus toutes les secondes.
fn cached() -> (Theme, bool) {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, theme, dark)) = cache.as_ref() {
        if at.elapsed() < Duration::from_secs(1) {
            return (theme.clone(), *dark);
        }
    }
    let theme = Theme::load();
    let dark = theme.resolve_dark();
    *cache = Some((Instant::now(), theme.clone(), dark));
    (theme, dark)
}

/// Thème sombre en ce moment (selon le réglage : comme Claude, clair ou sombre).
pub fn is_dark() -> bool {
    cached().1
}

/// Les couleurs à utiliser en ce moment.
pub fn current() -> Palette {
    let (theme, dark) = cached();
    if dark {
        theme.dark
    } else {
        theme.light
    }
}
