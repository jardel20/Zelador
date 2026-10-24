# Helper privilegiado + action do polkit. As ferramentas executadas como root são
# fixadas por caminho do store no build; nada vem do ambiente em tempo de execução.
{
  lib,
  rustPlatform,
  nix,
  systemd,
  flatpak,
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

  postInstall = ''
    install -Dm644 ${./br.dev.zelador.helper.policy.in} \
      $out/share/polkit-1/actions/br.dev.zelador.helper.policy
    substituteInPlace $out/share/polkit-1/actions/br.dev.zelador.helper.policy \
      --replace-fail @helper@ $out/bin/zelador-helper
  '';

  meta = {
    description = "Helper privilegiado do Zelador (chamado via pkexec)";
    mainProgram = "zelador-helper";
  };
}
