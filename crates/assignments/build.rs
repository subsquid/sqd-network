use std::process::Command;

fn main() {
    let status = Command::new("flatc")
        .current_dir("schema")
        .args(["--rust", "-o", "gen", "assignment.fbs"])
        .status()
        .expect("Failed to run flatc");
    assert!(status.success(), "flatc command failed");
}
