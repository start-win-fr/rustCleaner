//! Reads the icons of installed programs. Files are only read, never launched.

use crate::programs::expand_environment;
use crate::system::{literal, run_powershell};
use anyhow::{Context, Result};
use std::collections::{BTreeSet, HashMap};

/// The file named by a registry icon value, a command line or a plain path.
pub fn icon_file(source: &str) -> Option<String> {
    let expanded = expand_environment(source.trim());
    let lower = expanded.to_ascii_lowercase();
    let file = if let Some(rest) = expanded.strip_prefix('"') {
        &rest[..rest.find('"')?]
    } else if let Some(position) = lower.find(".exe") {
        &expanded[..position + 4]
    } else {
        // Registry icon values end with the icon's index: `app.ico,0`.
        match expanded.rsplit_once(',') {
            Some((path, index)) if index.trim().parse::<i32>().is_ok() => path,
            _ => expanded.as_str(),
        }
    };
    let file = file.trim();
    // Reading a network path could wait on an unreachable server.
    (!file.is_empty() && !file.starts_with("\\\\")).then(|| file.to_lowercase())
}

/// Returns a PNG data URL for every source whose file has an icon.
pub fn file_icons(sources: &[String]) -> Result<HashMap<String, String>> {
    let files: BTreeSet<String> = sources.iter().filter_map(|source| icon_file(source)).collect();
    if files.is_empty() {
        return Ok(HashMap::new());
    }
    let list = serde_json::to_string(&files).context("préparation de la liste d'icônes")?;
    let script = format!(
        "Add-Type -AssemblyName System.Drawing;\
         $list={} | ConvertFrom-Json;\
         $out=@{{}};\
         foreach($f in $list){{try{{\
         if(-not(Test-Path -LiteralPath $f -PathType Leaf)){{continue}};\
         $i=[Drawing.Icon]::ExtractAssociatedIcon($f);if(-not $i){{continue}};\
         $b=$i.ToBitmap();$m=New-Object IO.MemoryStream;\
         $b.Save($m,[Drawing.Imaging.ImageFormat]::Png);\
         $out[$f]=[Convert]::ToBase64String($m.ToArray());\
         $m.Dispose();$b.Dispose();$i.Dispose()\
         }}catch{{}}}};\
         ConvertTo-Json -InputObject $out -Compress",
        literal(&list)
    );
    let output = run_powershell(&script).context("lecture des icônes")?;
    let found: HashMap<String, String> =
        serde_json::from_str(output.trim()).context("réponse illisible pour les icônes")?;
    Ok(sources
        .iter()
        .filter_map(|source| {
            let image = found.get(&icon_file(source)?)?;
            Some((source.clone(), format!("data:image/png;base64,{image}")))
        })
        .collect())
}
