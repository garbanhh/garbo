#!/usr/bin/env bash
# Publica una versión de Garbo (solo cuando el propietario lo indique).
#   1. Sube la versión en Cargo.toml y en los metadatos AppStream.
#   2. Pasa los tests, crea el commit "Versión X.Y.Z" y la etiqueta vX.Y.Z en el repo privado.
#   3. Genera para el repo público un commit con el mismo contenido SIN docs/ (la documentación
#      interna nunca se publica), encima del último commit público, y lo sube con su etiqueta.
#      Allí GitHub Actions genera el RPM, el .deb y la release, y las apps instaladas avisan.
# Uso: scripts/publicar.sh 0.2.0
set -euo pipefail

V="${1:?Uso: scripts/publicar.sh X.Y.Z}"
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Versión no válida: $V (formato X.Y.Z)"; exit 1; }
cd "$(dirname "$0")/.."

[[ -z "$(git status --porcelain)" ]] || { echo "Hay cambios sin commit: no se publica."; exit 1; }
[[ "$(git branch --show-current)" == "main" ]] || { echo "Publica desde la rama main."; exit 1; }
! git rev-parse -q --verify "refs/tags/v$V" >/dev/null || { echo "La etiqueta v$V ya existe."; exit 1; }

# 1-2. Versión, pruebas, commit y etiqueta en el privado
sed -i "0,/^version = \".*\"/s//version = \"$V\"/" Cargo.toml
cargo update -p garbo --offline >/dev/null
sed -i "s|<releases>|<releases>\n    <release version=\"$V\" date=\"$(date +%F)\"/>|" \
    packaging/io.github.garbanhh.Garbo.metainfo.xml
cargo test --locked -q
git commit -qam "Versión $V"
git tag -a "v$V" -m "Garbo $V"
git push -q origin main "v$V"

# 3. Commit público: mismo árbol sin docs/, encima del último commit público
git fetch -q publico
INDEX="$(mktemp)"
trap 'rm -f "$INDEX"' EXIT
GIT_INDEX_FILE="$INDEX" git read-tree HEAD
GIT_INDEX_FILE="$INDEX" git rm -r -q --cached --ignore-unmatch docs
TREE="$(GIT_INDEX_FILE="$INDEX" git write-tree)"
PARENT="$(git rev-parse -q --verify publico/main || true)"
PUB="$(git commit-tree "$TREE" ${PARENT:+-p "$PARENT"} -m "Garbo $V")"
git push -q publico "$PUB:refs/heads/main"
git push -q publico "$PUB:refs/tags/v$V"
echo "Publicada v$V → https://github.com/garbanhh/garbo/actions"
