use std::process::Command;

fn main() {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("Failed to execute git command");

    let git_hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let git_hash: String = git_hash.chars().take(8).collect();

    println!("cargo:rustc-env=GIT_HASH={}", git_hash);
}
