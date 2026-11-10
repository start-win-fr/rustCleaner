use crate::system::start_elevated;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashSet;
use std::env;
use std::os::windows::process::CommandExt;
use std::process::Command;
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};

const UNINSTALL: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
const ERROR_ELEVATION_REQUIRED: i32 = 740;

#[derive(Debug, Clone, Serialize)]
pub struct Program {
    pub id: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub installed_on: String,
    pub bytes: Option<u64>,
    pub icon_source: String,
    #[serde(skip)]
    uninstall_command: String,
}

/// Splits a registry uninstall command into the program and its raw arguments.
pub fn split_command(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if let Some(rest) = line.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some((rest[..end].to_string(), rest[end + 1..].trim().to_string()));
    }
    // Unquoted paths may contain spaces, so the program ends at its extension.
    if let Some(position) = line.to_ascii_lowercase().find(".exe") {
        let end = position + 4;
        return Some((line[..end].to_string(), line[end..].trim().to_string()));
    }
    match line.split_once(char::is_whitespace) {
        Some((program, arguments)) => Some((program.to_string(), arguments.trim().to_string())),
        None => Some((line.to_string(), String::new())),
    }
}

/// Replaces `%NAME%` with the value of the environment variable, when set.
pub fn expand_environment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let resolved = after
            .find('%')
            .and_then(|end| env::var(&after[..end]).ok().map(|value| (end, value)));
        match resolved {
            Some((end, value)) => {
                out.push_str(&value);
                rest = &after[end + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn list() -> Vec<Program> {
    let mut programs = Vec::new();
    for (root, flags, label) in [
        (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY, "M64"),
        (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, "M32"),
        (HKEY_CURRENT_USER, 0, "U"),
    ] {
        read_hive(root, flags, label, &mut programs);
    }
    let mut seen = HashSet::new();
    programs.retain(|program| {
        seen.insert((program.name.to_lowercase(), program.version.to_lowercase()))
    });
    programs.sort_by_key(|program| program.name.to_lowercase());
    programs
}

fn read_hive(root: winreg::HKEY, flags: u32, label: &str, programs: &mut Vec<Program>) {
    let Ok(key) = RegKey::predef(root).open_subkey_with_flags(UNINSTALL, KEY_READ | flags) else {
        return;
    };
    for subkey_name in key.enum_keys().flatten() {
        let Ok(subkey) = key.open_subkey_with_flags(&subkey_name, KEY_READ | flags) else {
            continue;
        };
        let text = |name: &str| subkey.get_value::<String, _>(name).unwrap_or_default();
        let number = |name: &str| subkey.get_value::<u32, _>(name).ok();
        let name = text("DisplayName");
        let uninstall_command = text("UninstallString");
        let hidden = number("SystemComponent") == Some(1)
            || !text("ParentKeyName").is_empty()
            || !text("ReleaseType").is_empty();
        if name.trim().is_empty() || uninstall_command.trim().is_empty() || hidden {
            continue;
        }
        programs.push(Program {
            id: format!("{label}|{subkey_name}"),
            version: text("DisplayVersion"),
            publisher: text("Publisher"),
            installed_on: text("InstallDate"),
            bytes: number("EstimatedSize").map(|kb| u64::from(kb) * 1024),
            icon_source: icon_source(&text("DisplayIcon"), &uninstall_command),
            name,
            uninstall_command,
        });
    }
}

/// Publisher uninstallers usually carry the program's icon; msiexec only has its own.
fn icon_source(display_icon: &str, uninstall_command: &str) -> String {
    if display_icon.trim().is_empty() && !uninstall_command.to_lowercase().contains("msiexec") {
        uninstall_command.to_string()
    } else {
        display_icon.to_string()
    }
}

/// Starts the publisher's uninstaller. The command comes from the registry, never the caller.
pub fn uninstall(id: &str) -> Result<()> {
    let program = list()
        .into_iter()
        .find(|program| program.id == id)
        .context("ce programme n'est plus installé")?;
    let expanded = expand_environment(&program.uninstall_command);
    let (exe, arguments) = split_command(&expanded).context("commande de désinstallation vide")?;
    match Command::new(&exe).raw_arg(&arguments).spawn() {
        Ok(_) => Ok(()),
        Err(error) if error.raw_os_error() == Some(ERROR_ELEVATION_REQUIRED) => {
            start_elevated(&exe, &arguments)
        }
        Err(error) => Err(error).with_context(|| format!("lancement de {exe}")),
    }
}
