#!/usr/bin/env bash
# Extrai os textos traduzíveis do app para po/zelador.pot e atualiza os catálogos existentes.
# Os msgid são os próprios textos em pt-BR, marcados com tr(), trf() e tn().
# Uso: nix develop path:. -c tools/extract-pot.sh
set -euo pipefail
cd "$(dirname "$0")/.."
files=$(find crates/gtk/src -name '*.rs' | sort)
# shellcheck disable=SC2086
xgettext --language=C --from-code=UTF-8 --package-name=zelador --package-version=0.1.0 \
  --msgid-bugs-address="" --sort-by-file --add-location=file \
  --keyword=tr:1 --keyword=trf:1 --keyword=tn:1,2 \
  -o po/zelador.pot $files
for po in po/*.po; do
  [ -e "$po" ] || continue
  msgmerge --quiet --update --backup=none "$po" po/zelador.pot
done
echo "po/zelador.pot: $(grep -c '^msgid ' po/zelador.pot) mensagens"
