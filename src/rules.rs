use crate::paths::validate_template;
use anyhow::{Context, Result, bail};
use glob::Pattern;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

const BUILTIN: &str = include_str!("../rules/default.toml");

fn yes() -> bool {
    true
}

fn any_file() -> String {
    "*".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub category: String,
    #[serde(default = "yes")]
    pub enabled_by_default: bool,
    /// The rule is skipped while one of these executables is running.
    #[serde(default)]
    pub blocked_by_processes: Vec<String>,
    /// The rule applies only if one of these paths exists. Empty means always.
    #[serde(default)]
    pub detect: Vec<String>,
    #[serde(rename = "target")]
    pub targets: Vec<Target>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// Folder template. `*` may match one path level.
    pub path: String,
    /// File name mask.
    #[serde(default = "any_file")]
    pub pattern: String,
    #[serde(default)]
    pub recursive: bool,
    /// Files modified more recently than this are left alone.
    pub min_age_hours: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    #[serde(rename = "rule")]
    rules: Vec<Rule>,
}

pub fn parse(text: &str) -> Result<Vec<Rule>> {
    let file: RuleFile = toml::from_str(text)?;
    validate(&file.rules)?;
    Ok(file.rules)
}

pub fn builtin() -> Result<Vec<Rule>> {
    parse(BUILTIN).context("règles intégrées invalides")
}

/// Loads every `.toml` file of a folder, in name order.
pub fn load_dir(dir: &Path) -> Result<Vec<Rule>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("lecture de {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "toml") {
            files.push(path);
        }
    }
    files.sort();

    let mut rules = Vec::new();
    for path in files {
        let text =
            fs::read_to_string(&path).with_context(|| format!("lecture de {}", path.display()))?;
        let file: RuleFile =
            toml::from_str(&text).with_context(|| format!("analyse de {}", path.display()))?;
        rules.extend(file.rules);
    }
    validate(&rules)?;
    Ok(rules)
}

fn validate(rules: &[Rule]) -> Result<()> {
    let mut ids = HashSet::new();
    for rule in rules {
        if !ids.insert(rule.id.as_str()) {
            bail!("identifiant de règle en double : {}", rule.id);
        }
        if rule.targets.is_empty() {
            bail!("règle {} : aucune cible", rule.id);
        }
        for template in &rule.detect {
            validate_template(template)
                .map_err(|error| anyhow::anyhow!("règle {} : {template} : {error}", rule.id))?;
        }
        for target in &rule.targets {
            validate_template(&target.path).map_err(|error| {
                anyhow::anyhow!("règle {} : {} : {error}", rule.id, target.path)
            })?;
            if target.pattern.contains(['/', '\\']) {
                bail!(
                    "règle {} : le masque {} ne doit pas contenir de dossier",
                    rule.id,
                    target.pattern
                );
            }
            Pattern::new(&target.pattern)
                .with_context(|| format!("règle {} : masque {}", rule.id, target.pattern))?;
        }
    }
    Ok(())
}
