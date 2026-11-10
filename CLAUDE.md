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

- Scope (the owner's decision, work_history part 205): uinitconverter
  implements only what the real systemd programs that still run need from
  PID 1 (logind, `systemd --user`, journald) and what it takes to run
  Fedora's unit files under s6-rc. "systemd does it too" is not a reason to
  add a feature; add one only for a named real caller, and record that
  caller in work_history.md. See "Scope" in the docs/etc pair. Direction
  (part 215): keep the project small; use s6's and its ecosystem's own
  features (a skarnet tool's behaviour over a Rust re-model of it), and
  leave what only systemd can do to the real systemd program.
- Layout: the core is one Cargo workspace (`crates/`); its crates may depend
  on each other. `standalone/*` are separate projects (each its own
  `[workspace]`, Cargo.lock, README pair and spec) and must not depend on the
  core or on each other: no path dependencies, no shared code. The core must
  keep working without them.
- Names: the commands are `initconvert*` (`initconvert`, `initconvertd`,
  `initconvert-gen`, ..., the standalone `initconvert-fdstore`, ...);
  everything else stays `uinitconverter*`: packages, crates, project
  directories, paths (`/usr/libexec/uinitconverter/`, `/etc/uinitconverter`),
  s6 service names, the SELinux module, D-Bus names, kernel options and
  variables.
- Packaging: each core executable is its own RPM (`packages/NAME.spec`, all
  built from one workspace tarball by `packages/build-rpms.sh`), and they
  depend on each other with `Requires`; the library crates are not packaged.
  `user@UID.service` runs `systemd --user`; the s6 user manager
  (`uinitconverter-user`) was removed and `uinitconverterd` Obsoletes it.
  The `uinitconverter` package holds the boot setup (`dist/`) and requires the tools.
  The version is `[workspace.package] version` in Cargo.toml and the release
  is generated per build; the specs carry neither, so build them only with
  that script.
- Build and test: `cargo build`, `cargo test`; for each standalone project
  `cargo test --manifest-path standalone/NAME/Cargo.toml`; then the sandbox
  suite: `tests/sandbox.sh DIR && tests/check.sh DIR; tests/sandbox.sh DIR stop`
  (it builds the standalone tools), also with `SANDBOX_JOURNAL=no` and
  `SANDBOX_FDSTORE=no` in front of both scripts when touching those parts.
  `SANDBOX_S6NET_BIN=~/rpmbuild-skarnet/sysroot/usr/bin` (after
  `dist/rpm/build-skarnet-rpms.sh`) runs the `Accept=yes` TCP checks instead
  of skipping them. GNOME under `systemd --user` is checked in the VM:
  `tests/gnome-vm.sh` (fetch/provision once into `~/uinitconverter-vm`),
  then `tests/vm-user-manager.sh ~/uinitconverter-vm MODE`, modes
  `session`, `greeter`, `apps`, `lifecycle`, `confined`; `BOOT=systemd`
  (default, the stock boot as reference) or `BOOT=s6` (session, lifecycle,
  confined; every RPM and `initconvert init`). `VM_KEEP=yes` keeps the VM
  up for `tests/gnome-vm.sh ssh`. The s6 boot itself (the RPMs,
  `initconvert init`, `rc.init`, the runner's `user@`):
  `tests/vm-s6-boot.sh ~/uinitconverter-vm MODE`, modes `default`,
  `old-config`, `upgrade` (from 0.3.0, `OLD_RPMS=`), `rollback`, after `packages/build-rpms.sh --test DIR` with
  `CORE_RPMS=DIR/rpms` and `SKARNET_RPMS=~/rpmbuild-skarnet/rpms` (the
  standalone RPMs from `standalone/*/build-rpm.sh
  ~/uinitconverter-vm/rpmbuild/standalone`).
  Everything runs unprivileged and never touches the host's init system.
- Never modify skarnet's sources (skalibs, execline, s6, s6-rc,
  s6-linux-init, s6-frontend), and don't add patches for them in
  `dist/rpm/`. Reading the tarballs to understand their behaviour is fine.
  Fix things on uinitconverter's side (SELinux module, skel scripts, setup,
  crates); if that isn't possible, describe the upstream behaviour and
  leave the decision to the owner.
- Contracts between the tools (the `result` values, the runner's and the
  timers' commands, the fd store protocol, the config keys, the index): each
  has one definition in `uinitconverter-base` and tests on every side; see
  "Contracts between the tools" in the docs/etc pair. Use the constants,
  never literals, and change a definition together with its tests.
- Shell scripts must pass `bash -n` and `shellcheck -S warning`.
- Docs exist in Korean and English: `README.md`/`README-us.md` (first install
  only; everything else goes in `docs/etc.md`/`docs/etc-us.md`),
  `TESTING.md`/`TESTING-us.md`, `docs/PACKAGEBUILD.md`/`docs/PACKAGEBUILD-us.md`
  (test and release builds), and each standalone project's README pair.
  Change both together.
- The owner pushes to GitHub; this machine has no GitHub credentials.
- The development machine runs Artix with s6 as the real init. Never kill
  processes by a pattern that could match the system's or the owner's own s6
  trees, and use `pkill -x` rather than `pkill -f`.
