//! `groupchat doctor` / `groupchat prune`: install hygiene.
//!
//! Pure decision functions (which binaries to remove, is the keeper on PATH,
//! which identities to prune) live here, separated from the thin fs/daemon
//! side-effecting wrappers so the risky "what to delete" logic is unit-tested.

use std::path::{Path, PathBuf};

/// Every found binary except the keeper (compared by canonical path).
pub fn removal_set(found: &[PathBuf], keeper: &Path) -> Vec<PathBuf> {
    found.iter().filter(|p| p.as_path() != keeper).cloned().collect()
}

/// Whether the keeper's directory is present anywhere on PATH.
pub fn dir_on_path(path_dirs: &[PathBuf], keeper_dir: &Path) -> bool {
    path_dirs.iter().any(|d| d.as_path() == keeper_dir)
}

/// If some PATH entry *earlier* than the keeper's dir also contains a groupchat
/// binary, the keeper is shadowed; return that earlier dir. `binary_dirs` is the
/// set of dirs known to hold a groupchat binary.
pub fn shadowed_by(
    path_dirs: &[PathBuf],
    keeper_dir: &Path,
    binary_dirs: &[PathBuf],
) -> Option<PathBuf> {
    let keeper_idx = path_dirs.iter().position(|d| d.as_path() == keeper_dir)?;
    path_dirs
        .iter()
        .take(keeper_idx)
        .find(|d| binary_dirs.iter().any(|b| b.as_path() == d.as_path()))
        .cloned()
}

/// The cargo-dist self-updater that sits next to a binary.
pub fn updater_sibling(binary: &Path) -> PathBuf {
    binary.with_file_name("groupchat-update")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_set_excludes_only_the_keeper() {
        let a = PathBuf::from("/a/groupchat");
        let b = PathBuf::from("/b/groupchat");
        let c = PathBuf::from("/c/groupchat");
        let out = removal_set(&[a.clone(), b.clone(), c.clone()], &b);
        assert_eq!(out, vec![a, c]);
    }

    #[test]
    fn dir_on_path_detects_presence() {
        let dirs = vec![PathBuf::from("/usr/bin"), PathBuf::from("/home/me/.cargo/bin")];
        assert!(dir_on_path(&dirs, Path::new("/home/me/.cargo/bin")));
        assert!(!dir_on_path(&dirs, Path::new("/home/me/.local/bin")));
    }

    #[test]
    fn shadowed_by_finds_earlier_dir_with_binary() {
        let path = vec![
            PathBuf::from("/home/me/.local/bin"),
            PathBuf::from("/home/me/.cargo/bin"),
        ];
        let binary_dirs = vec![PathBuf::from("/home/me/.local/bin")];
        // keeper is in .cargo/bin, but .local/bin (earlier) also has one
        assert_eq!(
            shadowed_by(&path, Path::new("/home/me/.cargo/bin"), &binary_dirs),
            Some(PathBuf::from("/home/me/.local/bin"))
        );
        // no earlier dir holds a binary -> not shadowed
        assert_eq!(
            shadowed_by(&path, Path::new("/home/me/.local/bin"), &binary_dirs),
            None
        );
    }

    #[test]
    fn updater_sibling_is_next_to_binary() {
        assert_eq!(
            updater_sibling(Path::new("/home/me/.cargo/bin/groupchat")),
            PathBuf::from("/home/me/.cargo/bin/groupchat-update")
        );
    }
}
