use std::process::Command;

fn no_hang(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_no-hang"))
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("the binary runs");
    (
        out.status.code().expect("exited normally"),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn text_report_on_the_fixture_app() {
    let (code, out, _) = no_hang(&["tests/fixtures/app"]);
    assert_eq!(code, 1);
    let expected = "\
tests/fixtures/app/payments.py:12:12  NH001 error  requests.post() has no timeout, so it waits forever if the server stops answering.
tests/fixtures/app/payments.py:24:55  NH002 error  requests.get(timeout=None) turns the timeout off.
tests/fixtures/app/payments.py:28:35  NH900 note  requests.post(**options) may set timeout inside the dictionary, so it can't be checked.
tests/fixtures/app/payments.py:32:5  NH003 warning  subprocess.run() has no timeout, so a command that never exits blocks forever.
Found 2 errors and 1 warning in 2 files. 1 call couldn't be checked.
";
    let got: String = out
        .lines()
        .filter(|l| !l.starts_with("    fix:"))
        .map(|l| format!("{l}\n"))
        .collect();
    assert_eq!(got, expected);
}

#[test]
fn a_parameter_named_timeout_is_trusted() {
    // refund() passes its own timeout argument through; its value isn't known here.
    let (_, out, _) = no_hang(&["tests/fixtures/app/payments.py"]);
    assert!(!out.contains(":16:"), "{out}");
}

#[test]
fn clean_file_exits_zero() {
    let (code, out, _) = no_hang(&["tests/fixtures/app/clean.py"]);
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "All clear: checked 1 file, no calls that can wait forever.\n"
    );
}

#[test]
fn warnings_fail_only_with_strict() {
    let args = ["--ignore", "NH001,NH002", "tests/fixtures/app"];
    assert_eq!(no_hang(&args).0, 0);
    let strict = ["--strict", "--ignore", "NH001,NH002", "tests/fixtures/app"];
    assert_eq!(no_hang(&strict).0, 1);
}

#[test]
fn github_annotations() {
    let (_, out, _) = no_hang(&["--format", "github", "tests/fixtures/app/payments.py"]);
    let first = out.lines().next().unwrap();
    assert!(
        first.starts_with("::error file=tests/fixtures/app/payments.py,line=12,col=12,title=NH001 http-no-timeout::"),
        "{first}"
    );
    assert!(out.contains("::notice "), "{out}");
}

#[test]
fn json_report() {
    let (_, out, _) = no_hang(&["--format", "json", "tests/fixtures/app/payments.py"]);
    let rows: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 4);
    assert_eq!(rows[0]["rule"], "NH001");
    assert_eq!(rows[0]["severity"], "error");
    assert_eq!(rows[0]["line"], 12);
}

#[test]
fn explain_and_usage_errors() {
    let (code, out, _) = no_hang(&["--explain", "nh001"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("NH001 http-no-timeout (error)"), "{out}");
    assert!(out.contains("How to fix it:"));
    assert_eq!(no_hang(&["--explain", "NH999"]).0, 2);
    assert_eq!(no_hang(&["--ignore", "NH999"]).0, 2);
    assert_eq!(no_hang(&["does/not/exist"]).0, 2);
    assert_eq!(no_hang(&["--format", "xml"]).0, 2);
}

#[test]
fn follows_a_session_across_methods() {
    let (code, out, _) = no_hang(&["tests/fixtures/clients"]);
    assert_eq!(code, 1);
    assert!(
        out.starts_with(
            "tests/fixtures/clients/api.py:12:16  NH001 error  self.session.get() has no \
             timeout, and a requests.Session has no default, so it waits forever if the server \
             stops answering. Setting the session's .timeout does nothing in requests.\n"
        ),
        "{out}"
    );
    assert!(
        out.ends_with("Found 1 error and 0 warnings in 1 file.\n"),
        "{out}"
    );
}

#[test]
fn a_reader_that_stops_early_is_not_a_crash() {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_no-hang"))
        .args(["--explain", "NH001"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs");
    // Close the pipe before reading anything, like `no-hang | head -0`.
    drop(child.stdout.take());
    let status = child.wait().expect("it exits");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(status.success());
}

#[test]
fn ignored_files_need_no_ignore() {
    let (code, out, err) = no_hang(&["tests/fixtures/ignored"]);
    assert_eq!(code, 0);
    assert!(out.contains("checked 0 files"), "{out}");
    assert!(err.contains("--no-ignore"), "{err}");
    let (code, out, _) = no_hang(&["--no-ignore", "tests/fixtures/ignored"]);
    assert_eq!(code, 1);
    assert!(out.contains("vendored.py:2:1  NH001"), "{out}");
}

#[test]
fn ignore_comments() {
    let (code, out, _) = no_hang(&["tests/fixtures/ignores"]);
    assert_eq!(code, 1);
    let lines: Vec<&str> = out.lines().filter(|l| !l.starts_with("    fix:")).collect();
    assert_eq!(lines.len(), 2, "{out}");
    assert!(
        lines[0].starts_with("tests/fixtures/ignores/jobs.py:8:1  NH001"),
        "{out}"
    );
    assert_eq!(
        lines[1],
        "Found 1 error and 0 warnings in 1 file. 2 findings ignored by comments."
    );
}

#[test]
fn baseline_reports_only_new_findings() {
    let file = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("baseline.json");
    let file = file.to_str().unwrap();

    let (code, out, _) = no_hang(&["--write-baseline", file, "tests/fixtures/app"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("Saved 4 findings to "), "{out}");

    // The same code: nothing new.
    let (code, out, _) = no_hang(&["--baseline", file, "tests/fixtures/app"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.ends_with("4 findings already in the baseline.\n"),
        "{out}"
    );

    // New code: only its finding is reported.
    let (code, out, err) = no_hang(&[
        "--baseline",
        file,
        "tests/fixtures/app",
        "tests/fixtures/clients",
    ]);
    assert_eq!(code, 1);
    assert!(
        out.starts_with("tests/fixtures/clients/api.py:12:16  NH001"),
        "{out}"
    );
    assert!(err.is_empty(), "{err}");

    // Fixed code: the baseline says it has entries that no longer happen.
    let (code, _, err) = no_hang(&["--baseline", file, "tests/fixtures/clients"]);
    assert_eq!(code, 1);
    assert!(
        err.contains("4 findings in the baseline no longer happen"),
        "{err}"
    );
}

#[test]
fn a_broken_baseline_is_a_usage_error() {
    let (code, _, err) = no_hang(&["--baseline", "Cargo.toml", "tests/fixtures/app"]);
    assert_eq!(code, 2);
    assert!(err.contains("can't read the baseline"), "{err}");
}
