# Windows installer

PC Agent can be packaged as a per-user NSIS installer. It installs under the user's profile
(`installMode: currentUser`), so installing, updating and uninstalling never ask for
administrator rights. The installer is unsigned.

## Build (Windows)

```powershell
$env:CARGO_TARGET_DIR = "D:\pcagent-target"   # on the dev PC, C: is nearly full
cargo build --release --locked -p pcagent -p pcagent-elevated
cargo install tauri-cli --version "^2" --locked   # once
cd apps\gui
cargo tauri build --config tauri.installer.conf.json
```

`tauri.installer.conf.json` is merged over `tauri.conf.json` only for this command, so ordinary
`cargo build` and `cargo test` runs never need the release binaries it bundles. When
`CARGO_TARGET_DIR` points elsewhere, adjust the two `resources` paths, which expect the release
`pcagent-elevated.exe` and `pcagent.exe` next to the app.

## What it installs

- `pcagent-gui.exe` (the window), `pcagent-elevated.exe` (the read-only administrator helper,
  started only after a local review and a Windows consent prompt) and `pcagent.exe` (the CLI),
  side by side as the app expects.
- Start menu entry; WebView2 is installed by Microsoft's bootstrapper when missing.

## Uninstall

Before removing the files, the uninstaller runs `pcagent-gui.exe --remove-scheduled-report`,
which removes this account's daily report task (journaled, refused when elevated, no error when
absent); see `installer/hooks.nsh`. Settings, the history and reports in `%APPDATA%\pcagent` are
kept, as for any per-user app; delete that folder to remove them.

## Validation limits

The release includes a built NSIS installer. A successful build does not validate installation,
updating an older copy or uninstalling with a scheduled report task: those end-to-end flows still
need to be checked on Windows. The portable app's native smoke tests are not installer tests.
