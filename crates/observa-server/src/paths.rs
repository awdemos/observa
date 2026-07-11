use std::path::{Path, PathBuf};

/// Locate the project workspace root at runtime.
///
/// Tries, in order:
/// 1. `CARGO_MANIFEST_DIR` if it points inside a directory containing `templates/`.
/// 2. The directory containing the current executable, walking up parents until a
///    directory containing both `templates/` and `assets/` is found.
/// 3. The current working directory if it contains `templates/` and `assets/`.
/// 4. Falls back to `.` so callers can report a clear error instead of panicking.
pub fn workspace_root() -> PathBuf {
    if let Some(root) = from_manifest_dir() {
        return root;
    }
    if let Some(root) = from_current_exe() {
        return root;
    }
    if let Some(root) = from_current_dir() {
        return root;
    }
    PathBuf::from(".")
}

fn from_manifest_dir() -> Option<PathBuf> {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let base = Path::new(&manifest);
    let candidate = if base.file_name()?.to_str()? == "observa-server" {
        base.parent()?.parent()?
    } else {
        base
    };
    if has_project_dirs(candidate) {
        Some(candidate.to_path_buf())
    } else {
        None
    }
}

fn from_current_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?;
    loop {
        if has_project_dirs(dir) {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?
    }
}

fn from_current_dir() -> Option<PathBuf> {
    let dir = std::env::current_dir().ok()?;
    if has_project_dirs(&dir) {
        Some(dir)
    } else {
        None
    }
}

fn has_project_dirs(path: &Path) -> bool {
    path.join("templates").is_dir() && path.join("assets").is_dir()
}

#[cfg(test)]
mod tests {
    use super::has_project_dirs;
    use std::path::PathBuf;

    #[test]
    fn detects_project_root() {
        // The workspace root in test runs should always have both directories.
        let root =
            PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string()));
        let root = if root.file_name().and_then(|s| s.to_str()) == Some("observa-server") {
            root.parent().unwrap().parent().unwrap().to_path_buf()
        } else {
            root
        };
        assert!(has_project_dirs(&root));
    }
}
