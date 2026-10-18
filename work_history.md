# Work history

Running log of the work on sdcompat: goals, decisions and their reasons,
current state, and what is left. Newest session last. Read this before
starting work; append a new session entry when you finish.

## Project goal (from the owner)

- Run **Fedora with GNOME** while replacing **systemd as PID 1 with s6**
  (s6-linux-init + s6-rc). Not OpenRC.
- Keep Fedora's tooling: dnf, SELinux enforcing by default, fresh packages.
- Approach chosen by the owner: a **translation layer**. Programs keep using
  systemd's interfaces (`systemctl`, libsystemd, D-Bus), and sdcompat carries
  them out with s6-rc.
- The owner builds Fedora packages and tests in a Fedora VM. The development
  machine is Artix Linux (s6 init), user `fraufaust37`.
- The owner's disk is encrypted with LUKS2.

## Key design decisions (and why)

1. **Protocol-level emulation, not library replacement.** sdcompat serves
   `org.freedesktop.systemd1` on the system bus and on `/run/systemd/private`.
   The real `systemctl`/libsystemd talk to it unchanged. libsystemd works
   without systemd as PID 1, and clients that speak the protocols directly
   (Go/Python/Rust libraries) are covered too.
2. **Keep the `systemd` package installed (Mode A).** It still provides
   systemctl, udevd, tmpfiles, sysusers, kernel-install and libsystemd; only
   PID 1 changes. This avoids dnf's protected-package problems, and systemd's
   RPM file triggers (`daemon-reload`, `preset`) reach sdcompat.
3. **GNOME needs a real user manager.** GNOME 49 starts its session through
   `systemd --user`. Rewriting that was judged out of scope, so the **real
   systemd-logind and `systemd --user` run as supervised services**, and
   sdcompat provides the PID 1 API they need (transient slices/scopes with
   PIDFDs, on-demand `user@UID.service`, PAMName=, Delegate=, shutdown targets).
   elogind was rejected: no user manager, so no GNOME 49.
4. **s6-rc only orders; sdcompat enforces requirements.** s6-rc has no soft
   dependencies, so a failing wanted unit would block everything after it.
   Every start is reported to s6-rc as successful; the real result is recorded
   in state files; the runner of a dependent refuses to start (result
   `dependency`) when a `Requires=/BindsTo=/Requisite=` unit failed; the daemon
   retries failed requirements on start.
5. **Real journald as an s6 service** (default `JOURNAL=yes`). The runner
   connects service output to journald's stdout stream like PID 1 does, and
   the cgroup layout matches systemd's (`system.slice/<unit>`), so
   `journalctl -u` and `systemctl status` logs work. `JOURNAL=no` keeps
   rsyslog + per-service s6-log.
6. **The initramfs is still Fedora's systemd-based dracut image.** It unlocks
   LUKS and mounts root, then `init=/usr/sbin/init-selinux` loads the SELinux
   policy and execs s6-linux-init. No systemd process survives the switch.
7. **Safety at install:** the normal systemd boot stays the default; setup only
   adds a GRUB entry "Fedora (s6 + sdcompat)" (SELinux permissive, no plymouth),
   keeps backups, and offers rollback (`sdcompat.db=previous`, `sdcompat.reload=0`).

## Session 1: 2026-10-18 to 2026-10-19

Built the whole project from scratch in one long session. Commits, in order:

| Commit | What |
|---|---|
| `9f0fd45` | Core: unit parser/loader, s6-rc generator, runner, D-Bus daemon |
| `69c8e29` | Fedora dist files; `/run/systemd/private` (systemd 259's root systemctl uses only that socket) |
| `6e4e928` | Timer units (calendar engine, monotonic timers, Persistent=, list-timers) |
| `252d6da` | GNOME stack: logind + user manager support |
| `509f8b7` | `dist/setup-fedora.sh` |
| `101d05a` | Path/mount/swap units, systemd-run, journald integration, offline updates, kernel-install plugin |
| `dfea75e` | Fixes for a hostile review (16 findings) and an unprivileged-caller test phase |
| `b3c81d2` | RPM specs for skalibs, execline, s6, s6-rc, s6-linux-init (Fedora does not package them) |
| `97e1e4a` | `dist/rpm/build-skarnet-rpms.sh`; setup `--rpms=DIR` |
| `983ba8c` | README in Korean (`README.md`) and English (`README-us.md`) |
| `86a7e68` | Testing manual `TESTING.md` / `TESTING-us.md`; `tests/collect-diagnostics.sh` |
| `110e5f2` | LUKS: setup warns about crypttab volumes the initramfs does not unlock |

Hostile review highlights (all fixed in `dfea75e`): forged `ActivationRequest`
let any user start units as root (now only the bus daemon is accepted); one
failing wanted unit blocked all services (design change, see decision 4);
plus polkit, fd store, caches, job queue/cancel, capability and group fixes.

Bugs found while working, worth remembering:
- Rust 2021: a `MutexGuard` in an `if let` condition lives through the `else`
  branch. Re-locking there deadlocked the daemon. Bind the value first.
- Reading inotify events from an unaligned `u8` buffer panics in debug builds;
  use `read_unaligned`.
- `pkill -f` with a pattern contained in your own shell command line kills the
  shell; match on process name (`pkill -x`).
- zsh does not word-split `$var`; run such loops under bash.
- Heredocs nested in a heredoc: use distinct delimiters (`PYEOF`).

## Current state (end of session 1)

- **Sandbox suite:** `tests/check.sh` passes 102/102 on Artix (unprivileged,
  private s6-svscan/s6-rc/D-Bus), including Fedora 44's real `systemctl` and
  `systemd-run` (extracted RPM, `SYSTEMD_ROOT=`).
- **Verified only by staged build, not rpmbuild:** the five skarnet specs
  (no rpm tooling or containers on the Artix machine).
- **Implemented but never run (need root / a real Fedora boot):** cgroup
  delegation, PAMName=, sandboxing, transient scopes creating cgroups, real
  journald with the invocation-ID xattr, real logind and `systemd --user`,
  polkit granting access, an actual s6 boot, offline updates.
- **Unverified risk:** whether `systemctl daemon-reload` (s6-rc-update) during
  a GNOME session disturbs on-demand units such as `user@1000.service`.
- **Not implemented:** `Accept=yes` sockets and on-demand socket activation,
  `systemd-run --pipe/--stdio`, autofs, `AccuracySec=`/`WakeSystem=`, watchdog,
  credentials, more sandboxing (`PrivateDevices/Network`, seccomp), cancelling
  running jobs, `ExecStop=` when s6-rc stops units directly, crypttab volumes
  not unlocked by the initramfs.
- **Repository:** `https://github.com/shandyCho/sdcompat` (private). The owner
  pushes; this machine has no GitHub credentials. At the end of session 1,
  `110e5f2` and this file were not pushed yet.

## Next steps

1. Owner runs `TESTING.md` stages 1–6 on a Fedora 44 Workstation VM and sends
   `tests/collect-diagnostics.sh` archives. Likely trouble, in order: RPM
   `%files` lists, the first s6 boot, the GNOME login.
2. Optional, discussed but not started: a systemd-free initramfs for the s6
   entry only (dracut without its systemd modules, separate
   `initramfs-<kver>-s6.img`, rebuilt by the kernel-install plugin). Loses
   TPM2/FIDO2 auto-unlock. Waiting for the owner's answer on whether they use
   TPM2/FIDO2 or type the LUKS passphrase.
3. After testing: make the s6 entry the default (`grubby --set-default=…`).

## Session 2: 2026-10-19

Owner reported `dist/rpm/build-skarnet-rpms.sh` failing in the skalibs build
(first run on a real Fedora machine, fc45).

- **Bug:** `%make_install` passes `INSTALL=/usr/bin/install -p` to make. That
  replaces skarnet's bundled `tools/install.sh`, whose `-l` option makes
  symlinks (`libskarnet.so.2.15 -> …`); coreutils `install` has no `-l`, so
  `%install` failed with "invalid option -- 'l'".
- **Fix:** all five specs now use `%{__make} install DESTDIR=%{buildroot}`.
  execline, s6, s6-rc and s6-linux-init use the same Makefile pattern, so they
  would have failed the same way.
- **Verified:** skalibs builds with rpmbuild on fc45; the RPMs contain
  `libskarnet.so.2.15.1.0` and the `.so.2.15` / `.so` symlinks. The other four
  were not built here (the script installs each -devel package with sudo dnf).

Fix commit: `6c61147`.

Second report: `dnf install` of execline conflicted with bash on
`/usr/bin/cd`, `/usr/bin/umask`, `/usr/bin/wait` (bash ships POSIX wrapper
scripts around its builtins).

- **Decision:** bash keeps those paths. execline-2.9.9.2-2 drops the `cd`
  and `umask` symlinks (their targets `execline-cd`/`execline-umask` stay) and
  moves `wait` to `/usr/libexec/execline/wait`. Checked first: sdcompat's
  stage-2 scripts are `/bin/sh`, setup runs s6-linux-init-maker without `-C`,
  and in the s6/s6-rc/s6-linux-init sources only the container-mode scripts
  of s6-linux-init-maker call `/usr/bin/wait`. Hand-written execline scripts
  must use `execline-cd`/`execline-umask` and the full path of `wait`.
- **Script bug:** `build-skarnet-rpms.sh` collected RPMs with
  `find -name "$name-[0-9]*.rpm"`, so after a release bump it picked up both
  the old and new builds. It now deletes a package's earlier RPMs (in `RPMS/`
  and `rpms/`) before building it.
- **Verified:** execline-2.9.9.2-2 builds on fc45; none of its `/usr/bin`
  files belongs to an installed package. `bash -n` passes on the script;
  shellcheck is not installed on this machine, not run.

Fix commit: `ff4553f`.

Third report: s6 failed in `%files devel` with `File must begin with "/":
libs6auto.pc`.

- **Bug:** s6.spec listed `%{_libdir}/pkgconfig/libs6 libs6auto.pc` (two
  names fused on one line; rpm read the second as a relative path). s6-rc.spec
  had the same mistake with `libs6rc libs6rcrepo.pc`. Both now list each `.pc`
  file on its own line. No release bump: neither package was ever built.
- **Verified:** s6 builds with rpmbuild on fc45. s6-rc and s6-linux-init were
  built against the unpacked s6/s6-devel RPMs (same configure options as the
  specs, plus `--with-include/--with-lib/--with-dynlib`, since installing
  s6-devel needs sudo); their `make install` output matches the `%files`
  lists. No file of s6, s6-rc or s6-linux-init belongs to an installed package.

Fix commit: `73f688a`.

Sandbox suite on Fedora 45 (this machine): `tests/check.sh` passes 102/102
after a fresh `tests/sandbox.sh`; transient scope SKIPs (needs root for
`/sys/fs/cgroup/system.slice`). `check.sh` cannot be rerun against the same
sandbox: its last phase leaves a daemon running without `ALLOW_UIDS`, so a
second run fails ~27 checks with "Access denied". Stop and recreate the
sandbox between runs.

Then, at the owner's request ("make check.sh"), made `check.sh` safe to
rerun. Commits `f14df0e` (fix), and the tests/docs commit after it.

- **Real bug found on the way (`f14df0e`):** `gen::plan()` seeded every unit
  on the search path, and `TRANSIENT_DIR` is on it. A transient unit still
  loaded at daemon-reload (a failed `systemd-run` unit stays loaded, as in
  systemd) went into the s6-rc database while the daemon also supervised it
  from `RUN_DIR/dyn`. `s6-rc-update` failed ("unable to manage new service
  directories … File exists"); after the retry, enabled units (hello,
  tick.timer, watch.path, after-bad) were down. On a real system: any
  `daemon-reload` after a failed `systemd-run` would stop services. Fix:
  plan() skips units whose fragment is under `TRANSIENT_DIR`. Regression
  check added to the systemd-run phase (Reload, then hello and tick.timer
  still up); it fails on the unfixed build (6 failures) and passes now.
- **Not understood yet:** why the s6-rc-update retry after that failure left
  the enabled units down. The fix removes the trigger; the retry path in
  `gen::regenerate` may still be unsafe after other partial failures.
- **check.sh reruns, decision:** the checks assume the state right after
  `sandbox.sh` (bad.service failed, output files empty, sr-wait not created,
  fd store unused); a second run fails 8 checks on leftover state only.
  Making every check reset its state was judged not worth it. Instead:
  `check.sh` marks `$W/checked` and refuses a second run with the recreate
  command (exit 2); after the unprivileged phase it restarts the normal
  daemon, so the sandbox stays usable by hand. The daemon PID is now kept in
  `$W/daemon.pid` (sandbox.sh writes it, check.sh replaces it, `stop` kills
  it), replacing check.sh's `pkill -f`.
- **Docs:** check count 102 → 105 in both READMEs; both TESTING manuals say
  the `bad.service` failure printed by sandbox.sh is expected, and that
  check.sh runs once per sandbox.
- **Verified:** `cargo test` ok; `bash -n` on both scripts; fresh sandbox →
  check.sh 105/105 (1 SKIP: transient scope, needs root); second run refused;
  `sandbox.sh stop` leaves no sdcompat/dbus-daemon/s6-svscan behind.
  shellcheck not installed here, not run.

Owner ran TESTING step 4 (`sdcompat plan`) on this Fedora 45 machine:
83 longrun, 69 oneshot, 70 bundle; 165 skipped; one cycle broken.

- Reviewed: gdm, NetworkManager, accounts-daemon, journald, logind all
  translated. Skipped: 151 by the default EXCLUDE (systemd-*, dbus-broker,
  ...), 7 sockets bound by their service's runner, slices/devices not
  translated, `smb.service` referenced but samba not installed. chronyd is
  absent because chrony is not installed here.
- Cycle `dracut-shutdown.service -> sysinit.target`: dracut enables it in
  sysinit.target.wants while DefaultDependencies order it after sysinit.target.
  systemd resolves it by not ordering the target after the unit; sdcompat
  keeps it in the sysinit bundle and drops the unit's dependency. Harmless
  here (ExecStart=/bin/true; the work is in ExecStop). Not changed.
- **Bug fixed:** `FileDescriptorStoreMax=` was warned as ignored although the
  fd store implements it (missing from `gen::SERVICE_KEYS`), on journald and
  logind. Of the 38 distinct warned keys, it was the only one the code reads.
  Also, the D-Bus property `FileDescriptorStoreMax` always reported 0; it now
  reports the unit's value. Verified: unprivileged `sdcompat plan` differs from
  the owner's only by the two removed warnings; sandbox 105/105; property
  reads 4 for fdkeep.service.

### Current state

As in session 1, except: this machine is now Fedora 45 (not Artix). skalibs,
execline and s6 build with rpmbuild on it; s6-rc and s6-linux-init pass the
staged build against the real s6 RPMs but have not gone through rpmbuild.
Sandbox suite: 105/105 on Fedora 45 (102/102 before the transient-reload check).

### Next steps

1. Owner reruns `build-skarnet-rpms.sh` to finish s6, s6-rc, s6-linux-init.
2. Look at the s6-rc-update retry in `gen::regenerate`: find out why units
   were down after it, and whether a failed update should roll back instead.
3. The session 1 next steps still apply.
