use crate::clean::{Bin, CleanOutcome, Problem, recycle, verify_unchanged};
use crate::safety::Guard;
use crate::scan::Candidate;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::hash::Hash;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use walkdir::WalkDir;

/// Folders whose content is regenerated or would break if files disappeared.
const SKIPPED_FOLDERS: [&str; 5] = [
    ".git",
    "node_modules",
    "AppData",
    "$RECYCLE.BIN",
    "System Volume Information",
];
const PARTIAL_BYTES: u64 = 64 * 1024;
const READ_CHUNK: usize = 256 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// The number of files is unknown while listing, so only comparing has a percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum SearchProgress {
    Listing { files: u64 },
    Comparing { done_bytes: u64, total_bytes: u64 },
}

struct Tracker<'a> {
    on_progress: &'a dyn Fn(SearchProgress),
    last_sent: Instant,
    files: u64,
    done_bytes: u64,
    total_bytes: u64,
}

impl<'a> Tracker<'a> {
    fn new(on_progress: &'a dyn Fn(SearchProgress)) -> Self {
        Self {
            on_progress,
            last_sent: Instant::now(),
            files: 0,
            done_bytes: 0,
            total_bytes: 0,
        }
    }

    fn due(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last_sent) < PROGRESS_INTERVAL {
            return false;
        }
        self.last_sent = now;
        true
    }

    fn listed(&mut self) {
        self.files += 1;
        if self.due() {
            (self.on_progress)(SearchProgress::Listing { files: self.files });
        }
    }

    fn send_comparing(&self) {
        (self.on_progress)(SearchProgress::Comparing {
            done_bytes: self.done_bytes,
            total_bytes: self.total_bytes,
        });
    }

    fn start_comparing(&mut self, total_bytes: u64) {
        self.total_bytes = total_bytes;
        self.send_comparing();
    }

    fn advance(&mut self, bytes: u64) {
        self.done_bytes = (self.done_bytes + bytes).min(self.total_bytes);
        if self.due() {
            self.send_comparing();
        }
    }

    fn finish(&mut self) {
        self.done_bytes = self.total_bytes;
        self.send_comparing();
    }
}

/// Bytes read for one file of this size if it survives every comparison.
fn comparison_bytes(size: u64) -> u64 {
    if size <= PARTIAL_BYTES {
        size
    } else {
        PARTIAL_BYTES + size
    }
}

#[derive(Debug, Serialize)]
pub struct DuplicateGroup {
    pub bytes_each: u64,
    pub files: Vec<Candidate>,
}

#[derive(Debug, Default, Serialize)]
pub struct DuplicateReport {
    pub scanned_files: u64,
    pub wasted_bytes: u64,
    pub groups: Vec<DuplicateGroup>,
    pub refused: Vec<String>,
    pub errors: Vec<String>,
}

pub fn find(roots: &[PathBuf], min_bytes: u64, guard: &Guard) -> DuplicateReport {
    find_with_progress(roots, min_bytes, guard, &|_| {})
}

pub fn find_with_progress(
    roots: &[PathBuf],
    min_bytes: u64,
    guard: &Guard,
    on_progress: &dyn Fn(SearchProgress),
) -> DuplicateReport {
    let mut report = DuplicateReport::default();
    let min_bytes = min_bytes.max(1);
    let mut seen = HashSet::new();
    let mut by_size: HashMap<u64, Vec<Candidate>> = HashMap::new();
    let mut tracker = Tracker::new(on_progress);

    for root in roots {
        collect(root, min_bytes, guard, &mut seen, &mut by_size, &mut report, &mut tracker);
    }

    by_size.retain(|_, files| files.len() >= 2);
    tracker.start_comparing(
        by_size
            .iter()
            .map(|(size, files)| files.len() as u64 * comparison_bytes(*size))
            .sum(),
    );

    let mut confirmed = Vec::new();
    for (size, files) in by_size {
        let count = files.len() as u64;
        let groups = split_by(
            files,
            |file| {
                let hash = partial_hash(&file.path);
                tracker.advance(file.bytes.min(PARTIAL_BYTES));
                hash
            },
            &mut report.errors,
        );
        if size <= PARTIAL_BYTES {
            confirmed.extend(groups.into_iter().map(|group| (size, group)));
            continue;
        }
        // Files ruled out here will never be read in full.
        let kept: u64 = groups.iter().map(|group| group.len() as u64).sum();
        tracker.advance((count - kept) * size);
        for partial in groups {
            let matches = split_by(
                partial,
                |file| full_hash(&file.path, &mut tracker),
                &mut report.errors,
            );
            confirmed.extend(matches.into_iter().map(|group| (size, group)));
        }
    }
    tracker.finish();

    for (size, files) in confirmed {
        let mut files = distinct_physical_files(files, &mut report.errors);
        if files.len() >= 2 {
            files.sort_by(|a, b| a.path.cmp(&b.path));
            report.groups.push(DuplicateGroup {
                bytes_each: size,
                files,
            });
        }
    }
    report.refresh_totals();
    report
}

fn collect(
    root: &Path,
    min_bytes: u64,
    guard: &Guard,
    seen: &mut HashSet<PathBuf>,
    by_size: &mut HashMap<u64, Vec<Candidate>>,
    report: &mut DuplicateReport,
    tracker: &mut Tracker,
) {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) => {
            report.errors.push(format!("{} : {error}", root.display()));
            return;
        }
    };
    if !metadata.is_dir() {
        report
            .refused
            .push(format!("{} : n'est pas un dossier ordinaire", root.display()));
        return;
    }
    let canonical = match root.canonicalize() {
        Ok(path) => path,
        Err(error) => {
            report.errors.push(format!("{} : {error}", root.display()));
            return;
        }
    };
    if let Err(reason) = guard.check_tree(&canonical) {
        report.refused.push(format!("{} : {reason}", root.display()));
        return;
    }

    let walker = WalkDir::new(root)
        .follow_links(false)
        .min_depth(1)
        .into_iter()
        .filter_entry(|entry| {
            !(entry.file_type().is_dir()
                && SKIPPED_FOLDERS
                    .iter()
                    .any(|name| entry.file_name().eq_ignore_ascii_case(name)))
        });
    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report.errors.push(error.to_string());
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                report.errors.push(error.to_string());
                continue;
            }
        };
        report.scanned_files += 1;
        tracker.listed();
        if metadata.len() < min_bytes || !seen.insert(entry.path().to_path_buf()) {
            continue;
        }
        by_size.entry(metadata.len()).or_default().push(Candidate {
            path: entry.into_path(),
            bytes: metadata.len(),
            root: canonical.clone(),
            modified: metadata.modified().ok(),
        });
    }
}

fn split_by<K: Eq + Hash>(
    files: Vec<Candidate>,
    mut key: impl FnMut(&Candidate) -> io::Result<K>,
    errors: &mut Vec<String>,
) -> Vec<Vec<Candidate>> {
    let mut groups: HashMap<K, Vec<Candidate>> = HashMap::new();
    for file in files {
        match key(&file) {
            Ok(value) => groups.entry(value).or_default().push(file),
            Err(error) => errors.push(format!("{} : {error}", file.path.display())),
        }
    }
    groups.into_values().filter(|group| group.len() >= 2).collect()
}

fn partial_hash(path: &Path) -> io::Result<blake3::Hash> {
    let mut start = Vec::with_capacity(PARTIAL_BYTES as usize);
    File::open(path)?.take(PARTIAL_BYTES).read_to_end(&mut start)?;
    Ok(blake3::hash(&start))
}

fn full_hash(path: &Path, tracker: &mut Tracker) -> io::Result<blake3::Hash> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut chunk = vec![0; READ_CHUNK];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            return Ok(hasher.finalize());
        }
        hasher.update(&chunk[..read]);
        tracker.advance(read as u64);
    }
}

/// Hard links share their data: removing one would free nothing.
fn distinct_physical_files(files: Vec<Candidate>, errors: &mut Vec<String>) -> Vec<Candidate> {
    let mut identities = HashSet::new();
    let mut distinct = Vec::with_capacity(files.len());
    for file in files {
        let identity = File::open(&file.path)
            .and_then(|handle| winapi_util::file::information(&handle))
            .map(|info| (info.volume_serial_number(), info.file_index()));
        match identity {
            Ok(identity) => {
                if identities.insert(identity) {
                    distinct.push(file);
                }
            }
            Err(error) => errors.push(format!("{} : {error}", file.path.display())),
        }
    }
    distinct
}

impl DuplicateReport {
    fn refresh_totals(&mut self) {
        self.groups
            .sort_by_key(|group| std::cmp::Reverse(wasted(group)));
        self.wasted_bytes = self.groups.iter().map(wasted).sum();
    }

    /// Forgets files that no longer exist and groups left without a duplicate.
    pub fn prune(&mut self) {
        for group in &mut self.groups {
            group.files.retain(|file| fs::symlink_metadata(&file.path).is_ok());
        }
        self.groups.retain(|group| group.files.len() >= 2);
        self.refresh_totals();
    }
}

fn wasted(group: &DuplicateGroup) -> u64 {
    group.bytes_each * (group.files.len() as u64).saturating_sub(1)
}

/// Moves the chosen duplicates to `bin`. At least one copy of each group must stay.
pub fn recycle_selected(
    report: &DuplicateReport,
    paths: &[PathBuf],
    bin: &dyn Bin,
) -> Result<CleanOutcome, String> {
    let selected: HashSet<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let known: HashSet<&Path> = report
        .groups
        .iter()
        .flat_map(|group| group.files.iter().map(|file| file.path.as_path()))
        .collect();
    if let Some(path) = selected.iter().find(|path| !known.contains(*path)) {
        return Err(format!("fichier absent de la recherche : {}", path.display()));
    }
    for group in &report.groups {
        if group.files.iter().all(|file| selected.contains(file.path.as_path())) {
            return Err(format!(
                "au moins une copie doit rester : {}",
                group.files[0].path.display()
            ));
        }
    }

    let mut chosen = Vec::new();
    let mut skipped = Vec::new();
    for group in &report.groups {
        let (picked, kept): (Vec<&Candidate>, Vec<&Candidate>) = group
            .files
            .iter()
            .partition(|file| selected.contains(file.path.as_path()));
        if picked.is_empty() {
            continue;
        }
        if kept.iter().any(|file| verify_unchanged(file).is_ok()) {
            chosen.extend(picked);
        } else {
            skipped.extend(picked.into_iter().map(|file| Problem {
                path: file.path.clone(),
                reason: "la copie à garder a changé depuis la recherche".to_string(),
            }));
        }
    }
    let mut outcome = recycle(&chosen, bin);
    outcome.skipped.extend(skipped);
    Ok(outcome)
}
