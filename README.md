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

## Workspace

| Crate | Role |
|-------|------|
| [`regf-rs`](https://crates.io/crates/regf-rs) | Windows Registry hive (REGF) reader + in-place writer, pure Rust (published) |
| `os-switcher-bcd` | BCD semantics over `regf-rs` (default / boot sequence) |
| `os-switcher-efi` | UEFI `BootOrder` / `BootNext` over `efivar` |
| `os-switcher-core` | unified model over EFI + BCD, elevation, reboot/shutdown |
| `os-switcher-helper` | minimal privileged helper (the only thing that writes) |
| `os-switcher` | CLI + eframe GUI |

## Usage

GUI (no arguments):

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

Reading needs no privileges; changing the boot OS is delegated to a small
privileged helper (a polkit prompt on Linux). See [`packaging/`](packaging/).

## Build

```sh
cargo build --release
cargo test          # workspace tests (no hardware needed)
```

## License

GPL-3.0-or-later. The `regf-rs` engine is a separate crate under MIT OR
Apache-2.0.
