#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use rustcleaner::clean::{CleanOutcome, CleanProgress, RecycleBin, execute_with_progress};
use rustcleaner::duplicates::{self, DuplicateReport, SearchProgress};
use rustcleaner::paths::Env;
use rustcleaner::performance::{self, AppUsage};
use rustcleaner::programs::{self, Program};
use rustcleaner::rules::{self, Rule};
use rustcleaner::safety::Guard;
use rustcleaner::scan::{RuleStatus, ScanReport, scan};
use rustcleaner::startup::{self, StartupEntry};
use rustcleaner::system::{self, DiskInfo};
use rustcleaner::updates::{self, DriverUpdate, SoftwareUpdate};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;
use tauri::State;
use tauri::ipc::Channel;

const MAX_GROUPS_SHOWN: usize = 500;
const MAX_FILES_SHOWN: usize = 300;

#[derive(Default)]
struct AppState {
    scan: Mutex<Option<(ScanReport, Vec<Rule>)>>,
    duplicates: Mutex<Option<DuplicateReport>>,
}

fn text<T>(result: anyhow::Result<T>) -> Result<T, String> {
    result.map_err(|error| format!("{error:#}"))
}

#[derive(Serialize)]
struct RuleInfo {
    id: String,
    name: String,
    category: String,
    enabled_by_default: bool,
}

#[derive(Serialize)]
struct RuleView {
    id: String,
    name: String,
    status: &'static str,
    detail: Option<String>,
    bytes: u64,
    file_count: u64,
    refused: usize,
    unreadable: usize,
}

#[derive(Serialize)]
struct ScanView {
    bytes: u64,
    file_count: u64,
    rules: Vec<RuleView>,
}

#[derive(Serialize)]
struct FileView {
    path: String,
    bytes: u64,
}

#[derive(Serialize)]
struct OutcomeView {
    moved_files: u64,
    moved_bytes: u64,
    skipped: usize,
    blocked_rules: Vec<String>,
    failed: usize,
    failed_sample: Vec<String>,
}

impl From<CleanOutcome> for OutcomeView {
    fn from(outcome: CleanOutcome) -> Self {
        Self {
            moved_files: outcome.moved_files,
            moved_bytes: outcome.moved_bytes,
            skipped: outcome.skipped.len(),
            blocked_rules: outcome.blocked_rules,
            failed: outcome.failed.len(),
            failed_sample: outcome
                .failed
                .iter()
                .take(8)
                .map(|problem| problem.path.display().to_string())
                .collect(),
        }
    }
}

#[tauri::command]
fn list_rules() -> Result<Vec<RuleInfo>, String> {
    Ok(text(rules::builtin())?
        .into_iter()
        .map(|rule| RuleInfo {
            id: rule.id,
            name: rule.name,
            category: rule.category,
            enabled_by_default: rule.enabled_by_default,
        })
        .collect())
}

#[tauri::command(async)]
fn scan_cleaning(ids: Vec<String>, state: State<'_, AppState>) -> Result<ScanView, String> {
    let selected: Vec<Rule> = text(rules::builtin())?
        .into_iter()
        .filter(|rule| ids.contains(&rule.id))
        .collect();
    let report = scan(
        &selected,
        &Env::from_process(),
        &Guard::system(),
        &text(performance::running_names())?,
        SystemTime::now(),
    );
    let view = ScanView {
        bytes: report.bytes,
        file_count: report.file_count,
        rules: report
            .rules
            .iter()
            .map(|rule| {
                let (status, detail) = match &rule.status {
                    RuleStatus::Scanned => ("scanned", None),
                    RuleStatus::NotDetected => ("not_detected", None),
                    RuleStatus::BlockedBy(names) => ("blocked", Some(names.join(", "))),
                    RuleStatus::Unavailable(reason) => ("unavailable", Some(reason.clone())),
                };
                RuleView {
                    id: rule.id.clone(),
                    name: rule.name.clone(),
                    status,
                    detail,
                    bytes: rule.bytes,
                    file_count: rule.file_count,
                    refused: rule.refused.len(),
                    unreadable: rule.errors.len(),
                }
            })
            .collect(),
    };
    *state.scan.lock().map_err(|_| "état indisponible")? = Some((report, selected));
    Ok(view)
}

#[tauri::command(async)]
fn cleaning_files(rule_id: String, state: State<'_, AppState>) -> Result<Vec<FileView>, String> {
    let guard = state.scan.lock().map_err(|_| "état indisponible")?;
    let (report, _) = guard.as_ref().ok_or("aucune analyse en mémoire")?;
    let rule = report
        .rules
        .iter()
        .find(|rule| rule.id == rule_id)
        .ok_or("règle absente de l'analyse")?;
    let mut files: Vec<_> = rule.files.iter().collect();
    files.sort_by_key(|file| std::cmp::Reverse(file.bytes));
    Ok(files
        .into_iter()
        .take(MAX_FILES_SHOWN)
        .map(|file| FileView {
            path: file.path.display().to_string(),
            bytes: file.bytes,
        })
        .collect())
}

#[tauri::command(async)]
fn run_cleaning(
    on_progress: Channel<CleanProgress>,
    state: State<'_, AppState>,
) -> Result<OutcomeView, String> {
    let taken = state.scan.lock().map_err(|_| "état indisponible")?.take();
    let (report, rules) = taken.ok_or("lancez d'abord une analyse")?;
    // Checked again: a browser may have been opened since the analysis.
    let running = text(performance::running_names())?;
    let outcome = execute_with_progress(&report, &rules, &running, &RecycleBin, &|progress| {
        // The cleaning stays useful even if the window stops listening.
        let _ = on_progress.send(progress);
    });
    Ok(outcome.into())
}

#[derive(Serialize)]
struct GroupView {
    bytes_each: u64,
    paths: Vec<String>,
}

#[derive(Serialize)]
struct DuplicatesView {
    scanned_files: u64,
    wasted_bytes: u64,
    group_count: usize,
    groups: Vec<GroupView>,
    refused: Vec<String>,
    unreadable: usize,
}

fn duplicates_view(report: &DuplicateReport) -> DuplicatesView {
    DuplicatesView {
        scanned_files: report.scanned_files,
        wasted_bytes: report.wasted_bytes,
        group_count: report.groups.len(),
        groups: report
            .groups
            .iter()
            .take(MAX_GROUPS_SHOWN)
            .map(|group| GroupView {
                bytes_each: group.bytes_each,
                paths: group
                    .files
                    .iter()
                    .map(|file| file.path.display().to_string())
                    .collect(),
            })
            .collect(),
        refused: report.refused.clone(),
        unreadable: report.errors.len(),
    }
}

#[tauri::command(async)]
fn find_duplicates(
    roots: Vec<PathBuf>,
    min_bytes: u64,
    on_progress: Channel<SearchProgress>,
    state: State<'_, AppState>,
) -> Result<DuplicatesView, String> {
    let report = duplicates::find_with_progress(&roots, min_bytes, &Guard::for_search(), &|progress| {
        // The search stays useful even if the window stops listening.
        let _ = on_progress.send(progress);
    });
    let view = duplicates_view(&report);
    *state.duplicates.lock().map_err(|_| "état indisponible")? = Some(report);
    Ok(view)
}

#[derive(Serialize)]
struct RecycleView {
    outcome: OutcomeView,
    report: DuplicatesView,
}

#[tauri::command(async)]
fn recycle_duplicates(
    paths: Vec<PathBuf>,
    state: State<'_, AppState>,
) -> Result<RecycleView, String> {
    let mut guard = state.duplicates.lock().map_err(|_| "état indisponible")?;
    let report = guard.as_mut().ok_or("lancez d'abord une recherche")?;
    let outcome = duplicates::recycle_selected(report, &paths, &RecycleBin)?;
    report.prune();
    Ok(RecycleView {
        outcome: outcome.into(),
        report: duplicates_view(report),
    })
}

#[tauri::command(async)]
fn file_icons(sources: Vec<String>) -> Result<std::collections::HashMap<String, String>, String> {
    text(rustcleaner::icons::file_icons(&sources))
}

#[tauri::command(async)]
fn list_disks() -> Result<Vec<DiskInfo>, String> {
    text(system::list_disks())
}

#[tauri::command(async)]
fn list_startup() -> Result<Vec<StartupEntry>, String> {
    text(startup::list())
}

#[tauri::command(async)]
fn set_startup(id: String, enabled: bool) -> Result<(), String> {
    text(startup::set_enabled(&id, enabled))
}

#[tauri::command(async)]
fn list_programs() -> Vec<Program> {
    programs::list()
}

#[tauri::command(async)]
fn uninstall_program(id: String) -> Result<(), String> {
    text(programs::uninstall(&id))
}

#[tauri::command(async)]
fn list_apps() -> Result<Vec<AppUsage>, String> {
    text(performance::list())
}

#[tauri::command(async)]
fn set_efficiency(name: String, enabled: bool) -> Result<u32, String> {
    text(performance::set_efficiency(&name, enabled))
}

#[tauri::command(async)]
fn list_software_updates() -> Result<Vec<SoftwareUpdate>, String> {
    text(updates::list_software())
}

#[tauri::command(async)]
fn upgrade_software(id: String) -> Result<(), String> {
    text(updates::upgrade_software(&id))
}

#[tauri::command(async)]
fn list_driver_updates() -> Result<Vec<DriverUpdate>, String> {
    text(updates::list_drivers())
}

#[tauri::command(async)]
fn install_drivers(ids: Vec<String>) -> Result<bool, String> {
    text(updates::install_drivers(&ids))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            list_rules,
            scan_cleaning,
            cleaning_files,
            run_cleaning,
            find_duplicates,
            recycle_duplicates,
            file_icons,
            list_disks,
            list_startup,
            set_startup,
            list_programs,
            uninstall_program,
            list_apps,
            set_efficiency,
            list_software_updates,
            upgrade_software,
            list_driver_updates,
            install_drivers,
        ])
        .run(tauri::generate_context!())
        .expect("impossible de démarrer l'application");
}
