# CLAUDE.md

## Before you start

Read `work_history.md` first. It records the project goal, the design
decisions and why they were made, what has been verified and what has not,
and the next steps. Don't re-litigate decisions listed there unless the owner
asks.

## When you finish

Append a new session entry to `work_history.md`: date, what changed (commit
hashes), decisions and their reasons, bugs found, test results, and an updated
"Current state" and "Next steps". Keep the earlier entries; correct them only
if they turned out to be wrong.

## Working on this project

- Layout: the core is one Cargo workspace (`crates/`); its crates may depend
  on each other. `standalone/*` are separate projects (each its own
  `[workspace]`, Cargo.lock, README pair and spec) and must not depend on the
  core or on each other: no path dependencies, no shared code. The core must
  keep working without them.
- Packaging: each core executable is its own RPM (`packages/NAME.spec`, all
  built from one workspace tarball by `packages/build-rpms.sh`), and they
  depend on each other with `Requires`; the library crates are not packaged.
  The `sdcompat` package holds the boot setup (`dist/`) and requires the tools.
  The version is `[workspace.package] version` in Cargo.toml and the release
  is generated per build; the specs carry neither, so build them only with
  that script.
- Build and test: `cargo build`, `cargo test`; for each standalone project
  `cargo test --manifest-path standalone/NAME/Cargo.toml`; then the sandbox
  suite: `tests/sandbox.sh DIR && tests/check.sh DIR; tests/sandbox.sh DIR stop`
  (it builds the standalone tools), also with `SANDBOX_JOURNAL=no` and
  `SANDBOX_FDSTORE=no` in front of both scripts when touching those parts.
  Everything runs unprivileged and never touches the host's init system.
- Shell scripts must pass `bash -n` and `shellcheck -S warning`.
- Docs exist in Korean and English: `README.md`/`README-us.md`,
  `TESTING.md`/`TESTING-us.md`, and each standalone project's README pair.
  Change both together.
- The owner pushes to GitHub; this machine has no GitHub credentials.
- The development machine runs Artix with s6 as the real init. Never kill
  processes by a pattern that could match the system's or the owner's own s6
  trees, and use `pkill -x` rather than `pkill -f`.
