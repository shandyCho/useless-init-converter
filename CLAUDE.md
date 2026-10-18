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

- Build and test: `cargo build`, `cargo test`, then the sandbox suite:
  `tests/sandbox.sh DIR && tests/check.sh DIR; tests/sandbox.sh DIR stop`.
  Everything runs unprivileged and never touches the host's init system.
- Shell scripts must pass `bash -n` and `shellcheck -S warning`.
- Docs exist in Korean and English: `README.md`/`README-us.md`,
  `TESTING.md`/`TESTING-us.md`. Change both together.
- The owner pushes to GitHub; this machine has no GitHub credentials.
- The development machine runs Artix with s6 as the real init. Never kill
  processes by a pattern that could match the system's or the owner's own s6
  trees, and use `pkill -x` rather than `pkill -f`.
