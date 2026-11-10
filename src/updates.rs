use crate::system::{CREATE_NO_WINDOW, elevated_failure, run_elevated, run_powershell};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use std::process::{Command, Output};

const NO_INTERACTION: [&str; 2] = ["--accept-source-agreements", "--disable-interactivity"];
const REBOOT_REQUIRED: i32 = 3010;
const WU_INSTALL_NOT_ALLOWED: i32 = 0x8024_0016_u32 as i32;
const WINGET_INSTALLER_FAILED: i32 = 0x8A15_0006_u32 as i32;
const WINGET_HASH_MISMATCH: i32 = 0x8A15_0011_u32 as i32;
const WINGET_NO_APPLICABLE_UPDATE: i32 = 0x8A15_002B_u32 as i32;
const PACKAGE_NEEDS_ADMIN: i32 = 0x8007_3D28_u32 as i32;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SoftwareUpdate {
    pub name: String,
    pub id: String,
    pub current: String,
    pub available: String,
    /// False when winget truncated the identifier or the update must be done by hand.
    pub installable: bool,
    pub manual_note: Option<&'static str>,
}

/// Packages winget offers even though their installer refuses to upgrade in place.
const MANUAL_ONLY: [(&str, &str); 1] = [(
    "Wampserver.Wampserver",
    "L'installateur refuse de remplacer une version existante. Sauvegardez www et vos bases de données, puis suivez la procédure de WampServer.",
)];

pub fn manual_note(id: &str) -> Option<&'static str> {
    MANUAL_ONLY
        .iter()
        .find(|(package, _)| package.eq_ignore_ascii_case(id))
        .map(|(_, note)| *note)
}

pub fn is_valid_package_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
}

/// Reads winget's table by column position, whatever the display language.
pub fn parse_winget_table(text: &str) -> Vec<SoftwareUpdate> {
    // Progress spinners overwrite their line with carriage returns.
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r').rsplit('\r').next().unwrap_or(""))
        .collect();
    let Some(rule) = lines.iter().position(|line| {
        let line = line.trim();
        line.len() >= 20 && line.chars().all(|c| c == '-')
    }) else {
        return Vec::new();
    };
    if rule == 0 {
        return Vec::new();
    }

    let columns = column_starts(lines[rule - 1]);
    if columns.len() < 4 {
        return Vec::new();
    }
    let mut updates = Vec::new();
    for line in &lines[rule + 1..] {
        if line.trim().is_empty() {
            break;
        }
        let characters: Vec<char> = line.chars().collect();
        if characters.len() <= columns[3] {
            break;
        }
        let cell = |index: usize| -> String {
            let start = columns[index].min(characters.len());
            let end = columns
                .get(index + 1)
                .map_or(characters.len(), |next| (*next).min(characters.len()));
            characters[start..end].iter().collect::<String>().trim().to_string()
        };
        let id = cell(1);
        let manual_note = manual_note(&id);
        updates.push(SoftwareUpdate {
            name: cell(0),
            installable: is_valid_package_id(&id) && manual_note.is_none(),
            manual_note,
            id,
            current: cell(2),
            available: cell(3),
        });
    }
    updates
}

fn column_starts(header: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut previous_space = true;
    for (index, character) in header.chars().enumerate() {
        if !character.is_whitespace() && previous_space {
            starts.push(index);
        }
        previous_space = character.is_whitespace();
    }
    starts
}

fn winget(arguments: &[&str]) -> Result<Output> {
    Command::new("winget")
        .args(arguments)
        .args(NO_INTERACTION)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .context("winget est introuvable sur ce PC")
}

pub fn list_software() -> Result<Vec<SoftwareUpdate>> {
    let output = winget(&["upgrade"])?;
    Ok(parse_winget_table(&String::from_utf8_lossy(&output.stdout)))
}

pub fn upgrade_software(id: &str) -> Result<()> {
    if !is_valid_package_id(id) {
        bail!("identifiant de logiciel invalide");
    }
    if let Some(note) = manual_note(id) {
        bail!("{note}");
    }
    let output = winget(&[
        "upgrade",
        "--id",
        id,
        "--exact",
        "--silent",
        "--accept-package-agreements",
    ])?;
    if !output.status.success() {
        let code = output.status.code();
        if code == Some(PACKAGE_NEEDS_ADMIN) {
            return upgrade_elevated(id);
        }
        if let Some(hint) = code.and_then(winget_failure_hint) {
            bail!("{hint}");
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let reason = text
            .lines()
            .map(|line| line.rsplit('\r').next().unwrap_or("").trim())
            .rfind(|line| !line.is_empty())
            .unwrap_or("échec de winget");
        bail!("{reason}");
    }
    Ok(())
}

/// `id` has already passed `is_valid_package_id`, so it cannot leave its quotes.
fn upgrade_elevated(id: &str) -> Result<()> {
    let script = format!(
        "winget upgrade --id '{id}' --exact --silent --accept-package-agreements {}|Out-Null;exit $LASTEXITCODE",
        NO_INTERACTION.join(" ")
    );
    match run_elevated(&script)? {
        0 => Ok(()),
        code => match winget_failure_hint(code) {
            Some(hint) => bail!("{hint}"),
            None => bail!("{}", elevated_failure(code)),
        },
    }
}

pub fn winget_failure_hint(code: i32) -> Option<&'static str> {
    match code {
        WINGET_INSTALLER_FAILED => Some(
            "L'installateur a refusé de continuer, souvent parce que ce logiciel ou un programme qui en dépend est encore ouvert. Fermez-les ou redémarrez le PC, puis réessayez.",
        ),
        WINGET_HASH_MISMATCH => Some(
            "Le fichier de l'éditeur ne correspond pas à celui attendu par winget, qui a refusé de l'installer par sécurité. Réessayez plus tard, ou mettez à jour depuis le logiciel.",
        ),
        WINGET_NO_APPLICABLE_UPDATE => Some(
            "winget ne trouve aucune version compatible avec cette installation. Mettez à jour depuis le site de l'éditeur.",
        ),
        _ => None,
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DriverUpdate {
    pub id: String,
    pub title: String,
    pub class: Option<String>,
    pub maker: Option<String>,
    pub size: Option<f64>,
    /// Firmware failures can leave a device unusable, so it is never preselected.
    #[serde(default)]
    pub firmware: bool,
}

const SEARCH: &str = "$session=New-Object -ComObject Microsoft.Update.Session;\
$found=$session.CreateUpdateSearcher().Search(\"IsInstalled=0 and Type='Driver'\");";

pub fn list_drivers() -> Result<Vec<DriverUpdate>> {
    let script = format!(
        "{SEARCH}\
$items=@($found.Updates | ForEach-Object {{ [pscustomobject]@{{ id=$_.Identity.UpdateID; title=$_.Title; class=$_.DriverClass; maker=$_.DriverManufacturer; size=[double]$_.MaxDownloadSize }} }});\
ConvertTo-Json -InputObject $items -Compress"
    );
    let output = run_powershell(&script).context("recherche sur Windows Update")?;
    let mut drivers: Vec<DriverUpdate> =
        serde_json::from_str(output.trim()).context("réponse illisible de Windows Update")?;
    for driver in &mut drivers {
        driver.firmware = driver
            .class
            .as_deref()
            .is_some_and(|class| class.eq_ignore_ascii_case("Firmware"));
    }
    Ok(drivers)
}

pub fn is_valid_update_id(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(index, c)| match index {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Builds the script run after the authorization prompt. A restore point comes first.
pub fn driver_install_script(ids: &[String]) -> Result<String> {
    if ids.is_empty() {
        bail!("aucun pilote sélectionné");
    }
    if let Some(id) = ids.iter().find(|id| !is_valid_update_id(id)) {
        bail!("identifiant de pilote invalide : {id}");
    }
    let wanted = ids
        .iter()
        .map(|id| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!(
        "try{{Checkpoint-Computer -Description 'rustCleaner, avant pilotes' -RestorePointType DEVICE_DRIVER_INSTALL}}catch{{}};\
$wanted=@({wanted});{SEARCH}\
$list=New-Object -ComObject Microsoft.Update.UpdateColl;\
foreach($u in $found.Updates){{if($wanted -contains $u.Identity.UpdateID){{if(-not $u.EulaAccepted){{$u.AcceptEula()}};[void]$list.Add($u)}}}};\
if($list.Count -eq 0){{exit 2}};\
$d=$session.CreateUpdateDownloader();$d.Updates=$list;$dr=$d.Download();\
if($dr.HResult -ne 0){{exit $dr.HResult}};\
$i=$session.CreateUpdateInstaller();$i.Updates=$list;$r=$i.Install();\
if($r.HResult -ne 0){{exit $r.HResult}};\
for($n=0;$n -lt $list.Count;$n++){{$h=$r.GetUpdateResult($n).HResult;if($h -ne 0){{exit $h}}}};\
if($r.ResultCode -ne 2){{exit 1}};\
if($r.RebootRequired){{exit {REBOOT_REQUIRED}}}"
    ))
}

/// Returns true when Windows asks for a restart.
pub fn install_drivers(ids: &[String]) -> Result<bool> {
    match run_elevated(&driver_install_script(ids)?)? {
        0 => Ok(false),
        REBOOT_REQUIRED => Ok(true),
        2 => bail!("ces pilotes ne sont plus proposés par Windows Update"),
        WU_INSTALL_NOT_ALLOWED => {
            bail!("une autre installation est en cours ou un redémarrage est attendu")
        }
        code => bail!("{}. Un pilote a peut-être déjà été installé : relancez la recherche pour voir ce qui reste", elevated_failure(code)),
    }
}
