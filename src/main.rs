use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

use clap::{Parser, ValueEnum};

use no_hang::baseline::{self, Baseline};
use no_hang::finding::Finding;
use no_hang::report::Summary;
use no_hang::rules::{self, Severity, RULES};
use no_hang::{check_file, is_supported, report, Checked};

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Github,
    Json,
}

/// Find network, database and process calls that can wait forever.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Files or directories to check. Directories are searched, skipping .gitignored files.
    #[arg(default_value = ".")]
    paths: Vec<PathBuf>,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,

    /// Fail on warnings too, not just errors.
    #[arg(long)]
    strict: bool,

    /// Rule codes to skip, comma-separated, e.g. NH003.
    #[arg(long, value_delimiter = ',')]
    ignore: Vec<String>,

    /// Explain one rule in detail and exit.
    #[arg(long, value_name = "CODE")]
    explain: Option<String>,

    /// List every rule and exit.
    #[arg(long)]
    list_rules: bool,

    /// Check files that .gitignore and .ignore files leave out, like node_modules or .venv.
    #[arg(long)]
    no_ignore: bool,

    /// Only report findings that aren't in this baseline file.
    #[arg(long, value_name = "FILE")]
    baseline: Option<PathBuf>,

    /// Save every current finding to this baseline file and exit, so only new ones fail.
    #[arg(long, value_name = "FILE", conflicts_with = "baseline")]
    write_baseline: Option<PathBuf>,
}

const USAGE_ERROR: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.list_rules {
        let mut list = String::new();
        for r in RULES {
            list.push_str(&format!(
                "{}  {:<22} {:<8} {}\n",
                r.code,
                r.name,
                r.severity.as_str(),
                r.summary
            ));
        }
        emit(&list);
        return ExitCode::SUCCESS;
    }
    if let Some(code) = &cli.explain {
        return explain(code);
    }

    let mut ignored = HashSet::new();
    for code in &cli.ignore {
        match rules::find(code.trim()) {
            Some(rule) => {
                ignored.insert(rule.code);
            }
            None => {
                eprintln!("no-hang: unknown rule {code}. Try --list-rules.");
                return ExitCode::from(USAGE_ERROR);
            }
        }
    }

    let known = match &cli.baseline {
        Some(path) => match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|json| Baseline::parse(&json))
        {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("no-hang: can't read the baseline {}: {e}", path.display());
                return ExitCode::from(USAGE_ERROR);
            }
        },
        None => None,
    };

    let files = match collect_files(&cli.paths, cli.no_ignore) {
        Ok(files) => files,
        Err(message) => {
            eprintln!("no-hang: {message}");
            return ExitCode::from(USAGE_ERROR);
        }
    };

    let checked = check_all(&files);
    let mut totals = Summary {
        files: files.len(),
        ignored: checked.ignored,
        known: 0,
    };
    let mut findings: Vec<Finding> = checked
        .findings
        .into_iter()
        .filter(|f| !ignored.contains(f.rule.code))
        .collect();
    findings.sort_by(|a, b| (&a.path, a.line, a.col).cmp(&(&b.path, b.line, b.col)));

    if let Some(path) = &cli.write_baseline {
        if let Err(e) = std::fs::write(path, baseline::render(&findings)) {
            eprintln!("no-hang: can't write {}: {e}", path.display());
            return ExitCode::from(USAGE_ERROR);
        }
        emit(&format!(
            "Saved {} to {}. Pass --baseline {} to report only new ones.\n",
            plural(findings.len(), "finding"),
            path.display(),
            path.display()
        ));
        return ExitCode::SUCCESS;
    }
    if let Some(known) = known {
        let (rest, skipped, fixed) = known.filter(findings);
        findings = rest;
        totals.known = skipped;
        if fixed > 0 {
            eprintln!(
                "no-hang: {} in the baseline no longer happen. Run --write-baseline again to \
                 drop them.",
                plural(fixed, "finding")
            );
        }
    }

    let output = match cli.format {
        Format::Text => report::text(&findings, &totals),
        Format::Github => report::github(&findings),
        Format::Json => report::json(&findings),
    };
    emit(&output);

    let fails = |f: &&Finding| match f.severity() {
        Severity::Error => true,
        Severity::Warning => cli.strict,
        Severity::Note => false,
    };
    if findings.iter().any(|f| fails(&f)) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Check files on every core. Each thread takes the next unchecked file until none are left.
fn check_all(files: &[PathBuf]) -> Checked {
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(files.len().max(1));
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut found = Checked::default();
                    while let Some(file) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                        // A file that isn't UTF-8 can't be source we'd understand; skip it.
                        if let Ok(source) = std::fs::read_to_string(file) {
                            let checked = check_file(file, &source);
                            found.findings.extend(checked.findings);
                            found.ignored += checked.ignored;
                        }
                    }
                    found
                })
            })
            .collect();
        let mut all = Checked::default();
        for worker in workers {
            let found = worker.join().expect("a checker thread panicked");
            all.findings.extend(found.findings);
            all.ignored += found.ignored;
        }
        all
    })
}

fn explain(code: &str) -> ExitCode {
    let Some(rule) = rules::find(code) else {
        eprintln!("no-hang: unknown rule {code}. Try --list-rules.");
        return ExitCode::from(USAGE_ERROR);
    };
    emit(&format!(
        "{} {} ({})\n\n{}\n\nWhy it matters:\n{}\n\nHow to fix it:\n{}\n",
        rule.code,
        rule.name,
        rule.severity.as_str(),
        wrap(rule.summary),
        wrap(rule.why),
        rule.all_fixes()
            .lines()
            .map(wrap)
            .collect::<Vec<_>>()
            .join("\n")
    ));
    ExitCode::SUCCESS
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Write to stdout. A reader that stops early, like `no-hang | head`, closes the pipe; that
/// isn't an error worth a panic, and the exit code still reports the findings.
fn emit(text: &str) {
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}

/// Wrap at 80 columns with a two-space indent.
fn wrap(text: &str) -> String {
    let mut lines = vec![String::from(" ")];
    for word in text.split_whitespace() {
        let line = lines.last_mut().expect("there is always a line");
        if line.len() + 1 + word.len() > 80 && !line.trim().is_empty() {
            lines.push(format!("  {word}"));
        } else {
            line.push(' ');
            line.push_str(word);
        }
    }
    lines.join("\n")
}

fn collect_files(paths: &[PathBuf], no_ignore: bool) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for path in paths {
        if !path.exists() {
            return Err(format!("{} does not exist.", path.display()));
        }
        if path.is_file() {
            // A file named on the command line is checked even if it's gitignored.
            if is_supported(path) {
                files.push(path.clone());
            }
            continue;
        }
        let before = files.len();
        let walker = ignore::WalkBuilder::new(path)
            .git_ignore(!no_ignore)
            .git_global(!no_ignore)
            .git_exclude(!no_ignore)
            .ignore(!no_ignore)
            .parents(!no_ignore)
            .build();
        for entry in walker {
            let entry = entry.map_err(|e| e.to_string())?;
            let p = entry.path();
            if entry.file_type().is_some_and(|t| t.is_file()) && is_supported(p) {
                files.push(clean(p));
            }
        }
        if files.len() == before && !no_ignore {
            eprintln!(
                "no-hang: found nothing to check in {}. If its files are gitignored, \
                 --no-ignore checks them.",
                path.display()
            );
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// `./app/x.py` reads better as `app/x.py`.
fn clean(path: &Path) -> PathBuf {
    path.strip_prefix("./").unwrap_or(path).to_path_buf()
}
