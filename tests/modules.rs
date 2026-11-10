use rustcleaner::clean::{Bin, CleanProgress, recycle_with_progress};
use rustcleaner::duplicates::{SearchProgress, find, find_with_progress, recycle_selected};
use rustcleaner::icons::{file_icons, icon_file};
use rustcleaner::performance::{apply_to_pid, is_efficient, is_protected};
use rustcleaner::programs::{expand_environment, split_command};
use rustcleaner::safety::Guard;
use rustcleaner::startup::is_enabled;
use rustcleaner::system::{
    ELEVATION_REFUSED, elevated_failure, literal, run_hidden_with_verb, run_powershell,
};
use rustcleaner::updates::{
    driver_install_script, is_valid_package_id, is_valid_update_id, parse_winget_table,
    upgrade_software, winget_failure_hint,
};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;

struct FakeBin;

impl Bin for FakeBin {
    fn recycle(&self, paths: &[PathBuf]) -> Result<(), String> {
        for path in paths {
            fs::remove_file(path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

fn write(path: &Path, content: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn search(dir: &Path) -> rustcleaner::duplicates::DuplicateReport {
    find(&[dir.to_path_buf()], 1, &Guard::default())
}

#[test]
fn duplicates_need_identical_content() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("a/one.bin"), b"same content");
    write(&dir.path().join("b/two.bin"), b"same content");
    write(&dir.path().join("c/other.bin"), b"diff content");
    write(&dir.path().join("d/single.bin"), b"longer unique file");

    let report = search(dir.path());
    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].files.len(), 2);
    assert_eq!(report.wasted_bytes, 12);
}

#[test]
fn duplicates_beyond_the_partial_hash_are_compared_in_full() {
    let dir = TempDir::new().unwrap();
    let mut first = vec![7u8; 200_000];
    let copy = first.clone();
    let mut different = first.clone();
    *different.last_mut().unwrap() = 8;
    write(&dir.path().join("a"), &first);
    write(&dir.path().join("b"), &copy);
    write(&dir.path().join("c"), &different);
    first.clear();

    let report = search(dir.path());
    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].files.len(), 2);
    assert!(report.groups[0].files.iter().all(|f| !f.path.ends_with("c")));
}

#[test]
fn hard_links_are_not_duplicates() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("a"), b"shared data");
    fs::hard_link(dir.path().join("a"), dir.path().join("b")).unwrap();
    assert!(search(dir.path()).groups.is_empty());
}

#[test]
fn regenerated_folders_are_skipped() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("node_modules/x/a"), b"same");
    write(&dir.path().join(".git/b"), b"same");
    write(&dir.path().join("c"), b"same");
    assert!(search(dir.path()).groups.is_empty());
}

#[test]
fn every_copy_of_a_group_cannot_be_removed() {
    let dir = TempDir::new().unwrap();
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    write(&a, b"same");
    write(&b, b"same");
    let report = search(dir.path());

    assert!(recycle_selected(&report, &[a.clone(), b.clone()], &FakeBin).is_err());
    assert!(a.exists() && b.exists());
}

#[test]
fn selected_copy_is_removed_and_the_other_stays() {
    let dir = TempDir::new().unwrap();
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    write(&a, b"same");
    write(&b, b"same");
    let mut report = search(dir.path());

    let outcome = recycle_selected(&report, std::slice::from_ref(&a), &FakeBin).unwrap();

    assert_eq!((outcome.moved_files, outcome.moved_bytes), (1, 4));
    assert!(!a.exists() && b.exists());
    report.prune();
    assert!(report.groups.is_empty());
}

#[test]
fn copy_is_kept_when_the_other_one_changed() {
    let dir = TempDir::new().unwrap();
    let (a, b) = (dir.path().join("a"), dir.path().join("b"));
    write(&a, b"same");
    write(&b, b"same");
    let report = search(dir.path());
    fs::remove_file(&b).unwrap();

    let outcome = recycle_selected(&report, std::slice::from_ref(&a), &FakeBin).unwrap();

    assert_eq!(outcome.moved_files, 0);
    assert_eq!(outcome.skipped.len(), 1);
    assert!(a.exists());
}

#[test]
fn paths_outside_the_search_are_refused() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("a"), b"same");
    write(&dir.path().join("b"), b"same");
    let outside = dir.path().join("elsewhere");
    write(&outside, b"data");
    let report = search(dir.path());

    assert!(recycle_selected(&report, std::slice::from_ref(&outside), &FakeBin).is_err());
    assert!(outside.exists());
}

#[test]
fn startup_flag_follows_the_windows_convention() {
    assert!(is_enabled(None));
    assert!(is_enabled(Some(&[2, 0, 0])));
    assert!(is_enabled(Some(&[6, 0, 0])));
    assert!(!is_enabled(Some(&[3, 0, 0])));
    assert!(!is_enabled(Some(&[7, 0, 0])));
}

#[test]
fn uninstall_commands_are_split_correctly() {
    let split = |line: &str| split_command(line).unwrap();
    assert_eq!(
        split("\"C:\\Program Files\\A\\unins.exe\" /S"),
        ("C:\\Program Files\\A\\unins.exe".to_string(), "/S".to_string())
    );
    assert_eq!(
        split("C:\\Program Files\\A B\\unins.exe /x --y"),
        ("C:\\Program Files\\A B\\unins.exe".to_string(), "/x --y".to_string())
    );
    assert_eq!(
        split("MsiExec.exe /X{ABC}"),
        ("MsiExec.exe".to_string(), "/X{ABC}".to_string())
    );
    assert_eq!(
        split("\"C:\\a.exe\""),
        ("C:\\a.exe".to_string(), String::new())
    );
    assert!(split_command("   ").is_none());
}

#[test]
fn environment_variables_expand_and_unknown_ones_stay() {
    let windir = std::env::var("WINDIR").unwrap();
    assert_eq!(expand_environment("%WINDIR%\\x"), format!("{windir}\\x"));
    assert_eq!(expand_environment("100% %NOPE_X%"), "100% %NOPE_X%");
}

#[test]
fn hostile_names_stay_plain_text_in_powershell() {
    for value in ["'; Remove-Item x; '", "‘$(calc)’ $env:PATH `n", "é\"ç & | ;"] {
        let output = run_powershell(&format!("[Console]::Out.Write({})", literal(value))).unwrap();
        assert_eq!(output, value);
    }
}

#[test]
fn windows_internals_are_protected() {
    assert!(is_protected("LSASS.exe"));
    assert!(is_protected("rustcleaner-gui.exe"));
    assert!(!is_protected("chrome.exe"));
}

#[test]
fn efficiency_mode_toggles_on_a_real_process() {
    let mut child = Command::new("ping")
        .args(["-n", "30", "127.0.0.1"])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();

    apply_to_pid(pid, true).unwrap();
    let during = is_efficient(pid);
    apply_to_pid(pid, false).unwrap();
    let after = is_efficient(pid);

    child.kill().unwrap();
    child.wait().unwrap();
    assert!(during);
    assert!(!after);
}

const WINGET_FR: &str = "   - \r   \\ \r\
Nom                  ID                Version Disponible Source\n\
----------------------------------------------------------------\n\
Bionic 1.1.9+6       ElementLabs.Bionic 1.1.9+6 1.2.0+2   winget\n\
Tronqué              Very.Long.Package… 1.0     2.0       winget\n\
\n\
2 mises à niveau disponibles.\n";

#[test]
fn winget_table_is_read_by_column() {
    let text = "Nom            ID         Version Disponible Source\r\n\
--------------------------------------------------\r\n\
Git Tool       Git.Git    2.56.0  2.57.0     winget\r\n\
Coupé          Very.Lon…  1.0     2.0        winget\r\n\
\r\n\
2 mises à niveau disponibles.\r\n";
    let updates = parse_winget_table(text);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].name, "Git Tool");
    assert_eq!(updates[0].id, "Git.Git");
    assert_eq!(updates[0].current, "2.56.0");
    assert_eq!(updates[0].available, "2.57.0");
    assert!(updates[0].installable);
    assert!(!updates[1].installable);
}

#[test]
fn winget_spinner_before_the_table_is_ignored() {
    assert_eq!(parse_winget_table(WINGET_FR).len(), 2);
    assert!(parse_winget_table("Aucune mise à niveau trouvée.").is_empty());
}

#[test]
fn icon_sources_resolve_to_a_file() {
    assert_eq!(icon_file(r#""C:\A B\app.exe",0"#).unwrap(), r"c:\a b\app.exe");
    assert_eq!(icon_file(r"C:\A B\app.exe --flag").unwrap(), r"c:\a b\app.exe");
    assert_eq!(icon_file(r"C:\A B\app.ico,0").unwrap(), r"c:\a b\app.ico");
    assert!(icon_file(r"\\server\share\app.exe").is_none());
    assert!(icon_file("").is_none());
}

#[test]
fn icons_are_read_from_program_files() {
    let source = format!("{}\\System32\\notepad.exe", std::env::var("WINDIR").unwrap());
    let icons = file_icons(std::slice::from_ref(&source)).unwrap();
    assert!(icons[&source].starts_with("data:image/png;base64,"));
    assert!(file_icons(&[r"C:\nope\absent.exe".to_string()]).unwrap().is_empty());
}

#[test]
fn recycling_reports_progress_up_to_the_total() {
    let dir = TempDir::new().unwrap();
    write(&dir.path().join("a.bin"), b"same content");
    write(&dir.path().join("b.bin"), b"same content");
    let report = search(dir.path());
    let files: Vec<_> = report.groups[0].files.iter().collect();

    let seen = RefCell::new(Vec::new());
    let outcome = recycle_with_progress(&files, &FakeBin, &|progress| {
        seen.borrow_mut().push(progress);
    });
    assert_eq!(outcome.moved_files, 2);
    assert_eq!(seen.into_inner().last(), Some(&CleanProgress { done: 2, total: 2 }));
}

#[test]
fn known_winget_failures_are_explained() {
    assert!(winget_failure_hint(0x8A15_0011_u32 as i32).unwrap().contains("sécurité"));
    assert!(winget_failure_hint(0x8A15_002B_u32 as i32).unwrap().contains("compatible"));
    assert!(winget_failure_hint(0x8A15_0006_u32 as i32).unwrap().contains("encore ouvert"));
    assert!(winget_failure_hint(1).is_none());
}

#[test]
fn hidden_launcher_returns_the_script_exit_code() {
    assert_eq!(run_hidden_with_verb("Write-Output ok", "").unwrap(), 0);
    assert_eq!(run_hidden_with_verb("exit 42", "").unwrap(), 42);
}

#[test]
fn hidden_launcher_reports_the_error_hresult() {
    let code = run_hidden_with_verb("throw [ComponentModel.Win32Exception]::new(5)", "").unwrap();
    assert_eq!(code, 0x8000_4005_u32 as i32);
    assert!(elevated_failure(code).contains("0x80004005"));
    assert!(elevated_failure(ELEVATION_REFUSED).contains("refusée"));
}

#[test]
fn duplicate_search_reports_progress_up_to_the_total() {
    let dir = TempDir::new().unwrap();
    let content = vec![7u8; 200_000];
    fs::write(dir.path().join("a.bin"), &content).unwrap();
    fs::write(dir.path().join("b.bin"), &content).unwrap();
    fs::write(dir.path().join("unique.bin"), vec![1u8; 5_000]).unwrap();

    let seen = RefCell::new(Vec::new());
    let report = find_with_progress(&[dir.path().to_path_buf()], 1, &Guard::default(), &|progress| {
        seen.borrow_mut().push(progress);
    });
    assert_eq!(report.groups.len(), 1);

    let seen = seen.into_inner();
    let expected = 2 * (64 * 1024 + 200_000);
    assert_eq!(
        seen.last(),
        Some(&SearchProgress::Comparing { done_bytes: expected, total_bytes: expected })
    );
    let done: Vec<u64> = seen
        .iter()
        .filter_map(|progress| match progress {
            SearchProgress::Comparing { done_bytes, .. } => Some(*done_bytes),
            SearchProgress::Listing { .. } => None,
        })
        .collect();
    assert!(done.is_sorted());
}

#[test]
fn wampserver_is_marked_as_a_manual_update() {
    let text = "Nom        ID                     Version Disponible Source\r\n\
----------------------------------------------------------\r\n\
Wampserver Wampserver.Wampserver  3.3.7   3.4.0      winget\r\n\
Git Tool   Git.Git                2.56.0  2.57.0     winget\r\n";
    let updates = parse_winget_table(text);
    assert_eq!(updates[0].id, "Wampserver.Wampserver");
    assert!(!updates[0].installable);
    assert!(updates[0].manual_note.is_some());
    assert!(updates[1].installable);
    assert!(updates[1].manual_note.is_none());
}

#[test]
fn manual_packages_are_refused_before_winget_runs() {
    let error = upgrade_software("wampserver.wampserver").unwrap_err();
    assert!(error.to_string().contains("WampServer"));
}

#[test]
fn package_ids_cannot_carry_options() {
    assert!(is_valid_package_id("Git.Git"));
    assert!(is_valid_package_id("9NBLGGH4NNS1"));
    assert!(!is_valid_package_id("--force"));
    assert!(!is_valid_package_id("a b"));
    assert!(!is_valid_package_id("Very.Lon…"));
}

#[test]
fn driver_script_only_accepts_update_ids() {
    let good = "e216d3e4-2ab6-43c7-8078-c047b2c62fb7".to_string();
    assert!(is_valid_update_id(&good));
    assert!(!is_valid_update_id("'; calc; '"));
    assert!(driver_install_script(&[]).is_err());
    assert!(driver_install_script(&["'; calc; '".to_string()]).is_err());
    let script = driver_install_script(std::slice::from_ref(&good)).unwrap();
    assert!(script.contains(&good));
    assert!(script.starts_with("try{Checkpoint-Computer"));
}
