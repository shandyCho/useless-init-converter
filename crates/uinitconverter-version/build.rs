//! Works out the build's version once, for every tool's --version:
//! UINITCONVERTER_BUILD when set (packages/build-rpms.sh sets it to the RPM's
//! version-release), else the workspace version and the git commit, with
//! "-dirty" when the sources have uncommitted changes.

use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    // --no-optional-locks: `git status` must not rewrite the index, which
    // would make this script run again on every build.
    let out = Command::new("git").arg("--no-optional-locks").arg("-C").arg(dir).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=UINITCONVERTER_BUILD");
    let version = env!("CARGO_PKG_VERSION");
    let build = match std::env::var("UINITCONVERTER_BUILD") {
        Ok(b) if !b.is_empty() => b,
        _ => {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
            let root = dir.join("../..");
            match git(dir, &["rev-parse", "--short", "HEAD"]) {
                Some(commit) => {
                    // Run again when the commit or the index changes.
                    if let Some(gitdir) = git(dir, &["rev-parse", "--absolute-git-dir"]) {
                        let gitdir = Path::new(&gitdir);
                        let mut watch = vec![gitdir.join("HEAD"), gitdir.join("index"), gitdir.join("packed-refs")];
                        if let Some(r) = git(dir, &["symbolic-ref", "-q", "HEAD"]) {
                            watch.push(gitdir.join(r));
                        }
                        for p in watch.iter().filter(|p| p.exists()) {
                            println!("cargo:rerun-if-changed={}", p.display());
                        }
                    }
                    let root = root.to_string_lossy();
                    let dirty = git(dir, &["-C", &root, "status", "--porcelain", "--", "Cargo.toml", "Cargo.lock", "crates"])
                        .map(|s| !s.is_empty())
                        .unwrap_or(false);
                    format!("{version} (git {commit}{})", if dirty { "-dirty" } else { "" })
                }
                None => version.to_string(),
            }
        }
    };
    println!("cargo:rustc-env=UINITCONVERTER_BUILD_VERSION={build}");
}
