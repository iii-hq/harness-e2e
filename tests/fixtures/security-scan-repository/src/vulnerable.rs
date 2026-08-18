use std::process::Command;

/// Deliberately vulnerable fixture: untrusted input reaches a shell command.
/// This file is test data and is never compiled or executed by the E2E.
pub fn lookup(user_input: &str) {
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!("grep {user_input} /tmp/security-scan-e2e-records"))
        .status();
}
