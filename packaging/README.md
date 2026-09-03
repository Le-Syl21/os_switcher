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
`os-switcher.exe` is the CLI. Keep them side by side.

Reading the firmware boot entries needs an elevated token on Windows, so out of
the box every launch asks for **UAC** — the GUI once per session, the CLI once
per command. That works with nothing installed.

To stop the prompts, install the **service broker** (opt-in, one UAC prompt):
tick *"Skip the approval prompt"* in the app, or:

```powershell
os-switcher install               # one UAC prompt
os-switcher uninstall             # (add --purge to also drop the saved state)
os-switcher repair-service        # re-point the service after moving the files
```

Install copies both executables to `%ProgramFiles%\os-switcher\` and registers a
small Windows service (`os-switcher-broker`, running as LocalSystem, started on
demand when the app opens its pipe — nothing runs at boot). From then on the app
talks to it over a named pipe and never prompts. The service answers
exactly three requests — read the boot state, arm a selection, clear it — each
validated against the machine's real entries; it runs no arbitrary command and
takes no path from the caller. Remove it with `uninstall` (or from *Apps &
features*), and every launch goes back to a UAC prompt.

The signature is shown for information only: the project is open source, so a
build you compiled yourself works exactly the same — the UAC dialog naming the
publisher is what tells signed from unsigned.
