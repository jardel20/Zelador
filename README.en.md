# Zelador

**An operations panel for NixOS**: update the system with a review before applying, roll back to an earlier generation and free up disk space, all in a GTK4/libadwaita window, without having to remember commands.

[![CI](https://github.com/jardel20/Zelador/actions/workflows/ci.yml/badge.svg)](https://github.com/jardel20/Zelador/actions/workflows/ci.yml)

[Versão em português](README.md) · [Docs (Portuguese)](docs/) · [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md) · [MIT license](LICENSE)

<p align="center">
  <img src="design/screenshots/readme/claro/seu-sistema.png" alt="Your system screen" width="49%">
  <img src="design/screenshots/readme/escuro/seu-sistema.png" alt="Your system screen, dark theme" width="49%">
</p>

> **Status:** version 0.1, written for the author's daily use. Tested on `x86_64-linux`, `nixos-unstable` (26.11) and Nix 2.34, with automated tests and a NixOS VM test. Read [Limitations](#limitations) before installing. The interface is available in Portuguese (Brazil) and English; the English catalog was machine-translated and has not been reviewed by a person. The documentation in `docs/` is in Portuguese.

## What it is (and is not)

Zelador is an **operations** layer on top of the tools you already use (`nix`, `git`, `flatpak`, optionally `nvd`). It answers the everyday questions of someone who maintains a flake-based NixOS configuration:

- *What changed since I last applied? How many commits have not reached the system yet?*
- *If I update the inputs now, what really changes? Does it fit on my disk? Will it break?*
- *Which generation is running? Can I go back to the previous one?*
- *How much space is Nix using, and how much can I free without losing anything I use?*

It is **not** a configuration editor, a package store, a channel manager or a container manager. Rules the project never breaks:

1. **The app never edits your `.nix` files.** The only file of your flake it writes is `flake.lock`, and only after an update was applied successfully.
2. **The UI never runs as root.** Anything that needs privileges goes through a minimal helper, with a polkit authorization for every operation.
3. **Nothing destructive happens without confirmation.** Every cleanup can be simulated first, and every action shows what it will do.

## Features

| Screen | What it does |
|---|---|
| **Your system** | Running generation, how many configuration commits have not been applied (flagged when the count is approximate), age of each flake input, `/nix/store` size, state of automatic GC/optimisation and the **history** of recent operations. |
| **Update** | A 3-step wizard: choose inputs (all, just one, or pin to a revision) → build **without privileges**, with real progress → review package changes and apply (now, at next boot, or "until reboot"). |
| **Generations** | Lists system and Home Manager generations; compares two; deletes; goes back to one; makes one the boot default. |
| **Free up space** | Cleanup policy (keep the last N generations, days, forgotten `result` links, unused Flatpak runtimes, store deduplication), **simulation**, and execution with **a single password prompt**. |
| **Disk space** | `/nix/store` usage by category (in use, can go), largest packages and *why* each one is on the system. |
| **Applying** | Activation progress, step by step. Activation runs in its own systemd unit and carries on even if the window is closed. |

Extras: a disk check **before** the build (blocks if there is not enough space), build-failure handling (the relevant error, open the offending file, full log, "try again without updating input X"), an optional timer that **only notifies** (stale inputs, low disk, lots of garbage), optional `nvd` comparison, light/dark themes and translations (pt-BR and English).

<p align="center">
  <img src="design/screenshots/readme/claro/atualizar.png" alt="Update wizard" width="49%">
  <img src="design/screenshots/readme/claro/liberar-espaco.png" alt="Free up space" width="49%">
</p>

## Installation

Requirements: NixOS with flakes enabled (`nix-command flakes`), a polkit authentication agent in your graphical session (the one that shows the password prompt in other apps), and `git` if your flake is a repository.

### Try it without installing

```sh
nix run github:jardel20/Zelador#zelador
```

Without the module the app **opens and reads** your system, simulates cleanups and builds updates, but privileged actions (apply, roll back, delete generations, clean) are disabled, with a notice on screen.

### Install (NixOS module)

In your system's `flake.nix`:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    zelador.url = "github:jardel20/Zelador";
    zelador.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nixpkgs, zelador, ... }: {
    nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
      modules = [
        ./configuration.nix
        zelador.nixosModules.default
        { programs.zelador.enable = true; }
      ];
    };
  };
}
```

Then switch as usual (`sudo nixos-rebuild switch --flake .#my-host`, or `nh os switch`) and open **Zelador** from the application menu or with `zelador`.

The module installs the app, the `zelador-core` CLI, the privileged helper, the polkit action, the systemd socket that activates the helper, icons and fonts (Poppins and JetBrains Mono). **It does not use `pkexec` or setuid.**

### Module options

| Option | Default | Purpose |
|---|---|---|
| `programs.zelador.enable` | `false` | Turns everything on. |
| `programs.zelador.flakePath` | `/etc/nixos` | Flake folder watched by the check timer. |
| `programs.zelador.checkTimer.enable` | `false` | User timer that checks and **only notifies**. |
| `programs.zelador.checkTimer.interval` | `"weekly"` | When to check (systemd `OnCalendar`). |
| `programs.zelador.checkTimer.staleDays` | `14` | Warn when an input is older than this. |
| `programs.zelador.checkTimer.minFreeGiB` | `10` | Warn when free space drops below this. |
| `programs.zelador.checkTimer.reclaimableGiB` | `20` | Warn when more than this can be reclaimed (0 disables). |
| `programs.zelador.package` / `cliPackage` / `helperPackage` | the flake's packages | To override the packages. |

## Command line

`zelador-core` runs the same logic as the interface, handy for diagnostics and scripts:

```sh
zelador-core status                 # running generation, pending commits, inputs, Flatpak
zelador-core space                  # /nix/store usage by category
zelador-core clean                  # SIMULATES the cleanup (changes nothing)
zelador-core clean --execute --yes  # runs it (asks for the admin password once)
zelador-core update                 # builds the update without applying it
zelador-core check --notify         # what the timer does
zelador-core history                # recent operations
```

## How it works, on one page

- **`zelador-core`** (library + CLI) does everything that needs no privileges: reads generations, GC roots, sizes, `flake.lock`, `git`, Flatpak; computes estimates; builds plans; parses Nix progress.
- **`zelador-gtk`** (the app) only presents and orchestrates. No long operation runs on the UI thread.
- **`zelador-helper`** is the only code with privileges. systemd listens on `/run/zelador/helper.sock` and starts `zelador-helper --serve` as root for each connection. The helper accepts **only** a closed set of subcommands (`activate`, `rollback`, `set-boot-default`, `delete-generations`, `gc`, `optimise`, `flatpak-remove-unused` and the aggregate `clean`), validates everything itself, identifies the caller through the kernel (`SO_PEERCRED`) and asks polkit (`pkcheck`) before doing anything.

## Limitations

- **Tested only on `x86_64-linux` with `nixos-unstable`.** `aarch64-linux` is declared in the flake but has never been built or tested.
- **It depends on CLI output** (`nix`, `nvd`, `flatpak`). It is covered by real fixtures, but format changes in future versions may need adjustments.
- **Space estimates are an upper bound.** Nix reports the logical size of paths, which ignores hardlink-shared files; the app caps it by real disk usage, but the exact figure is only known after cleaning.
- **A polkit agent is required.** Without one, privileged actions are refused (and the app says why).
- **The timer notification** uses `notify-send`; it depends on your notification daemon accepting it.
- **Updating can break your configuration** if a new input removes options you use. Zelador builds **before** applying precisely so this shows up as a build error without touching the system.

## Development

```sh
nix develop
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
nix build .#checks.x86_64-linux.helper-vm   # helper test in a NixOS VM (needs KVM)
```

## Contributing

Issues and PRs are welcome, in Portuguese or English. See [CONTRIBUTING.md](CONTRIBUTING.md). Security issues: [SECURITY.md](SECURITY.md).

## License

MIT, see [LICENSE](LICENSE). [Phosphor](https://phosphoricons.com) icons (MIT) are embedded in the binary; Poppins and JetBrains Mono fonts (SIL OFL) are installed by the module.
