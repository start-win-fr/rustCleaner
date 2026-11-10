use crate::paths::{Env, ExpandError};
use crate::rules::{Rule, Target};
use crate::safety::Guard;
use glob::{MatchOptions, Pattern};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

const MATCH_OPTIONS: MatchOptions = MatchOptions {
    case_sensitive: false,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
pub enum RuleStatus {
    Scanned,
    NotDetected,
    BlockedBy(Vec<String>),
    Unavailable(String),
}

#[derive(Debug, Serialize)]
pub struct Candidate {
    pub path: PathBuf,
    pub bytes: u64,
    /// Canonical folder the file was found under.
    #[serde(skip)]
    pub root: PathBuf,
    #[serde(skip)]
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Serialize)]
pub struct RuleReport {
    pub id: String,
    pub name: String,
    pub category: String,
    pub status: RuleStatus,
    pub file_count: u64,
    pub bytes: u64,
    pub files: Vec<Candidate>,
    /// Folders the guard refused to enter.
    pub refused: Vec<String>,
    /// Paths that could not be read.
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ScanReport {
    pub simulation: bool,
    pub file_count: u64,
    pub bytes: u64,
    pub rules: Vec<RuleReport>,
}

/// `running` holds lowercase executable names.
pub fn scan(
    rules: &[Rule],
    env: &Env,
    guard: &Guard,
    running: &HashSet<String>,
    now: SystemTime,
) -> ScanReport {
    let mut seen = HashSet::new();
    let reports: Vec<RuleReport> = rules
        .iter()
        .map(|rule| scan_rule(rule, env, guard, running, now, &mut seen))
        .collect();
    ScanReport {
        simulation: true,
        file_count: reports.iter().map(|report| report.file_count).sum(),
        bytes: reports.iter().map(|report| report.bytes).sum(),
        rules: reports,
    }
}

fn scan_rule(
    rule: &Rule,
    env: &Env,
    guard: &Guard,
    running: &HashSet<String>,
    now: SystemTime,
    seen: &mut HashSet<PathBuf>,
) -> RuleReport {
    let mut report = RuleReport {
        id: rule.id.clone(),
        name: rule.name.clone(),
        category: rule.category.clone(),
        status: RuleStatus::Scanned,
        file_count: 0,
        bytes: 0,
        files: Vec::new(),
        refused: Vec::new(),
        errors: Vec::new(),
    };

    let blockers: Vec<String> = rule
        .blocked_by_processes
        .iter()
        .filter(|name| running.contains(&name.to_lowercase()))
        .cloned()
        .collect();
    if !blockers.is_empty() {
        report.status = RuleStatus::BlockedBy(blockers);
        return report;
    }

    match is_detected(rule, env) {
        Ok(true) => {}
        Ok(false) => {
            report.status = RuleStatus::NotDetected;
            return report;
        }
        Err(error) => {
            report.status = RuleStatus::Unavailable(error.to_string());
            return report;
        }
    }

    for target in &rule.targets {
        match env.expand(&target.path) {
            Ok(expanded) => scan_target(target, &expanded, guard, now, seen, &mut report),
            Err(error) => report.status = RuleStatus::Unavailable(error.to_string()),
        }
    }
    report
}

fn is_detected(rule: &Rule, env: &Env) -> Result<bool, ExpandError> {
    if rule.detect.is_empty() {
        return Ok(true);
    }
    for template in &rule.detect {
        let expanded = env.expand(template)?;
        let found = glob::glob_with(&expanded, MATCH_OPTIONS)
            .map(|mut paths| paths.any(|path| path.is_ok()))
            .unwrap_or(false);
        if found {
            return Ok(true);
        }
    }
    Ok(false)
}

fn scan_target(
    target: &Target,
    expanded: &str,
    guard: &Guard,
    now: SystemTime,
    seen: &mut HashSet<PathBuf>,
    report: &mut RuleReport,
) {
    let roots = match glob::glob_with(expanded, MATCH_OPTIONS) {
        Ok(roots) => roots,
        Err(error) => {
            report.errors.push(format!("{expanded} : {error}"));
            return;
        }
    };
    let Ok(mask) = Pattern::new(&target.pattern) else {
        report.errors.push(format!("masque {}", target.pattern));
        return;
    };
    let min_age = target
        .min_age_hours
        .map(|hours| Duration::from_secs(hours.saturating_mul(3600)));

    for root in roots {
        match root {
            Ok(root) => scan_root(&root, target, &mask, min_age, guard, now, seen, report),
            Err(error) => report.errors.push(error.to_string()),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn scan_root(
    root: &Path,
    target: &Target,
    mask: &Pattern,
    min_age: Option<Duration>,
    guard: &Guard,
    now: SystemTime,
    seen: &mut HashSet<PathBuf>,
    report: &mut RuleReport,
) {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) => {
            report.errors.push(format!("{} : {error}", root.display()));
            return;
        }
    };
    if metadata.file_type().is_symlink() {
        report
            .refused
            .push(format!("{} : lien symbolique", root.display()));
        return;
    }
    if !metadata.is_dir() {
        return;
    }
    let canonical = match root.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            report.errors.push(format!("{} : {error}", root.display()));
            return;
        }
    };
    if let Err(reason) = guard.check(&canonical) {
        report.refused.push(format!("{} : {reason}", root.display()));
        return;
    }

    let mut walker = WalkDir::new(root).follow_links(false).min_depth(1);
    if !target.recursive {
        walker = walker.max_depth(1);
    }
    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report.errors.push(error.to_string());
                continue;
            }
        };
        // Links and junctions are neither followed nor counted.
        if !entry.file_type().is_file() {
            continue;
        }
        if !mask.matches_with(&entry.file_name().to_string_lossy(), MATCH_OPTIONS) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                report.errors.push(error.to_string());
                continue;
            }
        };
        if let Some(min_age) = min_age {
            let old_enough = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= min_age);
            if !old_enough {
                continue;
            }
        }
        if !seen.insert(entry.path().to_path_buf()) {
            continue;
        }
        report.file_count += 1;
        report.bytes += metadata.len();
        report.files.push(Candidate {
            path: entry.into_path(),
            bytes: metadata.len(),
            root: canonical.clone(),
            modified: metadata.modified().ok(),
        });
    }
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Ko", "Mo", "Go", "To"];
    if bytes < 1024 {
        return format!("{bytes} o");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}
