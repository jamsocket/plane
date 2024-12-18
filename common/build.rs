use std::process::Command;

fn main() {
    let git_hash = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .map(|output| {
            let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
            hash.chars().take(8).collect::<String>()
        })
        .unwrap_or_else(|_| String::from("unknown"));

    println!("cargo:rustc-env=GIT_HASH={}", git_hash);
}
