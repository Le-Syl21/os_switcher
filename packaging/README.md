# Packaging & installation

## Linux

Two binaries and one polkit rule. `os-switcher` is the CLI (and, through
pkexec, the privileged half); `os-switcher-gui` is the graphical face, built
only with the `gui` feature.

```sh
cargo build --release --features gui   # builds both binaries

# The CLI — also what runs elevated through pkexec.
install -Dm755 target/release/os-switcher /usr/bin/os-switcher
# The GUI (skip this line for a headless, CLI-only install).
install -Dm755 target/release/os-switcher-gui /usr/bin/os-switcher-gui

# polkit rule: no prompt on the active local session, admin auth otherwise
install -Dm644 packaging/org.le-syl21.os-switcher.policy \
  /usr/share/polkit-1/actions/org.le-syl21.os-switcher.policy

# Desktop launcher (starts the GUI)
install -Dm644 packaging/os-switcher.desktop /usr/share/applications/os-switcher.desktop
```

For a CLI-only box, `cargo build --release` (no `gui` feature) builds just
`os-switcher`, with none of the GUI dependencies.

The policy's `exec.path` names `/usr/bin/os-switcher`, so the passwordless
active-session rule only applies once the binary is installed there. Run from a
build directory, `pkexec` falls back to asking for an admin password.

`allow_active=yes` is what makes the app usable without typing a password every
time you pick an OS. It does mean the user sitting at the machine can run
`os-switcher` as root without authenticating — including `--bcd`, which reads
and writes a hive at a path they choose. Drop that to `auth_admin_keep` if your
threat model does not accept it.

## Windows

Two self-contained executables, no Visual C++ redistributable needed:
`os-switcher-gui.exe` is the app you double-click (it opens no console window),
`os-switcher.exe` is the CLI. Keep them side by side, so `os-switcher elevation
install` can find and register the GUI binary next to it (the GUI itself needs
no sibling — it re-runs itself elevated).

Elevation is asked for at launch, because reading the firmware variables
already requires it. To be asked only once, tick *"Skip the approval prompt"*
in the app, or:

```powershell
os-switcher elevation install   # one UAC prompt, then never again
os-switcher elevation status
os-switcher elevation remove
```

This registers a scheduled task ("OS Switcher") that runs with highest
privileges; the app starts it with `schtasks /run`, which needs no consent. The
task's action is fixed — the GUI executable, with no arguments — so it cannot be
used to run anything else elevated. Move the executable and the app re-registers
the task the next time it runs elevated.
