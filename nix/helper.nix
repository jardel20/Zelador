# Helper privilegiado + action do polkit. As ferramentas executadas como root (e o pkcheck)
# são fixadas por caminho do store no build; nada vem do ambiente em tempo de execução.
{
  lib,
  rustPlatform,
  nix,
  systemd,
  flatpak,
  polkit,
}:
rustPlatform.buildRustPackage {
  pname = "zelador-helper";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../clippy.toml
      ../crates
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [ "--package" "zelador-helper" ];
  cargoTestFlags = [ "--package" "zelador-helper" ];

  ZELADOR_NIX_ENV = "${nix}/bin/nix-env";
  ZELADOR_NIX_STORE = "${nix}/bin/nix-store";
  ZELADOR_SYSTEMD_RUN = "${systemd}/bin/systemd-run";
  ZELADOR_FLATPAK = "${flatpak}/bin/flatpak";
  ZELADOR_PKCHECK = "${polkit}/bin/pkcheck";

  postInstall = ''
    install -Dm644 ${./br.dev.zelador.helper.policy.in} \
      $out/share/polkit-1/actions/br.dev.zelador.helper.policy
  '';

  meta = {
    description = "Helper privilegiado do Zelador (serviço do systemd com polkit)";
    mainProgram = "zelador-helper";
  };
}
