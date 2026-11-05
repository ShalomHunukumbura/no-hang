//! no-hang finds network, database and process calls that can wait forever.

pub mod finding;
pub mod go;
pub mod javascript;
pub mod python;
pub mod report;
pub mod rules;
pub mod syntax;

use std::path::Path;

use finding::Finding;
use javascript::Dialect;

enum Language {
    Python,
    Js(Dialect),
    Go,
}

fn language(path: &Path) -> Option<Language> {
    Some(match path.extension()?.to_str()? {
        "py" => Language::Python,
        "js" | "mjs" | "cjs" | "jsx" => Language::Js(Dialect::JavaScript),
        "ts" | "mts" | "cts" => Language::Js(Dialect::TypeScript),
        "tsx" => Language::Js(Dialect::Tsx),
        "go" => Language::Go,
        _ => return None,
    })
}

/// Check one file, choosing the checker by its extension. Unknown extensions give nothing.
pub fn check_file(path: &Path, source: &str) -> Vec<Finding> {
    // Forward slashes on every platform, so reports and annotations read the same.
    let shown = path.to_string_lossy().replace('\\', "/");
    match language(path) {
        Some(Language::Python) => python::check(&shown, source),
        Some(Language::Js(dialect)) => javascript::check(&shown, source, dialect),
        Some(Language::Go) => go::check(&shown, source),
        None => Vec::new(),
    }
}

pub fn is_supported(path: &Path) -> bool {
    // Type declarations describe code; they never run it.
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    language(path).is_some() && !name.ends_with(".d.ts")
}
