use std::process::Command;

#[test]
fn prints_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_signal-tui"))
        .arg("--version")
        .output()
        .expect("run signal-tui --version");

    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("version is UTF-8");
    let version = stdout.trim_end();
    assert!(
        is_version(version),
        "unexpected version format: {version:?}"
    );
}

#[test]
fn rejects_version_with_extra_args() {
    let output = Command::new(env!("CARGO_BIN_EXE_signal-tui"))
        .args(["--version", "extra"])
        .output()
        .expect("run signal-tui --version extra");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(stderr.contains("unknown argument"), "stderr: {stderr}");
}

fn is_version(version: &str) -> bool {
    let (tag, suffix) = version.split_once('+').unwrap_or((version, ""));
    let Some(rest) = tag.strip_prefix('v') else {
        return false;
    };
    let mut parts = rest.split('.');
    let numeric_tag = matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(major), Some(minor), Some(patch), None)
            if !major.is_empty()
                && !minor.is_empty()
                && !patch.is_empty()
                && major.chars().all(|c| c.is_ascii_digit())
                && minor.chars().all(|c| c.is_ascii_digit())
                && patch.chars().all(|c| c.is_ascii_digit())
    );
    numeric_tag
        && (suffix.is_empty()
            || (suffix.len() == 5 && suffix.chars().all(|c| c.is_ascii_hexdigit())))
}
