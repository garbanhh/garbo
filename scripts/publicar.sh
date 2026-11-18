#!/usr/bin/env bash
# Publica una versión de Garbo.
#   1. Sube la versión en Cargo.toml y en los metadatos AppStream.
#   2. Pasa los tests, crea el commit y la etiqueta vX.Y.Z.
#   3. Lo envía al repo privado (origin) y al público (publico). En el público, GitHub Actions
#      genera el RPM y la release, y las apps instaladas muestran "Actualizar a X.Y.Z".
# Uso: scripts/publicar.sh 0.2.0
set -euo pipefail

V="${1:?Uso: scripts/publicar.sh X.Y.Z}"
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Versión no válida: $V (formato X.Y.Z)"; exit 1; }
cd "$(dirname "$0")/.."

[[ -z "$(git status --porcelain)" ]] || { echo "Hay cambios sin commit: no se publica."; exit 1; }
[[ "$(git branch --show-current)" == "main" ]] || { echo "Publica desde la rama main."; exit 1; }
! git rev-parse "v$V" >/dev/null 2>&1 || { echo "La etiqueta v$V ya existe."; exit 1; }

sed -i "0,/^version = \".*\"/s//version = \"$V\"/" Cargo.toml
cargo update -p garbo --offline >/dev/null
sed -i "s|<releases>|<releases>\n    <release version=\"$V\" date=\"$(date +%F)\"/>|" \
    packaging/io.github.garbanhh.Garbo.metainfo.xml
cargo test --locked -q

git commit -qam "Versión $V"
git tag -a "v$V" -m "Garbo $V"
git push -q origin main "v$V"
git push -q publico main "v$V"
echo "Publicada v$V → https://github.com/garbanhh/garbo/actions"
