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

## Workspace

| Crate | Role |
|-------|------|
| [`regf-rs`](https://crates.io/crates/regf-rs) | Windows Registry hive (REGF) reader + in-place writer, pure Rust (published) |
| `os-switcher-bcd` | BCD semantics over `regf-rs` (default / boot sequence) |
| `os-switcher-efi` | UEFI `BootOrder` / `BootNext` over `efivar` |
| `os-switcher-core` | unified model over EFI + BCD, elevation, reboot/shutdown |
| `os-switcher` | the binary: CLI + eframe GUI |

## Usage

GUI (no arguments, or a double-click):

```sh
os-switcher
```

CLI (bypasses the UI):

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
  `os-switcher elevation install`). That registers a scheduled task set to run
  with highest privileges; later launches start it with `schtasks /run`, which
  needs no consent because consent was given when the task was created. The
  task takes no arguments and always just opens the GUI, so it cannot be used
  to run something else elevated. `os-switcher elevation remove` undoes it.
- **Linux** — install the polkit policy from [`packaging/`](packaging/). It
  lets the user physically at the machine act without a password, and falls
  back to admin authentication for anything remote.

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
