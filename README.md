# os-switcher

Pick the **next-boot** or **default** operating system on a UEFI multiboot
machine, from Linux or Windows — UEFI *and* Windows BCD aware.

Two selections, mirrored on both mechanisms:

- **Default** — the permanent choice (`BootOrder` / BCD `DefaultObject`).
- **Next boot** — a one-shot, consumed at the next reboot (`BootNext` / BCD
  `BootSequence`).

Both Windows layouts are handled: separate disks (each Windows has its own UEFI
entry) and a shared ESP (the single "Windows Boot Manager" entry is expanded
into one choice per BCD object, e.g. Windows 10 vs Windows 11).

## One binary

`os-switcher` is the whole program. Run it with a subcommand and it behaves
like a CLI; run it with none — or double-click it — and it opens the GUI. On
Windows it is a *windows*-subsystem executable, so double-clicking opens no
console window, and it re-attaches to the terminal's console when you do start
it from one (redirection with `>` still works).

The Microsoft C runtime is linked statically, so the `.exe` needs no
`vcruntime140.dll` or any other redistributable beside it.

## Structure

One crate, two binaries: `os-switcher` (the small CLI, no GUI stack) and
`os-switcher-gui` (the eframe/egui window, behind the `gui` feature). The
library modules:

| Module | Role |
|--------|------|
| `efi` | UEFI `BootOrder` / `BootNext` over `efivar` |
| `bcd` | BCD semantics over [`regf-rs`](https://crates.io/crates/regf-rs) (default / boot sequence) |
| `switcher` | unified model over EFI + BCD, elevation, reboot/shutdown |
| `winbroker` | Windows opt-in service broker (see [Privileges](#privileges)) |

## Usage

GUI (double-click, or run with no arguments):

```sh
os-switcher-gui
```

CLI:

```sh
os-switcher list                 # list bootable entries
os-switcher status               # current default and one-shot
os-switcher default <selector>   # set the default OS
os-switcher next <selector>      # arm a one-shot next boot
os-switcher clear                # clear the one-shot
os-switcher reboot | shutdown
```

`<selector>` is a 0-based index, an entry key, or a case-insensitive label
substring. `--bcd <path>` overrides the BCD hive location.

## Privileges

Changing the boot OS needs root on Linux and an administrator token on Windows.
There is no separate helper binary: when the program lacks the rights for what
you asked, it re-runs *itself* elevated with that same command line — through
`pkexec` on Linux, through the UAC consent dialog on Windows.

**On Windows the rule is stricter than it looks: even *reading* the firmware
variables needs an elevated token**, because `BootOrder` and friends are behind
`SeSystemEnvironmentPrivilege`. So the GUI elevates at launch rather than open
a window that can see nothing.

### Asking only once

Approving a prompt every single time is not a workflow. Both platforms have a
supported way to grant the permission once and keep it:

- **Windows** — tick *"Skip the approval prompt"* in the app (or run
  `os-switcher install`). That installs a small service (`os-switcher-broker`,
  LocalSystem) under `%ProgramFiles%`; the app then talks to it over a named
  pipe and never prompts. The service answers exactly three requests — read the
  boot state, arm a selection, clear it — each validated against the machine's
  real entries, and runs no arbitrary command. `os-switcher uninstall` undoes
  it. This is opt-in: without it, the app works through a per-use UAC prompt.
- **Linux** — tick the same banner, or run `os-switcher install`. It installs
  the polkit policy (`allow_active`) and the CLI at `/usr/bin/os-switcher`, so
  the user physically at the machine acts without a password while anything
  remote still authenticates. `os-switcher uninstall` removes it; the policy
  and manual steps are also in [`packaging/`](packaging/).

### How the boot store is reached

| | UEFI variables | Windows BCD |
|---|---|---|
| **From Linux** | `efivarfs` | the hive on the mounted ESP, edited in place |
| **From Windows** | `SetFirmwareEnvironmentVariable` | `bcdedit` — the running kernel holds the hive open, so the store is read with `bcdedit /export` and changed with `bcdedit /default` and `/bootsequence` |

Going through `bcdedit` on Windows keeps this working regardless of the system
language: only GUIDs and switches cross the boundary, never translated text.

## Build

```sh
cargo build --release
cargo test          # workspace tests (no hardware needed)
```

To drive the GUI from [`egui-mcp`](https://github.com/rerun-io/kittest_inspector)
while developing:

```sh
EGUI_INSPECTION=1 cargo run --features inspection
```

## License

GPL-3.0-or-later. The `regf-rs` engine is a separate crate under MIT OR
Apache-2.0.
