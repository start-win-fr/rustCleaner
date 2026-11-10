use crate::rules::Rule;
use crate::scan::{Candidate, RuleStatus, ScanReport};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

const BATCH_SIZE: usize = 200;

/// Destination of removed files. Abstracted so tests never touch the real bin.
pub trait Bin {
    fn recycle(&self, paths: &[PathBuf]) -> Result<(), String>;
}

pub struct RecycleBin;

impl Bin for RecycleBin {
    fn recycle(&self, paths: &[PathBuf]) -> Result<(), String> {
        trash::delete_all(paths).map_err(|error| error.to_string())
    }
}

#[derive(Debug, Serialize)]
pub struct Problem {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Default, Serialize)]
pub struct CleanOutcome {
    pub moved_files: u64,
    pub moved_bytes: u64,
    /// Left in place on purpose: the file changed since the scan.
    pub skipped: Vec<Problem>,
    /// Rules not applied because a blocking program started after the scan.
    pub blocked_rules: Vec<String>,
    /// Still in place although moving it was attempted.
    pub failed: Vec<Problem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CleanProgress {
    pub done: u64,
    pub total: u64,
}

/// Moves the scanned files to `bin`, except for rules blocked by a running program.
pub fn execute(
    report: &ScanReport,
    rules: &[Rule],
    running: &HashSet<String>,
    bin: &dyn Bin,
) -> CleanOutcome {
    execute_with_progress(report, rules, running, bin, &|_| {})
}

pub fn execute_with_progress(
    report: &ScanReport,
    rules: &[Rule],
    running: &HashSet<String>,
    bin: &dyn Bin,
    on_progress: &dyn Fn(CleanProgress),
) -> CleanOutcome {
    let mut blocked_rules = Vec::new();
    let mut candidates = Vec::new();
    for rule_report in &report.rules {
        if rule_report.status != RuleStatus::Scanned {
            continue;
        }
        let blocked = rules
            .iter()
            .find(|rule| rule.id == rule_report.id)
            .is_some_and(|rule| {
                rule.blocked_by_processes
                    .iter()
                    .any(|name| running.contains(&name.to_lowercase()))
            });
        if blocked {
            blocked_rules.push(rule_report.id.clone());
        } else {
            candidates.extend(rule_report.files.iter());
        }
    }
    let mut outcome = recycle_with_progress(&candidates, bin, on_progress);
    outcome.blocked_rules = blocked_rules;
    outcome
}

/// Moves files to `bin`. Only files verified unchanged since the scan are moved.
pub fn recycle(candidates: &[&Candidate], bin: &dyn Bin) -> CleanOutcome {
    recycle_with_progress(candidates, bin, &|_| {})
}

pub fn recycle_with_progress(
    candidates: &[&Candidate],
    bin: &dyn Bin,
    on_progress: &dyn Fn(CleanProgress),
) -> CleanOutcome {
    let total = candidates.len() as u64;
    let mut outcome = CleanOutcome::default();
    let mut batch: Vec<&Candidate> = Vec::with_capacity(BATCH_SIZE);
    for (index, candidate) in candidates.iter().enumerate() {
        if let Err(reason) = verify_unchanged(candidate) {
            outcome.skipped.push(Problem {
                path: candidate.path.clone(),
                reason,
            });
            continue;
        }
        batch.push(candidate);
        if batch.len() == BATCH_SIZE {
            move_batch(&batch, bin, &mut outcome);
            batch.clear();
            on_progress(CleanProgress {
                done: index as u64 + 1,
                total,
            });
        }
    }
    if !batch.is_empty() {
        move_batch(&batch, bin, &mut outcome);
    }
    on_progress(CleanProgress { done: total, total });
    outcome
}

pub fn verify_unchanged(candidate: &Candidate) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(&candidate.path).map_err(|_| "disparu depuis le scan".to_string())?;
    if !metadata.file_type().is_file() {
        return Err("n'est plus un fichier ordinaire".to_string());
    }
    if metadata.len() != candidate.bytes || metadata.modified().ok() != candidate.modified {
        return Err("modifié depuis le scan".to_string());
    }
    // A parent folder swapped for a junction would resolve outside the scanned root.
    let resolved = candidate
        .path
        .canonicalize()
        .map_err(|_| "chemin illisible".to_string())?;
    if !resolved.starts_with(&candidate.root) {
        return Err("sort du dossier analysé".to_string());
    }
    Ok(())
}

fn move_batch(batch: &[&Candidate], bin: &dyn Bin, outcome: &mut CleanOutcome) {
    let paths: Vec<PathBuf> = batch.iter().map(|c| c.path.clone()).collect();
    // One refused file can stop a whole batch, so remaining files are retried alone.
    if bin.recycle(&paths).is_err() {
        for candidate in batch.iter().filter(|c| still_present(c)) {
            let _ = bin.recycle(std::slice::from_ref(&candidate.path));
        }
    }
    for candidate in batch {
        if still_present(candidate) {
            outcome.failed.push(Problem {
                path: candidate.path.clone(),
                reason: "encore présent après la tentative".to_string(),
            });
        } else {
            outcome.moved_files += 1;
            outcome.moved_bytes += candidate.bytes;
        }
    }
}

fn still_present(candidate: &Candidate) -> bool {
    fs::symlink_metadata(&candidate.path).is_ok()
}
