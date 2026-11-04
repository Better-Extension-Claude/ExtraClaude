#!/usr/bin/env bash
# Compile BetterClaude (chaîne Rust GNU) et dépose l'exe, avec son icône, dans dist/BetterClaude.exe.
# Le dossier de build est hors du dépôt : dlltool ne supporte pas le « & » du chemin.
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$LOCALAPPDATA/BetterExt/build"
cargo build --release
mkdir -p dist
cp "$CARGO_TARGET_DIR/release/BetterClaude.exe" dist/BetterClaude.exe
# Le logo (dessiné par l'exe lui-même) devient l'icône du fichier.
"$CARGO_TARGET_DIR/release/BetterClaude.exe" --embed-icon "$(cygpath -w "$PWD/dist/BetterClaude.exe")"
echo "Prêt : dist/BetterClaude.exe"
