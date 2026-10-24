#!/usr/bin/env bash
# Importa os ícones Phosphor usados pelo Zelador para o gresource, como `<nome>-symbolic.svg`
# (o GTK recolore ícones simbólicos conforme o tema, inclusive o escuro).
# Uso: tools/import-phosphor.sh <pasta do pacote @phosphor-icons/core>
#   ex.: (cd /tmp && npm pack @phosphor-icons/core && mkdir x && tar xzf *.tgz -C x) \
#        && tools/import-phosphor.sh /tmp/x/package
set -euo pipefail
src="${1:?pasta do pacote @phosphor-icons/core}"
dst="$(dirname "$0")/../crates/gtk/resources/icons/scalable/actions"
mkdir -p "$dst"
for name in pulse arrows-clockwise stack broom hard-drives list x warning check \
            circle-notch circle dots-three caret-right folder-open git-branch; do
  cp "$src/assets/regular/$name.svg" "$dst/$name-symbolic.svg"
done
# Único ícone no peso Fill.
cp "$src/assets/fill/check-circle-fill.svg" "$dst/check-circle-fill-symbolic.svg"
cp "$src/LICENSE" "$dst/../../LICENSE-phosphor"
