use std::env;
use std::path::{Path, PathBuf};

/// Refuses cleaning roots that would reach system or personal folders.
#[derive(Debug, Default)]
pub struct Guard {
    /// Must not be cleaned, nor any of their parents. Sub-folders stay allowed.
    exact: Vec<PathBuf>,
    /// Must not be cleaned at any depth, nor any of their parents.
    subtree: Vec<PathBuf>,
}

impl Guard {
    /// Paths that do not exist on this machine are ignored.
    pub fn new(exact: Vec<PathBuf>, subtree: Vec<PathBuf>) -> Self {
        let resolve = |paths: Vec<PathBuf>| -> Vec<PathBuf> {
            paths
                .into_iter()
                .filter_map(|path| path.canonicalize().ok())
                .collect()
        };
        Self {
            exact: resolve(exact),
            subtree: resolve(subtree),
        }
    }

    pub fn system() -> Self {
        let var = |name: &str| env::var_os(name).map(PathBuf::from);
        let exact = [
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "PROGRAMDATA",
            "WINDIR",
            "PUBLIC",
        ]
        .iter()
        .filter_map(|name| var(name))
        .collect();

        let mut subtree: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .filter_map(|name| var(name))
            .collect();
        if let Some(windir) = var("WINDIR") {
            for folder in ["System32", "SysWOW64", "WinSxS"] {
                subtree.push(windir.join(folder));
            }
        }
        if let Some(profile) = var("USERPROFILE") {
            for folder in [
                "Desktop",
                "Documents",
                "Downloads",
                "Music",
                "OneDrive",
                "Pictures",
                "Videos",
            ] {
                subtree.push(profile.join(folder));
            }
        }
        Self::new(exact, subtree)
    }

    /// `root` must already be canonical.
    pub fn check(&self, root: &Path) -> Result<(), String> {
        if root.parent().is_none() {
            return Err("racine d'un lecteur".to_string());
        }
        if let Some(path) = self.exact.iter().find(|path| path.starts_with(root)) {
            return Err(format!("contient le dossier protégé {}", path.display()));
        }
        self.check_tree(root)
    }

    /// Windows and installed programs only: personal folders stay searchable.
    pub fn for_search() -> Self {
        let mut subtree: Vec<PathBuf> = ["WINDIR", "ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .filter_map(|name| env::var_os(name).map(PathBuf::from))
            .collect();
        subtree.dedup();
        Self::new(Vec::new(), subtree)
    }

    /// Like `check`, but a drive root is fine when nothing protected lies under it.
    pub fn check_tree(&self, root: &Path) -> Result<(), String> {
        if let Some(path) = self
            .subtree
            .iter()
            .find(|path| root.starts_with(path) || path.starts_with(root))
        {
            return Err(format!("touche le dossier protégé {}", path.display()));
        }
        Ok(())
    }
}
