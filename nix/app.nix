# O app GTK. Ícones do app e entrada .desktop vão para o prefixo; os ícones Phosphor e o CSS
# já estão embutidos no binário (gresource).
{
  lib,
  rustPlatform,
  pkg-config,
  wrapGAppsHook4,
  glib,
  gettext,
  gtk4,
  libadwaita,
  git,
  nix,
  flatpak,
  zelador-helper,
}:
rustPlatform.buildRustPackage {
  pname = "zelador";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../clippy.toml
      ../crates
      ../po
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [ "--package" "zelador-gtk" ];
  cargoTestFlags = [ "--package" "zelador-gtk" ];

  # A limpeza chama o helper por este caminho do store.
  ZELADOR_HELPER = "${zelador-helper}/bin/zelador-helper";

  nativeBuildInputs = [
    pkg-config
    wrapGAppsHook4
    glib # glib-compile-resources, usado pelo build.rs
    gettext # msgfmt, para compilar os catálogos de tradução
  ];
  buildInputs = [
    gtk4
    libadwaita
  ];

  # `nix`, `git` e `flatpak` no fim do PATH: o do sistema vence; isto só garante que existam.
  preFixup = ''
    gappsWrapperArgs+=(--set-default ZELADOR_LOCALEDIR $out/share/locale)
    gappsWrapperArgs+=(--suffix PATH : ${lib.makeBinPath [ nix git flatpak ]})
  '';

  postInstall = ''
    for po in po/*.po; do
      lang=$(basename "$po" .po)
      install -d "$out/share/locale/$lang/LC_MESSAGES"
      msgfmt -o "$out/share/locale/$lang/LC_MESSAGES/zelador.mo" "$po"
    done
    mkdir -p $out/share/icons
    cp -r --no-preserve=mode ${../crates/gtk/data/icons/hicolor} $out/share/icons/hicolor
    install -Dm644 ${../crates/gtk/data/br.dev.zelador.desktop} \
      $out/share/applications/br.dev.zelador.desktop
  '';

  meta = {
    description = "Painel operacional do NixOS: atualizar, voltar e liberar espaço";
    mainProgram = "zelador";
  };
}
