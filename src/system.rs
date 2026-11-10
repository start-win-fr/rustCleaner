//! Launching helpers. Values reach PowerShell base64-encoded, so a program or
//! task name read from the system can never be interpreted as code.

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Serialize;
use std::env;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Serialize)]
pub struct DiskInfo {
    pub mount: String,
    pub name: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

pub fn list_disks() -> Result<Vec<DiskInfo>> {
    let mut disks: Vec<DiskInfo> = sysinfo::Disks::new_with_refreshed_list()
        .context("lecture des disques")?
        .iter()
        .filter(|disk| disk.total_space() > 0)
        .map(|disk| DiskInfo {
            mount: disk.mount_point().display().to_string(),
            name: disk.name().to_string_lossy().into_owned(),
            total_bytes: disk.total_space(),
            free_bytes: disk.available_space(),
        })
        .collect();
    disks.sort_by(|a, b| a.mount.cmp(&b.mount));
    Ok(disks)
}

fn powershell_path() -> PathBuf {
    let windir = env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    PathBuf::from(windir).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe")
}

/// A PowerShell expression evaluating to exactly `value`.
pub fn literal(value: &str) -> String {
    format!(
        "[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{}'))",
        STANDARD.encode(value)
    )
}

fn encode_command(script: &str) -> String {
    let utf16: Vec<u8> = script
        .encode_utf16()
        .flat_map(|unit| unit.to_le_bytes())
        .collect();
    STANDARD.encode(utf16)
}

fn powershell() -> Command {
    let mut command = Command::new(powershell_path());
    command.creation_flags(CREATE_NO_WINDOW);
    command.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass"]);
    command
}

/// Runs a script with the current rights and returns its standard output.
pub fn run_powershell(script: &str) -> Result<String> {
    let wrapped = format!(
        "$ErrorActionPreference='Stop';[Console]::OutputEncoding=[Text.Encoding]::UTF8;{script}"
    );
    let output = powershell()
        .args(["-EncodedCommand", &encode_command(&wrapped)])
        .output()
        .context("lancement de PowerShell")?;
    if !output.status.success() {
        bail!("{}", last_error_line(&output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn last_error_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with('<'))
        .unwrap_or("échec de la commande")
        .to_string()
}

/// Exit code of `run_elevated` when the Windows authorization prompt is declined.
pub const ELEVATION_REFUSED: i32 = 1223;

/// Runs a script after the Windows authorization prompt and returns its exit code.
/// A script that throws exits with the error's HRESULT.
pub fn run_elevated(script: &str) -> Result<i32> {
    run_hidden_with_verb(script, "runas")
}

/// The launcher behind `run_elevated`. An empty verb keeps the current rights,
/// which lets tests exercise it without a prompt.
pub fn run_hidden_with_verb(script: &str, verb: &str) -> Result<i32> {
    let inner = format!(
        "$ErrorActionPreference='Stop';try{{{script}\nexit 0}}catch{{\
         $e=$_.Exception;while($e.InnerException){{$e=$e.InnerException}};\
         $h=$e.HResult;if(-not $h){{$h=1}};exit $h}}"
    );
    let outer = format!(
        "$i=New-Object Diagnostics.ProcessStartInfo;\
         $i.FileName={};\
         $i.Arguments='-NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {}';\
         $i.Verb='{verb}';$i.UseShellExecute=$true;$i.WindowStyle='Hidden';\
         try{{$p=[Diagnostics.Process]::Start($i)}}catch{{\
         if($_.Exception.InnerException.NativeErrorCode -eq {ELEVATION_REFUSED}){{exit {ELEVATION_REFUSED}}};exit 1}};\
         $p.WaitForExit();exit $p.ExitCode",
        literal(&powershell_path().to_string_lossy()),
        encode_command(&inner),
    );
    let wrapped = format!("$ErrorActionPreference='Stop';{outer}");
    let status = powershell()
        .args(["-EncodedCommand", &encode_command(&wrapped)])
        .status()
        .context("lancement de PowerShell")?;
    status.code().context("processus interrompu")
}

/// Starts a program after the Windows authorization prompt, without waiting for it.
pub fn start_elevated(exe: &str, arguments: &str) -> Result<()> {
    let argument_list = if arguments.is_empty() {
        String::new()
    } else {
        format!(" -ArgumentList ({})", literal(arguments))
    };
    run_powershell(&format!(
        "Start-Process -FilePath ({}){argument_list} -Verb RunAs",
        literal(exe)
    ))
    .map(|_| ())
}

/// Explains a non-zero exit code from `run_elevated`.
pub fn elevated_failure(code: i32) -> String {
    if code == ELEVATION_REFUSED {
        "autorisation Windows refusée, rien n'a été modifié".to_string()
    } else {
        format!("Windows a renvoyé l'erreur 0x{code:08X}")
    }
}
