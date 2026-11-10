# rustCleaner

A Windows maintenance utility written in Rust: free up disk space, find duplicate files, uninstall programs, control what starts with Windows, slow down background apps, and update software and drivers.

It comes as a desktop app (Tauri) and a small command-line tool, both built on the same library. Anything that removes files goes through the Recycle Bin, so a cleanup can be undone until the bin is emptied.

> The interface and the command-line messages are currently in French. The code and this document are in English.

## Features

| Section | What it does | How it works |
|---|---|---|
| **Overview** | Shows used and free space per drive, and a summary of every other section. | Reads drive information and the lists below. |
| **Cleaning** | Removes temporary files, crash dumps, archived error reports and browser caches. | Declarative TOML rules, with a preview of every file before anything moves. |
| **Duplicates** | Finds identical files in the folders you pick. | Groups by size, then compares a 64 KB hash, then a full BLAKE3 hash. |
| **Programs** | Lists installed programs and starts their uninstaller. | Reads the Windows Uninstall registry keys and runs the publisher's own uninstaller. |
| **Startup** | Turns startup items on or off without deleting them. | Uses the same `StartupApproved` values as Windows, plus scheduled tasks. |
| **Performance** | Applies Windows efficiency mode to programs you choose. | Lowers priority and enables power throttling until the program closes. |
| **Software** | Lists and installs available updates. | Runs `winget`. |
| **Drivers** | Lists and installs driver updates offered for this PC. | Uses the Windows Update Agent API, after attempting a restore point. |

Performance mode leaves the processor to the programs you are using. It does not close anything and does not free memory, and rustCleaner has no registry "speed-up" tweaks.

## Safety model

rustCleaner is deliberately cautious, because a cleaning tool can do real damage when it is wrong.

**Files**

- Files are moved to the Recycle Bin, never deleted directly.
- Scanning is read-only. Nothing moves until you confirm: a dialog in the app, or typing `oui` in the CLI.
- Just before moving a file, rustCleaner checks that its type, size and modification date have not changed, and that its real path is still inside the scanned folder. Otherwise the file stays in place.
- Symbolic links and junctions are not followed.
- Protected locations cannot be cleaned: Windows system folders, Program Files, and personal folders such as Documents, Desktop and Downloads.
- Temporary files less than 24 hours old are left alone, and browser caches are skipped while the browser is running.

**Duplicates**

- At least one copy of each group always stays. The app locks the last copy and the engine refuses a selection that would remove every copy.
- Hard links are not reported, since removing one frees nothing.
- `.git`, `node_modules`, `AppData`, `$RECYCLE.BIN` and `System Volume Information` are skipped.

**System changes**

- rustCleaner does not need to run as administrator. When an action needs more rights, Windows shows its normal authorization prompt for that action.
- Values read from the system reach PowerShell base64-encoded, so a program or task name cannot be interpreted as code.
- The interface sends only identifiers. Commands are looked up again by the backend, so the interface cannot inject a command to run.
- Firmware updates are never preselected.
- Major version upgrades are excluded from "update all", and need an individual confirmation.

## Download

**[Download the desktop app (`rustcleaner-gui.exe`)](https://github.com/start-win-fr/rustCleaner/releases/latest/download/rustcleaner-gui.exe)**

No installation is needed: download the file and double-click it. The command-line tool, `rustcleaner.exe`, and the SHA-256 checksums are on the [releases page](https://github.com/start-win-fr/rustCleaner/releases/latest).

The executables are not code-signed, so Windows SmartScreen may warn you on first launch. To continue, choose **More info**, then **Run anyway**.

## Requirements

- Windows 10 or 11, x64
- [Microsoft WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/), for the desktop app (included with Windows 11)
- [winget](https://learn.microsoft.com/windows/package-manager/winget/), for software updates
- To build from source: a recent stable [Rust toolchain](https://rustup.rs/) with the MSVC target (the project uses Rust edition 2024)

## Build

```powershell
git clone https://github.com/start-win-fr/rustCleaner.git
cd rustCleaner
cargo build --release --workspace
```

This produces two programs:

- `target\release\rustcleaner-gui.exe`: the desktop app
- `target\release\rustcleaner.exe`: the command-line tool

The interface is plain HTML, CSS and JavaScript embedded in the executable, so there is no Node.js step.

## Command-line usage

```powershell
# List the available rules
rustcleaner list

# Preview what would be removed. Nothing changes.
rustcleaner scan
rustcleaner scan --details
rustcleaner scan --only system.user-temp,browser.edge.cache
rustcleaner scan --json

# Move the files to the Recycle Bin, after typing "oui" to confirm
rustcleaner clean
rustcleaner clean --only system.user-temp
```

| Option | Applies to | Meaning |
|---|---|---|
| `--rules <DIR>` | all commands | Use the `.toml` files of this folder instead of the built-in rules |
| `--only <IDS>` | `scan`, `clean` | Limit the run to these comma-separated rule ids |
| `--all` | `scan`, `clean` | Include rules that are disabled by default |
| `--details` | `scan` | List every matching file |
| `--json` | `scan` | Output JSON |

Cleaning `C:\Windows\Temp` needs a terminal opened as administrator.

## Cleaning rules

Rules are declarative, so adding a target needs no code. The built-in set is in [`rules/default.toml`](rules/default.toml):

| Id | Target |
|---|---|
| `system.user-temp` | The user's temporary folder |
| `system.windows-temp` | The Windows temporary folder |
| `system.crash-dumps` | Application crash dumps |
| `system.error-reports` | Archived Windows error reports |
| `browser.chrome.cache` | Google Chrome cache |
| `browser.edge.cache` | Microsoft Edge cache |
| `browser.firefox.cache` | Mozilla Firefox cache |

A rule looks like this:

```toml
[[rule]]
id = "browser.example.cache"
name = "Example browser: cache"
category = "browser"
enabled_by_default = true                       # optional, true by default
blocked_by_processes = ["example.exe"]          # skipped while this program runs
detect = ["{LOCALAPPDATA}/Example/User Data"]   # applies only if one of these paths exists

[[rule.target]]
path = "{LOCALAPPDATA}/Example/User Data/*/Cache"   # "*" matches one folder level
pattern = "*.tmp"                                   # optional file mask, "*" by default
recursive = true                                    # optional, false by default
min_age_hours = 24                                  # optional, newer files are kept
```

Paths must start with one of these variables: `{TEMP}`, `{LOCALAPPDATA}`, `{APPDATA}`, `{USERPROFILE}`, `{WINDIR}`, `{PROGRAMDATA}`. Files are validated strictly: unknown fields, duplicate ids and paths that climb out of a folder with `..` are rejected. Custom rules are available through the CLI's `--rules` option. The desktop app uses the built-in set.

## Project layout

```text
rules/default.toml   Built-in cleaning rules
src/                 Library and command-line tool
  rules.rs, paths.rs   Rule parsing, validation and path expansion
  scan.rs, clean.rs    Read-only scan, verified move to the Recycle Bin
  safety.rs            Protected locations
  duplicates.rs        Duplicate search
  programs.rs          Installed programs and uninstallers
  startup.rs           Startup items and scheduled tasks
  performance.rs       Running programs and efficiency mode
  updates.rs           winget and Windows Update
  icons.rs             Program icons
  system.rs            PowerShell, authorization prompt, drive information
src-tauri/           Desktop app: commands exposed to the interface
ui/                  Interface (HTML, CSS, JavaScript, no framework)
tests/               Integration tests
```

## Tests

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets
```

The tests use a fake Recycle Bin and temporary folders, so they do not touch your files. A few of them start PowerShell. None of them shows an authorization prompt, installs anything or changes system settings.

## Known limitations

- Windows only, and the interface is French only.
- Developed and tested on a single Windows 11 PC so far. Treat it as early software and review what a cleanup will move.
- Updates depend on what each publisher ships. winget can offer an update whose installer then refuses to run, for example because the program is open or the published file changed. The app shows the reason for the failures it recognizes.
- WampServer is listed as a manual update, because its installer refuses to replace an existing version.
- Microsoft Store apps keep a coloured initial instead of their real icon, as do programs that declare no icon.
- A cleanup frees disk space only once the Recycle Bin is emptied.

## Disclaimer

rustCleaner changes system settings, runs third-party installers and moves files. It comes with no warranty. Keep backups of anything important, and be careful with driver and firmware updates.

This is an independent project, not affiliated with any other PC maintenance software. Its cleaning rules were written for this project.

## License

Released under the [MIT License](LICENSE).
