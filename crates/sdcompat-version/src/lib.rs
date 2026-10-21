//! The version every sdcompat tool prints for `--version`, worked out by
//! build.rs: the RPM's version-release, or the workspace version and git
//! commit.

/// For example "0.1.0-1.20261021071849.g0023111.fc45" or "0.1.0 (git 0023111)".
pub const VERSION: &str = env!("SDCOMPAT_BUILD_VERSION");

/// Whether the arguments ask for the version.
pub fn wanted(arg: &str) -> bool {
    matches!(arg, "--version" | "-V" | "version")
}

/// Prints "TOOL VERSION", the line `sdcompat --version` reads back.
pub fn print(tool: &str) {
    println!("{tool} {VERSION}");
}
