# Packaging & installation (Linux)

Two binaries and one polkit rule.

```sh
# Build in release
cargo build --release

# Unprivileged app + CLI
install -Dm755 target/release/os-switcher        /usr/bin/os-switcher

# Privileged helper (invoked via pkexec)
install -Dm755 target/release/os-switcher-helper /usr/libexec/os-switcher-helper

# polkit rule: no prompt on the active local session, admin auth otherwise
install -Dm644 packaging/org.le-syl21.os-switcher.policy \
  /usr/share/polkit-1/actions/org.le-syl21.os-switcher.policy

# Desktop launcher (GUI)
install -Dm644 packaging/os-switcher.desktop /usr/share/applications/os-switcher.desktop
```

The polkit action's `exec.path` points at `/usr/libexec/os-switcher-helper`, so
install the helper there for the passwordless active-session rule to apply. In
development, `run_helper_elevated` finds the helper next to the running binary
(or via `$OS_SWITCHER_HELPER`) and `pkexec` falls back to an admin password.

## Windows

The core, CLI and helper build on Windows; writes require an elevated process
(run the helper from an elevated context). Automatic UAC elevation / a scheduled
task is not wired up yet.
