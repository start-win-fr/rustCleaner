use rustcleaner::clean::{Bin, execute};
use rustcleaner::paths::{Env, ExpandError, validate_template};
use rustcleaner::rules::{self, Rule, Target};
use rustcleaner::safety::Guard;
use rustcleaner::scan::{RuleStatus, ScanReport, format_size, scan};
use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

fn env_for(dir: &Path) -> Env {
    Env::from_pairs([("TEMP".to_string(), dir.display().to_string())])
}

fn target(path: &str, pattern: &str, recursive: bool, min_age_hours: Option<u64>) -> Target {
    Target {
        path: path.to_string(),
        pattern: pattern.to_string(),
        recursive,
        min_age_hours,
    }
}

fn rule(id: &str, targets: Vec<Target>) -> Rule {
    Rule {
        id: id.to_string(),
        name: id.to_string(),
        category: "test".to_string(),
        enabled_by_default: true,
        blocked_by_processes: Vec::new(),
        detect: Vec::new(),
        targets,
    }
}

fn run(rules: &[Rule], dir: &Path) -> ScanReport {
    scan(
        rules,
        &env_for(dir),
        &Guard::default(),
        &HashSet::new(),
        SystemTime::now(),
    )
}

fn write(path: &Path, size: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![0u8; size]).unwrap();
}

fn age(path: &Path, hours: u64) {
    let file = File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(hours * 3600))
        .unwrap();
}

#[test]
fn builtin_rules_are_valid() {
    assert!(!rules::builtin().unwrap().is_empty());
}

#[test]
fn templates_reject_unsafe_paths() {
    assert!(validate_template("{TEMP}/cache").is_ok());
    assert!(validate_template("C:/Windows").is_err());
    assert!(validate_template("{TEMP}/../secret").is_err());
    assert!(validate_template("{NOPE}/cache").is_err());
    assert!(validate_template("{TEMP/cache").is_err());
}

#[test]
fn parse_rejects_duplicate_ids_and_unknown_fields() {
    let one = "id=\"a\"\nname=\"a\"\ncategory=\"c\"\n[[rule.target]]\npath=\"{TEMP}\"\n";
    let duplicated = format!("[[rule]]\n{one}[[rule]]\n{one}");
    assert!(rules::parse(&duplicated).is_err());
    assert!(rules::parse(&format!("[[rule]]\nextra=1\n{one}")).is_err());
    assert!(rules::parse(&format!("[[rule]]\n{one}")).is_ok());
}

#[test]
fn variable_values_are_not_read_as_globs() {
    let env = Env::from_pairs([("TEMP".to_string(), "C:/a[1]*".to_string())]);
    assert_eq!(env.expand("{TEMP}/x").unwrap(), "C:/a[[]1[]][*]/x");
    assert_eq!(
        Env::default().expand("{TEMP}"),
        Err(ExpandError::Unset("TEMP".to_string()))
    );
}

#[test]
fn counts_files_and_respects_recursion_and_mask() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("a.log"), 10);
    write(&dir.path().join("b.txt"), 20);
    write(&dir.path().join("sub/c.log"), 30);

    let flat = run(&[rule("r", vec![target("{TEMP}", "*.log", false, None)])], dir.path());
    assert_eq!((flat.file_count, flat.bytes), (1, 10));

    let deep = run(&[rule("r", vec![target("{TEMP}", "*.LOG", true, None)])], dir.path());
    assert_eq!((deep.file_count, deep.bytes), (2, 40));
}

#[test]
fn recent_files_are_kept() {
    let dir = TempDir::new().unwrap();
    let old = dir.path().join("old.tmp");
    write(&old, 5);
    age(&old, 48);
    write(&dir.path().join("new.tmp"), 7);

    let report = run(&[rule("r", vec![target("{TEMP}", "*", true, Some(24))])], dir.path());
    assert_eq!((report.file_count, report.bytes), (1, 5));
}

#[test]
fn overlapping_rules_count_a_file_once() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("a.tmp"), 9);
    let rules = [
        rule("one", vec![target("{TEMP}", "*", true, None)]),
        rule("two", vec![target("{TEMP}", "*.tmp", true, None)]),
    ];
    let report = run(&rules, dir.path());
    assert_eq!((report.file_count, report.bytes), (1, 9));
}

#[test]
fn wildcards_match_one_level_of_folders() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("p1/cache/a"), 1);
    write(&dir.path().join("p2/cache/b"), 2);
    write(&dir.path().join("p3/other/c"), 4);

    let report = run(&[rule("r", vec![target("{TEMP}/*/cache", "*", true, None)])], dir.path());
    assert_eq!((report.file_count, report.bytes), (2, 3));
}

#[test]
fn scanning_never_changes_the_files() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("sub/a.tmp");
    write(&file, 11);
    let before = fs::metadata(&file).unwrap().modified().unwrap();

    run(&[rule("r", vec![target("{TEMP}", "*", true, None)])], dir.path());

    let metadata = fs::metadata(&file).unwrap();
    assert_eq!(metadata.len(), 11);
    assert_eq!(metadata.modified().unwrap(), before);
}

#[test]
fn guard_refuses_protected_folders_and_their_parents() {
    let dir = TempDir::new().unwrap();
    let protected = dir.path().join("home");
    let personal = protected.join("Documents");
    let allowed = protected.join("cache");
    fs::create_dir_all(personal.join("deep")).unwrap();
    fs::create_dir_all(&allowed).unwrap();
    let guard = Guard::new(vec![protected.clone()], vec![personal.clone()]);
    let resolved = |path: &Path| path.canonicalize().unwrap();

    assert!(guard.check(&resolved(&protected)).is_err());
    assert!(guard.check(&resolved(dir.path())).is_err());
    assert!(guard.check(&resolved(&personal)).is_err());
    assert!(guard.check(&resolved(&personal.join("deep"))).is_err());
    assert!(guard.check(&resolved(&allowed)).is_ok());
    assert!(guard.check(Path::new(if cfg!(windows) { "C:\\" } else { "/" })).is_err());
}

#[test]
fn guarded_root_is_reported_and_not_scanned() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("a"), 1);
    let guard = Guard::new(vec![dir.path().to_path_buf()], Vec::new());
    let report = scan(
        &[rule("r", vec![target("{TEMP}", "*", true, None)])],
        &env_for(dir.path()),
        &guard,
        &HashSet::new(),
        SystemTime::now(),
    );
    assert_eq!(report.file_count, 0);
    assert_eq!(report.rules[0].refused.len(), 1);
}

#[test]
fn running_process_blocks_the_rule() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("a"), 1);
    let mut blocked = rule("r", vec![target("{TEMP}", "*", true, None)]);
    blocked.blocked_by_processes = vec!["Chrome.exe".to_string()];
    let running = HashSet::from(["chrome.exe".to_string()]);

    let report = scan(
        &[blocked],
        &env_for(dir.path()),
        &Guard::default(),
        &running,
        SystemTime::now(),
    );
    assert_eq!(report.file_count, 0);
    assert_eq!(
        report.rules[0].status,
        RuleStatus::BlockedBy(vec!["Chrome.exe".to_string()])
    );
}

#[test]
fn missing_detection_path_skips_the_rule() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("a"), 1);
    let mut absent = rule("r", vec![target("{TEMP}", "*", true, None)]);
    absent.detect = vec!["{TEMP}/absent".to_string()];

    let report = run(&[absent], dir.path());
    assert_eq!(report.rules[0].status, RuleStatus::NotDetected);
    assert_eq!(report.file_count, 0);
}

#[cfg(windows)]
#[test]
fn junctions_are_not_followed() {
    use std::process::Command;

    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    write(&outside.path().join("private.txt"), 100);
    write(&dir.path().join("own.tmp"), 1);
    let link = dir.path().join("link");
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(outside.path())
        .output()
        .unwrap()
        .status;
    assert!(status.success());

    let report = run(&[rule("r", vec![target("{TEMP}", "*", true, None)])], dir.path());
    assert_eq!((report.file_count, report.bytes), (1, 1));
}

/// Removes files instead of recycling them, and refuses any file named `locked`.
struct FakeBin;

impl Bin for FakeBin {
    fn recycle(&self, paths: &[PathBuf]) -> Result<(), String> {
        if paths.iter().any(|path| path.ends_with("locked")) {
            return Err("refusé".to_string());
        }
        for path in paths {
            fs::remove_file(path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

fn all_files_rule() -> Rule {
    rule("r", vec![target("{TEMP}", "*", true, None)])
}

fn no_process() -> HashSet<String> {
    HashSet::new()
}

#[test]
fn clean_moves_scanned_files() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("a"), 3);
    write(&dir.path().join("sub/b"), 4);
    let rules = [all_files_rule()];
    let report = run(&rules, dir.path());

    let outcome = execute(&report, &rules, &no_process(), &FakeBin);

    assert_eq!((outcome.moved_files, outcome.moved_bytes), (2, 7));
    assert!(!dir.path().join("a").exists());
    assert!(!dir.path().join("sub/b").exists());
}

#[test]
fn file_modified_after_scan_is_left_in_place() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("a");
    write(&file, 3);
    let rules = [all_files_rule()];
    let report = run(&rules, dir.path());
    fs::write(&file, b"changed content").unwrap();

    let outcome = execute(&report, &rules, &no_process(), &FakeBin);

    assert_eq!(outcome.moved_files, 0);
    assert_eq!(outcome.skipped.len(), 1);
    assert!(file.exists());
}

#[test]
fn program_started_after_scan_blocks_the_rule() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("a");
    write(&file, 3);
    let mut browser = all_files_rule();
    browser.blocked_by_processes = vec!["Chrome.exe".to_string()];
    let rules = [browser];
    let report = run(&rules, dir.path());
    let running = HashSet::from(["chrome.exe".to_string()]);

    let outcome = execute(&report, &rules, &running, &FakeBin);

    assert_eq!(outcome.moved_files, 0);
    assert_eq!(outcome.blocked_rules, vec!["r".to_string()]);
    assert!(file.exists());
}

#[test]
fn one_refused_file_does_not_stop_the_others() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("locked"), 1);
    write(&dir.path().join("a"), 2);
    write(&dir.path().join("b"), 4);
    let rules = [all_files_rule()];
    let report = run(&rules, dir.path());

    let outcome = execute(&report, &rules, &no_process(), &FakeBin);

    assert_eq!((outcome.moved_files, outcome.moved_bytes), (2, 6));
    assert_eq!(outcome.failed.len(), 1);
    assert!(outcome.failed[0].path.ends_with("locked"));
    assert!(dir.path().join("locked").exists());
}

#[cfg(windows)]
#[test]
fn folder_swapped_for_junction_after_scan_is_not_touched() {
    use std::process::Command;

    let dir = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let file = dir.path().join("sub/a");
    write(&file, 5);
    let rules = [all_files_rule()];
    let report = run(&rules, dir.path());

    // Same name, size and date on the far side of the junction.
    let modified = fs::metadata(&file).unwrap().modified().unwrap();
    let private = outside.path().join("a");
    write(&private, 5);
    File::options()
        .write(true)
        .open(&private)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    fs::rename(dir.path().join("sub"), dir.path().join("moved")).unwrap();
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(dir.path().join("sub"))
        .arg(outside.path())
        .output()
        .unwrap()
        .status;
    assert!(status.success());

    let outcome = execute(&report, &rules, &no_process(), &FakeBin);

    assert_eq!(outcome.moved_files, 0);
    assert_eq!(outcome.skipped.len(), 1);
    assert!(private.exists());
}

#[test]
fn sizes_are_human_readable() {
    assert_eq!(format_size(512), "512 o");
    assert_eq!(format_size(1536), "1.5 Ko");
    assert_eq!(format_size(1024 * 1024), "1.0 Mo");
}
