use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueEnum};

use no_hang::finding::Finding;
use no_hang::rules::{self, Severity, RULES};
use no_hang::{check_file, is_supported, report};

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
}

const USAGE_ERROR: u8 = 2;

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.list_rules {
        for r in RULES {
            println!(
                "{}  {:<22} {:<8} {}",
                r.code,
                r.name,
                r.severity.as_str(),
                r.summary
            );
        }
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

    let files = match collect_files(&cli.paths) {
        Ok(files) => files,
        Err(message) => {
            eprintln!("no-hang: {message}");
            return ExitCode::from(USAGE_ERROR);
        }
    };

    let mut findings: Vec<Finding> = Vec::new();
    for file in &files {
        // A file that isn't UTF-8 can't be Python source we'd understand; skip it.
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        findings.extend(
            check_file(file, &source)
                .into_iter()
                .filter(|f| !ignored.contains(f.rule.code)),
        );
    }
    findings.sort_by(|a, b| (&a.path, a.line, a.col).cmp(&(&b.path, b.line, b.col)));

    let output = match cli.format {
        Format::Text => report::text(&findings, files.len()),
        Format::Github => report::github(&findings),
        Format::Json => report::json(&findings),
    };
    print!("{output}");

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

fn explain(code: &str) -> ExitCode {
    let Some(rule) = rules::find(code) else {
        eprintln!("no-hang: unknown rule {code}. Try --list-rules.");
        return ExitCode::from(USAGE_ERROR);
    };
    println!("{} {} ({})\n", rule.code, rule.name, rule.severity.as_str());
    println!("{}\n", wrap(rule.summary));
    println!("Why it matters:\n{}\n", wrap(rule.why));
    println!("How to fix it:\n{}", wrap(rule.fix));
    ExitCode::SUCCESS
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

fn collect_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
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
        for entry in ignore::WalkBuilder::new(path).build() {
            let entry = entry.map_err(|e| e.to_string())?;
            let p = entry.path();
            if entry.file_type().is_some_and(|t| t.is_file()) && is_supported(p) {
                files.push(clean(p));
            }
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
