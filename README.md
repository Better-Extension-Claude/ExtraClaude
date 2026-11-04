# BetterExt

Gestionnaire d'extensions pour Claude desktop (Windows), en Rust. BetterExt est le **seul**
hook dans `~/.claude/settings.json` ; les extensions sont des applications Rust à part,
qui n'ont plus besoin de leur propre hook ni des limites des plugins Claude.

- **Activée** : lancée au début de chaque session de Claude (si elle ne tourne pas déjà),
  et elle reçoit les événements de Claude qu'elle écoute.
- **Désactivée** : pas lancée, ne reçoit rien ; la désactiver ferme ses processus.
- **Pas installée** : rien.

Un **bouton puzzle** apparaît sur Claude, juste à gauche de l'engrenage « Paramètres » en
bas de la barre latérale ; un clic ouvre la fenêtre BetterExt. Il suit la barre latérale
(trouvée par UI Automation) et se cache quand elle est repliée. `BETTEREXT_DEBUG=1` écrit
son journal dans `%LOCALAPPDATA%\BetterExt\button.log`.

BetterExt s'appelait ClaudeExt : au premier lancement, il reprend `%LOCALAPPDATA%\ClaudeExt`
(état, extensions, réglages des extensions), et l'installation remplace l'ancien hook.

## Installation

```bash
bash build.sh
```

Lancer `dist/BetterExt.exe` → **Installer et brancher**. BetterExt se copie dans
`%LOCALAPPDATA%\Programs\BetterExt`, ajoute son hook sur chaque événement de Claude
(`BetterExt.exe --hook`) et reprend GitPush : son hook est retiré et GitPush devient
l'extension `gitpush`. Une copie de l'ancien `settings.json` est gardée
(`settings.json.betterext.bak`). **Débrancher** retire le hook : plus aucune extension ne
démarre avec Claude.

Relancer `BetterExt.exe` (installé) ouvre la liste des extensions : case à cocher pour
activer, **Retirer**, et deux façons d'en ajouter :

- **Dossiers surveillés → Ajouter un dossier…** : le dossier où tu ranges tes extensions
  (par exemple `Plugins&mods Claude`). Toute extension rangée dedans, jusqu'à 3 niveaux
  (`GitPush\dist\extension.json`), apparaît et s'utilise sur place, sans copie.
- **Installer une extension…** : choisir le fichier `extension.json` d'une extension ; tout
  son dossier est copié dans `%LOCALAPPDATA%\BetterExt\extensions`. Une extension copiée
  l'emporte sur une extension du même id trouvée dans un dossier surveillé.

## Découverte

L'onglet **Découverte** liste les extensions publiées dans l'organisation des extensions
(et sur le compte `zomone`). **Installer** télécharge la release dans
`%LOCALAPPDATA%\BetterExt\extensions\<id>\`. Les mises à jour sont vérifiées à l'ouverture
de la fenêtre BetterClaude : **Mettre à jour (vX)** sur l'extension, ou **Tout mettre à
jour**.

Les extensions **publiques** se voient sans rien. Les **privées** se voient avec
**Se connecter à GitHub** : un code s'affiche (déjà copié), on le colle sur la page GitHub
qui s'ouvre, et c'est tout. Pas de GitHub CLI. BetterClaude ne contient aucun secret : ce
que le compte connecté peut lire, c'est GitHub qui le décide. La session est gardée
chiffrée pour le compte Windows (`session.dat`).

Réglages de l'appli GitHub (dans `github.rs`) : `CLIENT_ID` (public) et `ORG`.

## Panneau dev

L'onglet **Dev** n'apparaît que pour le propriétaire (`zomone`) connecté. Pour chaque dépôt
privé : qui y a accès, **Lecture** ou **Modification**, **Retirer**, et **Inviter** un nom
GitHub. La lecture seule n'existe que pour les dépôts d'une organisation : un dépôt d'un
compte personnel se transfère sur github.com (Settings → Transfer).

Pour publier une extension : la **dernière release** du dépôt doit contenir comme fichiers
`extension.json` (avec `version`) et l'exe, plus tout autre fichier nécessaire. Tous les
fichiers de la release sont copiés dans le dossier de l'extension.

`BetterClaude.exe --discover` affiche ce que voit l'onglet Découverte.

## Écrire une extension

Une extension est un dossier avec un `extension.json` et son exe. BetterExt la copie dans
`%LOCALAPPDATA%\BetterExt\extensions\<id>\`.

```json
{
  "id": "download-progress",
  "name": "Download Progress",
  "version": "1.0.0",
  "description": "Progression des téléchargements lancés par Claude.",
  "exe": "download-progress.exe",
  "start": ["--overlay"],
  "events": ["PreToolUse", "PostToolUse"]
}
```

- `id` : `[a-z0-9-]`, stable (dossier, pipe, réglages).
- `exe` : relatif au dossier du manifeste, ou absolu.
- `start` : arguments du lancement en début de session. Absent : jamais lancée par BetterExt.
- `events` : événements de Claude reçus (`"*"` pour tous) parmi `SessionStart`,
  `SessionEnd`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Notification`, `Stop`,
  `SubagentStop`, `PreCompact`. Chaque événement est le JSON du hook tel que Claude
  l'envoie (`hook_event_name`, `session_id`, `cwd`, `transcript_path`, `tool_name`,
  `tool_input`, `tool_response`…).

Règles :

1. **Une seule instance** : le hook peut la lancer plusieurs fois (plusieurs sessions).
   `betterext::process::single_instance("Local\\<id>")`.
2. **Se fermer avec Claude** : quitter quand `betterext::claude::is_open()` devient faux.
3. Les événements sont non bloquants : l'extension ne peut pas refuser un outil.
4. Réglages propres (position de fenêtre…) dans `betterext::extension_data_dir(id)`.

Le crate `sdk` (`betterext`) fournit tout ça :

```rust
betterext::events::listen("download-progress", |event| { /* serde_json::Value */ });
let (rect, scale) = betterext::claude::rect().unwrap(); // fenêtre de Claude
```

`sdk/examples/echo.rs` est une extension complète qui journalise les événements reçus.
Dans le `Cargo.toml` d'une extension :

```toml
betterext = { path = "../../BetterExt/sdk" }
```

## Comment ça marche

`BetterExt.exe --hook` lit le JSON de l'événement sur son entrée standard, lance les
extensions activées si c'est `SessionStart`, puis écrit l'événement sur le pipe nommé de
chaque extension activée qui l'écoute (`\\.\pipe\betterext-<id>`), et rend la main sans
rien écrire. L'état (extensions activées) est dans `%LOCALAPPDATA%\BetterExt\state.json`.
