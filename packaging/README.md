# Packaging & installation

## Linux

One binary and one polkit rule.

```sh
cargo build --release

# The program: CLI, GUI, and — through pkexec — its own privileged half.
install -Dm755 target/release/os-switcher /usr/bin/os-switcher

# polkit rule: no prompt on the active local session, admin auth otherwise
install -Dm644 packaging/org.le-syl21.os-switcher.policy \
  /usr/share/polkit-1/actions/org.le-syl21.os-switcher.policy

# Desktop launcher (GUI)
install -Dm644 packaging/os-switcher.desktop /usr/share/applications/os-switcher.desktop
```

The policy's `exec.path` names `/usr/bin/os-switcher`, so the passwordless
active-session rule only applies once the binary is installed there. Run from a
build directory, `pkexec` falls back to asking for an admin password.

`allow_active=yes` is what makes the app usable without typing a password every
time you pick an OS. It does mean the user sitting at the machine can run
`os-switcher` as root without authenticating — including `--bcd`, which reads
and writes a hive at a path they choose. Drop that to `auth_admin_keep` if your
threat model does not accept it.

## Windows

Nothing to install: `os-switcher.exe` is self-contained, needs no Visual C++
redistributable, and opens no console window when double-clicked.

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
task's action is fixed — the executable, with `--gui` — so it cannot be used to
run anything else elevated. Move the executable and the app re-registers the
task the next time it runs elevated.
