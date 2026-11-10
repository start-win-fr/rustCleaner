use crate::system::{elevated_failure, literal, run_elevated, run_powershell};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, REG_BINARY};
use winreg::types::FromRegValue;

const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_32: &str = "Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Run";
const APPROVED: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved";
const STARTUP_FOLDER: &str = "Microsoft\\Windows\\Start Menu\\Programs\\Startup";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hive {
    User,
    Machine,
}

impl Hive {
    fn open(self) -> RegKey {
        RegKey::predef(match self {
            Hive::User => HKEY_CURRENT_USER,
            Hive::Machine => HKEY_LOCAL_MACHINE,
        })
    }
}

#[derive(Debug, Clone)]
enum Source {
    Approved {
        hive: Hive,
        section: &'static str,
        value: String,
    },
    Task {
        path: String,
        name: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct StartupEntry {
    pub id: String,
    pub name: String,
    pub command: String,
    pub location: String,
    pub enabled: bool,
    /// Changing this entry shows the Windows authorization prompt.
    pub needs_admin: bool,
    #[serde(skip)]
    source: Source,
}

/// Windows marks a disabled entry with an odd first byte.
pub fn is_enabled(approved: Option<&[u8]>) -> bool {
    approved
        .and_then(|bytes| bytes.first())
        .is_none_or(|first| first & 1 == 0)
}

fn approved_bytes(enabled: bool) -> Vec<u8> {
    let mut bytes = vec![0u8; 12];
    bytes[0] = if enabled { 2 } else { 3 };
    bytes
}

fn read_approved(hive: Hive, section: &str, value: &str) -> bool {
    let raw = hive
        .open()
        .open_subkey(format!("{APPROVED}\\{section}"))
        .and_then(|key| key.get_raw_value(value));
    is_enabled(raw.ok().as_ref().map(|value| value.bytes.as_ref()))
}

pub fn list() -> Result<Vec<StartupEntry>> {
    let mut entries = Vec::new();
    for (hive, key, section, label) in [
        (Hive::User, RUN, "Run", "Registre, utilisateur"),
        (Hive::Machine, RUN, "Run", "Registre, tous les utilisateurs"),
        (Hive::Machine, RUN_32, "Run32", "Registre, tous les utilisateurs (32 bits)"),
    ] {
        read_run_key(hive, key, section, label, &mut entries);
    }
    read_folder(Hive::User, env::var_os("APPDATA"), "Dossier Démarrage, utilisateur", &mut entries);
    read_folder(
        Hive::Machine,
        env::var_os("PROGRAMDATA"),
        "Dossier Démarrage, tous les utilisateurs",
        &mut entries,
    );
    entries.extend(read_tasks()?);
    entries.sort_by_key(|entry| entry.name.to_lowercase());
    Ok(entries)
}

fn read_run_key(
    hive: Hive,
    path: &str,
    section: &'static str,
    label: &str,
    entries: &mut Vec<StartupEntry>,
) {
    let Ok(key) = hive.open().open_subkey(path) else {
        return;
    };
    for (name, value) in key.enum_values().flatten() {
        let Ok(command) = String::from_reg_value(&value) else {
            continue;
        };
        entries.push(StartupEntry {
            id: format!("reg|{hive:?}|{section}|{name}"),
            enabled: read_approved(hive, section, &name),
            needs_admin: hive == Hive::Machine,
            location: label.to_string(),
            command,
            source: Source::Approved {
                hive,
                section,
                value: name.clone(),
            },
            name,
        });
    }
}

fn read_folder(
    hive: Hive,
    base: Option<std::ffi::OsString>,
    label: &str,
    entries: &mut Vec<StartupEntry>,
) {
    let Some(base) = base else {
        return;
    };
    let folder = PathBuf::from(base).join(STARTUP_FOLDER);
    let Ok(items) = fs::read_dir(&folder) else {
        return;
    };
    for item in items.flatten() {
        let file_name = item.file_name().to_string_lossy().into_owned();
        if file_name.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        let name = item
            .path()
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_name.clone());
        entries.push(StartupEntry {
            id: format!("folder|{hive:?}|{file_name}"),
            enabled: read_approved(hive, "StartupFolder", &file_name),
            needs_admin: hive == Hive::Machine,
            location: label.to_string(),
            command: item.path().display().to_string(),
            source: Source::Approved {
                hive,
                section: "StartupFolder",
                value: file_name,
            },
            name,
        });
    }
}

#[derive(Deserialize)]
struct RawTask {
    path: String,
    name: String,
    state: String,
    command: Option<String>,
}

fn read_tasks() -> Result<Vec<StartupEntry>> {
    let script = r#"
$tasks = @(Get-ScheduledTask | Where-Object {
    $_.TaskPath -notlike '\Microsoft\*' -and
    ($_.Triggers | Where-Object { $_.CimClass.CimClassName -in 'MSFT_TaskLogonTrigger','MSFT_TaskBootTrigger' })
} | ForEach-Object {
    [pscustomobject]@{ path=$_.TaskPath; name=$_.TaskName; state=[string]$_.State; command=(($_.Actions | ForEach-Object { $_.Execute }) -join ' ') }
})
ConvertTo-Json -InputObject $tasks -Compress
"#;
    let output = run_powershell(script).context("lecture des tâches planifiées")?;
    let tasks: Vec<RawTask> =
        serde_json::from_str(output.trim()).context("réponse illisible du planificateur")?;
    Ok(tasks
        .into_iter()
        .map(|task| StartupEntry {
            id: format!("task|{}|{}", task.path, task.name),
            enabled: !task.state.eq_ignore_ascii_case("Disabled"),
            needs_admin: false,
            location: "Tâche planifiée".to_string(),
            command: task.command.unwrap_or_default(),
            name: task.name.clone(),
            source: Source::Task {
                path: task.path,
                name: task.name,
            },
        })
        .collect())
}

/// The entry is looked up again, so only an entry that really exists can change.
pub fn set_enabled(id: &str, enabled: bool) -> Result<()> {
    let entry = list()?
        .into_iter()
        .find(|entry| entry.id == id)
        .context("cet élément n'existe plus")?;
    match entry.source {
        Source::Approved {
            hive: Hive::User,
            section,
            value,
        } => {
            let (key, _) = RegKey::predef(HKEY_CURRENT_USER)
                .create_subkey(format!("{APPROVED}\\{section}"))
                .context("ouverture du registre")?;
            key.set_raw_value(
                    &value,
                    &winreg::RegValue {
                        bytes: approved_bytes(enabled).into(),
                        vtype: REG_BINARY,
                    },
                )
                .context("écriture du registre")
        }
        Source::Approved {
            hive: Hive::Machine,
            section,
            value,
        } => {
            let bytes = approved_bytes(enabled)
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let script = format!(
                "$k='HKLM:\\{APPROVED}\\{section}';\
                 if(-not(Test-Path -LiteralPath $k)){{New-Item -Path $k -Force|Out-Null}};\
                 New-ItemProperty -LiteralPath $k -Name ({}) -PropertyType Binary -Value ([byte[]]({bytes})) -Force|Out-Null",
                literal(&value)
            );
            require_success(run_elevated(&script)?)
        }
        Source::Task { path, name } => {
            let verb = if enabled { "Enable" } else { "Disable" };
            let script = format!(
                "{verb}-ScheduledTask -TaskPath ({}) -TaskName ({})|Out-Null",
                literal(&path),
                literal(&name)
            );
            if run_powershell(&script).is_ok() {
                return Ok(());
            }
            require_success(run_elevated(&script)?)
        }
    }
}

fn require_success(code: i32) -> Result<()> {
    if code != 0 {
        bail!("{}", elevated_failure(code));
    }
    Ok(())
}
