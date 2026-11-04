//! no-hang finds network, database and process calls that can wait forever.

pub mod finding;
pub mod python;
pub mod report;
pub mod rules;

use std::path::Path;

use finding::Finding;

/// Check one file, choosing the checker by its extension. Unknown extensions give nothing.
pub fn check_file(path: &Path, source: &str) -> Vec<Finding> {
    // Forward slashes on every platform, so reports and annotations read the same.
    let shown = path.to_string_lossy().replace('\\', "/");
    match path.extension().and_then(|e| e.to_str()) {
        Some("py") => python::check(&shown, source),
        _ => Vec::new(),
    }
}

pub fn is_supported(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("py"))
}
