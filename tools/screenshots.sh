#!/usr/bin/env bash
# Captura cada tela do app num compositor Wayland sem tela (cage headless), para comparar
# com design/mockups/. Não abre janelas na sua sessão.
# Uso: [ESPERA=segundos] [ZELADOR_DEV_ACTION=select-two|compare|why|simulate|confirm-clean] [ZELADOR_SCROLL_END_AFTER=segundos] [ZELADOR_FLAKE=/etc/nixos] [ESQUEMA=prefer-dark] [TEMA_SISTEMA=1] [PAGINAS="seu-sistema geracoes"] \
#        nix develop path:. -c tools/screenshots.sh <pasta de saída> [binário]
set -euo pipefail
out="${1:?pasta de saída}"
bin="${2:-target/debug/zelador}"
mkdir -p "$out"
for page in ${PAGINAS:-seu-sistema atualizar geracoes liberar-espaco espaco-em-disco aplicando}; do
  rt="$(mktemp -d)"
  chmod 700 "$rt"
  mkdir -p "$rt/config/zelador"
  if [ -n "${TEMA_SISTEMA:-}" ]; then
    printf "[appearance]\nuse-system-theme=true\n" > "$rt/config/zelador/config.ini"
  fi
  (
    # XDG_CONFIG_HOME vazio: o ~/.config/gtk-4.0/gtk.css do usuário não interfere na captura.
    export XDG_RUNTIME_DIR="$rt" XDG_CONFIG_HOME="$rt/config" WLR_BACKENDS=headless WLR_RENDERER=pixman \
      WLR_LIBINPUT_NO_DEVICES=1 GDK_BACKEND=wayland ADW_DEBUG_COLOR_SCHEME="${ESQUEMA:-default}" \
      ZELADOR_START_PAGE="$page" ZELADOR_FLAKE="${ZELADOR_FLAKE:-$HOME/nixos-config}"
    nix shell nixpkgs#cage nixpkgs#grim -c sh -c '
      unset WAYLAND_DISPLAY
      cage -s -- "'"$bin"'" >/dev/null 2>&1 &
      sock=""
      for _ in $(seq 1 100); do
        sock="$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -name "wayland-*" ! -name "*.lock" -printf "%f" -quit)"
        [ -n "$sock" ] && break; sleep 0.1
      done
      sleep "${ESPERA:-3}"
      WAYLAND_DISPLAY="$sock" grim "'"$out/$page.png"'"
      kill %1 2>/dev/null || true
    '
  )
  rm -rf "$rt"
  echo "capturado: $out/$page.png"
done
