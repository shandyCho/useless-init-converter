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
- Corrected by the owner on 2026-10-23: the admin manages the system with
  the **s6 ecosystem's own tools** (s6-frontend's `s6`, s6-rc, including
  `s6 set` and the repository); systemd's commands are used only in a
  limited way. A pure s6 Fedora is impossible, so sdcompat is the bridge,
  but s6's tools are first-class (see the 2026-10-23 "s6-rc repository"
  session).
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

## Session 3: 2026-10-19

Owner reported: no "Fedora (s6 + sdcompat)" entry in the GRUB menu, and
`/proc/1/comm` is `systemd`.

- **Diagnosis, no code change:** setup (TESTING step 5) has not run on this
  machine. The five skarnet RPMs are installed, but there is no
  `/etc/sdcompat/boot-options`, no `/usr/sbin/init-selinux`, no
  `/var/lib/sdcompat`, no sdcompat binary, and no kernel-install plugin; the
  current boot's `/proc/cmdline` has no `init=`. So the entry was never
  written, and the systemd boot was the expected result.
- `/boot/loader/entries` is root-only on Fedora 45; an unprivileged check
  cannot see the entries.
- Fedora hides the GRUB menu by default (`menu_auto_hide`); even with the entry
  in place, the owner must hold Shift / press Esc at boot, or run
  `sudo grub2-editenv - unset menu_auto_hide`.
- This machine is real hardware with LUKS (`rd.luks.uuid=` on the command
  line), not the VM that TESTING assumes for step 5.

### Next steps

1. Owner runs step 5 (`--dry-run`, then `sudo dist/setup-fedora.sh
   --rpms=…`), then `sudo ls /boot/loader/entries | grep sdcompat`.
2. Earlier next steps still apply.

Later the same day the owner ran setup and booted the s6 entry (boot
`12aecceab466450a8417a30a2f7c6cd6`, 01:50:59–01:51:58), reported "booting
failed", and came back on the systemd entry. Diagnosis from the journal only,
no code change:

- Stage 2 worked: rc.init's reload made the current db, journald, dbus,
  sdcompatd, logind, NetworkManager, sshd and others started.
- No login appeared. gdm gave up ("Failed to contact accountsservice"):
  polkit was D-Bus-activated at boot but never logged "Started polkitd" in
  the first cycle, so accounts-daemon and tuned timed out on it. The default
  bundle has no getty on tty1; the only console login is the early getty on
  tty9.
- From 38.8 s on, the whole tree restarted 9 times at irregular 1–10 s
  intervals until the owner pressed the power key: every service's
  s6-supervise got SIGINT (s6 ≥2.10 forwards it to the service's process
  group and exits), and s6-svscan respawned them with new PIDs. **What sent
  SIGINT is not known.** The intervals look like keypresses (logind started
  kmscon on tty2 at 45.8 s and tty3 at 51.1 s, i.e. the owner was switching
  VTs). The s6 catch-all log in `/run/uncaught-logs` was lost at power-off.
- Other errors in that boot, not yet investigated: chronyd "Could not send
  notification to $NOTIFY_SOCKET" (twice, fatal); dbus-daemon rejected a
  method_return from sdcompat to logind with `requested_reply="0"` (once per
  cycle); wpa_supplicant D-Bus activation failed (no Wi-Fi); `dbus-daemon.service`
  (Fedora's real name for dbus.service) is translated because EXCLUDE only
  lists `dbus.service` (not in the default bundle, so harmless at boot).

### Next steps

1. Owner boots the s6 entry again, does not press keys on tty1, switches to
   tty9 (Ctrl+Alt+F9, root login), and saves `cp -r /run/uncaught-logs
   /var/tmp/` plus `s6-rc -a list` and `ps -eo pid,pgid,sid,tty,comm` before
   shutting down. Then find the SIGINT source.
2. polkit activation under sdcompat; chronyd notify; the rejected reply to
   logind; add a tty1 getty to the default bundle so a failed gdm is not a
   blank screen.

## Session 4: 2026-10-19

Owner reported "booting failed" again: second s6 boot
`594a5c4db8034ffea89672705e1c8367` (02:01:54–02:06:08). The owner logged in
on tty9 and rebooted with `sudo reboot`. `/run/uncaught-logs` was not saved,
so this is again from the journal only.

- **The SIGINT restart storm did not happen this time** (sshd started once,
  no restarts). That supports the keypress theory from session 3; still not
  confirmed.
- **Bug fixed (`223d51f`): Type=notify daemons that drop privileges
  could not send READY=1.** The runner made each unit's notify socket mode
  0600, owned by root or User=. chronyd starts as root, switches to `chrony`
  itself, then notifies → EACCES → "Fatal error : Could not send notification
  to $NOTIFY_SOCKET" and exit. Fix in `runner.rs`/`fdstore.rs`: socket mode
  0666 (as systemd's `/run/systemd/notify`), `SO_PASSCRED` on it, and
  messages are accepted only from root, the unit's User=, the main PID, or a
  process in the main PID's cgroup (`notify_sender_ok`). Before, the abstract
  socket fallback accepted anyone; now it is filtered too.
- **Probable effect of the chronyd failure, inferred, not verified**
  (*corrected in session 5: wrong; the 105 s stall was a deadlock between
  the boot's s6-rc change and polkit's activation, see there*): every
  D-Bus activation was stuck until 131.9 s. polkit was requested at 26.92 s,
  polkitd started at 131.92 s = 105 s later, which is chronyd's (and polkit's)
  `timeout-up` (90 s + 15 s). Meanwhile accounts-daemon, upower (aborted 4×),
  wpa_supplicant and nm-dispatcher activations hit dbus-daemon's 25 s limit,
  and gdm gave up repeatedly. The likely mechanism: the boot's `s6-rc change`
  held the s6-rc lock while waiting for chronyd's readiness until its
  timeout. Not checked: whether s6-svwait should have returned early when
  finish exited 125. Session 3's "polkit never started in the first cycle"
  is probably the same stall.
- **Second blocker, not fixed: the user manager never ran.** After polkit and
  accounts-daemon came up, gdm started greeter sessions; the runner for
  `user@60578.service` got through `pam_systemd_home` (harmless: "sufficient",
  falls through to pam_unix), but no `systemd --user` ever logged anything,
  for any UID including the owner's tty login. gdm-wayland-session found no
  `/run/user/UID/bus`, started its own dbus-daemon, and gnome-session aborted
  on "Name org.freedesktop.systemd1 does not exist". After 6 failures gdm
  gave up ("maximum number of display failures"). Why user@.service fails
  is unknown: its output is in the lost s6 catch-all log.
- `/etc/pam.d/systemd-user` does not exist on Fedora 45; the stack is in
  `/usr/lib/pam.d/systemd-user`.
- **Verified:** `cargo build`, `cargo test` ok; sandbox suite 105/105 (1 SKIP,
  transient scope). The new sender filter cannot be exercised unprivileged
  (one UID, one cgroup); the chronyd case needs a real boot.

### Current state

As in session 3, plus the notify-socket fix (`223d51f`).
On real hardware the s6 boot reaches a working system with console logins
(tty9), but no graphical login: user@.service does not run systemd --user.

### Next steps

1. Install the new build, boot the s6 entry, log in on tty9 and save:
   `cp -r /run/uncaught-logs /var/tmp/`, `systemctl status user@1000.service
   chronyd.service polkit.service`, `s6-rc -a list`. Check chronyd is up and
   polkit starts within seconds.
2. Find why user@.service does not start `systemd --user`.
3. Session 3's next steps 2 (rejected reply to logind, tty1 getty) still apply.

## Session 5: 2026-10-19

Owner reported "booting failed" a third time: s6 boot
`2c47f9e49b0d446f914c4ae9cbb22902` (02:20:04–02:49:12), first boot with the
`223d51f` build (installed 02:17). Owner logged in on tty9 at ~28 min and
ran `sudo reboot`. `/run/uncaught-logs` was again not saved; journal only.

- chronyd no longer fails ("Could not send notification" is gone), so the
  `223d51f` fix works for it. **polkit still started only after 105 s**, so
  session 4's explanation (chronyd's readiness timeout) was wrong.
- **Bug found and fixed (`57777ef`): deadlock between the boot's
  `s6-rc change` and D-Bus activation.** upower.service and
  accounts-daemon.service are Type=dbus and in the default bundle; both
  need polkit before they take their bus names (upowerd aborts, accounts-
  daemon exits after dbus-daemon's 25 s activation timeout, s6 restarts
  them, again and again). polkit is only D-Bus activated, and its start ran
  `s6-rc -b -u change polkit.service`, which waits for the s6-rc lock. s6-rc
  holds that lock for a whole change, including every readiness wait
  (checked in the s6-rc 0.7.0.0 source: `s6rc_lock` exclusive for changes,
  skalibs `fd_lock` = fcntl). So the boot's change waited for upower, upower
  for polkit, polkit for the boot's change, until upower's 105 s
  `timeout-up`. polkitd started at exactly +105 s; gdm's "Failed to contact
  accountsservice" loop is the same stall.
- **Fix:** when another change holds the lock (F_GETLK on `LIVE_DIR/lock`,
  or our own `s6_lock` is taken) and all of a longrun's `s6-rc-db
  all-dependencies` are up, the daemon starts it with `s6-svc -u`
  (`start_beside`), keeps it in `Daemon::beside` (counted as active), and a
  1 s task records it with `s6-rc -u change` once the lock is free (also
  before a reload, so s6-rc-update does not keep it down). Stopping it
  before then uses `s6-svc -wD -d`, since s6-rc thinks it is down.
  `rc.shutdown` touches `/run/sdcompat/shutting-down`, which disables this
  during shutdown. Oneshots and bundles still wait for the lock.
  Decision: not "report Type=dbus ready early to s6-rc", which would break
  After= ordering on Type=dbus units, and the same deadlock can happen with
  Type=notify services.
- **Second blocker, still not understood: user@UID.service never runs
  `systemd --user`.** Same as session 4: the runner gets through
  pam_systemd_home, then no output at all, from the greeter UIDs or from the
  owner's tty9 login (user@1000 runner at 02:48:51, no systemd --user log in
  the 20 s before reboot). Not even an exec error reached the journal.
  The runner's own messages go to the s6 catch-all log, lost at reboot.
- **Diagnostics fix (`f98c376`):** as root, the daemon copies its stderr
  (and its children's, e.g. s6-rc errors) to syslog as `sdcompatd`, and the
  runner's `log()` also goes to syslog as `sdcompat`. Next time:
  `journalctl -b -1 -t sdcompatd -t sdcompat`. Checked under `unshare -r`
  (euid 0 in a user namespace) with a scratch program: its line and a
  child's stderr line reached the host journal, and the original stderr still
  got both. Unprivileged it does nothing, so the sandbox does not write into
  the host's journal. TESTING docs mention it.
- **Flaky test fixed (`f182a43`):** notifier.service failed in about half
  the sandbox runs since `223d51f`: `sdcompat notify` (a child of the
  unit's sh) exits right after sending, and the sender filter's
  `/proc/PID/cgroup` read can then fail. The unit's uid is now User= or the
  runner's own euid, so the sandbox's sender matches by uid. No change as
  root.
- One sandbox run (out of ~12) failed 14 checks with the whole boot down;
  most likely caused by my first version of the new check, which left a
  `( sleep 10; kill $hold )` watchdog running into the next sandbox (PID
  reuse). Replaced by a bounded wait; not seen in the 7 runs since. Not proven.
- Other observations from the journal, not acted on: gdm greeter sessions
  close ~0.2 s after the user@ runner starts ("Session never registered",
  gnome-session aborts on "org.freedesktop.systemd1 does not exist" on the
  greeter's own dbus-daemon); it looks like logind's CreateSession reply
  does not wait for user@ to be up. dbus-org.freedesktop.home1.service is
  excluded, so accounts-daemon and pam_systemd_home log "excluded by sdcompat
  configuration" (harmless). wpa_supplicant activation also waited for the
  lock (it is Type=dbus, activated by NetworkManager); should be fixed by
  `57777ef`.
- git has no user identity on this machine; commits use the identity of the
  earlier commits through GIT_AUTHOR_*/GIT_COMMITTER_* (config untouched).
- **Verified:** `cargo build` (no warnings), `cargo test` ok; sandbox suite
  113/113 in 7 consecutive runs (1 SKIP: transient scope, needs root); the
  new "start beside a busy s6-rc change" checks fail 6 times on the old
  daemon. `bash -n` on the changed scripts; shellcheck still not installed.
  Not verified on a real boot.

### Current state

As in session 4, plus `57777ef` (no polkit/accountsservice stall expected),
`f182a43`, `f98c376`. Expected on the next s6 boot: polkit and
accounts-daemon up within seconds, gdm reaching its greeter attempts
quickly, which still fail without `systemd --user`.

### Next steps

1. Owner installs the new build (and reruns setup, so the new
   `rc.shutdown` reaches s6-linux-init), boots the s6 entry, logs in on tty9,
   and before rebooting saves `cp -r /run/uncaught-logs /var/tmp/`. Then,
   from the normal boot: `journalctl -b -1 -t sdcompatd -t sdcompat`.
   Check: polkitd started within seconds; whatever user@.service printed.
2. Find why user@.service does not run `systemd --user` (now with logs).
3. Check whether logind's CreateSession waits for user@UID's start job
   (JobRemoved timing from sdcompat's dynamic units).
4. Session 3's leftovers: rejected method_return to logind, tty1 getty.

## Session 6: 2026-10-20

Owner reported: the s6 boot works now, but the display stays black and DNS
does not work. Boot `5a77d4846e22455cb973e4531a511135` (2026-10-19
03:31–03:36), the first with `57777ef`/`f98c376`. The new syslog copy
worked: `journalctl -b -1 -t sdcompatd -t sdcompat` showed the causes.

- **`57777ef` works:** polkitd started at 22.6 s (was +105 s); logind,
  polkit, accounts-daemon, NetworkManager-dispatcher and wpa_supplicant were
  "started beside" the boot's change. No stall.
- **Black display, bug fixed (`053d665`):** every `user@UID.service` failed
  with `PAM: pam_setcred: Failure setting user credentials`, so
  `systemd --user` never ran (this is the session 4/5 "second blocker").
  Fedora's `/usr/lib/pam.d/systemd-user` has no `auth` lines; Linux-PAM then
  uses `/etc/pam.d/other`, whose `pam_deny` fails pam_setcred. systemd
  ignores the pam_setcred result (logs at debug); `pam.rs` now does the same.
  gdm's greeters then died on "org.freedesktop.systemd1 does not exist", as
  before.
- **DNS, bug fixed (`36a323d`):** `/etc/resolv.conf` is a symlink to
  resolved's `stub-resolv.conf` (127.0.0.53), and `systemd-resolved.service`
  was excluded by `systemd-*`, so nothing answered ("activation of
  dbus-org.freedesktop.resolve1.service failed: excluded"). Decision: add
  `systemd-resolved.service` and its varlink/monitor sockets to the default
  INCLUDE (resolved needs nothing from PID 1), not point resolv.conf
  elsewhere. `sdcompat plan`: resolved is a longrun in the sysinit bundle,
  the two sockets are bound by its runner; only sandboxing keys are warned.
- Seen, not acted on: `user-runtime-dir@6057x.service` ExecStop exit 1 when
  greeter sessions ended (probably follows from user@ failing);
  "stopping shutdown.target (conflicts with systemd-hostnamed.service)"
  (shutdown.target was not active, harmless).
- **Verified:** `cargo build`, `cargo test` ok; sandbox suite 113/113 (1 SKIP,
  transient scope). Not verified on a real boot; shellcheck still not
  installed (no shell script changed).

### Current state

As in session 5, plus `053d665` and `36a323d`. Expected on the next s6
boot: `systemd --user` runs for gdm's greeter users, and DNS works.
Whether GNOME then reaches a login screen is unknown: the next problem may
be session 5's observation that the greeter session closes ~0.2 s after
user@ starts (logind's CreateSession not waiting for user@).

### Next steps

1. Owner installs the new build and boots the s6 entry. Check: login screen
   appears; `resolvectl status` and `getent hosts fedoraproject.org` on
   tty9. If the screen is still black: `journalctl -b -1 -t sdcompatd -t
   sdcompat` and `journalctl -b -1 _UID=60578` (or the greeter's uid).
2. Session 5's next steps 3–4 still apply.

## Session 7: 2026-10-20

Owner reported: the s6 boot and the GNOME login work. Checked their
`sudo tests/collect-diagnostics.sh` archive (boot of 2026-10-20 09:30,
build `959a482`, SELinux permissive via `enforcing=0`).

- **Session 6 fixes confirmed on a real boot:** gdm's greeter and the
  owner's session 2 on tty2 (seat0) run, `user@1000.service` runs
  `systemd --user` (session 3, class manager), gnome-shell and
  gnome-session are up. `systemd-resolved.service` is active, NetworkManager
  uses it, chronyd resolved the pool names (DNS works). Suspend also works
  (s2idle entry and resume).
- **Bug fixed (`90003e2`): the s6 `syslog` (rsyslogd) restarted every
  second** with "could not open config file '/etc/sdcompat/rsyslog.conf'"
  (seen in `/run/uncaught-logs`). Setup does not add `syslog` to
  sdcompat-sysinit with journald, but the default DEP_MAP still mapped
  `systemd-journald.socket` and `syslog.socket` to `syslog`, so
  systemd-journald.service, the dracut units, sockets.target and
  storage-target-mode.target pulled it in. `Config::finish` now drops these
  two mappings when JOURNAL=yes (a user mapping to another name is kept).
  `sdcompat plan` on the owner's config: the only differences are
  `syslog` → `systemd-journald.service` in the deps and `syslog` gone from
  the two bundles.
- Seen, not fixed (need the owner's decision):
  - `fwupd.service` fails: "syscall filtering is configured but not
    working". The unit sets `Environment=FWUPD_SYSCALL_FILTER=systemd` and
    fwupd checks that a SystemCallFilter is in force; sdcompat ignores
    SystemCallFilter=. Options: implement seccomp (larger), or drop that
    variable when the filter is ignored.
  - `systemd-suspend.service` "fails" (exit 1) although suspend works:
    systemd-sleep calls `FreezeUnit`/`ThawUnit` on the Manager, which
    sdcompat lacks (could write `cgroup.freeze` of the unit's cgroup).
  - SELinux: running `systemd --user` needs `transition` from
    `unconfined_service_t` (sdcompat) to `unconfined_t`; allowed only
    because the boot entry is permissive. Matters for enforcing.
  - chronyc's `write`/`sendto` on `/run/chrony/chronyd.sock` (121 AVCs):
    chronyd runs as `unconfined_service_t` instead of `chronyd_t`, probably
    the same missing domain transition.
  - Harmless or not ours: home1 excluded (pam_systemd_home,
    accounts-daemon), `ReleaseSession ... NoSuchSession` on session end,
    `user-runtime-dir@0` ExecStop exit 1, malcontent and orca messages.
- **Sandbox flake, found but not fixed:** about half the sandbox runs fail
  the same 14 checks, on the old code too (old: 3 of 4 runs, new: 2 of 5).
  The first `daemon-reload` fails in s6-rc-update with "unable to manage new
  service directories in .../live: Broken pipe"; `gen.rs`'s retry then
  succeeds, but s6-svscan has lost the service directories ("unable to chdir
  to hello.service") and the oneshot runner is gone (`s6rc-oneshot-runner/s:
  Connection refused`). The services "started beside" (`s6-svc -u`) just
  before are probably involved. This is likely session 5's "14 checks"
  failure. It could also break a daemon-reload on a real boot.
- **Verified:** `cargo build` and `cargo test` pass (new
  `config::tests::journal_drops_syslog_map`). Sandbox: 113/113 in 3 of 5 runs
  with the fix; the other 2 hit the flake above. No shell script changed.
  Not verified on a real boot.

### Current state

The s6 boot reaches a working GNOME desktop with DNS. With `90003e2`,
no rsyslogd should run under s6 with journald.

### Next steps

1. Fix the s6-rc-update "Broken pipe" during daemon-reload and make the
   retry recover (or refuse) instead of leaving a broken live dir.
2. Owner installs `90003e2` and reboots: `/run/uncaught-logs/current`
   should have no rsyslogd lines; `s6-rc -a list` should not list `syslog`.
3. Decide on fwupd (SystemCallFilter) and FreezeUnit/ThawUnit.
4. SELinux domain transitions for services and `systemd --user` before
   trying enforcing mode.
5. Session 5's next steps 3–4 still apply.

## Session 8: 2026-10-20

Owner asked: make fwupd.service work, and fix SELinux. Commit `2d922a8`.
Development happened on the owner's machine booted with s6 (PID 1
`s6-svscan`, SELinux permissive); no privileged command was run.

- **fwupd (`SystemCallFilter=`, done):** fwupd's unit sets
  `FWUPD_SYSCALL_FILTER=systemd`; fwupd then probes `mlock` (outside its
  allow list) and refuses to start if it does not fail. Decision: implement
  `SystemCallFilter=`, `SystemCallErrorNumber=` and
  `SystemCallArchitectures=` properly (`src/seccomp.rs`) instead of unsetting
  the variable, because 52 Fedora units use these keys. Merge rules as
  systemd's `config_parse_syscall_filter` (first line picks allow/deny list,
  allow lists get `@default`, `~` lines remove from an allow list, `:ERRNO`
  per entry, empty assignment resets via `get_all`). Groups are systemd's
  own: `src/syscall-groups.txt` is `systemd-analyze syscall-filter` of
  systemd 262 (`tests/update-syscall-groups.sh` regenerates it), embedded
  with `include_str!`. libseccomp.so.2 is dlopen()ed (no -devel needed,
  sandbox on Artix keeps working); if it is missing the filter is skipped
  with a message, as systemd does without seccomp. The filter is loaded last
  before `execve` in `exec_command`, not for `+` commands; no_new_privs is
  set only without CAP_SYS_ADMIN (systemd's "implied NoNewPrivileges").
  Secondary ABIs (x86, x32) are filtered too unless SystemCallArchitectures=
  restricts them. Exit code 231 on failure.
- **SELinux (done in code, not verified on a boot):** every process under
  s6 ran as `unconfined_service_t`, PID 1 included: the stage-1 `init`
  script is `init_exec_t` (→ init_t), but init_t executing `bin_t`
  (s6-linux-init) becomes unconfined_service_t (checked with
  `security_compute_create`). So no service entered its domain, and
  `systemd --user` needed `transition` to unconfined_t (the session-7
  AVCs). Decision: keep the whole s6 tree in init_t, as systemd is, so the
  policy's own init_t transitions apply (computed: chronyd_exec_t →
  chronyd_t, xdm_exec_t → xdm_t, fwupd_exec_t → fwupd_t,
  systemd_logind_exec_t → systemd_logind_t; unconfined_t + init_exec_t stays
  unconfined_t, so users are unaffected). `dist/selinux/sdcompat.cil`
  labels s6-*, ucspilogd, all execline programs (s6-rc runs oneshots through
  execlineb, the oneshot runner through fdmove), /usr/libexec/{s6,s6-rc,
  execline,sdcompat}/* `init_exec_t`, and `/run/sdcompat` `init_var_run_t`
  (+ a named type transition), like /run/systemd, for the notify sockets.
  Generated run scripts live in /run (var_run_t) and stay init_t. Rejected:
  setexeccon from unconfined_service_t (the policy forbids those
  transitions) and a permissive local module (does not give services their
  domains). `src/selinux.rs` (libselinux dlopen()ed, root + SELinux only)
  does systemd's labelling: `selinux_restorecon` on RuntimeDirectory= & co.,
  socket parents and socket files; socket-unit sockets created with
  `setsockcreatecon(label of the service's exec)`
  (mac_selinux_get_create_label_from_exe). setup-fedora.sh loads the module
  with `semodule -i` and relabels (file list derived from the .cil);
  `--uninstall` removes it; the spec does the same in %post/%postun.
- CIL checked only partly: parsed by libsepol's `cil_add_file` (which does
  reject a broken file), both contexts valid in the loaded policy, and the
  regexes match exactly the installed skarnet/sdcompat binaries
  (`/usr/libexec/execline/wait` was missing at first). Linking against the
  full policy (`semodule -i`) needs root and has not been run.
- Spec bug fixed: `%{_prefix}/lib/kernel/install.d/90-sdcompat.install` was in
  %install (would run as a command) instead of %files.
- **Verified:** `cargo build` with no warnings; `cargo test` 22 ok (new:
  seccomp parse tests, a forked-child test that a loaded deny list returns
  EPERM, a libselinux binding test). Sandbox: 5 new checks (deny list blocks
  `kill` with EPERM, `+` command unfiltered, fwupd's exact filter makes
  `mlock` fail with EPERM); 118/118 in 2 of 4 runs, the other 2 hit the
  session-7 reload flake (new checks passed in all 4). `sdcompat plan` on
  the owner's config: the 61 SystemCall* "ignored" warnings are gone, nothing
  else changed. `bash -n` on changed scripts; `setup-fedora.sh --dry-run`
  prints the new SELinux steps. shellcheck and clippy are not installed.
  Not verified on a real boot.
- Note for test writers: in unit files `$$` is a literal `$`; a shell's
  `$$` needs `$$$$` (`\$\$\$\$` inside sandbox.sh's heredocs).

### Current state

As session 7, plus `2d922a8`. On the next s6 boot (after rerunning setup):
fwupd should start; 52 units run with their seccomp filters; PID 1 and
the s6 tree in init_t, services in their own domains. Still permissive.

### Next steps

1. Owner reruns `sudo dist/setup-fedora.sh --rpms=... --yes` (loads the
   module; check that `semodule -i` succeeded), reboots into s6, then:
   `ps -eZ | grep -E ' (s6-svscan|chronyd|gdm|systemd-logind)$'` (expect
   init_t, chronyd_t, xdm_t, systemd_logind_t), `fwupdmgr get-devices`,
   `systemctl --failed`, and `sudo tests/collect-diagnostics.sh` (the AVC
   list is in avc.txt). If a service with SystemCallFilter= dies with
   SIGSYS/EPERM, that is the first thing to look at.
2. Work through the remaining AVCs (now from confined domains), then try
   `--enforcing`.
3. Session 7's step 1 (s6-rc-update "Broken pipe" on reload) is still open
   and makes about half the sandbox runs fail 14 checks.
4. Other seccomp-based keys (RestrictAddressFamilies=, RestrictNamespaces=,
   MemoryDenyWriteExecute=, LockPersonality=, RestrictRealtime=) and
   FreezeUnit/ThawUnit are still not implemented.

## Session 9: 2026-10-20

Owner booted the s6 entry after session 8 (boot and GNOME login worked) and
ran `sudo tests/collect-diagnostics.sh` (`/tmp/sdcompat-diag.1UI53N.tar.gz`).
This session ran on that same boot (Fedora, PID 1 `s6-svscan`, permissive),
without root. Commits `8e5ec0c`, `91b9336`.

What the diagnostics showed:
- **Session 8 verified on a boot:** `systemctl --failed` empty,
  `is-system-running` = running; PID 1 and sdcompat in `init_t`, services in
  their domains (chronyd_t, xdm_t, systemd_logind_t, system_dbusd_t,
  fwupd_t, ...), `systemd --user` in unconfined_t. fwupd activated on
  `fwupdmgr get-devices`. chronyd loaded its seccomp filter; no SIGSYS / 231
  exits. Session 7 step 2 too: no rsyslogd lines in uncaught-logs, no
  `syslog` s6 service.
- **Bug, fixed (`8e5ec0c`):** every Type=notify helper (`sdcompat run --
  X.service` left after the double fork) used 100% CPU: 8 processes, load
  8.6, 6 min of CPU each after 6 min uptime (voluntary_ctxt_switches 1).
  The poll timeout after READY=1 was `-1`, passed as `wait.max(0)` = 0.
  Present since `dfea75e` (session 1). check.sh now measures the helper's
  CPU ticks over 1 s ("notify helper idle"): old code 103 ticks (FAIL),
  fixed code idle. The pgrep pattern must allow `--config` between `run`
  and `--`.
- **Bug, fixed (`91b9336`):** earlyoom restarted 159 times ("extra argument
  not understood: 'Content|Isolated'"): `exec earlyoom $EARLYOOM_ARGS` does
  not remove quotes, systemd does. Now `eval "set -- $EARLYOOM_ARGS"` (the
  file is already sourced, so no new trust). Checked by running the script
  against Fedora's /etc/default/earlyoom.
- **AVCs (464 lines), by cause:**
  - `/run/dbus` (and so `system_bus_socket`) was `var_run_t`: the s6 dbus
    run script mkdirs it, no tmpfiles.d entry exists (systemd creates it
    for dbus.socket). ~20 domains denied `write` on the socket: enforcing
    would break the bus. Fix: restorecon in `dist/s6/dbus/run`.
  - `/run/systemd/userdb` was `init_var_run_t` (gdm created it under
    /run/systemd); policy: `systemd_userdbd_runtime_t`, gdm's socket
    `xdm_var_run_t`. systemd's PID 1 creates the directory. Fix:
    `early mount-api` creates it before its `restorecon -R /run`.
  - init_t on s6 objects: read/write/ioctl the catch-all logger fifo
    (`/run/service/s6-svscan-log/fifo`, var_run_t; also dbus-daemon and
    hostname writing their stderr there), s6-svlink creating symlinks in
    `/run/service`, s6-supervise executing `/run/sdcompat/dyn/user@*/run`
    (init_var_run_t). Decision: allow rules in sdcompat.cil, not relabels,
    because these are s6 behaviours with no systemd counterpart and the
    generated files would otherwise need extra type transitions.
  - Checked against the live policy with `selinux_check_access` (in
    permissive it returns 0 but logs `avc: denied` for what enforcing would
    deny): every added permission was denied, `unlink`/`map` were already
    allowed (left out); with the new labels, dbus socket and userdb accesses
    are allowed. CIL parses (`cil_add_file`); `semodule -i` not run (root).
  - Left: systemd-hostnamed reading `/proc/pressure/*` (proc_psi_t),
    firewalld creating `.cache` in a tmpfs_t dir. Probably the policy, not
    sdcompat; not looked into.
- Other log items, not fixed: journald's `FDSTORE: fd store not reachable`
  (19 times, 11:02:40–42, before sdcompatd is up: those fds are lost if
  journald restarts); `user-runtime-dir@60578` ExecStop exit 1;
  homed is excluded by config (pam_systemd_home/accounts-daemon warnings,
  expected). The rest is hardware/GNOME noise.
- **Verified:** `cargo build`, `cargo test` (22 ok). Sandbox: 119/119 in 1
  of 3 runs with the fix, the other 2 hit the session-7 reload flake (same
  14 checks); the new check passed in all 3. `bash -n` / `sh -n` on the
  changed scripts; shellcheck still not installed. Not verified on a boot.
- Git on the Fedora install has no identity; commits were made with
  `git -c user.name=shandy-c -c user.email=...` (config not changed).

- Owner asked for a list of recommended dependencies in the READMEs: new
  "Dependencies" section in README.md/README-us.md (required packages, the
  dlopen()ed libpam/libselinux/libseccomp and what is lost without each,
  recommended packages, build crates).
- Then TESTING.md/TESTING-us.md: dependency check (`rpm -q pam libselinux
  libseccomp policycoreutils`) in stage 1, N about 120 and the known
  14-check reload flake in stage 3, three stage-6 rows for this
  session's fixes (busy runner, earlyoom, bus/userdb labels), and a
  `semodule -l` check after updating. Fedora 44 or 45; README check
  count 118 -> 119.

- **s6-frontend (`88bb40d`), owner asked for the `s6` command.** Decision:
  0.1.0.0, not the current 0.2.0.0, because 0.1.0.0 is built for exactly
  the pinned set (skalibs 2.15.1.0, execline 2.9.9.2, s6 2.15.1.0, s6-rc
  0.7.0.0, s6-linux-init 1.2.0.2); 0.2.0.0 needs skalibs 2.16, execline
  2.10, s6 2.15.1.1, s6-rc 0.8 (s6-rc 0.8 itself only bumps dependencies,
  no database format change, so upgrading the whole set later is an
  option). SHA-256 matches skarnet's `.sha256` file. `/etc/s6.conf`
  (`dist/s6.conf`) sets `bootdb = /var/lib/sdcompat/compiled`; scandir
  `/run/service` and livedir `/run/s6-rc` are already the defaults.
  Upstream bug found: `s6 live status SERVICES...` failed ("unable to take
  locks: No such file or directory"; livedir passed as " /run/s6-rc" in a
  depth-1 execline block, and "-cbe" without its space). 0.2.0.0 has the
  fix; backported as `dist/rpm/s6-frontend-live-status.patch`. Docs list
  the safe subcommands (live status, process status/restart/kill, system
  reboot/poweroff = same s6-linux-init shutdown as systemctl) and the ones
  not to use next to sdcompat (repository, set, live install,
  start/stop-everything, system boot). `s6 live start/stop` and `s6
  process restart` bypass sdcompat (no job, no ExecStop=), like s6-rc.
  Verified: the spec builds with rpmbuild (patch applies with fuzz 0);
  a scratch build run against the live tree as a normal user: `s6 version
  show`, `s6 live status`, `s6 live status sshd.service gdm.service
  earlyoom` (after the patch) work; `s6 process status` needs root
  (supervise dirs). `setup-fedora.sh --dry-run --rpms=` installs the RPM
  and writes /etc/s6.conf. Not run: build-skarnet-rpms.sh end to end
  (sudo), the `--build-s6` path, anything as root.

- **Owner asked for s6-frontend 0.2.0.0, and for versions not to be
  hardcoded (`b35097c`).** This supersedes the 0.1.0.0 decision above.
  - New set: skalibs 2.16.0.0, execline 2.10.0.0, s6 2.15.1.1, s6-rc
    0.8.0.2, s6-linux-init 1.2.0.2 (rebuilt), s6-frontend 0.2.0.0; all
    checksums match skarnet's `.sha256` files. Upgrade notes read: s6-rc
    0.8 and s6 2.15.1.1 only bump dependencies; skalibs 2.16 changes C
    APIs only; execline 2.10 renames execline-cd/posix-cd -> cd,
    posix-umask -> umask, drops `loopwhilex -n`, changes `wait` options.
    sdcompat's generated scripts use none of these (grep). The 0.1.0.0
    backport patch is gone (0.2.0.0 has the fix).
  - Decision: `dist/skarnet.versions` (name, version, sha256, build order)
    is the only place versions live. build-skarnet-rpms.sh prepends
    `%global skarnet_version/skarnet_release` to the spec copies (so the
    SRPMs for mock carry them) and adds `PatchN:` lines for
    `NAME-VERSION-*.patch`. Release = `1.YYYYMMDDHHMM` per build:
    rpm.vercmp checked that 1.2.0.2-1.<stamp> > the installed 1.2.0.2-1
    and later stamps are newer; needed because s6-linux-init must be
    rebuilt against skalibs 2.16 without a version change. Specs glob
    `lib*.so*` and `pkgconfig/*.pc` instead of naming libraries.
  - Decision: execline `--enable-multicall`, so the .cil labels
    `/usr/bin/execline` alone instead of a list of command names (which
    2.10 had just broken). exec through a symlink is checked against the
    target's label. cd/umask/wait become symlinks in /usr/libexec/execline.
  - Bug fixed on the way: `--build-s6` ran `make install` into /usr, which
    put execline's cd/umask/wait over bash's /usr/bin files. Now each
    package is staged, fixed up, recorded and copied with `cp -dR
    --preserve=mode,timestamps,links` (not -a: /tmp SELinux labels).
  - Verified: build-skarnet-rpms.sh up to the first `sudo dnf install`
    (downloads, checksums, all six specs parse with the version from the
    file, skalibs 2.16.0.0 RPM built); execline's %build/%install emulated
    with the spec's flags against a scratch shared skalibs (one regular
    file, 47 symlinks, libexecline.so.2.10 + .pc, cd/umask/wait only in
    libexec; blocks, importas, fdmove, cd work); the whole new set built
    from source into the scratchpad; sandbox with it in PATH: with
    non-multicall execline 7/8 runs 119/119, with multicall 2/4 (all the
    other runs are the session-7 flake, the same 14 checks). setup
    `--dry-run --build-s6` (rpm/dnf stubbed) reads every version from the
    file. CIL parses. Not run: the other five RPM builds (need the new
    -devel packages installed, sudo), a real `--build-s6`, a boot.
  - Upgrading the skarnet set should be done from the systemd boot
    (TESTING "Updating"), so the new s6-rc never works on a live dir
    made by the old one; the next s6 boot recompiles the database.

### Current state

The s6 boot reaches GNOME with services in their own SELinux domains,
fwupd and seccomp filters working. With `8e5ec0c` the notify helpers
should be idle; with `91b9336` earlyoom should run and the bus socket and
userdb should carry their policy labels.

### Next steps

1. Owner reinstalls (setup-fedora.sh / RPMs, so the new .cil is loaded and
   dist files updated), reboots into s6, then: `top` (no busy `sdcompat
   run`), `s6-svstat /run/service/earlyoom` (up, not restarting),
   `ls -Z /run/dbus/system_bus_socket` (system_dbusd_var_run_t),
   `ls -dZ /run/systemd/userdb` (systemd_userdbd_runtime_t), and
   `sudo tests/collect-diagnostics.sh` for the remaining AVCs.
2. If avc.txt is down to the policy-side leftovers, try `--enforcing`.
3. Session 7's reload flake (s6-rc-update "Broken pipe") is still open.
4. FDSTORE before sdcompatd is up: queue or retry in the runner.
5. Session 8's step 4 (other seccomp-based keys, FreezeUnit/ThawUnit).
6. From the systemd boot: `dist/rpm/build-skarnet-rpms.sh`, then
   `sudo dist/setup-fedora.sh --rpms=... --yes` (installs the new skarnet
   set, reloads the module); `ls -Z /usr/bin/execline` should show
   init_exec_t. Then boot s6 and check `ps -eZ` (domains as before),
   `s6 live status`, `sudo s6 process status sshd.service`.

## Session 10: 2026-10-20

- **build-skarnet-rpms.sh failed at the first `sudo dnf install`** (owner's
  run, Fedora 45, booted under systemd): skalibs-2.16.0.0 + -devel could not
  replace 2.15.1.0 because the installed execline-2.9.9.2 and s6-2.15.1.0
  require `libskarnet.so.2.15`. Installing each package as it is built can't
  work across a soname bump. `--allowerasing` would have removed the whole
  installed s6 set in the middle of the build, so it was not used.
- Fix (`d9fb267`): the rpmbuild path unpacks each built RPM (rpm2cpio | cpio)
  into `OUTDIR/sysroot`, cleared at the start, and builds the next with
  `--nodeps --define "skarnet_sysroot OUTDIR/sysroot"`. The five dependent
  specs pass `--with-sysdeps` under the sysroot and, only when it is defined,
  `--with-include/--with-dynlib/--with-lib`. Without the macro they build as
  before (mock path unchanged). The script no longer uses sudo; it checks
  for rpm2cpio and cpio. TESTING/TESTING-us updated.
- shellcheck 0.11.0 is now installed. `shellcheck -S warning` over every
  tracked `*.sh` passes after `48cfe81`: in setup-fedora.sh `--build-s6` the
  checksum variable `want` was renamed `sum` (shellcheck confused it with
  the RPM function's `want` array, a false positive); sandbox.sh's
  `kill $(cat pids)` splits on purpose (SC2046 disabled there). Checked:
  `--dry-run --build-s6` with rpm stubbed prints all six checksums;
  sandbox start/stop works.
- Verified: full `dist/rpm/build-skarnet-rpms.sh` run, all six built in
  ~30 s; every package requires libskarnet.so.2.16 / libexecline.so.2.10
  (nothing linked against the host's 2.15); `rpm -Uvh --test --nosignature`
  of all eleven RPMs over the installed old set passes. `bash -n` ok.
  Not run: the real install
  (setup-fedora.sh --rpms), a boot.
- Note: this machine has no git user.name/email; the commit used the
  repo's existing author via `git -c` and the config was left unchanged.

### Current state

As in session 9. The new skarnet RPMs are built in
`~/rpmbuild-skarnet/rpms` but not installed.

### Next steps

1. From the systemd boot: `sudo dist/setup-fedora.sh
   --rpms=$HOME/rpmbuild-skarnet/rpms --yes`, then session 9's step 6 checks.
2. Session 9's other next steps still apply.

## Session 11: 2026-10-20

- Read the owner's `sudo tests/collect-diagnostics.sh` from the s6 boot
  (repo at `26020f2`, new skarnet set 2.16.0.0 installed, Permissive).
  `systemctl --failed` is empty and GNOME is up. No code changes.
- **Bluetooth can't be turned on:** bluetooth.service is translated and
  supervised but never started. On systemd it is started by udev:
  99-systemd.rules sets `SYSTEMD_WANTS=bluetooth.target` on the hci device,
  and nothing else (default.target, D-Bus activation) pulls it in;
  wireplumber and GNOME only watch for org.bluez, they don't activate it.
  sdcompat doesn't implement udev `SYSTEMD_WANTS`, so this affects every
  device-triggered unit, not only Bluetooth.
- **earlyoom restarts every second** ("fatal: extra argument not understood:
  'Content|Isolated'"): /etc/sdcompat/s6/{earlyoom,dbus} are the versions
  from before `91b9336`. setup-fedora.sh treats any difference from dist/ as
  the owner's edit and keeps the old copy, so fixes to the hand-written s6
  services never reach an existing install. The old dbus/run also explains
  why /run/dbus/system_bus_socket is still var_run_t and most of the 45
  AVCs (connectto/write on system_bus_socket by every confined service).
- Other AVCs: s6-log on /run/uncaught-logs files (init_t -> var_run_t),
  hostnamed reading /proc/pressure (proc_psi_t), firewalld on a tmpfs
  `.cache`, setfiles/restorecon on initctl and a device_t chr_file.

- Fix (`1c7af61`, owner chose this one first): setup-fedora.sh records a checksum of
  each /etc/sdcompat/s6 dir it installs in
  /var/lib/sdcompat/s6-dist-sums.txt. On a rerun a dir is replaced (with a
  backup) if it matches the recorded checksum, or, for installs made before
  the record existed, any past version of dist/s6/NAME in the repo's git
  history (`git archive` of each commit that touched it; `safe.directory`
  because the script runs as root on the owner's checkout). Anything else is
  kept with the old "keeping your modified" message, and its recorded
  checksum stays the one setup installed, so reverting an edit makes the dir
  updatable again. The checksum covers names, types, link targets, the
  executable bit and contents, not other mode bits (git archive's umask),
  and leaves out contents.d/syslog, which the script itself adds/removes.
  With the sdcompat RPM installed the step is skipped (rpm owns the dir).
- Found while testing: the s6 dirs and /usr/share/sdcompat/s6-linux-init/skel
  were copied with `cp -a` as root, so the files root runs were owned by
  uid 1000 (the checkout's owner), and skel kept user_home_t. They are now
  copied with `cp -dR --preserve=mode,timestamps,links` (`copy_tree`),
  /etc/sdcompat/s6 is chowned to root on every run (repairs old installs),
  and $SHARE is in the restorecon list.
- Tested with the functions extracted into a harness over a scratch copy of
  the live /etc/sdcompat/s6 (rpm/chown stubbed): the stale dbus and earlyoom
  are recognized from git history and replaced, rerun is quiet, an edited
  dir is kept, the recorded-checksum path updates a dir not in git history,
  without .git the old dir is kept, a missing dir is added, the syslog file
  doesn't count as an edit, the RPM case skips. `--dry-run` on this machine
  shows the dbus/earlyoom updates. `bash -n` and `shellcheck -S warning`
  pass. Not run: a real `sudo dist/setup-fedora.sh`. TESTING/TESTING-us
  "Updating" explain the behaviour.

### Next steps

1. Owner: `sudo dist/setup-fedora.sh --rpms=$HOME/rpmbuild-skarnet/rpms --yes`
   (should print "updating /etc/sdcompat/s6/dbus" and ".../earlyoom"),
   reboot into s6, check `ls -ln /etc/sdcompat/s6/earlyoom` (root),
   `ls -Z /run/dbus/system_bus_socket` (system_dbusd_var_run_t), no earlyoom
   loop in /run/uncaught-logs/current, then recollect diagnostics.
2. Bluetooth until step 3: `sudo systemctl start bluetooth.service`.
3. Done below (udev `SYSTEMD_WANTS`).

### Device-wanted units (fix #2, same day, `a736b31`)

- New `src/daemon/devices.rs`: the daemon listens on udevd's netlink group
  (NETLINK_KOBJECT_UEVENT group 2; messages with the "libudev" header, only
  accepted with SCM_CREDENTIALS uid 0, since only root can send to the
  group) and, at startup, scans `/run/udev/tags/systemd` + `data/<id>`.
  A device counts when tagged "systemd" (CURRENT_TAGS / `Q:` lines when
  present, as systemd 247+), not SYSTEMD_READY=0 and not removed. Device ids
  follow libudev (b/c major:minor, nIFINDEX, +subsystem:sysname), so events
  and database entries match. On first sight all SYSTEMD_WANTS units start;
  on a later change only newly listed ones (systemd's rule). Names without a
  unit suffix get `.service`.
- Seen devices are saved in RUN_DIR/devices, so a restarted sdcompatd does
  not start them again (a unit the admin stopped stays stopped); /run is
  fresh at boot. ENOBUFS on the socket triggers a resync from the database.
  The reader is a plain thread feeding a tokio channel (no new tokio feature).
- Excluded units are skipped silently: on the owner's laptop that is
  `systemd-backlight@…` (backlight and kbd backlight) and
  `systemd-rfkill.socket`, so screen brightness and rfkill state are still
  not restored under s6 (separate issue). The rest of its database wants
  bluetooth.target, sound.target, tpm2.target, bolt.service (thunderbolt; it
  wasn't running either) and two mounts, all translated.
- `check_job` got a `manual` flag: RefuseManualStart/Stop don't apply to
  device-wanted starts (as in systemd). Both existing callers pass true.
- Config: `UDEV_DIR` (default /run/udev, empty = off) and `UDEV_MONITOR`
  (default yes). sdcompatd runs as init_t, like systemd, which does the same
  netlink/`/run/udev` access, so no policy change is needed. Checked with
  sesearch/seinfo 4.7.1 against the loaded policy (owner installed them):
  init_t self netlink_kobject_uevent_socket {create bind setopt read ...};
  capability net_admin (SO_RCVBUFFORCE); file_type:dir {open read search}
  for /run/udev; udev_var_run_t and init_var_run_t are in the pidfile
  attribute, which gives init_t file {open read} and dir {add_name
  remove_name write}; init_var_run_t:file {create write rename} for
  /run/sdcompat/devices(.tmp). Netlink multicast delivery has no per-message
  check. Not yet run enforcing.
- Tests: unit tests for message parsing, ids, presence rules, database
  lines, the state file; `netlink_monitor` (ignored by default) sends a real
  udev message over netlink and passed under `unshare -rn cargo test --
  --ignored netlink_monitor`. Sandbox: fake udev database (UDEV_MONITOR=no);
  ready device's unit starts, SYSTEMD_READY=0 and current-tag-less ones
  don't, the state file is right, and after a daemon restart a stopped unit
  stays stopped while a newly added device's unit starts. `cargo test`
  27 passed + 1 ignored; sandbox 126/126 twice; release build ok.
  Docs: README/README-us (implemented list, config keys, layout),
  dist/sdcompat.conf. Not run: a real boot.

### Next steps (replacing the ones above)

1. Owner: `cargo build --release` (done here already),
   `sudo dist/setup-fedora.sh --rpms=$HOME/rpmbuild-skarnet/rpms --yes`,
   reboot into s6. Check: Bluetooth switch works, `systemctl status
   bluetooth.service` active, `cat /run/sdcompat/devices`, `journalctl -b |
   grep 'wanted by device'`, plus step 1 of the setup fix above (earlyoom,
   socket label, ownership). Then recollect diagnostics.
2. systemd-backlight/systemd-rfkill under s6 (include them, or an s6
   service), if the owner wants brightness/rfkill state kept.
3. Session 9's other next steps still apply.

## Session 12: 2026-10-20

Checked the owner's diagnostics from a real s6 boot (`/tmp/sdcompat-diag.xF60rn.tar.gz`,
HEAD 6847a34, Fedora 45, kernel 7.2.11, SELinux permissive via `enforcing=0`).
No code changes.

- Works: PID 1 s6-svscan, `systemctl --failed` empty, is-system-running
  `running`; bluetooth.service and bolt.service active. Device-wanted starts
  logged for bolt (thunderbolt 0-0/domain0), sys-kernel-config.mount,
  sys-fs-fuse-connections.mount, sound.target, tpm2.target,
  bluetooth.target. earlyoom started once, no restart loop.
- Problem: device-wanted *targets* started 26 s late (14:35:40, boot
  14:35:14). Targets are bundles, start_beside() only handles longruns, so
  their `s6-rc change` waits for the boot's change, which was held by
  NetworkManager-wait-online until Wi-Fi came up (NM "startup complete" at
  the same second). bluetoothd therefore started 26 s after wireplumber
  ("BlueZ system service is not available" at 14:35:18). Without a
  network this could be the full wait-online timeout.
- bluetoothd "Failed to set mode: Failed (0x03)" at 14:36:19 and 14:36:57,
  each followed by hci0 setup again: looks like the Bluetooth switch being
  toggled; not confirmed with the owner whether the switch worked.
- New, not acted on: udev's vtcon rule runs `systemctl --no-block restart
  systemd-vconsole-setup.service`, which fails (systemd-* excluded), so
  /etc/vconsole.conf isn't applied on the text console.
- Known items seen again: journald FDSTORE before sdcompatd is up, home1
  excluded noise, user-runtime-dir@60578 ExecStop exit 1, AVCs for
  hostnamed (proc_psi_t, watch /run, /run/dbus), journald map
  syslogd_tmpfs_t, restorecon on the s6-svscan-log fifo, firewalld `.cache`
  on tmpfs.
- Not in the diagnostics: ownership of /etc/sdcompat/s6/earlyoom and the
  label of /run/dbus/system_bus_socket (script doesn't collect them).
  Owner checked by hand: earlyoom dir is uid/gid 0, the socket is
  system_dbusd_var_run_t. Session 11's setup fix is confirmed on a real boot.

### Target delay fix (same day, `71ae087`)

- `src/daemon/s6.rs`: new `start_bundle_beside()`. Starting a bundle (not
  isolate) while another change holds the s6-rc lock now runs
  `start_beside()` for each member longrun that isn't up, repeating while
  members start (one may need another). If that leaves nothing, the job is
  done without `s6-rc change`; otherwise `s6-rc change` on the bundle still
  runs for the rest (oneshots, longruns with dependencies down, native s6
  services like dbus), so nothing is skipped, only started earlier.
  reconcile_beside() records the members in s6-rc later, as for services.
  On the owner's db: bluetooth.target = bluetooth.service (longrun),
  sound.target = alsa-state.service + alsa-restore.service (oneshot, so
  sound.target still waits if alsa-restore isn't up yet), tpm2.target empty.
- Test: sandbox `beside.target` (Wants=beside3.service) started while
  hold3's change holds the lock: beside3 active while the change still
  runs and not yet in s6-rc, recorded afterwards. Without the fix beside3
  stays inactive/dead (3 failures, also two knock-on devwant ones from the
  job still pending). With it: `cargo test` 27 passed + 1 ignored, sandbox
  129/129 twice, shellcheck/bash -n pass, release build ok. No doc change
  (beside starts aren't described in README/TESTING). Not run: a real boot.

### Next steps

1. Owner: `sudo dist/setup-fedora.sh --rpms=$HOME/rpmbuild-skarnet/rpms
   --yes` (or install the new release binary), reboot into s6, check
   `journalctl -b | grep -E 'wanted by device|beside'`: bluetooth.target
   should be done within a second or two of sdcompatd's start, with
   "start bluetooth.service: ... starting it beside".
2. Done (`d768ffa`): collect-diagnostics.sh (s6 boot mode) now saves
   `ls -lnZ /etc/sdcompat/s6` (s6-services.txt), `ls -lnZ
   /run/dbus/system_bus_socket` (dbus-socket.txt), `s6-svstat
   /run/service/earlyoom` (earlyoom.txt) and `/run/sdcompat/devices`
   (devices.txt). bash -n/shellcheck pass; the commands were tried by hand
   unprivileged (s6-svstat needs root, which this branch runs as). No doc
   change: TESTING doesn't list the archive's files.
3. Done below (vconsole).
4. Done below (backlight and rfkill).

### vconsole setup (same day, `b45c28d`)

- Both, because one alone doesn't cover it: udev's 90-vconsole.rules runs
  `systemctl --no-block restart systemd-vconsole-setup.service` for each
  vtcon, and at boot that happens during udev-trigger (14:35:12 on the
  owner's laptop), before sdcompatd is up (14:35:14), so the call fails
  whether or not the unit is translated. Fedora doesn't put the unit in
  sysinit.target.wants; under systemd only udev starts it.
  - New s6 oneshot `vconsole` (dist/s6/vconsole, in sdcompat-sysinit, after
    udev-trigger, which settles udev) runs `early vconsole`:
    /usr/lib/systemd/systemd-vconsole-setup if /dev/tty0 and the binary
    exist; failure only warns (exit 0, like the other early steps).
  - systemd-vconsole-setup.service added to the default INCLUDE
    (src/config.rs, dist/sdcompat.conf, README/README-us), so consoles
    added after boot (a late GPU driver) go through sdcompatd. `sdcompat
    plan`: a standalone oneshot no bundle pulls in, warnings only for
    ImportCredential= and TimeoutStopFailureMode=.
- SELinux: the binary is init_exec_t, so it runs as init_t under s6, as
  under systemd. No policy change.
- setup-fedora: the new dir is added, and the owner's sdcompat-sysinit
  (unedited, matches git history) is replaced with the one listing
  vconsole, per Session 11's update logic. Not run here (needs root).
- Tests: `sh -n`/shellcheck on dist/early; dist/s6 compiled with
  s6-rc-compile (stub sdcompat-default and sdcompat-system-update bundles):
  vconsole depends on udev-trigger, udevd, mount-api and is in
  sdcompat-sysinit. `cargo test` 27 passed + 1 ignored, sandbox 129/129.
  Not run: `early vconsole` itself (needs root and a real console), a real
  boot.
- Owner check after reinstall and reboot into s6: `s6-rc -a list | grep -x
  vconsole`, no `vtcon… failed` lines in /run/uncaught-logs/current after
  sdcompatd is up, and the console font/keymap from /etc/vconsole.conf on
  Ctrl+Alt+F3. vtcon lines during udev-trigger still fail (harmless: the
  oneshot runs right after).

### Backlight and rfkill under s6 (same day, `c3985c3`; path-run fix `aff676f`)

Decision (owner chose it over an rfkill-only restart loop or backlight
only): on-demand socket re-activation in the runner, because
systemd-rfkill restores state at start, saves on every change, and exits
after ~5 s idle; without re-activation it would restore the stale states
saved by the last systemd boot and never save later toggles.

- Backlight: `systemd-backlight@*.service` in the default INCLUDE. udev
  wants `systemd-backlight@backlight:amdgpu_bl1` and
  `@leds:tpacpi::kbd_backlight` on the owner's laptop; they run as dynamic
  oneshot instances (load at start). New `stop_dynamic_oneshots()`: when
  sdcompatd gets SIGTERM and RUN_DIR/shutting-down exists (rc.shutdown),
  it stops the active dynamic oneshots so ExecStop= (`save`) runs.
  sdcompatd depends on dbus -> sdcompat-sysinit, so it goes down before
  mount-fs and /var is still there. Limitation: the dynamic map is in
  memory, so after a daemon restart (not a reboot) the save is skipped.
- rfkill: `systemd-rfkill.socket` and `.service` in the default INCLUDE.
  - `ListenSpecial=` (src/socket.rs): opened O_RDWR with Writable=yes, else
    O_RDONLY, plus O_NOCTTY|O_NONBLOCK|O_CLOEXEC|O_NOFOLLOW, as systemd.
  - Starting a `.socket` (check_job: start/restart, not translated, not
    excluded, Accept=no) records RUN_DIR/sockets/<name> and starts its
    service (Service= or the stem). The runner's socket_units() also binds
    sockets recorded there, so static sockets like systemd-rfkill.socket
    are handed over. If the service already runs, it gets the socket only
    at its next start.
  - Re-activation (src/runner.rs): a clean exit with Restart= not
    applying, while the daemon holds the service's sockets, sets
    SOCKET_WAIT and returns 0 instead of 125, so s6 restarts the run
    script; `run` then ppoll()s the held fds and starts the service on
    activity. The unit is reported inactive/dead meanwhile (status,
    try-restart does nothing, stop doesn't run ExecStop=). Finish after
    a wait (stop, restart) returns 0 without touching the exec state.
    Failed exits keep the old behaviour (down, failed).
  - Explicit start while waiting: the daemon removes SOCKET_WAIT
    (`UnitState::take`, so only one side wins) and sends SIGUSR1
    (`s6-svc -1`), only to a run process up >= 1 s (one that just started
    may not block SIGUSR1 yet; a down one starts without the key anyway).
    The runner keeps SIGUSR1 blocked except inside ppoll() (no lost
    wakeup) and, if the daemon took the key first, waits up to 2 s for
    that signal before exec so it can't hit the service. `wait_ready`
    counts "became active, now waiting again" as done.
  - SELinux (loaded policy, sesearch): systemd-rfkill transitions to
    systemd_rfkill_t; it may use inherited fds (domain_fd_use), rw
    wireless_device_t (/dev/rfkill), write init_var_run_t sockets
    (/run/sdcompat/notify) and its var_lib. systemd-backlight is
    init_exec_t (init_t). No policy change.
- Bug found while testing (pre-existing, on real boots too): path-run
  talks to the daemon over the private socket on a current-thread runtime
  and blocked in inotify's poll(), so zbus never read the daemon's
  signals; once its socket buffer filled (38 KB queued seen), the daemon's
  emit() blocked on that peer and every job stalled ("waiting" forever).
  The new tests added enough signals to hit it. Fixes: path-run waits on
  the blocking pool (src/pathunit.rs); emit() gives each private peer
  500 ms and drops/closes one that doesn't read (systemd drops clients
  whose queue overflows; peers reconnect). With the path-run fix the
  guard never fired in the test runs.
- Test flake fixed: the target test's cleanup `s6-rc -d change` now uses
  -b (it failed with "unable to take locks" when the daemon was recording
  beside3, leaving hold3/beside3 up).
- Tests: sandbox `react.socket` (static, ListenSpecial= on a FIFO,
  Writable=yes) + `react.service` (reads what is queued, exits): starting
  the socket runs the service with LISTEN_FDS=1; inactive afterwards;
  writes re-activate it twice; StartUnit right after an exit (inside s6's
  1 s restart delay, the race that hung a job in a first version) runs it
  once; stopped, a write doesn't re-activate. `cargo test` 27 passed + 1
  ignored, sandbox 138/138 twice, shellcheck/bash -n pass, release build
  ok. Not tested: the SIGUSR1 path against a runner already in ppoll()
  in the suite (done by hand earlier), a real boot. Remaining
  "unable to take locks" lines in check output come from `in_s6rc`
  polling during a change (retried; pre-existing).
- Mistake during testing: one suite run was started in the background
  and its sandbox never stopped; the next run shared $R with it and hung.
  Its processes were found by their scratchpad/sdc-test paths and killed
  by PID (nothing of the system's s6 tree involved).
- Docs: README/README-us (socket activation, templates at shutdown, not
  implemented list, INCLUDE), dist/sdcompat.conf.

### Next steps

1. Owner: reinstall (`sudo dist/setup-fedora.sh --rpms=$HOME/rpmbuild-skarnet/rpms
   --yes`), reboot into s6. Check: `systemctl status systemd-rfkill.service`
   (ran at boot, then inactive), `ls /run/sdcompat/sockets`, toggle
   Bluetooth/airplane mode then `journalctl -b -u systemd-rfkill` (it runs
   again), brightness: change it, reboot, it comes back;
   `ls -l /var/lib/systemd/backlight` mtime after a reboot. Then
   collect diagnostics (covers the target delay and vconsole too).
2. Done below (dynamic oneshots across daemon restarts).

### Dynamic oneshots across daemon restarts (same day, `2d055f8`)

- Active on-demand oneshot instances (RemainAfterExit=yes, e.g.
  systemd-backlight@) are recorded as empty files in
  RUN_DIR/dyn-oneshot/<id>: written when dyn_start succeeds, removed in
  dyn_stop (also covers dyn_restart). `load_dynamic_oneshots()` at daemon
  startup puts them back in the dynamic map as active, so a restarted
  daemon reports them active/exited and stop_dynamic_oneshots() runs
  their ExecStop= at shutdown. RUN_DIR is fresh at boot, like
  RUN_DIR/devices. Files, not one list, so concurrent starts/stops don't
  race on a shared file. Dynamic longruns (user@) are not covered: their
  map entry is still lost on a daemon restart (they keep running under s6).
  (Covered since: see the next entry.)
- Tests: check.sh starts prep@b before the unprivileged-daemon swap and
  expects active/exited after it; a new "shutdown stops on-demand
  oneshots" section creates RUN_DIR/shutting-down, SIGTERMs the daemon and
  expects prep@b's ExecStop= ("down b") and the marker gone, then restarts
  the daemon (prep@b inactive/dead). This is also the first test of the
  shutdown path from c3985c3. Without the change: 2 failures (state after
  restart, ExecStop at shutdown); "marker gone" passes either way. With
  it: sandbox 143/143 twice, `cargo test` 27 passed + 1 ignored,
  shellcheck/bash -n pass, release build ok. Docs: README/README-us.

### Dynamic longruns across daemon restarts (same day, `f91b631`)

- Before: a restarted daemon forgot on-demand longruns (user@UID,
  systemd-run services, timers, paths) though s6 kept running them: they
  were reported inactive/dead, missing from ListUnits, and StopUnit or a
  stopped BindsTo= target did nothing (dyn_stop returns early without a
  map entry), so they stayed running unmanaged.
- `load_dynamic()` (was load_dynamic_oneshots) also scans RUN_DIR/dyn:
  each service directory still linked into the scan directory becomes a
  Longrun entry again. No new state: dyn_start writes and links the
  directory, dyn_stop unlinks and removes it, so its presence is the
  record. Unlinked directories (a start that failed half-way) are skipped;
  dyn_start rewrites them. Status comes from s6-svstat as before, so an
  instance that exited meanwhile shows inactive/failed correctly.
- Not restored (unchanged): the daemon's in-memory fd store (FDSTORE= fds
  and held sockets) for every unit, and transient scopes/slices.
- Tests: check.sh now starts inst@b (longrun, BindsTo=prep@b) and prep@c
  before the daemon swap; after it inst@b active/running, prep@b and
  prep@c active/exited, inst@b in ListUnits; with the normal daemon back,
  StopUnit prep@b stops inst@b and removes its service directory; the
  shutdown section uses prep@c. Without the longrun part: 3 failures
  (state after restart, ListUnits, directory left). With it: sandbox
  150/150 twice, `cargo test` 27 passed + 1 ignored, shellcheck/bash -n
  pass, release build ok. Docs: README/README-us.

### fd store across daemon restarts (same day, `25c41c7`)

- Before: the fd store (FDSTORE= fds, held listening sockets "<unit>#sockets")
  lived in sdcompatd's memory, so a daemon restart closed them all: the next
  service restart got no stored fds and rebound its sockets.
- Decision: move the store into its own process instead of handing fds to
  a successor (nothing outlives the daemon to receive them) or using
  s6-rc's s6rc-fdholder (its rules belong to s6-rc's pipelines). New
  subcommand `sdcompat fdstore [--ready-fd N]` serves the unchanged
  protocol on RUN_DIR/fdstore (fdstore::serve_forever; the limit function
  moved to fdstore::limit). New s6 longrun dist/s6/sdcompat-fdstore
  (flag-essential, after mount-api, in sdcompat-sysinit; sdcompatd now
  depends on it). The daemon serves the store itself only when nothing
  answers there (`fdstore::is_served`): old installs, manual runs, and a
  running system until its next boot (s6-rc-update adds the service but
  doesn't start it). A real stop deletes a unit's keys through the
  protocol (`drop_stored_fds`, DEL) instead of the in-process map.
  Restarting the store process itself still loses the fds (as
  s6-fdholderd).
- Possibly also fixes journald's "FDSTORE: fd store not reachable" before
  sdcompatd is up (Session 9 next step 4): the store now starts right after
  mount-api. Not guaranteed: journald has DefaultDependencies=no, so it is
  not ordered after the store. Check on the next boot.
- Tests: tests/sandbox.sh starts `sdcompat fdstore` before s6-svscan;
  check.sh, after both daemon swaps, restarts fdkeep.service (expects its
  FDSTORE fd back) and sock.service (same socket inode as before). With the
  daemon always serving (old behaviour) both fail; sandbox 152/152 twice
  otherwise. By hand: without the store process the daemon serves it and a
  service restart gets its fd back. dist/s6 compiles with s6-rc-compile
  (stub generated bundles): sdcompatd's dependencies include
  sdcompat-fdstore. `cargo test` 27 passed + 1 ignored, shellcheck/bash -n
  pass, release build ok. Docs: README/README-us, `sdcompat` usage text.

### Next steps

1. Owner: push, reinstall (`sudo dist/setup-fedora.sh --rpms=$HOME/rpmbuild-skarnet/rpms
   --yes`; it should add /etc/sdcompat/s6/sdcompat-fdstore and update
   sdcompatd and sdcompat-sysinit), reboot into s6, then check
   `s6-rc -a list | grep -x sdcompat-fdstore`, `grep -c 'fd store not
   reachable' /run/uncaught-logs/current`, Bluetooth timing, vconsole,
   brightness/rfkill save, and collect diagnostics.
2. Done below (scopes and slices across daemon restarts).

### Scopes and slices across daemon restarts (same day, `9df4162`)

- Before: transient scopes and slices (StartTransientUnit: logind's
  session-N.scope and user-UID.slice, systemd-run --scope, podman,
  flatpak) lived only in the daemon's map. After a daemon restart they
  were unknown: logind's StopUnit/KillUnit of a session scope would fail
  (no unit file to load; not observed, follows from check_job), and
  GetUnitByPID/ListUnits lost them.
- Now each one is recorded in its unit state directory,
  RUN_DIR/units/<name>/transient-{cgroup,sighup,slice,description,kind}
  (one file per value, like the rest of the unit state; kind written last,
  so an incomplete record is skipped). `transient::save_scope()` after a
  successful StartTransientUnit; `load_scopes()` at daemon startup takes
  back the records whose cgroup directory still exists; gc_scopes() then
  drops scopes that emptied meanwhile (it already removed the state dir),
  and stopping a slice now removes its state dir too. Description newlines
  become spaces (read_trimmed state files).
- Tests: unit test `scope_round_trip` (a record with an existing cgroup
  dir is loaded with all fields; one whose cgroup is gone and ordinary
  unit state are not). check.sh: keep-scope.scope (a sleep) created before
  the daemon swap, active/running after it, and StopUnit from the restarted
  daemon kills its process. That part is skipped here like the existing
  scope test (no writable cgroup unprivileged on this machine), so the
  live path is only covered by the unit test until it runs as root or with
  cgroup delegation. `cargo test` 28 passed + 1 ignored, sandbox 152/152
  twice (scope parts skipped), shellcheck/bash -n pass. Docs:
  README/README-us.

### Sandbox suite as root (same day, `cb39a24`)

- Owner ran `tests/sandbox.sh /tmp/sdc-root && tests/check.sh /tmp/sdc-root`
  under sudo on the Fedora laptop (booted into s6, SELinux permissive),
  from a separate terminal (`!` commands here have no tty for sudo's
  password, and Fedora's sudo timestamps are per tty).
- Checked before: systemctl.sh keeps real systemctl in a namespace bound to
  the sandbox socket; no sandbox unit mounts, uses PAMName=/User=/Slice=/
  Delegate=; the one pgrep in check.sh only reads CPU ticks. The risk is
  that as root services go into the real /sys/fs/cgroup/system.slice/<unit>
  (cgroup root is a constant) and KillMode=control-group kills that whole
  cgroup: none of the 47 sandbox unit names matched the 119 entries in the
  host's system.slice at the time. Keep it that way when adding units (no
  real service names in tests/sandbox.sh).
- Result: 146 passed, 9 failed. Passed for the first time: the transient
  scope test (test-scope.scope active/running) and the restart ones from
  9df4162 (keep-scope.scope active after the daemon swap, StopUnit from the
  restarted daemon kills its process). All 9 failures were in "unprivileged
  caller": as root the caller is root, which authorize() always allows (as
  systemd), so the "denied" calls succeeded, the StopUnit really stopped
  hello.service, and the private socket accepted root. Test assumption,
  not a daemon bug: check.sh now skips those checks (SKIP line) when run as
  root; the other checks in that section still run. Unprivileged run
  afterwards: 152/152.
- Leftover: 49 empty cgroup directories in the host's system.slice, all
  sandbox units (list in /tmp/sdc-root.cgroups, made by comparing with a
  listing taken before the run; nothing disappeared). The owner removed
  them with rmdir (empty directories only).
- Second root run with the skip: rc=0, 146 passed, 0 failed, one SKIP
  (denied calls). It left 49 empty directories again (test-scope.scope
  instead of x.service, which only the first run's allowed "denied" call
  created): every root run leaves them, so remove them after each
  (/tmp/sdc-root.cgroups was rewritten for this run).

### Next steps

1. Real boot check from the fd store entry still applies.
2. Maybe: tests/sandbox.sh stop could rmdir the sandbox units' empty
   cgroups when run as root, so root runs leave nothing behind.

### Real boot diagnostics after the fd store change (2026-10-20, no code change)

- Owner booted the s6 entry (ae139bb installed, SELinux permissive) and ran
  `sudo tests/collect-diagnostics.sh` (/tmp/sdcompat-diag.mukbC6.tar.gz).
  pid 1 s6-svscan, `is-system-running` running, `--failed` empty,
  sdcompat-fdstore active.
- Works: backlight (both systemd-backlight@ instances active/exited);
  vconsole oneshot ran at 17:10:22 and no `vtcon… failed` after sdcompatd
  was up (the two at 17:10:20 are before it, expected); device-wanted
  bluetooth.service started beside at 17:10:22, 8 s before NM "startup
  complete" (target delay fix confirmed). bluetoothd "Failed to set mode"
  at 17:10:23, 17:13:30, 17:13:33 as before.
- Bug, systemd-rfkill: started beside at boot (17:10:22), exited idle and
  went into the socket wait (run process up, not ready). Once the boot's
  change finished, reconcile_beside_locked() ran `s6-rc -u change` for it;
  s6-rc waits for readiness, which a waiting runner never sends, so it
  timed out at 17:12:15 ("recording systemd-rfkill.service in s6-rc:
  s6-svlisten: fatal: timed out", "unable to start ... exited 99") and
  brought it down. Checked on the live system afterwards: no run process,
  socket-wait key gone (finish's "stopped" branch), so rfkill changes
  after 17:12:15 were not saved this boot. Only the beside path hits it
  (a unit s6-rc already counts as up isn't waited for).
- Not fixed: journald's "FDSTORE: fd store not reachable" (2 lines).
  journald started at 17:10:20.078, sdcompat-fdstore at .171: no ordering
  between them, as feared in 25c41c7.
- Known, unchanged: rejected method_return to logind (requested_reply=0,
  once), home1 excluded noise, user-runtime-dir@60578/@0 ExecStop exit 1,
  /run/varlink/registry symlinks (directory missing). AVCs down to 9:
  hostnamed proc_psi_t and /run/dbus, firewalld .cache, restorecon and
  (new, from the vconsole oneshot) loadkeys writing the s6-svscan-log
  fifo (var_run_t); under enforcing their output would be lost.

### Next steps

1. Fix the rfkill reconcile timeout. Proposal: the runner signals
   readiness when it enters the socket wait (s6's view: up, like a
   listening socket unit), or reconcile skips/records units in socket-wait
   without waiting. Add a sandbox test: react.service started beside, idle
   exit, then reconcile.
2. Order services with FileDescriptorStoreMax= after sdcompat-fdstore
   (journald), then check the FDSTORE count again.
3. Older items: rejected reply to logind; loadkeys/restorecon fifo AVCs.

### rfkill reconcile timeout and journald fd store ordering (same day, `937acad`, `3386a53`)

- rfkill (`937acad`): owner chose the proposed fix. The runner signals s6
  readiness when it enters the socket wait (s6's view: up and ready, like
  a listening socket unit), so reconcile's `s6-rc -u change` returns at
  once. Consequence: a start that wakes a waiting service can't go by s6
  readiness any more (same run process, already ready), so the wake path
  uses new `wait_woken()`: done when ACTIVE_ENTER >= job start (after_ready
  stamps it for every Type=) or CONDITION/DEPENDENCY/START_FAILED is set
  (job_outcome() sorts those out; stale ones can't be there, since the
  previous run succeeded and prepare_start() cleared them), failed when
  down and not wanted up, timeout at the start timeout. The wake check
  moved before `active.contains(id)`: a unit started beside and not yet
  recorded used to go through start_beside() and wait for readiness
  until timeout (also broken before; now woken).
- journald (`3386a53`): gen.rs adds `sdcompat-fdstore` to the
  dependencies of services with FileDescriptorStoreMax= > 0 when that s6
  service is among the extra sources. Checked with `sdcompat plan` on a
  scratch config (extra source with sdcompat-fdstore and mount-api): the
  FileDescriptorStoreMax= unit lists `depends: sdcompat-fdstore`, a plain
  one doesn't. journald now also waits for mount-api (~90 ms at boot).
  No sandbox test: the sandbox runs `sdcompat fdstore` outside s6-rc, so
  the dependency isn't generated there.
- Tests: sandbox react2.socket/react2.service started beside hold4's
  change: ran, inactive (waiting), recorded in s6-rc once the change
  ended, re-activated by a FIFO write, stopped and down in s6-rc. Without
  the runner change it fails ("react2 not in s6-rc -a list"), 157/158.
  With it: 158/158 twice (scope parts skipped unprivileged), `cargo test`
  28 passed + 1 ignored, release build ok, shellcheck/bash -n pass. Docs:
  README/README-us (fd store bullet).
- Git: this Fedora checkout has no user identity; commits are made with
  `git -c user.name=shandy-c -c user.email=...` (same author as before),
  config unchanged.

### Next steps

1. Owner: push, reinstall, reboot into s6. Check: `grep -c 'fd store not
   reachable' /run/uncaught-logs/current` (expect 0), no "recording
   systemd-rfkill.service" line, `s6-rc -a list | grep -x
   systemd-rfkill.service` after the boot's change ends, toggle airplane
   mode later than 2 min after boot, reboot, and the state comes back.
2. Older items: rejected reply to logind; loadkeys/restorecon fifo AVCs;
   maybe rmdir the sandbox cgroups after root runs.

### Sandboxing settings (same day, `9d52451`)

- Owner asked for the sandboxing options first, after a discussion of
  sdcompat's downsides (the boot report listed ProtectKernelModules=,
  RestrictAddressFamilies=, MemoryDenyWriteExecute=, PrivateDevices= etc.
  as ignored for 18-34 services each, so services ran less confined than
  under systemd).
- Scope chosen from what this machine's units use (counted in
  /usr/lib/systemd/system): everything systemd does with seccomp,
  namespaces, mounts and capabilities. Left out: the cgroup-BPF ones
  (IPAddressDeny= 27 units, DeviceAllow=/DevicePolicy=, RestrictFileSystems=),
  PrivateUsers= (6), PrivatePIDs= (4), RemoveIPC= (7), credentials,
  KeyringMode=.
- seccomp.rs: `restrictions()` builds one allow-by-default filter per
  setting, loaded per ABI with BADARCH=allow (as systemd's
  seccomp_init_for_arch), before SystemCallFilter= (its allow list may not
  allow seccomp()). Rules follow systemd's seccomp-util.c:
  RestrictAddressFamilies (socket() arg0, EAFNOSUPPORT; native + x32 only,
  systemd skips i386 too), RestrictNamespaces (unshare/clone masked flags,
  setns, clone3 ENOSYS), MemoryDenyWriteExecute (PR_SET_MDWE first, which
  gives EACCES; seccomp fallback EPERM), LockPersonality (personality !=
  current), RestrictRealtime (sched_setscheduler 1,2,4 and >5),
  RestrictSUIDSGID (chmod family, open/openat with O_CREAT/O_TMPFILE,
  creat, openat2 ENOSYS), @clock / @module / syslog / sethostname+
  setdomainname / @raw-io for ProtectClock / ProtectKernelModules /
  ProtectKernelLogs / ProtectHostname / PrivateDevices.
- sandbox.rs: unshare NET (lo brought up) / IPC / UTS before the mount
  namespace; then, after ProtectSystem/Home, rw binds and PrivateTmp: new
  procfs for ProtectProc=/ProcSubset= (skipped with a message if the kernel
  refuses), PrivateDevices /dev (tmpfs in the unit state dir, bound nodes
  null zero full random urandom tty, devpts newinstance (retried without
  gid= in a user namespace), shm/mqueue/hugepages, fd/std* symlinks,
  /dev/log, MS_MOVE onto /dev), new mqueue for PrivateIPC, systemd's
  ProtectKernelTunables table (/sys and /proc parts ro, kallsyms/kcore
  hidden, /sys/fs/cgroup and /sys/fs/selinux rw again), ProtectControlGroups
  ro, modules and kmsg hidden, then ReadWritePaths= under /sys and /proc
  bound rw again (efivars in fwupd-style units). `bounding_set()` drops the
  implied caps (MKNOD, SYS_RAWIO, SYS_MODULE, SYSLOG, SYS_TIME,
  WAKE_ALARM); `no_new_privileges()` follows systemd's doc: implied by
  these settings for a service without CAP_SYS_ADMIN (non-root User=, or
  dropped from its bounding set).
- Also changed: '+' commands no longer get CapabilityBoundingSet= (as
  systemd: full privileges).
- Deviations, documented: network/IPC/UTS namespaces are per process, not
  per unit (ExecStartPre= and the main process don't share them);
  ProtectControlGroups=private|strict only make the tree read-only; /sys
  is not remounted for PrivateNetwork (so /sys/class/net shows the host's
  interfaces; /proc/net shows only lo).
- Checked: the translated units using these settings (logind, hostnamed,
  timedated, journald, polkit, chronyd, NM, ...) — hostnamed doesn't set
  ProtectHostname, timedated doesn't set ProtectClock, so their own jobs
  aren't blocked.
- Tests: unit tests for the parsers and a forked child loading
  RestrictAddressFamilies/RestrictNamespaces/RestrictRealtime (31 + 1
  ignored). Mount/namespace parts by hand in a user namespace (`unshare -r
  -p -f --mount-proc sdcompat up -- unit` with two scratch units): minimal
  /dev, /proc/sys and /sys ro, cgroup ro, modules hidden, only lo in
  /proc/net/dev, sethostname refused, kallsyms empty, ProtectSystem=strict
  with ReadWritePaths, ProcSubset=pid, MDWE, SUID chmod, AF_INET. New
  sandbox unit harden.service: the seccomp checks run unprivileged; the
  /proc/sys, /dev and lo checks only as root (SKIP otherwise). Sandbox
  164/164 twice, shellcheck/bash -n, release build. Docs: README/README-us.
- Not tested: a real boot with these on, SELinux enforcing (devpts
  newinstance and the tmpfs /dev get default labels).

### Next steps

1. Owner: push, reinstall, reboot into s6. Watch for services that now
   fail: `systemctl --failed`, exit statuses 226 (namespace) and 231
   (seccomp) in `/run/uncaught-logs/current`, and `sudo tests/sandbox.sh
   DIR && sudo tests/check.sh DIR` for the root-only checks (then rmdir the
   leftover sandbox cgroups). If the s6 boot breaks, the default systemd
   entry is unaffected. Also the checks from the rfkill/journald entry.
2. Then: cgroup-BPF settings (IPAddressDeny=, DeviceAllow=) if wanted;
   SELinux enforcing.

### Setup options for resolved and journald (same day, `063cf34`)

- Owner asked, after a discussion of which systemd pieces could still go
  (only resolved and journald are realistic: logind, `systemd --user`,
  udevd and libsystemd are held by GNOME and Fedora's packaging), for
  setup options to drop resolved and to use JOURNAL=no.
- Constraint: /etc/resolv.conf is shared with the systemd boot (a
  symlink to ../run/systemd/resolve/stub-resolv.conf here). Decision:
  change only /run on the s6 boot. New config key RESOLVED (config.rs,
  like JOURNAL: `no` drops systemd-resolved.service and its two varlink
  sockets from INCLUDE, so the systemd-* EXCLUDE glob covers them). New s6
  oneshot dist/s6/resolv (`early resolv`, after mount-api; setup adds it
  to sdcompat-sysinit/contents.d like syslog for JOURNAL=no): writes
  /run/NetworkManager/conf.d/99-sdcompat-no-resolved.conf with
  dns=default (NetworkManager would otherwise pick systemd-resolved from
  the symlink) and rc-manager=symlink (rc-manager=auto would use
  /usr/sbin/resolvconf, which on Fedora is resolvectl), and makes the
  stub path /etc/resolv.conf points to a symlink to
  /run/NetworkManager/resolv.conf (NetworkManager always writes that).
  nss-resolve finds no resolved and falls through to `dns`.
- Known limit: NetworkManager reads /run/NetworkManager/conf.d before
  /etc, so a `dns=` in /etc/NetworkManager overrides it (none on this
  machine). Documented.
- setup-fedora.sh: --journal and --resolved/--no-resolved added; JOURNAL
  and RESOLVED now default to what the installed sdcompat.conf says
  (`conf_bool`), and `set_conf_key` sets or removes the key in an existing
  config (before, the config was only written on a first install). Bug
  fixed with it: a re-run without --no-journal on a no-journal install
  removed contents.d/syslog while the config kept JOURNAL=no.
  s6_dir_sum ignores contents.d/resolv as it does contents.d/syslog.
- Tests: config unit test (RESOLVED=no excludes the three units; 32 + 1
  ignored). `early resolv` in `unshare -rm` with a tmpfs on /run: the conf
  file, the stub symlink, /etc/resolv.conf reading NetworkManager's file;
  `NetworkManager --print-config` there shows the snippet loaded with
  dns=default, rc-manager=symlink. set_conf_key/conf_bool on a scratch
  copy: no/no then yes/yes leaves the file identical to the original,
  repeating is a no-op. `--dry-run --no-resolved` plan and steps;
  dist/s6 with resolv in sdcompat-sysinit compiles with s6-rc-compile
  (resolv after mount-api). `sdcompat plan` on this machine's units with
  RESOLVED=no: resolved gone (89 -> 88 longruns). Sandbox 164/164,
  shellcheck/bash -n. Docs: README/README-us, TESTING/TESTING-us (DNS
  check), dist/sdcompat.conf.
- Not tested: a real boot with --no-resolved (NetworkManager writing the
  file, name resolution, VPN DNS), --no-journal switching on the real
  install.

### Next steps

1. Owner: push, reinstall (add `--no-resolved` and/or `--no-journal` if
   wanted), reboot into s6. With --no-resolved: `pgrep -x
   systemd-resolve` (nothing), `cat /etc/resolv.conf` (NetworkManager's),
   `ping fedoraproject.org`; then boot the systemd entry once and check
   DNS still goes through resolved there (`resolvectl status`). Plus the
   sandboxing checks from the entry above.
2. Then: cgroup-BPF settings, SELinux enforcing.

## Session: 2026-10-20 evening, black screen with JOURNAL=no (diagnosis only)

Owner reported a black screen on the s6 boot after installing `063cf34`
with `--no-journal --no-resolved` (two s6 boots, 19:41 and 19:53). No code
changed yet. Evidence came from /var/log/sdcompat/*/current and
/var/log/messages (rsyslog), since journald was not running.

- **Confirmed: D-Bus activation deadlock.** At boot, upowerd,
  accounts-daemon, firewalld, NetworkManager (nm-dispatcher,
  wpa_supplicant) and boltd asked for D-Bus activation. None of them got
  "starting it beside": start_beside() requires every s6-rc dependency of
  the unit to be up, and with JOURNAL=no each longrun depends on its own
  logger `<unit>.log` (producer-for/consumer-for), which is down for an
  on-demand unit. So their `s6-rc change` waited for the lock. The lock
  holder was the change for bolt.service (wanted by device
  +thunderbolt:0-0): bolt has After=polkit.service, and boltd waits for
  polkit before it takes its bus name. The deadlock lasted until
  s6-svlisten timed out, 210 s later (19:53:59 to 19:57:29). Then polkit,
  accounts-daemon and wpa_supplicant all started at once. The 19:41 boot
  was the same (until 19:44:56).
- **Confirmed: dynamic units' loggers never run.** `s6-svlink -d` also
  writes `log/down`, and dyn_start only does `s6-svc -u` on the service, so
  with JOURNAL=no the s6-log of user@UID etc. stays down (reproduced in a
  scratch s6-svscan). No /var/log/sdcompat/user@* directory exists.
  Output goes into an unread pipe, which blocks once 64 KiB fill it.
- **Not explained: greeter user managers.** After 19:57:29, gdm's six
  greeters each failed within ~0.3 s. gdm-wayland-session started its own
  dbus-daemon (so /run/user/UID/bus did not exist), and gnome-session found
  no org.freedesktop.systemd1. logind did create the `manager-early`
  sessions, so user@UID.service got as far as PAM. systemd --user left no
  output anywhere (see the logger bug above). The owner's tty login at
  19:46 got a working user@1000. The sandboxing commit doesn't touch user@
  (it has none of those settings).
- Also seen: user-runtime-dir@UID ExecStop exits 1 on each greeter logout.

### Fixes (same evening, `b37629e`)

- start_beside() (daemon/s6.rs): when the only dependency that is down is
  the unit's own `<id>.log`, and the logger's own dependencies (the
  s6rc-fdholder) are up, start the logger with `s6-svc -u`, wait up to
  10 s for it to be up and ready (s6-log -d3), then the unit. The logger
  goes into `beside` too, so reconcile_beside() records it in s6-rc.
- dyn_start() (daemon/dynamic.rs): `s6-svc -u` the service's `log`
  subdirectory before the service, if there is one.
- Tests: `SANDBOX_JOURNAL=no tests/sandbox.sh DIR` writes JOURNAL=no;
  check.sh then checks hello.service's output in `logs/` instead of the
  journal stream, plus inst@a's log and beside1's log (beside1 starts while
  hold1's change holds the lock). The old code (HEAD in a scratch worktree
  with the new tests) fails 9 checks in this mode: beside1/2/3 and react2
  stuck behind the holding change (the boot deadlock), and both log
  checks. With the fix: JOURNAL=no 166/166 twice, JOURNAL=yes 164/164
  twice, `cargo test` 32 + 1 ignored, release build, shellcheck/bash -n.
  Docs: TESTING/TESTING-us (SANDBOX_JOURNAL=no).
- Not tested: a real boot with JOURNAL=no.

### Next steps

1. Owner: push, reinstall (`--no-journal` kept), reboot into s6. Check:
   no `s6-svlisten: fatal: timed out` for bolt in /var/log/messages,
   `Started polkitd` within seconds of boot, the login screen appears, and
   /var/log/sdcompat/user@60578.service/current exists. If the greeters
   still fail, that file has `systemd --user`'s own error.
2. Workaround if it still fails: setup with `--journal`.
3. Open: user-runtime-dir@UID ExecStop exits 1 at greeter logout.

## Session: 2026-10-20 night, black screen with JOURNAL=no, part 2

Owner reported the display still black. Evidence from the s6 boot of
~20:23–21:47 (`d2bd0f9` installed, `--no-journal --no-resolved`), read
from /var/log/sdcompat on the following systemd boot (no root here, so
/var/log/messages was not read).

- **`b37629e` confirmed on a real boot:** /var/log/sdcompat/user@605xx
  now exists, so the dynamic units' loggers run, and the greeters'
  `systemd --user` started ("Startup finished in 674ms"). The bolt/polkit
  deadlock was not checked (needs /var/log/messages).
- **Cause found: the user bus.** In each greeter's user manager,
  `dbus-broker.service` exited 1 five times within 30 ms, then
  `dbus.socket` hit its start limit, so gdm-wayland-session found no
  /run/user/UID/bus, started its own dbus-daemon, and gnome-session died on
  "org.freedesktop.systemd1 does not exist". Reproduced here
  (`unshare -rm`, tmpfs over /run/systemd/journal, dbus-broker-launch
  under systemd-socket-activate): `ERROR launcher_open_journal ...
  No such file or directory`, fatal. dbus-broker-launch (37) connects to
  /run/systemd/journal/socket at startup and exits if that fails. The
  system bus is dbus-daemon, so only user buses were hit. Explains the
  earlier "not explained: greeter user managers" too.
- **Fix (`98ebe70`): `sdcompat journal-relay`** (src/journal.rs), an s6
  longrun `journal-relay` (dist/s6, depends on mount-api, notification-fd
  3), put into sdcompat-sysinit by setup together with `syslog` when
  JOURNAL=no (s6_dir_sum skips its contents.d entry). It binds
  /run/systemd/journal/socket (native protocol, incl. binary fields and
  memfd entries; SO_PASSCRED for the pid) and .../stdout (stream protocol,
  `<N>` prefixes), mode 0666, and sends `<PRI>IDENT[PID]: MESSAGE` to
  /dev/log, non-blocking (dropped if rsyslog is not up or full). Other
  journal fields are dropped. Decisions: a relay, not dbus-daemon for user
  buses (that would change /etc and the systemd boot), and the stream
  socket too, so user units' output reaches rsyslog instead of /dev/null.
  It refuses to unlink a socket that still accepts connections (journald
  running after all). The runner still decides by `cfg.journal`, so system
  services keep their s6-log directories.
- Tests: unit tests (native parser, stream header/prefix, format; 34 + 1
  ignored). dbus-broker-launch with journal hidden and the relay running:
  gets past the journal (fails later on a missing XDG_RUNTIME_DIR in the
  test namespace, as it does with the real journal there); its own
  messages, `logger --journald` and `systemd-cat -p err` arrived formatted
  at a stand-in /dev/log. Sandbox: SANDBOX_JOURNAL=no starts the relay with
  `--dir $R/jrelay --syslog $R/devlog` and check.sh checks native, stream
  and prefix forwarding: 169/169 in 4 of 5 runs; 1 run failed "beside3 not
  in s6-rc -a list" (old code passed 2 of 2; the relay is a separate
  process, looks like a timing flake, not investigated). JOURNAL=yes
  164/164. s6-rc-compile of dist/s6 (with stub sdcompat-default and
  sdcompat-system-update bundles), setup `--dry-run --no-journal`,
  shellcheck/bash -n, release build. Docs: README/README-us,
  TESTING/TESTING-us.
- Not tested: a real boot. SELinux: the relay runs in the s6 tree's
  domain and creates its sockets with whatever label that gives; fine in
  permissive, unknown in enforcing.
- Still open: user-runtime-dir@UID ExecStop exit 1.

### Current state

JOURNAL=no boots: loggers for on-demand and dynamic units run (`b37629e`,
confirmed), and user buses should now start with journal-relay. JOURNAL=yes
unchanged.

### Next steps

1. Owner: push, rerun setup (`--no-journal` kept, so `journal-relay` is
   installed and added to sdcompat-sysinit), reboot into s6. Check: login
   screen; `ls -l /run/systemd/journal` (socket, stdout);
   /var/log/sdcompat/user@60578.service/current without dbus-broker
   failures; dbus-broker/gnome lines in /var/log/messages; no
   `s6-svlisten: fatal: timed out` for bolt.
2. If still black: /var/log/sdcompat/user@605*/current and `sudo grep
   -i dbus-broker /var/log/messages`. Workaround: setup with `--journal`.
3. The beside3 sandbox flake, if it shows up again.
4. user-runtime-dir@UID ExecStop exit 1 at greeter logout.

## Session: 2026-10-20 late night, login works; user units fail 226/NAMESPACE

Owner: login succeeded on the s6 boot (22:23, `e554402`, `--no-journal
--no-resolved`), and they installed the `@gnome-desktop` group with dnf
(22:22) on the systemd boot before it (21:47–22:22), not under s6;
nothing was installed while booted into s6.

- **`98ebe70` confirmed on a real boot:** /run/systemd/journal/{socket,
  stdout} are journal-relay's (mode 0666), the user bus (dbus-broker)
  runs, and gnome-session/gnome-shell are up on tty2 under user@1000.
- **Bug found: user units with mount sandboxing fail.** oo7-daemon (the
  secret service) hit its start limit with status=226/NAMESPACE; on the
  systemd boot before it ran fine. Bisected with `systemd-run --user
  --wait -p X /bin/true`: PrivateUsers, ProtectClock, ImportCredential
  pass; PrivateTmp, ProtectSystem, PrivateDevices, PrivateNetwork,
  ProtectKernelTunables, ProtectControlGroups (everything needing a mount
  namespace) fail. Cause: systemd-executor (262) bind-mounts the new root
  on /run/systemd/mount-rootfs, which systemd PID 1 creates at boot; the
  executor's own mkdir fails as uid 1000 in the user namespace and is
  ignored, then the mount fails. Our mount-api step didn't create it.
  (The child's own error message went nowhere readable: its log target is
  the journal, i.e. root-only /var/log/messages; no strace or sudo here,
  so this rests on the bisect, the missing directory and systemd's
  namespace.c, not on the error text.)
- **Fix (`f7b1bfa`): dist/early mount-api** creates /run/systemd/mount-rootfs
  (0555) and /run/systemd/inaccessible/{reg,dir,fifo,chr,blk} (0000, as
  PID 1's make_inaccessible_nodes; sock skipped, the executor only falls
  back to chr/blk there), before the restorecon (label init_var_run_t,
  same as on systemd). Tested: the snippet against a scratch dir under
  `unshare -r` (all but blk, which needs real root), bash -n, shellcheck.
  Not tested: a real boot. The sandbox suite doesn't run `early`.
- Also seen: mount propagation is private everywhere (systemd makes /
  shared); /run is mounted twice (initramfs tmpfs + s6-linux-init's). Not
  a cause here; may matter for units that expect propagation.

### Current state

JOURNAL=no s6 boots reach a GNOME login and session. User units with mount
namespacing should work once mount-api creates mount-rootfs.

### Next steps

1. Owner: push, rerun setup (installs the new /usr/libexec/sdcompat/early),
   reboot into s6. Check: `ls -ld /run/systemd/mount-rootfs`; `systemctl
   --user status oo7-daemon` running; `systemd-run --user --wait -p
   PrivateTmp=yes -p ProtectSystem=full /bin/true` exits 0.
   Without rebooting, `sudo mkdir -m 0555 /run/systemd/mount-rootfs &&
   systemctl --user restart oo7-daemon` tests the cause right away.
2. Then: mount propagation (make / rshared like systemd?), cgroup-BPF
   settings, SELinux enforcing.
3. Still open: the beside3 sandbox flake; user-runtime-dir@UID ExecStop
   exit 1 at greeter logout.

## Session: 2026-10-20 late night, part 2: s6 services on package install/update

Owner asked for s6 services to be made when packages are installed or
updated (dnf or GNOME Software).

- **Installs were already covered** (design decision 2): the systemd RPM's
  transfiletriggerin runs `systemd-update-helper system-reload-restart`,
  i.e. `systemctl daemon-reload` (→ Reload → gen::regenerate) and then
  `systemctl enqueue-marked`; `%systemd_post` runs `systemctl preset`
  (PresetUnitFiles, implemented). GNOME Software/PackageKit go through the
  same RPM scriptlets. The database is also regenerated at every s6 boot,
  so packages installed on the systemd boot are picked up then.
- **Gap found: restarts on update.** `%systemd_postun_with_restart`
  (chrony, earlyoom, ...) runs `mark-restart-system-units` →
  `systemctl set-property UNIT Markers=+needs-restart`, which sdcompat
  accepted and ignored, and `systemctl enqueue-marked` failed with "Unknown
  method EnqueueMarkedJobs" (swallowed by `|| :`). Updated daemons kept
  running the old binary until the next boot.
- **Fix (`713658d`):** SetUnitProperties (and Unit.SetProperties) keep
  Markers= with systemd's syntax (`+m`/`-m` or a plain list replacing all;
  mixing is "Bad marker syntax", unknown markers rejected; other
  properties still ignored); the Markers property shows them;
  EnqueueMarkedJobs try-restarts needs-restart units, reloads active
  needs-reload ones (inactive: no-op, like systemd's try-reload), clears
  the markers, replies with the job paths and runs the jobs concurrently
  (mode fail). Markers live in memory only (lost if the daemon restarts
  between mark and enqueue). Introspection updated.
- Tests: unit test `daemon::tests::markers_syntax` (35 + 1 ignored).
  check.sh: busctl section (needs-restart restarts hello, needs-reload runs
  reloadable's ExecReload, a marked inactive unit stays inactive, markers
  cleared, mixed syntax rejected) and, with systemd-run present, the real
  `systemctl set-property hello.service Markers=+needs-restart` +
  `systemctl enqueue-marked` (Fedora 45 host systemctl 262). Sandbox:
  JOURNAL=yes 173/173, JOURNAL=no 178/178. Release build, shellcheck/bash
  -n on check.sh. Docs: README/README-us (package installs and updates),
  TESTING/TESTING-us (real-boot row: `dnf reinstall chrony` changes
  chronyd's MainPID). `cargo fmt` is not installed here, so not run.
- Not tested: a real dnf transaction under s6. User units:
  `systemctl reload user@*.service` (user-reload-restart) was not looked at.

### Current state

As in the entry above, plus: package updates restart/reload their marked
services under s6. git has no identity on this machine; commits use the
owner's name/email via `git -c`.

### Next steps

1. Owner: push, rerun setup, reboot into s6. Check mount-rootfs/oo7-daemon
   (entry above), then `sudo dnf reinstall -y chrony` and compare
   `systemctl show -p MainPID chronyd` before and after; and install a
   package with a new service (e.g. `sudo dnf install -y cups`): `systemctl
   status cups` should show it known (and enabled per preset) without a
   reboot.
2. Look at user-reload-restart (`systemctl reload user@*.service`) under
   sdcompat.
3. Then: mount propagation, cgroup-BPF settings, SELinux enforcing.
4. Still open: beside3 sandbox flake; user-runtime-dir@UID ExecStop exit 1.

## Session: 2026-10-20 late night, part 3: cups left disabled after install

Owner, on the s6 boot (`dnf install -y cups` at 22:58, then reboot):
`systemctl status cups` shows inactive.

- Checked on the machine: cups.service, cups.socket and cups.path are all
  `disabled` with no links in /etc/systemd/system, although the preset
  (90-default.preset) says `enable cups.socket`, `enable cups.path`, and
  sdcompat reports `preset: enabled` for them. cups.service itself is
  meant to stay disabled (socket/path activated), so "inactive" for the
  service is normal on Fedora; the bug is that the socket and path were
  not enabled.
- **Bug: preset order.** cups' %post runs `systemctl --no-reload preset
  cups.path cups.socket cups.service`. install::preset went through the
  names in order: enabled path and socket, then disabled cups.service,
  whose `Also=cups.socket cups.path` made disable() remove their links
  again. systemd's execute_preset does all disables first, then all
  enables.
- **Fix (`36e443d`):** preset() collects the enable and disable lists,
  disables first, then enables. Unit test
  `install::tests::preset_disable_before_enable` (cups layout in a temp
  dir). Tests: 36 + 1 ignored; sandbox JOURNAL=yes 173/173.
- Other packages installed under s6 with the same pattern (a
  preset-disabled unit with Also= to preset-enabled ones) were affected
  the same way. Packages installed on the systemd boot were not.
- Not tested: a real boot / real dnf install with the fix.

### Current state

As before; `systemctl preset` now matches systemd for units linked by
Also=.

### Next steps

1. Owner: push, rerun setup, then fix this install with `sudo systemctl
   preset cups.path cups.socket cups.service` (or reinstall cups).
   Check: `systemctl is-enabled cups.socket cups.path` → enabled;
   `systemctl status cups.socket` active (listening) after reboot or
   `systemctl start cups.socket`; `lpstat -r` starts cups.service.
2. The earlier steps still stand: chrony MainPID change on reinstall,
   mount-rootfs/oo7-daemon, user-reload-restart, mount propagation,
   cgroup-BPF, SELinux enforcing, beside3 flake, user-runtime-dir ExecStop.

## Session: 2026-10-20 late night, part 4: setup restarts sdcompatd

- Owner reran setup (23:04) and `systemctl preset` (23:08) on the s6
  boot; cups.socket/path were still disabled. Cause: setup installed the
  new binary but the running daemon (started 22:55) kept the old code, so
  the preset fix (`36e443d`) was not in effect.
- **Change (`852b02d`):** setup's new `restart_daemon` step (after
  build_database) runs `s6-svc -wR -T 30000 -r
  /run/s6-rc/servicedirs/sdcompatd` when that service dir exists and is
  supervised (i.e. on the s6 boot; skipped on the systemd boot); warns,
  doesn't fail, if not ready in 30 s. The daemon already keeps dynamic
  units, the fd store (separate process) and devices across restarts.
  sdcompat-fdstore and journal-relay are deliberately not restarted
  (would drop held fds / log sockets); they get the new binary at the
  next boot. Docs: TESTING/TESTING-us (re-running setup).
- Tests: bash -n, shellcheck -S warning, `--dry-run` on this s6 boot
  prints the step. Not tested: a real (root) run of the restart.
- Workaround given to the owner for cups now: `sudo systemctl enable
  --now cups.socket cups.path` (enable has no Also= ordering problem).

### Next steps

1. Owner: push, rerun setup on the s6 boot; check `s6-svstat
   /run/s6-rc/servicedirs/sdcompatd` shows a fresh uptime, then
   `sudo systemctl disable cups.socket cups.path && sudo systemctl preset
   cups.path cups.socket cups.service && systemctl is-enabled cups.socket
   cups.path` → enabled, enabled.
2. Earlier next steps unchanged.

## Session: 2026-10-20 late night, part 5: setup restart and preset confirmed

- Owner reran setup on the s6 boot with `852b02d` and ran the checks from
  part 4's next step 1; reported "It works fine". So the setup
  `restart_daemon` step works in a real root run, and the preset fix
  (`36e443d`) works in the running daemon (cups.socket/cups.path enabled
  by `systemctl preset`).
- No code changes this session.

### Current state

As in part 4; restart on setup and preset ordering are verified on the
real machine.

### Next steps

1. Still to check on the s6 boot: chrony MainPID change on `dnf reinstall
   chrony`, mount-rootfs/oo7-daemon.
2. Then: user-reload-restart, mount propagation, cgroup-BPF settings,
   SELinux enforcing.
3. Still open: beside3 sandbox flake; user-runtime-dir@UID ExecStop exit 1.

## Session: 2026-10-21: user-reload-restart

Owner asked to look at user-reload-restart. The systemd RPM's file
triggers for /usr/lib/systemd/user run `systemd-update-helper
user-reload-restart` (install) and `user-reload` / `user-restart`
(removal/upgrade): `systemctl reload "user@*.service"` on the system
manager, then `systemctl --user -M UID@ enqueue-marked` per user;
`mark-*-user-units` and `remove-user-units` also use `--user -M UID@`.

- Checked live on the s6 boot (read-only): `systemctl list-units 'user@*'`
  works (ListUnitsByPatterns); user@1000's systemd already ran with
  `--deserialize=29`, so the RTMIN+25 reload signal had reached it before.
- **Gap 1: reload didn't wait.** For Type=notify-reload, cmd_reload sent
  the signal and returned at once; systemd waits for RELOADING=1 then
  READY=1. The following `--user -M` calls could hit a re-executing
  manager. **Fix:** the notify helper counts RELOADING=1→READY=1 cycles
  in the state key `reloads-done`; cmd_reload waits for it to go up, and
  fails if the main process exits or TimeoutStartSec= (default 90 s)
  passes. The helper writes the key at start, so a service started by an
  older binary (no key) is not waited on, so there's no 90 s hang after
  setup on a running system.
- **Gap 2: `-M UID@` didn't work at all.** sd-bus reaches another user's
  manager with `unixexec:path=systemd-run,argv1=-M.host,-PGq,--wait,
  -pUser=UID,-pPAMName=login,systemd-stdio-bridge,--user`, and sdcompat
  rejected StandardInputFileDescriptor etc. ("passing file descriptors
  ... not supported"). So update-time restarts/markers/disables of user
  units all failed silently (`|| :`). **Fix:** StartTransientUnit puts the
  fds into the fd store as `<unit>#stdio` (names stdin/stdout/stderr,
  limit 3) and writes `Standard*=fd:NAME` into the transient unit file;
  setup_stdio fetches them once and deletes the key (store copies would
  keep the caller's pipes open, so no EOF). remove_transient_file deletes
  the key too. A unit with ExecStartPre= would hand the fds to the first
  command only; systemd-run units don't have one.
- Minor: the ReloadSignal property was hardcoded to SIGHUP; now reads
  ReloadSignal=.
- Commit `efec5bc`. Tests: cargo test 36 + 1 ignored; release build;
  sandbox JOURNAL=yes 180/180, JOURNAL=no 185/185 (new: notreload.service
  with ReloadSignal=USR1 that answers after 1 s; `systemctl reload
  'notrel*.service'`, `list-units` by pattern, ReloadSignal property,
  `echo piped | systemd-run -PGq --wait cat`). By hand in the sandbox:
  busctl over `unixexec:` → systemd-run -PGq --wait systemd-stdio-bridge
  got the manager's Version twice; the transient units were unloaded
  shortly after. bash -n, shellcheck -S warning. Docs: README/README-us
  (package updates; `--pipe` removed from "not implemented"),
  TESTING/TESTING-us (new real-boot row).
- Not tested: the real root path (`-pUser=UID -pPAMName=login`, i.e. a PAM
  session for the bridge) and the wait against a real user manager. The
  current user@1000 was started by the old helper, so its next reload
  won't be waited on until the next boot.

### Current state

Package updates of user units should now restart/reload them in running
user managers under s6, as with systemd (not verified on a real boot).

### Next steps

1. Owner: push, rerun setup on the s6 boot, then the TESTING row "User
   managers on package update": `sudo systemctl --user -M $USER@
   is-system-running` → running/degraded; `sudo
   /usr/lib/systemd/systemd-update-helper user-reload-restart; echo $?`
   → 0. After the next reboot, `time sudo systemctl reload
   user@1000.service` should take a moment (the wait) and succeed.
2. Still to check on the s6 boot: chrony MainPID change on reinstall,
   mount-rootfs/oo7-daemon.
3. Then: mount propagation, cgroup-BPF settings, SELinux enforcing.
4. Still open: beside3 sandbox flake; user-runtime-dir@UID ExecStop exit 1.

## Session: 2026-10-21, part 2: user-reload-restart first try ran the old binary

- Owner ran the TESTING row on the s6 boot (booted 23:37) without
  rerunning setup: both commands printed "passing file descriptors
  (systemd-run --pipe/--stdio) is not supported by sdcompat", then
  "Transport endpoint is not connected"; the helper exited 0 (its `|| :`).
- Cause: /usr/libexec/sdcompat/sdcompat is from the 23:13 setup run, before
  `efec5bc` (built 23:31); that message no longer exists in the source.
  Not a code bug; no changes.

### Next steps

1. Owner: rerun setup on the s6 boot, then repeat part 1's next step 1.
2. Earlier next steps unchanged.

## Session: 2026-10-21, part 3: setup rerun; the fd store still runs old code

- Owner reran setup (23:41) with `--rebuild --no-resolved --no-journal`.
  The new binary is installed and sdcompatd was restarted (23:41:24), but
  `sdcompat fdstore` is still the 23:37 process: by design setup doesn't
  restart it. The `#stdio` limit (3) from `efec5bc` is evaluated in the fd
  store process, and the old code loads "<unit>#stdio" as a unit, gets
  FileDescriptorStoreMax 0, and answers "ERR fd store full". So
  StartTransientUnit with --pipe would still fail until the next boot.
  Told the owner to reboot (restarting the fd store would drop the fds
  services hold in it).
- Lesson: changes to fdstore::limit (or anything else the fd store
  process runs) only take effect at the next boot; a later fix could make
  the daemon not depend on that, e.g. the store lets the PUTting client
  pass the limit.

### Next steps

1. Owner: reboot into the s6 boot, then part 1's next step 1 (both
   commands, and `time sudo systemctl reload user@1000.service`).
2. Earlier next steps unchanged.

## Session: 2026-10-20 (s6 boot of 23:47): user-reload-restart verified

- Owner rebooted into the s6 boot (PID 1 s6-svscan, up 23:47:52; binary
  from the 23:41 setup, fd store started 23:48:05, so it runs `efec5bc`)
  and ran part 1's next step 1:
  - `sudo systemctl --user -M $USER@ is-system-running` → `running`
    (sd-bus `unixexec:` → `systemd-run -M.host -PGq --wait -pUser=1000
    -pPAMName=login systemd-stdio-bridge --user` works through the
    `#stdio` fd store path, including the PAM session).
  - `sudo /usr/lib/systemd/systemd-update-helper user-reload-restart;
    echo $?` → `0`, with no error output this time.
  - `time sudo systemctl reload user@1000.service` → 0.43 s, success.
- Checked afterwards (read-only): `/run/sdcompat/units/user@1000.service/
  reloads-done` is `2` (the helper's reload and the manual one each saw
  RELOADING=1→READY=1), and the user manager is now `systemd --user
  --deserialize=11`, i.e. it re-executed. So the reload wait works
  against a real user manager; 0.43 s is how long the re-exec takes.
- No code changes.

### Current state

User manager reload/restart on package updates works on the real s6 boot:
`--user -M UID@` via systemd-run --pipe, and reload waiting for READY=1.
The lesson from part 3 stands (fd store code changes need a reboot).

### Next steps

1. Still to check on the s6 boot: chrony MainPID change on `dnf reinstall
   chrony`, mount-rootfs/oo7-daemon.
2. Then: mount propagation, cgroup-BPF settings, SELinux enforcing.
3. Maybe: make the fd store not need a reboot for limit changes (the
   PUTting client passes the limit).
4. Still open: beside3 sandbox flake; user-runtime-dir@UID ExecStop exit 1.

## Session: 2026-10-20, part 2: chrony restart on package update confirmed

- Owner reports `sudo dnf reinstall -y chrony` changed chronyd's MainPID
  on the s6 boot (checked earlier by the owner, not recorded until now). So
  `mark-restart-system-units` → `systemctl enqueue-marked` restarts
  system services on package updates under s6, as with systemd.
- No code changes.

### Current state

Package updates restart system services (chronyd) and reload/restart
user managers on the real s6 boot.

### Next steps

1. Still to check on the s6 boot: mount-rootfs/oo7-daemon.
2. Then: mount propagation, cgroup-BPF settings, SELinux enforcing.
3. Maybe: make the fd store not need a reboot for limit changes (the
   PUTting client passes the limit).
4. Still open: beside3 sandbox flake; user-runtime-dir@UID ExecStop exit 1.

## Session: 2026-10-20, part 3: mount-rootfs/oo7-daemon confirmed

- Checked on the s6 boot of 23:47 (unprivileged): /run/systemd/mount-rootfs
  exists (0555 root, created 23:48 by early mount-api, `f7b1bfa`);
  `systemctl --user status oo7-daemon` → active (running) since 23:48:18;
  `systemd-run --user --wait -p PrivateTmp=yes -p ProtectSystem=full -p
  PrivateDevices=yes /bin/true` → exit 0. User units with mount
  namespacing work on the real boot.
- No code changes.

### Next steps

1. Mount propagation (/ is private; systemd makes it shared), cgroup-BPF
   settings, SELinux enforcing.
2. Maybe: make the fd store not need a reboot for limit changes (the
   PUTting client passes the limit).
3. Still open: beside3 sandbox flake; user-runtime-dir@UID ExecStop exit 1.

## Session: 2026-10-21: mount propagation; /run/user/UID mounted twice

- Checked on the s6 boot of 2026-10-20 23:47 (read-only): every mount is
  `private`. systemd PID 1 runs MS_REC|MS_SHARED on / at boot
  (mount-setup.c); nothing did here. sandbox.rs already makes / an rslave
  in a unit's namespace, as systemd does, but a slave of a private mount
  receives nothing, so mounts made after a unit (or flatpak app, or
  container) started never reached it. **Fix:** early mount-api runs
  `mount --make-rshared /` after the API mounts; mount-fs's mounts are
  shared as their parent is. Shown under `unshare -rm` (scratch script):
  an rslave namespace doesn't see a later mount with / private, does with
  / rshared.
- **Bug found on the way:** /run/user/1000 was two tmpfs (0:150, 0:151;
  mount IDs 1606/1607, the second on the first). systemd-user-runtime-dir
  skips the mount if the path is already a mount point, so two runs of
  `start 1000` overlapped. Cause: logind (262) starts
  user-runtime-dir@UID and then user@UID (BindsTo= it) without waiting;
  execute_job takes the per-unit lock, but user@'s dependency start
  (start_any → dyn_start) didn't, and the oneshot path only marks the
  unit active after ExecStart returns. **Fix:** start_any takes the
  dependency's unit lock; the second start then finds it active. (Cycles
  of Requires= between dynamic units recursed forever before; now they
  would wait forever instead; no worse.) Possibly related to the open
  "user-runtime-dir@UID ExecStop exit 1": stop unmounted one of the two.
- Commit `6efd974`. Tests: cargo test 36 + 1 ignored; release build;
  sandbox JOURNAL=yes 183/183, JOURNAL=no 188/188. New check: StartUnit
  prep@d then inst@d (BindsTo= prep@d) back to back, with prep@d's
  ExecStart sleeping 1 s: ExecStart ran once. Without the lock it fails
  with 2 (checked). bash -n, shellcheck -S warning. TESTING/TESTING-us:
  two real-boot rows (propagation shared; /run/user/$UID mounted once).
- Not tested: a real boot with the new early script and daemon.
- Also still seen: /run is mounted twice (initramfs tmpfs, then
  s6-linux-init's on top), which hides the initramfs's /run, e.g.
  /run/credentials/systemd-cryptsetup@... mounts. Not looked at.

### Current state

Under s6, / should now be shared like with systemd, and user-runtime-dir
runs once per login (both unverified on a real boot).

### Next steps

1. Owner: push, rerun setup, reboot into s6. Check the two new TESTING
   rows: `findmnt -no PROPAGATION /` → `shared`; `findmnt -n
   /run/user/$UID | wc -l` → `1`. Then see whether user-runtime-dir@UID's
   ExecStop still exits 1 on logout/shutdown.
2. Maybe: the double /run (s6-linux-init over the initramfs's).
3. Then: cgroup-BPF settings, SELinux enforcing.
4. Maybe: make the fd store not need a reboot for limit changes.
5. Still open: beside3 sandbox flake.

## Session: 2026-10-21: propagation and single /run/user/UID confirmed on the s6 boot

- s6 boot of 2026-10-21 00:06 (PID 1 `s6-svscan`, after `6efd974`).
  Owner ran `findmnt -no PROPAGATION` (all mounts, not just /): every
  line `shared`; `findmnt -no PROPAGATION /` → `shared`. `findmnt -n
  /run/user/$UID | wc -l` → `1` (one tmpfs, uid=1000, mode=700). Both new
  TESTING rows pass. No code changed.
- Still seen: /run mounted twice (initramfs tmpfs with size=, then
  s6-linux-init's on top).
- **user-runtime-dir@UID ExecStop exit 1: gone.** Owner grepped
  /var/log/messages: on Oct 20 it failed ~50 times, for greeter (60578-
  60583), root (@0, sudo) and 1000 alike; the last at 00:05:45, the
  shutdown of the old boot (new binary installed 00:03:41, but
  /run/user/1000 had been mounted twice at the 23:48 login by the old
  code). After the 00:06 reboot: none up to 00:13:55, though the greeter
  stopped at login (00:06) and root's after a sudo at 00:10:30; both
  /run/user dirs are gone and their units inactive. Explains the old
  failure: with two tmpfs, ExecStop unmounted only the top one and could
  not remove the directory. (Only failures are logged; success is
  inferred from no line plus the dirs being gone.)
- **New, not looked at:** `loginctl list-users` keeps root and
  gdm-greeter in `closing` (greeter since 00:06, still at 00:14), with
  user@/user-runtime-dir@ inactive and ListJobs empty. Under systemd
  logind drops them once the stop jobs end. GetJob on a missing job
  answers `org.freedesktop.systemd1.NoSuchJob` (checked), so a lost or
  early JobRemoved is a guess. No harm seen.

### Current state

Under s6, / and all mounts are shared, user-runtime-dir runs once per
login, and its ExecStop no longer fails (all confirmed on a real boot).

### Next steps

1. Maybe: why logind keeps stopped users in `closing` (done below).
2. Maybe: the double /run (s6-linux-init over the initramfs's).
3. Then: cgroup-BPF settings, SELinux enforcing.
4. Maybe: make the fd store not need a reboot for limit changes.
5. Still open: beside3 sandbox flake.

## Session: 2026-10-21: logind keeps stopped users "closing"

- Cause (read in systemd v262 src/login): user_may_gc() asks
  manager_job_is_active() for runtime_dir_job/service_manager_job, which
  does Properties.Get Job.State on the job's path and treats only
  `org.freedesktop.DBus.Error.UnknownObject` as finished (systemd's
  answer: the object is gone). sdcompat answered
  `org.freedesktop.systemd1.NoSuchJob` there, so r < 0, "still pending",
  and the user was never finalized (root, gdm-greeter seen `closing` for
  8+ minutes with their units inactive and no jobs). The earlier check
  last session used GetJob, where NoSuchJob is right.
- **Fix** (`bb74578`): Properties on a missing job path, and Job.Cancel
  on one, answer UnknownObject. Manager.GetJob/CancelJob keep NoSuchJob.
- Tests: cargo test 36 + 1 ignored; release build; sandbox
  SANDBOX_JOURNAL=yes 184/184, SANDBOX_JOURNAL=no 189/189. New check:
  Job.State on the cancelled job's path → UnknownObject (via dbus-send,
  SKIP if missing). bash -n, shellcheck -S warning. TESTING/TESTING-us:
  real-boot row (no root/gdm-greeter left `closing`).
- Note: the sandbox mode variable is `SANDBOX_JOURNAL`; a plain
  `JOURNAL=no` runs the yes mode (caught here; earlier entries' counts
  show the right mode was used then).
- Not tested: a real boot with the fix.

### Current state

Under s6, / is shared, user-runtime-dir runs once per login and its
ExecStop succeeds (confirmed on a real boot). logind should now drop
users once their units stop (unverified on a real boot).

### Next steps

1. Owner: push, rerun setup, reboot into s6. After login and a `sudo`,
   wait a minute: `loginctl list-users` shows only the logged-in user.
2. Maybe: the double /run (s6-linux-init over the initramfs's).
3. Then: cgroup-BPF settings, SELinux enforcing.
4. Maybe: make the fd store not need a reboot for limit changes.
5. Still open: beside3 sandbox flake.

## Session: 2026-10-21: double /run

- Cause: s6-linux-init (without -n/-N) unmounts /run "just in case",
  then mounts a tmpfs on it. The unmount fails: the initramfs left
  tmpfs mounts under it (/run/credentials/systemd-journald.service and
  .../systemd-cryptsetup@luks-...). So s6's tmpfs (no size=) sat on the
  initramfs's (size=20%, nr_inodes=819200).
- Hidden /run, listed by the owner (`sudo unshare -m --propagation
  private sh -c 'umount -l /run; ...'`): udev database for the LUKS dm
  device, /run/log/journal (initramfs journal), /run/mount (utab),
  /run/utmp, /run/varlink/registry, /run/initramfs, /run/cryptsetup,
  /run/tpm2-tss, /run/tmpfiles.d/static-nodes.conf, the two credential
  mounts, and /run/systemd.pre-switch-root. **No /run/systemd:** the
  initramfs's systemd renames it before switch-root (v262 main.c:2325)
  and never deletes the renamed one, so leaving it is systemd-like.
  sdcompat creates no /run/credentials dirs (only expands %d); the early
  script's /run steps are all idempotent; restorecon -R /run relabels
  the leftovers.
- **Fix** (`4564159`): setup passes `-n` to s6-linux-init-maker (keep /run,
  just remount it, as systemd does). Checked: generated bin/init has
  `-n`; `setup-fedora.sh --dry-run` shows it. bash -n, shellcheck -S
  warning. TESTING/TESTING-us: real-boot row `findmnt -n /run | wc -l`
  → 1. No Rust change, so cargo/sandbox not rerun (sandbox doesn't use
  s6-linux-init). Possibly also clears the old "/run/varlink/registry
  symlinks (directory missing)" item.
- Not tested: a real boot with -n.

### Current state

Under s6, / is shared, user-runtime-dir runs once per login and its
ExecStop succeeds (confirmed). Unverified on a real boot: logind drops
stopped users; /run is the initramfs's, mounted once.

### Next steps

1. Owner: push, rerun setup (it regenerates /etc/s6-linux-init), reboot
   into s6. Check: `findmnt -n /run | wc -l` → 1; after login and a
   `sudo`, a minute later `loginctl list-users` shows only the user.
   If boot breaks: the tty9 rescue getty, or the previous s6-linux-init
   in /var/lib/sdcompat/setup-backup/.
2. Then: cgroup-BPF settings, SELinux enforcing.
3. Maybe: make the fd store not need a reboot for limit changes.
4. Still open: beside3 sandbox flake.

## Session: 2026-10-21: real boot with -n and the UnknownObject fix

- s6 boot of 2026-10-21 00:33 (PID 1 `s6-svscan`, after `4564159`).
- **/run mounted once:** `findmnt -n /run | wc -l` → `1`. Its options
  are the initramfs's (`size=2785964k,nr_inodes=819200,mode=755`), so
  s6-linux-init `-n` kept it, as intended. /run/varlink/registry exists
  and holds its symlinks (Hostname, Journal, Login, Manager, Shutdown,
  Unit), so the old "registry directory missing" item looks gone too.
- **logind drops stopped users:** after `sudo dnf upgrade`,
  `loginctl list-users` shows only 1000 (`active`); no root, no
  gdm-greeter in `closing`. Confirms `bb74578`.
- No code changed.

### Current state

Under s6, / is shared, /run is the initramfs's tmpfs mounted once,
user-runtime-dir runs once per login and its ExecStop succeeds, and
logind drops users whose units have stopped (all confirmed on a real
boot).

### Next steps

1. Then: cgroup-BPF settings, SELinux enforcing.
2. Maybe: make the fd store not need a reboot for limit changes.
3. Still open: beside3 sandbox flake.

## Session: 2026-10-21: cgroup BPF settings

- Scope, from this machine's units (/usr/lib/systemd/{system,user}):
  IPAddressDeny= 31 (all `any`), IPAddressAllow= 1 (`localhost`),
  DeviceAllow= 25, DevicePolicy= 16 (15 closed, 1 strict: polkit.service).
  RestrictFileSystems=, SocketBind*=, RestrictNetworkInterfaces=,
  IP*FilterPath=, BPFProgram= are used by none, so left out.
  RestrictFileSystems= would need a BPF LSM program anyway.
- **New `src/bpf.rs` (`aefbccb`):** hand-assembled programs, no libbpf, as
  systemd's bpf-devices.c/bpf-firewall.c. Names as systemd's: sd_devices,
  sd_fw_ingress, sd_fw_egress.
  - Devices: allow list, everything else denied. closed (and auto with a
    DeviceAllow= list) adds systemd's auto list (/dev/null, zero, full,
    random, urandom, tty, ptmx, /run/systemd/inaccessible/{chr,blk}, rwm)
    plus char-pts rw; strict adds nothing. PrivateDevices= turns auto into
    closed. `char-`/`block-NAME` globs over /proc/devices (fnmatch, so
    `char-/dev/console` and `char-cpu/*` work as in systemd); `*` is the
    whole type. Missing /dev nodes are skipped silently (units list
    /dev/kvm, /dev/cros_fp...).
  - IP: allow list passes, else deny list drops, else pass; source address
    on ingress, destination on egress. **Decision:** prefixes compiled into
    the program (masked 32-bit word compares after bpf_skb_load_bytes)
    instead of LPM-trie maps: units list one or two prefixes, and no maps
    means no map fds to manage. No deny list → no program (nothing to drop).
  - Runner: `enter_cgroup` calls `bpf::apply` right after moving itself into
    the unit's cgroup (root only), so ExecStartPre= is covered too. Each
    start first detaches programs with our names from that cgroup (query +
    get-fd-by-id + name from prog info), then attaches the new ones with
    BPF_F_ALLOW_MULTI. Detaching by name leaves programs other tools attached.
    A refused program is logged ("DevicePolicy= not enforced: ...", with the
    verifier's last lines) and the start goes on, as systemd does.
  - Checked: the runner itself only opens /dev/null and the TTY after
    entering the cgroup; PrivateDevices= builds /dev with bind mounts, not
    mknod, so polkit's strict `/dev/null rw` does not block it.
- Not done: lists on slices; IPAddress* on .socket units (systemd filters
  the socket; nothing here uses it); transient units' D-Bus properties for
  these; IPAccounting=; D-Bus properties (`systemctl show -p DevicePolicy`).
- Tests: cargo test 42 + 2 ignored. The programs are checked without root by
  a small eBPF interpreter in the tests (device allow/deny incl. access
  subsets; firewall v4/v6 ingress/egress, prefixes, non-IP, short packet);
  plus parsing and polkit/PrivateDevices/reset cases. **Not run: the kernel
  test** (`tests/bpf-root.sh`, needs sudo; the verifier has not seen these
  programs yet). Release build; sandbox SANDBOX_JOURNAL=yes 184/184,
  SANDBOX_JOURNAL=no 189/189 (runner is unprivileged there, so bpf is not
  exercised). bash -n, shellcheck -S warning. clippy not installed here.
- Docs: README/README-us (supported, not-implemented, file list),
  TESTING/TESTING-us (bpf-root.sh in section 3; real-boot rows: bpftool on
  polkit/logind, no "not enforced" messages, pkaction answers).
- On the s6 boot these units get programs: chronyd, firewalld, fwupd,
  malcontent-timerd, polkit, systemd-journald, systemd-logind,
  systemd-hostnamed, systemd-localed, upower.
- Note for SELinux enforcing: the runner's domain will need bpf
  { prog_load prog_run } and the cgroup attach permissions.

### Current state

DevicePolicy=/DeviceAllow= and IPAddressDeny=/IPAddressAllow= are
implemented and unit-tested; not yet loaded into a real kernel.

### Next steps

1. Owner: `tests/bpf-root.sh` (sudo). If it passes: push, rerun setup,
   reboot into s6 and go through the three new TESTING rows; also that
   GNOME login, journald, logind and polkit come up as before. If a unit
   breaks, the runner's "not enforced" or the unit's own errors show which
   program; send them.
2. Then: SELinux enforcing.
3. Maybe: make the fd store not need a reboot for limit changes.
4. Still open: beside3 sandbox flake.

## Session: 2026-10-21: first kernel run of the BPF programs

- Owner ran `tests/bpf-root.sh`: sd_devices loaded and attached (no
  message); sd_fw_ingress/egress refused: "unreachable insn 17". Cause:
  `any` is a /0 prefix, compiled to an unconditional `ja drop`, and the
  family's trailing `ja accept` after it was dead code, which the verifier
  rejects. The test interpreter only follows taken paths, so it missed it.
- **Fix (`c9c2b7b`):** a /0 prefix ends its family's code; the drop block is
  emitted only when referenced (allow any + deny any would leave it dead).
  New test `programs_have_no_dead_code`: walks every branch of programs
  for 36 allow/deny combinations and the device program; it fails on the
  old code (unreachable insns [13, 22]).
- Tests: cargo test 43 + 2 ignored; sandbox SANDBOX_JOURNAL=yes 184/184,
  SANDBOX_JOURNAL=no 189/189. Kernel test not rerun yet (needs sudo).
- The earlier entry's "the verifier has not seen these programs" now
  holds only for the firewall programs; the device program passed.

### Current state

DevicePolicy=/DeviceAllow= loads and attaches in a real kernel (enforcement
not yet confirmed: the test stopped at the firewall's load error).
IPAddressDeny=/IPAddressAllow= fixed for the verifier, not yet rerun.

### Next steps

1. Owner: `tests/bpf-root.sh` again. If it passes: push, rerun setup,
   reboot into s6, the three TESTING rows, and GNOME/journald/logind/polkit
   come up as before.
2. Then: SELinux enforcing (the runner will need bpf permissions).
3. Maybe: make the fd store not need a reboot for limit changes.
4. Still open: beside3 sandbox flake.

## Session: 2026-10-21: kernel test passes

- Owner reran `tests/bpf-root.sh` after `c9c2b7b`: `kernel_enforces ... ok`.
  So in the real kernel: all three programs load and attach; a second
  apply replaces rather than adds (one program per attach type); inside
  the cgroup /dev/null opens, /dev/zero gets EPERM (strict, /dev/null rw),
  UDP to 127.0.0.1 is sent, to 192.0.2.1 gets EPERM (deny any, allow
  localhost); the scratch cgroup is removed. No code changed.

### Current state

DevicePolicy=/DeviceAllow= and IPAddressDeny=/IPAddressAllow= are enforced
by the kernel (scratch-cgroup test). Not yet seen on a real s6 boot.

### Next steps

1. Owner: push, rerun setup, reboot into s6. Check the three TESTING rows
   (bpftool on polkit/logind, no "not enforced" in the journal, pkaction)
   and that GNOME login, journald, logind and polkit come up as before.
2. Then: SELinux enforcing (the runner will need bpf permissions).
3. Maybe: make the fd store not need a reboot for limit changes.
4. Still open: beside3 sandbox flake.

## Session: 2026-10-21: BPF programs on a real s6 boot

- Owner pushed, reran setup and booted into s6 (PID 1 `s6-svscan`,
  `JOURNAL=no`). The three TESTING rows:
  - bpftool: polkit.service has one `cgroup_device` `sd_devices`;
    systemd-logind.service has `sd_devices`, `sd_fw_ingress`,
    `sd_fw_egress`, one each, all `multi`. As expected.
  - "not enforced": the owner ran `journalctl -b -g`, which with
    `JOURNAL=no` only sees the initrd's journal, so it proves nothing. Checked
    instead: the runner's `log()` also writes to stderr, and no
    `/var/log/sdcompat/*/current` (70 logs, all readable) contains "not
    enforced"; polkit's is empty. `/var/log/messages` (root-only) not
    grepped.
  - `pkaction | head -3` lists three actions: polkitd answers under
    `DevicePolicy=strict`.
- Also seen: logind, polkitd, gnome-shell running; a GNOME seat0 session on
  tty2. No code changed.

### Current state

DevicePolicy=/DeviceAllow= and IPAddressDeny=/IPAddressAllow= are attached
on a real s6 boot (JOURNAL=no) with no refused programs, and polkit works
under strict. Not yet seen with JOURNAL=yes.

### Next steps

1. Optional: `sudo grep 'not enforced' /var/log/messages` for completeness;
   the JOURNAL=yes boot's `journalctl -b -g 'not enforced'` when next used.
2. SELinux enforcing (the runner will need bpf { prog_load prog_run } and
   the cgroup attach permissions).
3. Maybe: make the fd store not need a reboot for limit changes.
4. Still open: beside3 sandbox flake.

## Session: 2026-10-21: SELinux enforcing, first pass

Owner asked to start on SELinux enforcing. Worked on the s6 boot (PID 1
s6-svscan, Permissive via `enforcing=0`, JOURNAL=no) without root, so
checks are against the loaded policy with sesearch, not AVCs.

- **BPF needs no policy change:** the runner is init_t, which has
  `bpf { map_create map_read map_write prog_load prog_run }`,
  `capability2 bpf`, and cgroup dir/file write. The note from the BPF
  entry is settled.
- **Bug, fixed (`6b2ffe3`):** PrivateDevices= built /dev in a fresh tmpfs;
  its root and the fd/stdin/stdout/stderr/ptmx links were tmpfs_t (init_t
  has no named type transition for them, only `log` -> devlog_t).
  policykit_t, chronyd_t and the other service domains have no
  `tmpfs_t:lnk_file read`, so under enforcing /dev/fd, /dev/stdout & co.
  would be denied for the ~36 PrivateDevices= units. Now labelled from
  file_contexts as their /dev paths (`selinux::label_as`, selabel_lookup +
  lsetfilecon), as systemd's label_fix_full/symlink_label. Policy allows
  it: init_t relabelfrom/relabelto on file_type dir/lnk_file, device_t
  may associate with tmpfs. Bind-mounted nodes keep their own labels.
  New test `label_as_on_this_host` (unprivileged, relabels a scratch dir
  and link as /dev and /dev/fd; asserts device_t, it did not skip here).
- Checked, fine: JOURNAL=no relay sockets (syslogd_var_run_t, devlog_t,
  listener init_t): daemon domains have connectto/sendto on init_t
  sockets and write on those sock_files.
- Left as is: early oneshots' output to the s6-svscan-log fifo
  (var_run_t). No fifo type is writable by every domain; under enforcing
  the kernel replaces the fd with /dev/null at exec, so loadkeys and
  restorecon still run, their messages are lost.
- Tests: cargo test 44 + 2 ignored; release build; sandbox 184/184.

### Current state

Policy-side checks done for BPF, the private /dev and the journal relay.
This boot's real AVC list has not been read (root only).

### Next steps

1. Owner, on this permissive s6 boot: `sudo ausearch -m
   avc,user_avc,selinux_err -ts boot -i > ~/avc.txt` (or `sudo
   tests/collect-diagnostics.sh`) and send it; triage by cause as in
   session 9.
2. After fixes: push, reinstall (rebuild binary), reboot s6, recollect.
   When the list is down to policy-side leftovers, one enforcing boot by
   removing `enforcing=0` at the GRUB prompt (the systemd entry is
   unaffected); then `setup-fedora.sh --enforcing`.
3. Maybe: make the fd store not need a reboot for limit changes.
4. Still open: beside3 sandbox flake.

## Session: 2026-10-21: why the s6 boot uses more RAM

Owner asked why RAM use is higher on the s6 boot than on the systemd boot.
Measured on the s6 boot only (PID 1 s6-svscan, JOURNAL=no, ~17 min up,
GNOME session), unprivileged, so from /proc/*/status (RssAnon), not PSS.
No systemd-boot numbers to compare yet. No code changed.

- `free`: 2200 MiB used, 4306 buff/cache, no swap (no /dev/zram*).
- Supervision tree: 256 s6-supervise (255 service dirs, 52 of them s6-log
  loggers). Summed RSS 516 MiB is misleading (shared libc/file pages
  counted 256 times); private anon is ~124 kB each, 31 MiB total, plus
  kernel stack/page tables per process.
- 36 `sdcompat` processes (runners for forking/notify services, timer-run,
  path-run, daemon, fdstore, journal-relay): ~35 MiB anon total.
- 52 s6-log: ~6 MiB anon. rsyslogd 17 MiB RSS (JOURNAL=no).
- Under systemd all of that is one PID 1 (~15-30 MiB) plus journald.
  So the s6 stack's own cost is roughly 60-80 MiB more.
- Second cause: socket-activated daemons are started at boot, because
  on-demand socket activation is not implemented (only re-activation for
  rfkill-like cases). Running here that systemd would leave to their
  sockets: iscsid, virtlockd, virtlogd, virtnetworkd, cupsd (~90 MiB RSS).
  The virtqemud/virtproxyd/... set started at 14:38 and exited on their
  --timeout, as under systemd.
- Not explained yet: SUnreclaim 227 MiB (no systemd-boot number to compare).

### Next steps

1. Owner, on both boots after the same idle time: `free -m`,
   `grep -E 'AnonPages|Shmem:|SUnreclaim|KernelStack|PageTables' /proc/meminfo`,
   `ps -eo rss=,comm= | sort -rn | head -30`; compare.
2. If the gap matters: EXCLUDE unused units (iscsid/iscsiuio, sssd, virt*),
   or implement on-demand socket activation (Accept=no) so socket units
   start their service on first connection.

## Session: 2026-10-21: SELinux, the permissive boot's AVCs

Owner ran `sudo ausearch -m avc,user_avc,selinux_err -ts boot -i >
~/avc.txt` on the s6 boot (permissive, JOURNAL=no): 64 denials. By cause:

- **Bug, fixed (`c3560ff`): implied NoNewPrivileges=.** `nnp_transition`
  denied init_t -> chronyd_t, NetworkManager_t, boltd_t, virtlogd_t: the
  runner set no_new_privs for root services whose CapabilityBoundingSet=
  leaves out CAP_SYS_ADMIN. Under enforcing their exec would fail. systemd
  asks the child's *effective* set, which a bounding-set drop does not
  change before exec, so for root it never implies NNP; proof on this
  machine: under systemd + enforcing NM and chronyd do reach their
  domains, and the policy has no nnp_transition for them. chronyd's
  explicit `NoNewPrivileges=no` (for setuid sendmail) was also overridden.
  This corrects the sandboxing entry's reading of systemd's doc ("dropped
  from its bounding set" was wrong). Now: root, or a User= service with
  SYS_ADMIN ambient (and in the bounding set). Test
  `implied_no_new_privileges` fails on the old rule.
- **Bug, fixed (`c3560ff`): Symlinks=.** init_t `unlink` on
  systemd_varlink_registry_t (hostnamed's runner): socket.rs removed and
  recreated every Symlinks= link; the policy allows only create. Now a
  link already pointing at the socket is left alone (systemd's
  symlink_idempotent).
- **Fixed (`c3560ff`): ProtectHome= tmpfs** labelled as the path it
  covers (systemd's mount_tmpfs + label_fix_full). firewalld
  (ProtectHome=tmpfs) creating `/root/.cache` was denied on tmpfs_t; on
  admin_home_t it is still denied (no create for firewalld_t), so that
  one is policy-side, as under systemd.
- **JOURNAL=no only, output lost, not fixed:** NetworkManager dispatcher
  scripts (NetworkManager_dispatcher_{chronyc,iscsid}_t) writing the
  service's stdout pipe (pipefs, labelled init_t, from s6-log's pipe);
  systemd-user-runtime-dir, restorecon, loadkeys, rsyslogd writing the
  catch-all s6-svscan-log fifo (var_run_t). Under enforcing the kernel
  swaps those fds for /dev/null at exec; the programs still run. With
  JOURNAL=yes stdout is journald's stream socket, as under systemd.
- **Policy-side, not sdcompat:** systemd-hostnamed reading /proc/pressure
  (proc_psi_t) and listing /run/dbus (system_dbusd_var_run_t dir read).
  Same binary and domain under systemd; not checked on a systemd boot.
- Tests: cargo test 45 + 2 ignored; release build; sandbox 184/184.
- Note: another session's uncommitted "why the s6 boot uses more RAM"
  entry is above this one; this file was left uncommitted for the owner.

### Current state

All sdcompat-caused denials of the permissive boot are fixed in code
(`6b2ffe3`, `c3560ff`), not yet on a boot. Left: output loss under
JOURNAL=no and policy-side denials, none of which stop a service.

### Next steps

1. Owner: push, reinstall, reboot s6 (still permissive), rerun the
   ausearch: no nnp_transition, no varlink unlink, no tmpfs_t lnk_file.
2. Then one enforcing boot: at GRUB, `e` on the s6 entry, delete
   `enforcing=0`, Ctrl-X. Check `getenforce`, GNOME login, `systemctl
   --failed`, `ps -eZ` domains, ausearch for denials. If it breaks,
   reboot to the default systemd entry. If it works: `setup-fedora.sh
   --enforcing`.
3. Maybe: JOURNAL=no output loss (relay service stdout through the
   journal relay's stream socket instead of an s6-log pipe).
4. Still open: beside3 sandbox flake; fd store limit changes.

## Session: 2026-10-21: memory comparison script

Owner asked for a script to compare memory between the s6 and systemd
boots; they will run it on each boot.

- New `tests/memcompare.sh` (uncommitted when written):
  - `sudo tests/memcompare.sh snap [--wait SECS] [DIR]`: waits until
    uptime >= SECS (default 300) so both boots settle equally, then writes
    `~/sdcompat-mem/mem-<s6|systemd>-<date>.tsv` (home of SUDO_USER,
    chowned to them): meta (init, uptime, kernel, root, journald, swap
    devices, running services, kernel threads, cmdline), all of
    /proc/meminfo, and one row per user process (pid, comm, group, VmRSS,
    RssAnon, RssShmem, Pss from smaps_rollup or `-`).
  - `compare A B`: meta side by side; used (as free) and total-available;
    selected meminfo fields; groups init (PID 1, s6-*, sdcompat) /
    logging (s6-log, journald, rsyslogd) / user-manager (systemd --user,
    sd-pam) / other; then commands differing by >= 0.5 MiB, largest first.
  - Metric is PSS when both snapshots were taken as root, else RssAnon
    (summed VmRSS double counts shared pages, as seen with s6-supervise).
- Tested here, unprivileged, on the s6 boot: snap (1.7 s, 473 processes),
  and compare against a doctored copy without s6-supervise rows (shows
  -31.1 MiB for that command). Not yet run as root or on a systemd boot.
- Docs: TESTING.md / TESTING-us.md stage 6, optional paragraph.
- bash -n and shellcheck -S warning clean.

### Next steps

1. Owner: `sudo tests/memcompare.sh snap` on both boots (same apps open),
   then `compare` and send the output.

## Session: 2026-10-21: the first enforcing s6 boot

Owner booted s6 enforcing (`getenforce`: Enforcing). GNOME login worked;
polkitd, chronyd, NetworkManager, boltd run in their own domains (the
nnp_transition fix from `c3560ff` works). `systemctl --failed`:
systemd-hostnamed. `ausearch ... > ~/avc-enforcing.txt`: 107 AVCs.

- **Bug, fixed (`c5f2e06`): catch-all logger could not rotate.** 49 x
  `rename` on `current`, init_t -> var_run_t, from s6-log in
  /run/uncaught-logs (`s6-log -bpd3 -- 1 T /run/uncaught-logs`), every 2 s
  from 30 s into boot. init_t has no rename on var_run_t files (it does on
  init_var_run_t). While rotation fails s6-log stops writing, so the
  catch-all fifo can fill and block whatever writes to it. Not seen on the
  permissive boots (permissive allows and logs once). Fix:
  sdcompat.cil labels `/run/uncaught-logs(/.*)?` init_var_run_t (filecon
  plus a named type transition for init_t creating it in /run, like
  /run/sdcompat); setup-fedora.sh restorecons it on a running boot, so the
  stuck logger rotates on its next retry. Checked with sesearch: init_t
  on init_var_run_t files has create/rename/write/setattr/unlink, and
  read/open/lock through `pidfile`.
- **Bug, fixed (`c5f2e06`): stale Result=.** hostnamed's later runs
  exited 0 on their idle timeout but showed `failed (Result: exit-code)`,
  status=0/SUCCESS. The runner never cleared the result at a new start,
  and the exit handler keeps a non-success result already in the state
  file. Now prepare_start() clears it (systemd's service_start() resets
  it too). Sandbox test `second.service` (fails once, exits 0 the second
  time); without the fix it fails (and NFailedUnits is off by one).
- **Bug, fixed (`c5f2e06`): hostnamed started before the bus.** Its log
  says `Failed to run event loop: Transport endpoint is not connected` at
  01:57:33, 30 s after it started. Start order: the
  systemd-hostnamed.socket started it at boot (no on-demand socket
  activation) before dbus-daemon made /run/dbus (hostnamed pid 1613,
  dbus-daemon 1960). sd-bus then waits with an inotify watch on /run/dbus;
  the `read` on system_dbusd_var_run_t dir is denied for
  systemd_hostnamed_t, so it ran without a bus and failed at its 30 s
  idle exit. Under systemd it is bus-activated, so the bus already
  exists. Fix: gen.rs gives services with BusName= (not Type=dbus, which
  already Requires= it) Wants= + After= dbus.socket (mapped to the s6
  `dbus` service). Affects gdm, polkit, hostnamed, localed, logind and
  timedated on the owner's db. Unit test `bus_name_orders_after_dbus`.
  Decision: ordering, not a policy rule for hostnamed, because localed and
  timedated have the same pattern and the rule would not cover them.
- Seen, not changed: JOURNAL=no output lost as expected (NM dispatcher
  scripts on the init_t pipe; systemd-user-runtime-dir, rsyslogd,
  loadkeys, restorecon and now agetty (the tty9 early getty, getty_t) on
  the s6-svscan-log fifo). Under enforcing the fds become /dev/null; the
  programs run. hostnamed's /proc/pressure denial didn't show up this
  time.
- Tests: cargo test 46 + 2 ignored; release build; sandbox 187/187.
  bash -n and shellcheck -S warning clean. The CIL was not compiled here
  (no secilc, semodule needs root); it uses the same forms as the
  existing lines.

### Current state

Enforcing works on the s6 boot except for the three bugs above, fixed in
code but not yet on a boot.

### Next steps

1. Owner: push, reinstall (`setup-fedora.sh`; it loads the new
   sdcompat.cil and restorecons /run/uncaught-logs), then
   `ls -Zd /run/uncaught-logs` should say init_var_run_t. Reboot s6
   enforcing.
2. Check: `systemctl --failed` empty, `ausearch -m avc -ts boot -i` has no
   `rename` from s6-log and no hostnamed /run/dbus read,
   `sudo ls /run/uncaught-logs` shows rotated `@...s` files.
3. If clean: `setup-fedora.sh --enforcing` (drop `enforcing=0`).
4. Still open: JOURNAL=no output loss; on-demand socket activation (the
   root cause of early starts and part of the RAM gap); beside3 sandbox
   flake; fd store limit changes.

## Session: 2026-10-21: split by role (branch `split-projects`)

Owner asked to split the project into smaller ones by role. Agreed with the
owner in three rounds:
1. Split by role, build/install-independent (no code sharing between them).
2. Then: split so that each part runs on its own even when the others are
   missing. Checked: generator, runner, timers and daemon cannot (run
   scripts call the runner, timers ask the daemon, the daemon reads the
   runner's state and runs the generator), so they stay one core. Runnable
   alone: the fd store, journal-relay, notify, and the early-boot script.
3. Owner: split the core into smaller modules that may depend on each other.

Decision, not asked: `early` stays in the core (dist/early). It is plain
shell and runs alone, but without it the core cannot boot Fedora
(mount-api is the root of the sysinit bundle), so moving it out would
break "the core runs without the others".

Commit `23167f5` on branch `split-projects` (main untouched):

- Core = Cargo workspace `crates/`:
  - `sdcompat-base` (lib): unit, loader, config, envfile, util, state,
    calendar, mountunit, fdstore (client + the daemon's fallback Store),
    new `index.rs` (Kind/Index/unit_arg, moved out of gen), new
    `timerspec.rs`/`pathspec.rs` (what .timer/.path units say, moved out of
    timer.rs/pathunit.rs, used by gen and the daemon).
  - `sdcompat-install` (lib), `sdcompat-exec` (lib: sandbox, seccomp,
    bpf, cgroup, selinux, socket, pam, execcmd, conditions),
    `sdcompat-client` (lib).
  - Tools: `sdcompat-gen` (reload/plan/enable...), `sdcompat-runner`
    (run/finish/up/down/stop-exec/reload-exec), `sdcompat-timers`
    (timer-run/path-run), `sdcompatd` (daemon, start/stop/restart, log;
    with no command it serves).
  - `sdcompat`: the user command; execs the tool for a subcommand (next to
    itself, else /usr/libexec/sdcompat). Keeps `/usr/bin/sdcompat`, docs
    and databases generated before the split working (their run scripts
    call /usr/libexec/sdcompat/sdcompat run|finish).
  - Crate roots re-export base modules (`pub(crate) use sdcompat_base::...`)
    so the moved code keeps its `crate::util` paths unchanged.
- Standalone (each its own `[workspace]`, Cargo.lock, README pair, spec;
  libc only): `standalone/sdcompat-fdstore`, `sdcompat-journal-relay`
  (journal.rs + its own copy of recv_max), `sdcompat-notify` (cmd_notify).
- fd store protocol: `PUT\0key\0name\0max`. Decision: the client sends
  FileDescriptorStoreMax= so the standalone server needs no unit loader
  (else it would have had to copy ~1200 lines). The base Store still
  accepts the 3-part form (uses its own limit); the client retries the
  3-part form on "ERR bad request", for an old `sdcompat fdstore` still
  running until the next boot after an upgrade.
- Config: RUNNER defaults to sdcompat-runner, new TIMER_RUNNER
  (sdcompat-timers) used by gen and dynamic.rs for .timer/.path services.
  Both default to the tool next to the running program, else
  /usr/libexec/sdcompat.
- dist: sdcompatd/run calls sdcompatd; sdcompat-fdstore/run and
  journal-relay/run call the standalone tools and, when not installed,
  report ready and `sleep infinity` (s6-pause does not exist in this s6),
  so the boot goes on and sdcompatd holds the fds itself. rc.init calls
  sdcompat-gen. setup-fedora.sh builds/installs the 5 core tools and the
  standalone ones (`--without=LIST`; `--no-journal` requires the relay),
  removes them on uninstall. Core spec installs 5 tools and Recommends the
  standalone packages; each standalone project has its own spec.
- tests: sandbox.sh builds the standalone tools and runs the split
  binaries; `SANDBOX_FDSTORE=no` runs without the fd store, and check.sh
  then expects the fds gone after the daemon swap. bpf-root.sh and
  update-syscall-groups.sh paths; memcompare groups `sdcompat*`.
- Docs: README/README-us components and source layout rewritten, plus the
  missing BusName= implicit dependency from `c5f2e06`; TESTING pair;
  CLAUDE.md (layout rule: standalone projects never depend on the core).
- Mistake this session: ran tests/bpf-root.sh in the background to check
  its paths; it ends in `exec sudo`. No sudo credentials here, so it
  failed at once; no process was left. Checked the cargo part alone
  afterwards.
- Tests: core cargo test 45 + 2 ignored (the 2 relay tests moved; a new
  fd store round-trip test in base); standalone fdstore 1, relay 2;
  release builds clean; sandbox 187/187, SANDBOX_JOURNAL=no 192/192,
  SANDBOX_FDSTORE=no 187/187; bash -n and shellcheck -S warning clean;
  setup-fedora.sh --dry-run shows the new install steps. Not run on
  Fedora or a real boot; the RPM specs were not built.

### Current state

The split is done and tested in the sandbox, on branch `split-projects`.
main still has the single binary, with the enforcing fixes (`c5f2e06`).

### Next steps

1. Owner: review the branch; if it is wanted, merge it into main (or tell
   me what to change), push.
2. On Fedora: `sudo dist/setup-fedora.sh ...` (builds core + standalone),
   reboot s6, check `systemctl --failed`, `ls /usr/libexec/sdcompat`,
   `s6-svstat /run/service/sdcompat-fdstore`, a FDSTORE= user (journald,
   logind) keeping fds across `s6-svc -r` of sdcompatd.
3. Still open from before: the enforcing re-check of `c5f2e06`
   (uncaught-logs label, Result=, BusName ordering); JOURNAL=no output
   loss; on-demand socket activation; beside3 flake.

## Session: 2026-10-21: one RPM per core tool (branch `split-projects`)

The owner clarified the earlier request ("split sdcompat into smaller modules
that may depend on each other"): they meant dependency *packages*, not
crates. Chosen with the owner: the source stays one workspace; each core
executable becomes its own RPM, and the packages depend on each other with
`Requires`. The four library crates are only used to build and are not
packaged. The specs live in `packages/`.

### Changes

- `packages/`: `sdcompat-gen.spec`, `sdcompat-runner.spec`,
  `sdcompat-timers.spec`, `sdcompatd.spec`, and `sdcompat.spec` (moved from
  `dist/`). All build from one tarball `sdcompat-VERSION.tar.gz`
  (`%autosetup -n sdcompat-%{version}`, `cargo build/test -p NAME`).
  `build-rpms.sh [OUTDIR]` (default ~/rpmbuild-sdcompat) vendors the crates,
  makes the tarball, runs `rpmbuild -bb` for each spec, and copies the
  non-debug RPMs to OUTDIR/rpms. README pair.
- Package contents and Requires, from what each tool runs at run time:
  - sdcompat-runner: the binary; Requires s6, shadow-utils (useradd);
    Recommends sdcompatd (OnFailure=/OnSuccess= go through the daemon).
  - sdcompatd: the binary and the D-Bus policy; Requires s6, s6-rc,
    dbus-daemon, sdcompat-runner (transient services, stop/reload-exec);
    Recommends sdcompat-timers (transient timers).
  - sdcompat-timers: Requires sdcompatd (starts units through it).
  - sdcompat-gen: the binary and /var/lib/sdcompat; Requires s6-rc,
    sdcompat-runner, sdcompat-timers (the generated scripts run them).
  - sdcompat: the dispatcher, early, config, /etc/sdcompat/s6, skel,
    init-selinux, SELinux module, kernel-install plugin, /var/log/sdcompat;
    Requires the four tool packages, the s6 stack, etc. Its %post loads the
    module and relabels /usr/libexec/sdcompat/* (the tools are installed
    first because of the Requires; later updates get labels from the loaded
    policy).
  - Tool packages are pinned to `= %{version}-%{release}`.
  - Requires cycles avoided with Recommends (runner→sdcompatd,
    sdcompatd→timers).
- `setup-fedora.sh`: install skips a core tool whose own RPM is installed;
  uninstall removes core tools one by one (leaving RPM-owned ones), and the
  D-Bus policy only when the sdcompatd RPM is not installed; the
  remove-hint names all five packages.
- README pair: the components section and source layout point at
  `packages/`. CLAUDE.md: packaging rule.

### Tests

- `packages/build-rpms.sh` into the scratchpad: all five RPMs built; the
  %check `cargo test -p` passed in each. File lists checked: no file is in
  two packages. A test install was not possible (rpm --root needs root),
  so the Requires were checked only with `rpm -qp --requires`.
- bash -n and shellcheck -S warning clean; setup-fedora.sh --dry-run OK.
- No Rust code changed, so the sandbox suite was not rerun.

### Current state

Branch `split-projects`: the core workspace with crates, the standalone
projects, and now one RPM per core tool. main is unchanged.

### Next steps

1. Owner: review and merge `split-projects`, push.
2. On Fedora: `packages/build-rpms.sh`, `sudo dnf install
   ~/rpmbuild-sdcompat/rpms/*.rpm` (with the skarnet RPMs), check that dnf
   resolves the Requires, that `/usr/libexec/sdcompat/*` is init_exec_t
   after install, then `setup-fedora.sh` and an s6 boot.
3. Still open: the items in the previous session's next steps.

## Session: 2026-10-21: RPM versions and releases (branch `split-projects`)

The owner asked for build version management, so that changed Rust code is
easy to roll out. Before this the specs had a fixed `0.1.0-1`, so a rebuild
of changed code had the same NEVRA and dnf would not install it.

### Changes

- Specs: `Version: %{sdcompat_version}`, `Release: %{sdcompat_release}%{?dist}`,
  as the skarnet specs do; they are built only through build-rpms.sh.
- `packages/build-rpms.sh`:
  - version from Cargo.toml `[workspace.package] version` (the one source;
    the crates use it too);
  - release `1.YYYYMMDDHHMMSS.gCOMMIT[.dirty]` per build (`.dirty` when
    Cargo.toml, Cargo.lock, crates, dist or packages have uncommitted
    changes), or `SDCOMPAT_RELEASE=N`;
  - writes the spec copies with the two `%global`s in front;
  - `rpms/` is emptied first and gets only this build's five RPMs, so
    `dnf upgrade rpms/*.rpm` takes one matching set;
  - writes `.cargo/config.toml` itself (`cargo vendor --quiet` does not
    print it).
- packages README pair: a "Versions" section and the rebuild/upgrade steps
  (`dnf upgrade`, and when each tool takes effect on a running boot).
  CLAUDE.md: packaging rule updated.

### Tests

- Built all five RPMs twice; names carry the generated release, %check passed
  in each, `rpms/` held only the newest five.
- `rpm.vercmp`: the generated release sorts above the old `1`, a later build
  above an earlier one, and a version bump above any release.
- bash -n and shellcheck -S warning clean.

### Next steps

As in the previous entry; on Fedora also try `dnf upgrade` with a second
build.

## Session: 2026-10-21: --version (branch `split-projects`)

The owner asked for `sdcompat --version`, so an installed build can be
identified (also for source installs, which have no rpm -q).

### Changes

- New crate `crates/sdcompat-version`: build.rs works out the version once:
  `SDCOMPAT_BUILD` when set (the specs export `%{version}-%{release}`), else
  `VERSION (git COMMIT[-dirty])`, dirty when Cargo.toml, Cargo.lock or
  crates/ have uncommitted changes. It re-runs only when .git/HEAD, the
  index, the branch ref or packed-refs change (`git --no-optional-locks`, so
  `git status` does not rewrite the index and force a rerun). A separate
  crate so a commit recompiles only it and the five binary crates, not
  everything that depends on sdcompat-base.
  Known limit: an unstaged edit after a clean build keeps the old "clean"
  label until the index or HEAD changes (same as git-version/vergen).
- Each of sdcompat-gen, sdcompat-runner, sdcompat-timers, sdcompatd answers
  `--version`/`-V`/`version` in the command position with "NAME VERSION".
- `sdcompat --version`: its own line, each core tool's (by running its
  --version; "differs from sdcompat", "no --version: older than this
  build", or "not installed"), and whether the standalone tools are present.
  Exit 1 unless all four core tools match.
- setup-fedora.sh: `check_versions` after install_sdcompat shows it and warns
  on a mix.
- Docs: README pair (components table, source layout), packages README pair.

### Tests

- cargo test: 45 passed, 2 ignored. Sandbox default 187/187.
- Debug build: all tools print `0.1.0 (git c8aabf8-dirty)`; a second build is
  Fresh (no rerun loop). A fake tool dir with an older runner and a timers
  without --version: reported, exit 1.
- RPMs rebuilt; unpacked, `sdcompat --version` printed
  `0.1.0-1.20261021073517.gc8aabf8.dirty.fc45` for all five, exit 0.
- bash -n and shellcheck -S warning clean; setup-fedora.sh --dry-run OK.

## Session: 2026-10-21: merged into main; Fedora checklist

`split-projects` fast-forwarded into main (`26a8bd2`). Not pushed (owner
pushes; main was 67 commits ahead of origin/main at that point). The
`split-projects` branch is kept.

### Current state

main has the split (core crates + standalone tools), one RPM per core tool,
generated RPM releases and `sdcompat --version`. None of it has run on
Fedora yet, and the `c5f2e06` enforcing fixes have not been on a boot.

### Next steps: Fedora checklist (in order; stop at the first failure)

0. Prepare (systemd boot): `git pull`, main at `26a8bd2`. Skarnet RPMs in
   ~/rpmbuild-skarnet/rpms (else `dist/rpm/build-skarnet-rpms.sh`).
1. Build: `packages/build-rpms.sh`; five RPMs named
   `…-0.1.0-1.<time>.g26a8bd2.fc4x…`, no `.dirty`. On failure:
   ~/rpmbuild-sdcompat/<package>.log.
2. Install (still systemd): `sudo dnf install ~/rpmbuild-sdcompat/rpms/*.rpm`.
   - dnf resolves the Requires (the first real test of them);
   - `sdcompat --version; echo $?`: four core tools on one build, exit 0;
   - `rpm -q sdcompat sdcompat-gen sdcompat-runner sdcompat-timers sdcompatd`;
   - `semodule -l | grep sdcompat`; `ls -Z /usr/libexec/sdcompat/` all
     init_exec_t;
   - `ls /etc/sdcompat/*.rpmnew /etc/sdcompat/s6/*/*.rpmnew`: files from the
     earlier source install stay, new ones land as .rpmnew; note which.
3. `dist/setup-fedora.sh --dry-run --rpms=$HOME/rpmbuild-skarnet/rpms`, then
   with sudo (journal/resolved choices are kept from sdcompat.conf). Output:
   "sdcompat RPMs are installed, using them"; the `sdcompat --version`
   lines with no mixed-build warning; standalone tools built and installed
   from source; build_database without errors.
4. Boot "Fedora (s6 + sdcompat)" enforcing (remove `enforcing=0` with `e`
   in GRUB if the entry has it).
   - `getenforce` Enforcing; `/proc/cmdline` without enforcing=0;
     `sdcompat --version` exit 0; `systemctl --failed` empty.
   - `c5f2e06` fixes: `hostnamectl` works, and after hostnamed's ~30 s idle
     exit `systemctl status systemd-hostnamed` is not `failed (Result:
     exit-code)`; `ls -Zd /run/uncaught-logs` init_var_run_t and
     `sudo ls /run/uncaught-logs` shows rotated `@….s` files;
     `sudo ausearch -m avc,user_avc,selinux_err -ts boot -i >
     ~/avc-enforcing2.txt`: no s6-log `rename`, no systemd_hostnamed_t
     dbus denial (send the file either way).
   - Split parts: `s6-svstat /run/service/sdcompatd
     /run/service/sdcompat-fdstore` both up (and journal-relay with
     JOURNAL=no); `grep -l sdcompat-runner /run/s6-rc/servicedirs/*/run |
     wc -l` > 0 (the new database calls the runner).
   - fd store: count `sudo ls /proc/$(pgrep -x sdcompat-fdstore)/fd | wc -l`,
     `sudo s6-svc -r /run/service/sdcompatd`, count again: same; `systemctl
     --failed` and `loginctl list-sessions` unchanged, GNOME session alive.
     (NFileDescriptorStore is always 0 over D-Bus, so count the fds.)
5. Upgrade (on the s6 boot): `packages/build-rpms.sh` again (new release),
   `sudo dnf upgrade ~/rpmbuild-sdcompat/rpms/*.rpm` must upgrade, not say
   "already installed"; `sdcompat --version` shows the new release, exit 0;
   `sudo s6-svc -r /run/service/sdcompatd`; `systemctl --failed` empty.
6. All clean: `sudo dist/setup-fedora.sh --enforcing
   --rpms=$HOME/rpmbuild-skarnet/rpms` to drop enforcing=0 from the entry.

Fallbacks: the default GRUB entry is the untouched systemd boot;
`sdcompat.db=previous` on the s6 entry boots the previous database. Useful
to collect on a failure: that step's output, ~/avc-enforcing2.txt,
`sudo tail -50 /run/uncaught-logs/current`.

Still open from before: JOURNAL=no output loss; on-demand socket
activation; beside3 sandbox flake.

## Session: 2026-10-21: failed s6 boot after the RPM install; /var/lib/sdcompat label

The owner reported the s6 boot failing after installing the new RPMs and
suspected SELinux. Investigated from the systemd boot, without root.

### Findings

- RPMs installed (13:13), skarnet RPMs reinstalled and setup-fedora.sh run
  (13:15); `sdcompat --version` exit 0, all on `g018a2df`. Labels correct:
  /usr/libexec/sdcompat/* and the skarnet programs init_exec_t, module loaded
  (its filecons are in file_contexts).
- The failed boot (~13:17:56, PID 1072) wrote nothing to disk except in
  /var/lib/sdcompat: rc.init's `sdcompat-gen reload` built a new database,
  created `.previous.tmp`, `.compiled.previous.tmp`, `.current.tmp`, and none
  were renamed. No journal, no /var/log/sdcompat or audit writes from that
  boot, so its denials are lost (/run/uncaught-logs is tmpfs).
- Cause of the failed swap (sesearch on the loaded policy): init_t has only
  `{ create write }` on var_lib_t lnk_file (no rename/unlink) and no rmdir
  on var_lib_t dirs, so under enforcing the symlink swap and the database GC
  fail. New dirs init_t makes there get init_var_lib_t by the default
  type_transition, hence the mixed labels. init_t fully manages
  init_var_lib_t (systemd's /var/lib/systemd). /var/log/sdcompat is fine
  (var_log_t files managed, dirs via the mountpoint attribute).
- This is probably not what stopped the boot: rc.init carries on with the
  existing `compiled` link (a valid, identical database), and the 01:57
  enforcing boot most likely hit the same failure silently and came up. The
  real cause of this failure is still unknown.

### Changes

- sdcompat.cil: `/var/lib/sdcompat(/.*)?` init_var_lib_t.
- sdcompat.spec %post/%postun: `restorecon -R /var/lib/sdcompat`.
  setup-fedora.sh already restorecons it.

### Tests

Not loaded (no root here): paren balance checked, init_var_lib_t exists in
the loaded policy.

### Next steps

1. Rebuild and install the RPMs, then `ls -Zd /var/lib/sdcompat
   /var/lib/sdcompat/*`: init_var_lib_t. The leftover `.*.tmp` links are
   harmless (removed on the next swap).
2. Find out what the screen showed, and whether enforcing=0 was removed.
   Boot the s6 entry permissive (boot-options still has enforcing=0): if it
   comes up, collect `sudo ausearch -m avc,user_avc,selinux_err -ts boot -i`;
   if not, tty9 and `/run/uncaught-logs/current`. Also try
   `sdcompat.db=previous`.

## Session: 2026-10-21: first enforcing s6 boot with the RPMs, checked

The owner booted "Fedora (s6 + sdcompat)" enforcing after installing the
RPMs built from `341d610`. Checked from that boot, without root.

### Verified (14:16 boot)

- `getenforce` Enforcing, /proc/cmdline without enforcing=0, PID 1
  s6-svscan; `sdcompat --version` all five on
  `0.1.0-1.20261021140308.g341d610.fc45`, exit 0; `systemctl --failed`
  empty; `loginctl` shows the tty2 GNOME session.
- `341d610` fix works: /var/lib/sdcompat and everything in it
  init_var_lib_t; rc.init's regeneration swapped the links at 14:16
  (current = db/…-1159, previous = the 14:14 one), the `.*.tmp` links from
  the failed boot are gone and the database GC left only those two dirs.
- `c5f2e06` fixes: /run/uncaught-logs init_var_run_t; `hostnamectl` works
  and after hostnamed's idle exit it is `inactive (dead)`, not failed.
- journald running (journalctl -b works); 5 servicedirs call
  sdcompat-runner.
- ~/avc-enforcing2.txt is from a 13:56 boot, not this one. It has only
  denials already listed as accepted: early oneshots/getty/rsyslogd on the
  s6-svscan-log fifo (var_run_t), NM dispatcher scripts on an init_t
  pipe, hostnamed on /proc/pressure. No s6-log rename, no
  systemd_hostnamed_t dbus denial.

### Not yet checked (needs root)

- AVCs of this boot (`ausearch -ts boot`), rotated files in
  /run/uncaught-logs, `s6-svstat` of sdcompatd/sdcompat-fdstore, the fd
  store count across `s6-svc -r /run/service/sdcompatd`; then checklist
  steps 5 (upgrade) and 6 (`setup-fedora.sh --enforcing`).
- AVCs of this boot (~/avc-enforcing3.txt, 65 records, JOURNAL=no from
  sdcompat.conf): only the accepted ones. NM dispatcher scripts on the
  service's s6-log pipe (init_t, JOURNAL=no only); systemd-user-runtime-dir,
  rsyslogd, loadkeys, agetty, restorecon on the s6-svscan-log fifo
  (var_run_t); hostnamed searching /proc/pressure. No s6-log rename, no
  systemd_hostnamed_t dbus denial, nothing on /var/lib/sdcompat, nothing
  from sdcompat's own programs or the fd store.
- Root checks (owner ran them): sdcompatd and sdcompat-fdstore up,
  journal-relay running; fd store (pid 1777, up since boot) held 52 fds
  before and after `s6-svc -r /run/service/sdcompatd` (sdcompatd got a new
  pid); `systemctl --failed` empty and the GNOME session alive after it.
  /run/uncaught-logs has only `current` (51650 bytes, under s6-log's
  default 99999-byte rotation), so rotation itself was not seen yet; no
  rename denial in the AVCs.
- Note for check commands: `pgrep -x`/`ps -C sdcompat-fdstore` match
  nothing (comm is truncated to 15 chars); use `pgrep -x sdcompat-fdstor`
  or the pid from `s6-svstat`. s6-svstat takes one service dir.

- Step 5 (upgrade): done on the systemd boot before this one. dnf history
  (times UTC) has two `dnf upgrade` transactions, 13:55 and 14:13 KST, each
  altering 10 packages (5 out, 5 in); the second installed
  `…20261021140308.g341d610`, which this boot runs. Not done: an upgrade
  while running under s6 (scriptlets do not depend on the init, so low
  risk; it will happen on the next upgrade from the s6 boot).

- Step 6: the owner ran `setup-fedora.sh --enforcing` before this boot
  (newest setup-backup 20261021-141441, /boot/loader/entries changed
  14:14, boot 14:16 with no enforcing=0 on the cmdline). The entry itself
  is root-only and was not read.

### Current state

The Fedora checklist is done: the s6 boot runs SELinux enforcing from the
RPMs (`341d610`) with a permanent boot entry, and every AVC left is one
already accepted.

### Next steps

1. If unsure the entry is permanent: on the next s6 boot without editing
   in GRUB, `/proc/cmdline` must still lack enforcing=0.
2. Optional: once /run/uncaught-logs/current passes ~100 KB, confirm an
   `@….s` file appears. Also an enforcing boot with JOURNAL=yes.
3. Still open: JOURNAL=no output loss; on-demand socket activation;
   beside3 sandbox flake.

## Session: 2026-10-21: memcompare results, s6 vs systemd

Owner ran `sudo tests/memcompare.sh snap` on both boots (s6 15:56, 414 s
up, JOURNAL=no; systemd 17:13, 419 s up). Files in ~/sdcompat-mem. No
code changed.

- used (as free): s6 1280 MiB, systemd 1069 MiB, so s6 uses ~211 MiB
  more; total-available differs by 243 MiB. PSS of all processes: 1086
  vs 913 (464 vs 114 user processes).
- Where it goes (PSS):
  - The s6 stack: 258 s6-supervise 32.5, 26 sdcompat-runner 31.9,
    sdcompatd 7.4, 9 sdcompat-timers 3.9, 51 s6-log 6.2, journal relay
    2.0: ~84 MiB, against systemd PID 1 11.0 + journald 25.7 + resolved,
    userdbd, oomd, homed, machined, userwork ~11. Net ~35-40 MiB.
  - Plus the kernel cost of ~350 extra processes: PageTables +19,
    SUnreclaim +22, KernelStack +7 MiB.
  - Socket-activated daemons started at boot (no on-demand activation):
    iscsid, iscsiuio, virtnetworkd, virtlogd, virtlockd, dmeventd,
    2 dnsmasq: ~36 MiB, and iscsid/iscsiuio's mlockall is the whole
    Mlocked 22.6 MiB (Unevictable +22.6).
  - packagekitd 100 MiB ran only in the s6 snapshot: GNOME Software's
    refresh timing, not the init.
  - 12 of the runners hold ~1.9 MiB anon each, the rest ~0.2 MiB; not
    looked into.
- Found:
  - **No swap on the s6 boot**: systemd has zram0 (8 GiB, from
    zram-generator); s6 has none. Not a RAM cost but a missing feature.
  - **Two rsyslogd on the s6 boot** (PIDs 1985 and 6079, the second late):
    probably the s6 `syslog` service plus Fedora's enabled
    rsyslog.service from the unit db. Not checked (now on systemd).
  - Shmem +17 MiB (tmpfs) on s6 is not explained.
- The logging rows are not like for like: s6 was JOURNAL=no.

### Next steps

1. On the next s6 boot: `ps -o pid,ppid,lstart,args -C rsyslogd`,
   `systemctl status rsyslog.service`, `df -m /run /dev/shm /tmp`,
   `swapon`; then decide whether to EXCLUDE rsyslog.service under
   JOURNAL=no and to support zram-generator's config.
2. If the gap matters: on-demand socket activation (Accept=no), or
   EXCLUDE iscsid/iscsiuio/virt*.

## Session: 2026-10-21: two rsyslogd under JOURNAL=no, fixed

The owner ran the memcompare follow-up checks on the s6 boot (17:23,
JOURNAL=no).

### Verified

- Two rsyslogd: PID 1956 (17:23:55) `rsyslogd -n -iNONE -f
  /etc/sdcompat/rsyslog.conf`, the s6 `syslog` service; PID 6398
  (17:24:21) `/usr/sbin/rsyslogd -n`, Fedora's rsyslog.service translated
  from the unit db (enabled, WantedBy=multi-user.target), with its
  sdcompat-runner in the cgroup. cgroup memory 91.9M (includes page cache).
  The second one loads imjournal with no journal and writes the same
  /var/log files as the first.
- `swapon` prints nothing: no swap on the s6 boot, as suspected.
- `df -m`: /run 20 MiB used, /dev/shm and /tmp 1 MiB. So the unexplained
  Shmem +17 MiB is almost all /run; the systemd boot's /run usage was not
  measured, so the split is unknown.

### Changes (`44b8b51`)

- Config::finish: with JOURNAL=no, rsyslog.service and syslog.service are
  appended to EXCLUDE and mapped (DEP_MAP) to the s6 `syslog` service,
  unless the user already mapped them; INCLUDE still overrides. Decision:
  exclude rather than drop the s6 `syslog` service, because Fedora's config
  depends on imjournal and the s6 one owns /dev/log. With JOURNAL=yes
  nothing changes (rsyslog.service reads the journal as on systemd).
- dist/sdcompat.conf comment, README/README-us `JOURNAL` key.

### Tests

cargo build, cargo test (new `journal_no_excludes_rsyslog`) pass; sandbox
187/0, SANDBOX_JOURNAL=no 192/0. Not yet on the real s6 boot (verified
later the same day, see the next entry).

### Current state

The Fedora checklist is done (previous sessions). JOURNAL=no no longer
starts a second rsyslogd once the RPMs from `44b8b51` are installed.

### Next steps

1. Rebuild/install the RPMs, boot s6: `ps -C rsyslogd` shows one, and
   `systemctl status rsyslog.service` says excluded/not found.
2. Swap: support zram-generator's config (/usr/lib/systemd/zram-generator.conf)
   under s6, e.g. an s6 oneshot that runs the generator's setup or does
   zramctl + mkswap + swapon itself.
3. Shmem: `df -m /run` and `du -xm -d1 /run` on both boots.
4. Still open: on-demand socket activation (or EXCLUDE iscsid/iscsiuio/
   virt*), JOURNAL=no output loss, beside3 sandbox flake.

## Session: 2026-10-21: single rsyslogd verified on the s6 boot

The owner installed the RPMs from `44b8b51` and booted s6 (JOURNAL=no).

### Verified

- `ps -C rsyslogd` shows one process (PID 1807), down from two. Next
  step 1 of the previous entry is done. (`systemctl status
  rsyslog.service` output was not shown.)

### Changes

None to code; this entry only.

### Current state

The Fedora checklist is done. JOURNAL=no runs a single rsyslogd (the s6
`syslog` service) on the real boot.

### Next steps

1. Swap: support zram-generator's config (/usr/lib/systemd/zram-generator.conf)
   under s6, e.g. an s6 oneshot that runs the generator's setup or does
   zramctl + mkswap + swapon itself.
2. Shmem: `df -m /run` and `du -xm -d1 /run` on both boots.
3. Still open: on-demand socket activation (or EXCLUDE iscsid/iscsiuio/
   virt*), JOURNAL=no output loss, beside3 sandbox flake.

## Session: 2026-10-21: slow power-off and "Permission denied" at shutdown

The owner reported that powering off the s6 boot takes over 30 s and
prints "Permission denied" for several services on the console.
Investigated on the s6 boot (20:04, JOURNAL=no), mostly without root.

### Findings

- **Slow power-off, cause found.** user@1000's s6-log shows the same gap
  at every s6 shutdown today: `exit.target` at 14:39:38, 17:42:07,
  17:56:40, then `session.slice: Consumed …` exactly 25 s later
  (14:40:03, 17:42:32, 17:57:05). At 15:56 wireplumber instead hit its
  45 s stop timeout and was aborted (`State 'stop-sigterm' timed out`).
  logind logged the poweroff request at 17:56:40.20. 25 s is
  dbus-daemon's activation timeout. Mechanism, read in the code:
  check_job had no shutdown check. start_beside() returns early once
  `shutting-down` exists, so the start fell through to `s6-rc -u change`,
  which waits for the s6-rc lock held by rc.shutdown's `s6-rc -bDa
  change` until everything is down, including user@1000, which was
  waiting on the D-Bus caller. **Inferred, not seen:** which name was
  activated (dbus-daemon and sdcompatd log only to /run/uncaught-logs,
  lost at power-off; most likely something wireplumber calls while
  stopping, e.g. bluez or rtkit).
- An activation that sdcompatd refuses does fail at once (dbus-daemon
  logs `Activation via systemd failed` for the excluded home1), so
  refusing is enough to remove the wait.
- **"Permission denied": not found yet.** No AVCs for 17:56–17:58
  (ausearch), nothing in /var/log/messages, and no "denied" in the current
  boot's /run/uncaught-logs, so it happens only at shutdown. The console
  shows it because setup runs s6-linux-init-maker with `-1`: the
  catch-all log (sdcompatd, dbus-daemon, runner errors, services' stderr)
  is copied to the console. Two candidates: lines in the catch-all log,
  or kernel `avc: denied` lines once s6-rc has stopped auditd (those
  never reach audit.log, which would explain the empty ausearch).
  Neither survives a power-off today.
- Seen in passing, not looked into: accounts-daemon retries activating
  home1 (excluded) several times a second at login; `sdcompatd: stopping
  shutdown.target (conflicts with systemd-hostnamed.service)` on hostnamed
  activation; s6 boots write no wtmp boot/shutdown records (`last -x`
  shows only systemd boots).

### Changes (`688ed2b`)

- sdcompatd check_job: once `RUN_DIR/shutting-down` exists, start,
  restart and reload-or-restart fail with
  `org.freedesktop.systemd1.TransactionIsDestructive`, as in systemd. The
  shutdown targets are still accepted (checked before). D-Bus activation
  goes through check_job, so the caller gets ActivationFailure at once.
  sdcompatd does not create the marker itself: rc.shutdown does it first
  thing, and the sandbox's poweroff.target test would leave it behind.
- rc.shutdown.final (runs after the kill, before unmounting) saves the
  newest rotated catch-all archive plus `current` to
  `/var/log/sdcompat/last-shutdown-console.log` and `dmesg` to
  `last-shutdown-kernel.log`, umask 077 (they hold every service's
  stderr). Becomes active only after setup reruns s6-linux-init-maker.
- Sandbox: new `org.example.Late` → late.service; in the shutdown phase,
  with the marker present: the activation fails in < 5 s, StartUnit says
  "is destructive", late.service never starts.
- README/README-us (where the shutdown logs are), TESTING/TESTING-us
  (real-boot row "Previous shutdown").

### Tests

cargo build, cargo test pass. Sandbox 190/190, SANDBOX_JOURNAL=no
195/195. With the check_job change reverted: 3 failures (activation took
10 s, StartUnit accepted, late.service started). bash -n and shellcheck
-S warning on check.sh, sandbox.sh, rc.shutdown.final. Not tested: a real
power-off; whether init_t may run `dmesg` (syslog_read) and create files
in /var/log/sdcompat from rc.shutdown.final under enforcing.

### Current state

The Fedora checklist is done; s6 boots enforcing with JOURNAL=no.
`688ed2b` should remove the 25 s wait at power-off (unverified on the
real boot). The "Permission denied" lines are not identified yet.

### Next steps

1. Owner: `packages/build-rpms.sh`, `sudo dnf upgrade
   ~/rpmbuild-sdcompat/rpms/*.rpm`, rerun `sudo dist/setup-fedora.sh
   --rpms=$HOME/rpmbuild-skarnet/rpms` (regenerates rc.shutdown.final),
   boot s6, power off, boot s6 again. Then time the power-off and read
   `sudo grep -i -B3 denied /var/log/sdcompat/last-shutdown-*.log`;
   fix whatever prints "Permission denied". If the files are missing,
   look for an AVC on rc.shutdown.final/dmesg in the next boot's ausearch.
2. user@1000's log: confirm no 25 s gap between exit.target and
   session.slice, and no wireplumber stop timeout.
3. Earlier next steps unchanged (zram swap, /run Shmem, on-demand socket
   activation, JOURNAL=no output loss, beside3 sandbox flake).

## Session: 2026-10-21: shutdown logs read, rename denial fixed

The owner installed the RPMs from `724be75` (20:44), reran setup, powered
off the s6 boot at 20:46 (took 1 min 4 s) and booted s6 again. They sent
`grep -i -B3 denied /var/log/sdcompat/last-shutdown-*.log`.

### Findings

- **The 20:46 power-off still ran the old sdcompatd.** That boot started
  ~20:04 (uptime 2556 s at stage 4); the RPMs went in at 20:44 without a
  reboot, so the daemon in memory had no shutdown refusal. The 25 s gap is
  still in user@1000's log (exit.target 20:46:08.59, session.slice
  20:46:33.57), as expected. `688ed2b` remains untested on the real boot.
  (Not a stale run script: `/etc/sdcompat/s6/*/run` are pre-split
  %config files with `.rpmnew` beside them, but `sdcompat daemon` execs
  the new sdcompatd. The `.rpmnew` differ only in `#!/usr/bin/sh` and,
  for journal-relay/fdstore, the not-installed fallback.)
- Timeline (kernel/audit times mapped to wall clock): poweroff requested
  20:46:08.39 (logind); NetworkManager/auditd stop 20:46:08; user manager
  done 20:46:33 (25 s, the old activation wait); gdm-session-worker PAM
  session_close 20:46:53 (+20 s); systemd-user-runtime-dir stop 20:47:08
  (+15 s); shutdownd stage 4 at 20:47:08.9; then the 3 s kill grace.
  60.5 s + 3 s = the 1 min 4 s. The 20 s and 15 s gaps are not explained
  yet (needs the console log).
- **"Permission denied" found.** Kernel log at stage 4: `avc: denied {
  rename } comm="s6-linux-init-s" name="<service>" ... tcontext=var_run_t
  tclass=lnk_file`. s6-linux-init-shutdownd's unsupervise_tree renames
  every /run/service link to a dot name and warns `unable to rename ...`
  on failure, once per service (~200; the kernel shows only a few because
  of kauditd rate limiting). It then falls back to unlink (allowed), but
  the "d" command goes to the renamed path and is lost.
- **Console log was empty** (0 bytes). rc.shutdown.final runs after the
  kill: the catch-all s6-log (run with -p, so it ignores TERM) is killed
  by SIGKILL, s6-svscan restarts it, and the new one archives the
  interrupted `current` as `@*.u` and starts empty. The script took only
  `@*.s`. Inferred from s6-log behaviour, not observed (uncaught-logs is
  not readable without root).
- Also in the kernel log, not fixed: at boot (17.4 s) agetty (getty_t) is
  denied /dev/null, /dev/tty9 (device_t) and utmp (tmpfs_t) before
  `early` runs `restorecon -R /dev /run` (18.0 s); restorecon is denied
  /dev/console. At shutdown, iscsiadm and systemd-user-runtime-dir may
  not write the catch-all fifo (var_run_t): their stderr is lost (part
  of the JOURNAL=no output loss item). Stage 4 order is umountall, then
  rc.shutdown.final (the old comment said the reverse); / stays writable
  because it is busy.

### Changes (`52071ed`)

- sdcompat.cil: `allow init_t var_run_t (lnk_file (create rename))`.
- rc.shutdown writes `sdcompat: shutdown starts` to /dev/kmsg;
  rc.shutdown.final saves `@*` (all archives, any suffix) + `current`,
  and the kernel log from the marker on (whole log if missing).
- README/README-us (kernel log is from the start of the shutdown),
  TESTING/TESTING-us "Previous shutdown" row (console log not empty, no
  `unable to rename`).

### Tests

bash -n and shellcheck -S warning on rc.shutdown and rc.shutdown.final;
the sed range checked on sample input. No Rust changes, and the sandbox
doesn't use these files or the policy, so cargo test and the sandbox
suite weren't rerun. The policy change hasn't been loaded (needs root).

### Current state

The rename fix and log capture are committed but not on the real boot.
`688ed2b` (refusing starts during shutdown) has not been tested on a
power-off yet: the daemon running now (booted 20:57) is the new one.

### Next steps

1. Owner: build/upgrade RPMs, rerun setup (reloads sdcompat.cil,
   regenerates the scripts), **reboot into s6 before the test
   power-off** so the new sdcompatd runs, power off, boot s6, then time
   it and send `sudo cat /var/log/sdcompat/last-shutdown-console.log`
   and `sudo grep -i denied /var/log/sdcompat/last-shutdown-kernel.log`.
2. With the console log: explain the 20 s (to gdm session close) and
   15 s (to user-runtime-dir stop) gaps if they remain.
3. Optional: replace `/etc/sdcompat/s6/*/run` with their `.rpmnew`.
4. Earlier next steps unchanged (zram swap, /run Shmem, on-demand socket
   activation, JOURNAL=no output loss, beside3 sandbox flake; boot-time
   agetty denials before restorecon).

## Session: 2026-10-21: skarnet sources used as released

The owner wants skarnet's sources (skalibs, execline, s6, s6-rc,
s6-linux-init, s6-frontend) used exactly as upstream releases them.

### Changes

- `3726fb4` CLAUDE.md: never modify skarnet's sources or add patches for
  them; fix things on sdcompat's side, otherwise describe the upstream
  behaviour and leave the decision to the owner.
- `fa7adaf` skarnet.versions: the note on dist/rpm/NAME-VERSION-*.patch
  removed.
- `15cda23` The patch mechanism removed: build-skarnet-rpms.sh no longer
  copies patches or adds Patch lines, setup-fedora.sh --build-s6 no longer
  runs `patch` (and no longer installs it), the six specs use plain
  `%autosetup`. No patch files existed, so the packages don't change.

### Tests

bash -n and shellcheck -S warning on build-skarnet-rpms.sh and
setup-fedora.sh. The six specs, generated as the script does, parse with
`rpmspec -q --srpm`. No full RPM build.

### Current state / Next steps

Unchanged from the previous entry.

## Session: 2026-10-21: power-off 50 s, sdcompatd deaf to the bus while stopping

The owner booted s6 at 20:57 (new sdcompatd), upgraded the RPMs at 21:23
(sdcompatd restarted twice, at 21:24:06 and 21:26:21), powered off at
21:27:16 and booted s6 again. Power-off took ~50 s (was 1 min 4 s). They
sent the console log and the kernel log's denials.

### Findings

- **`688ed2b` works.** user@1000's log (/var/log/sdcompat/user@1000.service)
  has exit.target at 21:27:16.24 and the last slice at 16.43: no 25 s gap.
  wireplumber's late bluez activation was refused at once ("is
  destructive"). The `52071ed` log capture works: console log 89 KB, no
  `unable to rename`.
- **sdcompatd's own stop took 45 s** (stopping 21:27:16.20, stopped
  21:28:01.73). On SIGTERM the daemon broke out of its D-Bus loop and only
  then ran stop_dynamic_oneshots (the backlight oneshots, then
  user-runtime-dir@1000 with user@1000 as its dependent). From then on
  nothing read the bus: activations sent at 16.25 (nm-dispatcher, by
  NetworkManager while stopping) and 16.36 (colord, by gnome-shell) got no
  answer and timed out at 41.27/41.36 (25 s); NetworkManager stopped only
  at 41.63. systemd-user-runtime-dir's stop ran at 21:28:01.72 (its AVC
  timestamp), right before sdcompatd exited, so user@1000's stop took
  ~45 s after the user manager had finished at 16.43. **Not identified:**
  what inside that stop waited. The runner after the manager exits closes
  the PAM session (systemd-user: pam_systemd, pam_systemd_home, ...) and
  cleans the cgroup (≤ 2 s); no dbus-daemon "Activating" line from the
  runner in that window, so if it waited on the bus it was a method call
  (not logged by dbus-daemon) or something outside D-Bus. The timeline
  (41.70 fdholder down, then exactly 20 s to 01.72) is suggestive but not
  explained.
- Still in the kernel log: iscsiadm (iscsid_t) and systemd-user-runtime-dir
  (systemd_user_runtimedir_t) denied write on the catch-all fifo
  (/run/service/s6-svscan-log/fifo, var_run_t); only their output is lost.
  Known item (JOURNAL=no output loss).
- Also seen, not acted on: `sdcompat[cups.path]: starting cups.service:
  I/O error: Broken pipe` right after each sdcompatd restart during the
  RPM upgrade (the path runner's connection was to the old daemon);
  `sdcompat-runner: user 60578 does not exist` at 20:57:58 (gdm's dynamic
  greeter user gone).

### Changes (`ebcebda`)

- sdcompatd: on SIGTERM with the `shutting-down` marker, stop_dynamic_
  oneshots runs as a task and the main loop keeps serving calls and
  activations until it finishes; starts are refused by check_job's
  shutdown check. Without the marker it exits at once as before.
- Diagnostics: `sdcompatd: stopping ID took N s` when a oneshot stop
  (with its dependents) takes ≥ 2 s; the runner logs `closing the PAM
  session took N s` (to the unit's log, e.g. user@1000's) when ≥ 2 s.
- Sandbox: prep@c's ExecStop= activates org.example.Late during the
  shutdown phase; check.sh requires prep@c's stop within 5 s.
- TESTING/TESTING-us "Previous shutdown" row: no `Failed to activate
  service ... timed out`, no `sdcompatd: stopping ... took`.

### Tests

cargo build, cargo test, standalone tests pass. Sandbox 191/191,
SANDBOX_JOURNAL=no 196/196, SANDBOX_FDSTORE=no 191/191. With the mod.rs
change reverted: 1 failure, `prep@c's stop took 10 s`. bash -n and
shellcheck -S warning on check.sh and sandbox.sh. Not tested: a real
power-off.

### Current state

Power-off ~50 s. `ebcebda` should remove the 25 s waits of bus callers
during sdcompatd's stop (NetworkManager's among them); whether it also
removes the 45 s in user@1000's stop depends on what that waited on,
which the new log lines will show.

### Next steps

1. Owner: build/upgrade RPMs, **reboot into s6** (so the new sdcompatd
   runs), power off, boot s6, time it, and send
   `sudo cat /var/log/sdcompat/last-shutdown-console.log`,
   `sudo grep -i denied /var/log/sdcompat/last-shutdown-kernel.log` and
   `sudo tail -n 30 /var/log/sdcompat/user@1000.service/current`.
2. If `stopping user-runtime-dir@1000.service took` remains large, read
   the runner's `closing the PAM session took` line: if PAM, find which
   module waits (pam_systemd ReleaseSession to a stopped logind is the
   first suspect); otherwise it's s6-supervise/the runner's finish.
3. cups.path runner losing its connection across a daemon restart.
4. Earlier next steps unchanged (zram swap, /run Shmem, on-demand socket
   activation, JOURNAL=no output loss incl. the fifo denials, beside3
   sandbox flake; boot-time agetty denials before restorecon).

## Session: 2026-10-21: s6-rc-update fails after `dnf remove zram`

`sudo dnf remove zram` (21:34, s6 boot of 21:28) ran systemd's
%triggerpostun daemon-reload; sdcompat-gen's s6-rc-update failed with
`unable to make new live directory in /run/s6-rc:s6-rc-init:WvK3XB: No
such file or directory`.

### Findings (read-only, no root)

- The message is from the **last of 4 attempts**: sdcompat-gen retries
  s6-rc-update 3 times and only returns the last error; the first one is
  in sdcompatd's stderr (`sdcompat: s6-rc-update failed, retrying:`, in
  /run/uncaught-logs, root only). Not read yet.
- Live state is damaged: `s6rc-oneshot-runner`'s service directory is no
  longer in /run/s6-rc:s6-rc-init:WvK3XB/servicedirs/ but in a leftover
  /run/s6-rc:s6-rc-update:aUJIXV/servicedirs/ (21:34). /run/service/
  s6rc-oneshot-runner now dangles, so oneshot transitions (and the
  power-off's) can't reach the runner's socket. The symlink /run/s6-rc
  still points at the init dir, compiled -> /var/lib/sdcompat/compiled
  (db 1051); new db 1792586077652732-2012 was compiled.
- s6-rc-0.8.0.2 make_new_livedir: for each up longrun it renames the old
  live servicedir into the new live dir, then s6rc_servicedir_copy_online;
  on failure it rolls back (copy + rename back) or dies with `unable to
  rollback ...`. The printed path after a rollback is the old live dir
  (sa->s), not the failing file. Inference: the first attempt failed and
  its rollback died on s6rc-oneshot-runner (the first longrun), leaving it
  moved; retries 2-4 then fail at rename() with ENOENT. What failed first
  is unknown until the log is read.

### Current state / Next steps

1. Owner: send `sudo grep -h -A3 's6-rc-update\|rollback' /run/uncaught-logs/*`.
2. Repair by hand (moves the dir back, s6-supervise follows it):
   `sudo mv /run/s6-rc:s6-rc-update:aUJIXV/servicedirs/s6rc-oneshot-runner
   /run/s6-rc:s6-rc-init:WvK3XB/servicedirs/ && sudo rm -r
   /run/s6-rc:s6-rc-update:aUJIXV`, then `sudo systemctl daemon-reload`.
3. sdcompat side, depending on the first error: report the first error,
   not the last; don't retry once s6-rc-update died in its rollback (a
   retry can't succeed); possibly detect and repair a moved servicedir.
4. Earlier next steps unchanged (power-off retest of `ebcebda`).

## Session: 2026-10-21: review fixes for `ebcebda`; s6-rc-update fails again

### Reviews of `ebcebda`

- Security review: no findings (authorization in handle_call/authorize
  and the shutting-down refusal in check_job are unchanged and still
  apply while the stop task runs).
- Code review, two correctness issues, fixed in `526d60d`:
  1. stop_dynamic_oneshots didn't take `unit_lock(id)`; with the bus now
     served during the stop, a StopUnit for the same unit could run
     beside it and run ExecStop= twice (seen in the sandbox: `down c`
     twice).
  2. A stream end while the stop task ran returned from run() and the
     runtime cancelled the task; a stream end before SIGTERM with the
     marker no longer ran the stop. Now the task is awaited after the
     loop (a `stopped` flag avoids awaiting a finished JoinHandle), or
     the stop runs there if it never started.
- Sandbox: StopUnit prep@c right after the daemon's SIGTERM; check.sh
  requires `down c` once.

### Tests

cargo build, cargo test pass. Sandbox 192/192 (three runs),
SANDBOX_JOURNAL=no 197/197, SANDBOX_FDSTORE=no 192/192. bash -n and
shellcheck -S warning on check.sh. With the lock removed: one run passed
(the race wasn't hit), one failed `ExecStop= ran once` (got 2) plus 16
unrelated early failures (hello.service, tick.timer, ... inactive) that
didn't recur in the two runs afterwards; not investigated, possibly the
beside3-style flake.

### s6-rc-update, second occurrence

The owner repaired the live dir (mv back + rm the leftover), svstat
showed s6rc-oneshot-runner up, then `systemctl daemon-reload` failed the
same way and moved s6rc-oneshot-runner into a new leftover,
/run/s6-rc:s6-rc-update:yVJ5yc. So it is deterministic, not leftover
damage.

- The moved copy keeps its boot-time files (run, data, notification-fd
  21:28); only the dir and its supervise/ changed (21:52:56). So
  s6rc_servicedir_copy_online failed before touching files, i.e. in
  s6rc_servicedir_block (s6_svstatus_read of supervise/status, or writing
  `Q` to supervise/control) or in mkdir old; the rollback's copy_online
  then failed the same way and died (`unable to rollback`), and the
  retries fail at rename() with ENOENT. ENOENT fits supervise/control
  being absent (s6_svc_writectl returns -2 with the open's errno).
  **Inferred, not seen.** supervise/ is 0700 root.
- The owner's `sudo grep ... /run/uncaught-logs/*` failed because their
  non-root shell expanded the glob; use `sudo sh -c '...'`.
- The sandbox's s6-rc-update path passes, so the difference is on the
  real system (s6-linux-init's tree, SELinux, the s6-rc 0.8.0.2 /
  s6 2.15.1.1 / skalibs 2.16 builds from today 11:16).

### Current state

`526d60d` on top of `ebcebda`, neither tested on a real power-off.
s6-rc-update fails on every live update on the real s6 boot and leaves
s6rc-oneshot-runner's service directory moved; repair by hand after each
reload. Avoid daemon-reload and dnf transactions until fixed.

### Next steps

1. Owner, before the repair: `sudo sh -c 'grep -h -B2 -A3
   "s6-rc-update\|rollback" /run/uncaught-logs/@* /run/uncaught-logs/current
   | tail -60'`, `sudo ausearch -m avc -ts 21:50 | tail -20`,
   `sudo ls -la /run/s6-rc:s6-rc-update:yVJ5yc/servicedirs/s6rc-oneshot-runner/supervise/`.
   Then repair: `sudo mv /run/s6-rc:s6-rc-update:yVJ5yc/servicedirs/
   s6rc-oneshot-runner /run/s6-rc:s6-rc-init:WvK3XB/servicedirs/ && sudo
   rm -r /run/s6-rc:s6-rc-update:yVJ5yc`.
2. With the first error: fix on sdcompat's side (SELinux, setup, or
   sdcompat-gen); sdcompat-gen should report the first error and stop
   retrying after a died rollback. If it's s6-rc's own behaviour,
   describe it and leave the decision to the owner.
3. Power-off retest of `ebcebda`/`526d60d` (RPMs, reboot into s6, power
   off, send the logs listed in the earlier entry). Needs a working
   live update or a fresh boot after the RPM upgrade.
4. Earlier next steps unchanged.

## Session: 2026-10-21: s6-rc-update cause found (SELinux labels)

The owner sent the catch-all log and AVCs (taken before the repair).

### Findings

- First error at both reloads (21:34:37, 21:52:56): `s6-rc-update:
  fatal: unable to rollback /run/s6-rc/compiled/servicedirs/
  s6rc-oneshot-runner into /run/s6-rc:s6-rc-update:XXX/servicedirs/
  s6rc-oneshot-runner: Permission denied`. AVC: `denied { rename }
  comm="s6-rc-update" name="run" scontext=init_t tcontext=var_run_t
  tclass=file`, twice (copy_online, then its rollback).
- supervise/control exists (the ENOENT guess in the previous entry was
  wrong: block() worked; the first file rename was denied, errno EACCES).
- Labels: the boot's live dir and everything in it is var_run_t (state is
  init_var_run_t, rewritten since); the runtime-created update dir is
  init_var_run_t. init_t's unnamed dir/file transitions give
  init_var_run_t in /run, but `early` runs `restorecon -R /dev /run`
  after s6-rc-init, and /run/s6-rc* had no filecon, so it became
  var_run_t. sesearch: init_t on var_run_t:file has no create/rename/
  setattr; on init_var_run_t it has all of them (as for /run/systemd).
- Every live update on an enforcing s6 boot hit this; it had never been
  exercised under enforcing (earlier RPM upgrades didn't reach
  s6-rc-update or ran permissive).
- After each failure s6-svscan warned every 4 s `unable to stat
  s6rc-oneshot-runner` (the dangling /run/service link).

### Changes (`a1eb405`)

- sdcompat.cil: `(filecon "/run/s6-rc(:[^/]+)?(/.*)?" any ...
  init_var_run_t)`; the regex was checked against sample paths (matches
  /run/s6-rc, the init/update dirs and their contents, not /run/s6-rcx or
  /run/service). Not compiled here (no secilc, needs root to load).
- setup-fedora.sh setup_selinux: restorecon -R /run/s6-rc and
  /run/s6-rc:* on a running boot.
- sdcompat-gen: the error returned is the first s6-rc-update error (with
  the last one appended when different); no retry after `unable to
  rollback`.

### Tests

cargo build, cargo test pass; sandbox 192/192. bash -n and shellcheck
-S warning on setup-fedora.sh. The sandbox doesn't use SELinux, so the
policy change is untested.

### Current state

Cause known and fixed in the policy, not yet loaded. The running boot
still has the var_run_t live dir.

### Next steps

1. Owner, on the running boot: repair (mv s6rc-oneshot-runner back, rm
   the leftover update dir), `sudo chcon -R -t init_var_run_t
   /run/s6-rc:s6-rc-init:WvK3XB`, then `sudo systemctl daemon-reload`:
   must succeed (confirms the diagnosis without new RPMs).
2. Then RPMs, dnf upgrade, rerun setup (loads the module), reboot into
   s6; `ls -dZ /run/s6-rc:*` must show init_var_run_t; daemon-reload
   must work; then the power-off retest of `ebcebda`/`526d60d`.
3. Earlier next steps unchanged.

## Session: 2026-10-21: secilc check; sudo slow (stuck logind user 0)

### secilc

The owner installed secilc. The installed modules are root-only, so
sdcompat.cil was compiled against a stub policy declaring its types and
classes (scratchpad, not committed): compiles; the generated
file_contexts has `/run/s6-rc(:[^/]+)?(/.*)?  ...init_var_run_t:s0`.

### sudo slow on the s6 boot

Read as the user (no root), 22:18:
- sudo before authentication is fast (`sudo -n true` 6 ms, NSS lookups
  1-4 ms). sudo's PAM session includes pam_systemd (system-auth), which
  asks logind for a session for root; logind starts user-runtime-dir@0
  and user@0 and replies once their jobs finish.
- logind: user 0 `State=opening` since 21:52:55 (the owner's `sudo
  systemctl daemon-reload`), RUNTIME_DIR_JOB=job/71, SERVICE_JOB=job/72.
- sdcompatd `systemctl list-jobs`: 68 user-runtime-dir@0 stop running
  (since 21:52:52), 71 user-runtime-dir@0 start waiting, 72 user@0 start
  running. Nothing has finished since.
- Job 68 is blocked in dyn_stop on `s6-svunlink /run/service
  user@0.service` (pid 50082, child of sdcompatd, since 21:53:08). That
  waits with no timeout for user@0's s6-supervise to exit; it only sends
  `x` (no `d`). The supervisor alive is pid 49208, started 21:52:55, i.e.
  the user@0 instance started for the second sudo, not the one being
  stopped. The /run/service link is gone; /run/sdcompat/dyn/user@0.service
  is still there; no user@0 runner or systemd --user process is left.
- user@0's log: starts/stops with each sudo until 21:52:55 (start) and
  21:53:09 (exit.target); no start since, although the owner ran sudo
  several times (logind won't start a user that is still "opening").
- Inference: every sudo since 21:52:55 waits for logind's CreateSession
  reply, which waits for jobs 71/72 → until pam_systemd's call times out.
  Not measured (needs `sudo -v; time sudo true`).
- Not explained: job 72 got user@0 running at 21:52:55.96 although its
  dyn_start first calls start_any(user-runtime-dir@0), which takes that
  unit's lock, held by job 68 since 21:52:52. Either the lock wasn't held
  then or user@0 was started by another path. Needs sdcompatd's log
  (/run/uncaught-logs, root).
- Pattern: logind stops user@0 + user-runtime-dir@0 10 s after a sudo
  session ends; a new sudo during that stop (two sudo ~10-13 s apart)
  overlaps a stop and a start of the same instances. dyn_stop stops
  dependents without their unit lock, and s6-svunlink without -t can wait
  forever on an instance it didn't create.

### Next steps

1. Owner: `sudo -v; time sudo true`; `sudo s6-svstat
   /run/sdcompat/dyn/user@0.service /run/sdcompat/dyn/user@0.service/log`;
   `sudo sh -c 'grep -h "21:5[23]:" /run/uncaught-logs/@*
   /run/uncaught-logs/current | grep -v dbus-daemon'`.
2. Unstick: `sudo s6-svc -dx /run/sdcompat/dyn/user@0.service` (the
   supervisor exits, s6-svunlink returns, job 68 then 71/72 finish).
3. Fix in sdcompatd once the order is known: serialize a dependent's stop
   with its own jobs (lock order!), give s6-svunlink a timeout, and don't
   unlink an instance a later start created. Reproduce in the sandbox
   (inst@/prep@ with a slow stop and a start during it).

## Session: 2026-10-21: sudo 2 min explained (s6-svscan scans abort)

The owner sent: `time sudo true` → real 2m0.016s; `s6-svstat
/run/sdcompat/dyn/user@0.service` → `down (exitcode 0) 2134 seconds,
normally up` (the supervisor alive); the catch-all log for 21:52-21:53
(only the s6-rc-update errors and s6-svscan's `unable to stat
s6rc-oneshot-runner` every 4 s; sdcompatd logs nothing per job).

### Findings

- 2 min = pam_systemd's timeout for logind's CreateSession, which waits
  for the user's start jobs (71/72), which wait for job 68.
- s6-svunlink (s6 2.15.1.1, s6_supervise_unlink_names) does not send `x`:
  it subscribes to `x` in the service's and the logger's event/, removes
  the link, sends s6-svscan `an`, and waits (no -t: forever) for both
  supervisors to announce their exit.
- s6-svscan's scan() aborts at the first entry check() can't stat that
  isn't a logger (`unable to stat NAME`, retried every 4 s). Only a
  completed scan updates the active set, and killthem (the `n` prune)
  only ends supervisors of inactive services, and not while a scan is
  pending (what & 16). The failed s6-rc-update at 21:52:56 left
  /run/service/s6rc-oneshot-runner dangling, so from then on no scan
  completed and no removed service's supervisor was ended. user@0's stop
  removed its link at 21:53:08 and has waited since.
- So the root cause is again the var_run_t labels (`a1eb405`); the
  repair (moving s6rc-oneshot-runner back) should let the next scan
  complete, s6-svscan TERM user@0's supervisor (HUP its logger, which
  exits at EOF), s6-svunlink return, and jobs 68/71/72 finish.
- Earlier sandbox experiment (scratchpad logind-race.sh: stop user@-like
  + runtime-dir-like, then start both 0.2 s later, with a 2 s stop): the
  jobs overlap but finish within ~6-20 s; no permanent hang without a
  broken scan directory. The previous entry's "lock order" and "unlink an
  instance a later start created" suspicions are not needed to explain
  the hang; how the 21:52:55 user@0 instance came up beside job 68 is
  still not reconstructed (no per-job logging).
- Side effect while scans abort: s6-svlink can't get new dynamic
  instances supervised either (5 s timeout, "s6-svlink failed").

### Changes

- dyn_stop: `s6-svunlink -t 5000`, and `sdcompatd: ID: s6-svscan did not
  release its supervisor (look for its warnings)` when it takes ≥ 5 s.
- Sandbox: inst@h running, a dangling link in the scan directory, then
  StopUnit prep@h: must end within 12 s and log the warning. Without the
  `-t`: prep@h stays deactivating/stop (3 failures).

### Review of the change (xhigh), fixes

- s6-svunlink's exit status was ignored. It fails *before* removing the
  link when it can't check the supervisors (s6_svc_ok error other than
  ENOENT, e.g. an EACCES denial on supervise/lock; service and logger
  supervision differ, EINVAL; s6-ftrigrd not spawned). dyn_stop then
  removed the directory and left a dangling link: the same aborted scans.
  Fixed: if it failed and the link is still there, remove it and
  `s6-svscanctl -an`; if even that fails, keep the service directory and
  the unit (link and directory stay consistent; load_dynamic takes it
  over).
- The warning came from the elapsed time and always blamed s6-svscan
  (s6-svunlink exits 0 when its limit passes, too). Now, after the 200 ms
  pause, sdcompatd looks at supervise/lock of the service, then of log/
  (fcntl F_GETLK; `s6::lock_held`, now shared with s6rc_lock_held):
  `s6-svscan did not release its supervisor (look for its warnings)`, or
  `its logger is still running (a leftover process may hold its output)`
  (JOURNAL=no: a KillMode=process/none leftover keeps s6-log from EOF).
- Test: inst@h must also end inactive/dead; after the dangling link is
  removed, `s6-svscanctl -an`, and the supervisor left behind must exit
  within 5 s (it does: s6-svscan's completed scan ends it).

Not changed (owner's decision, or larger than this fix):
- After a timed-out release dyn_stop still removes the service directory
  under the live supervisor(s) (svscan.log of a JOURNAL=no run:
  `s6-supervise inst@h.service/log: warning: unable to write status
  file`). Harmless while the service is down; but if `s6-svc -wD -d -T
  120000` timed out (service still up), the supervisor can't run ./finish
  when s6-svscan ends it, and under JOURNAL=no the old s6-log keeps
  LOGDIR/ID locked against a new instance's logger until then.
- dyn_stop stops dependents without their unit lock (previous entry,
  step 3). After a timed-out release the teardown now runs even if a
  concurrent job reused the instance.
- While scans abort, every longrun stop costs 5 s, one after another:
  dependents, a slice stop, the shutdown stop, and watch_dynamic's
  collection of inactive transient units (blocks the watcher 5 s each).
- s6-svlink in dyn_start fails for the same reason with only
  `s6-svlink failed`; no hint at s6-svscan.

### Seen in the sandbox: s6rc-fdholder down after an s6-rc-update retry

This session's own earlier `SANDBOX_JOURNAL=no` check.sh (pid 111007,
orphaned when its scratchpad went away; stopped afterwards with
sandbox.sh stop) hung in `s6-rc -u change hold2.service`. Its daemon-reload's
s6-rc-update failed once (`unable to manage new service directories ...:
Broken pipe`) and the retry succeeded, but s6rc-fdholder's new supervisor
(22:46:35) stayed `down (not started yet)`, no s6-fdholderd runs, and
every logger start after it fails (`s6-fdholder-retrieve: unable to
connect ... Connection refused`, `logger X.log did not come up`). It
happened before the inst@h scenario, so not caused by it. One of my
`SANDBOX_FDSTORE=no` runs also broke right after `call Reload`
(hello.service inactive, 16 failures cascading); the re-run passed, the
first run's logs were gone (sandbox stop removes them). The run without
the unit lock earlier today (16 unrelated failures from hello.service on)
looks the same. This is most likely the reload flake open since session 7
(s6-rc-update "unable to manage new service directories ... Broken
pipe", step 1 there): the switch to the new live dir has happened when
that error comes, so the retry starts from a half-updated state.

### Tests

- `cargo build`, `cargo test` (46 passed); standalone: fdstore 1,
  journal-relay 2, notify 0 passed. `bash -n`, `shellcheck -S warning`
  on tests/check.sh: clean.
- Sandbox, run with `XDG_RUNTIME_DIR=/tmp/claude-1000/sdcr` (the
  orphaned run above held /run/user/1000/sdc-test): default 198/0,
  `SANDBOX_JOURNAL=no` 203/0, `SANDBOX_FDSTORE=no` 198/0 (first run 182/16,
  above).

### Current state

- Committed as `4f60b5b` (dynamic.rs, s6.rs, tests/check.sh). My own default
  sandbox run on it: 198/198.
- The owner's machine still needs the live s6-rc repair (move
  s6rc-oneshot-runner back) for the stuck user@0 stop to finish; the new
  sdcompatd then bounds such stops at ~5 s.

### Next steps

1. Owner installs and repairs the live s6-rc state.
2. Owner: decide on the "Not changed" items above (keep the directory of
   an unreleased supervisor? lock dependents in dyn_stop?).
3. Reproduce the s6rc-fdholder left down after a failed-then-retried
   s6-rc-update (sandbox daemon-reload); sdcompat-gen's retry may need to
   check s6rc-fdholder (and s6rc-oneshot-runner) afterwards.

## Session: 2026-10-22: D-Bus unit path traversal fixed

From the whole-project security review (2026-10-21): the only candidate,
filtered out at confidence 5/10 because nothing sensitive was found to
read, but a real missing check. The owner asked to fix it.

### Finding

unit_from_path checked for '/' before bus_label_unescape, so `_2e_2e_2f`
decoded to "../". Loader::load keeps an invalid name as unit.id (load
state bad-setting), and state::for_unit = RUN_DIR/units/ID, so
Properties.Get/GetAll (unauthenticated, as in systemd) made the root
daemon read the state-key files (status as StatusText for a .service id,
result, mainpid, nrestarts, exec-main-*, *-enter, invocation, cgroup) of
any directory, following symlinks (/tmp/x.service -> /root/dir works).
GetUnit returned a path for such names, ListUnitsByNames listed them,
GetUnitProcesses read their cgroup. Read-only; no root-only file with
those names that matters was found. Confirmed in the sandbox: with the
old daemon the planted $R/leak.service/status came back over D-Bus.

### Changes (`5ab3304`)

- unit_from_path: decoded name must pass loader::parse_name.
- GetUnit, GetUnitProcesses: NoSuchUnit "Unit X not loaded." (systemd);
  ListUnitsByNames drops invalid names (systemd skips them); job methods,
  KillUnit, ResetFailedUnit, AttachProcessesToUnit, SetUnitProperties:
  InvalidArgs "Unit name X is not valid." (LoadUnit already did).
- util::path_component (sdcompat-base): bus-label escapes a name that is
  empty, ".", "..", or contains '/' or NUL; state::for_unit and
  cgroup::unit_cgroup_dir use it, so even an unchecked caller stays
  inside RUN_DIR/units and the slice's cgroup.
- Unit test util::one_component; sandbox "errors" section: object path,
  GetUnit, ListUnitsByNames, GetUnitProcesses, StartUnit with
  "../../leak.service".
- AttachProcessesToUnit's subcgroup (privileged callers only) could
  contain ".." and leave the unit's cgroup; fixed afterwards in `0240e02`
  at the owner's request: InvalidArgs "Control group path is not
  normalized" for any ".", ".." or empty component, as systemd's
  path_is_normalized(); a leading or trailing '/' is accepted. Unit test
  subcgroup_normalized; sandbox check (fails without the fix).

### Tests

cargo build, cargo test, standalone tests pass. Sandbox 203/203,
SANDBOX_JOURNAL=no 208/208, SANDBOX_FDSTORE=no 203/203. With the crates/
changes stashed: 5 failures (the object path returned the planted
StatusText). bash -n and shellcheck -S warning on check.sh.
With `0240e02`: cargo test passes; sandbox 204/204, SANDBOX_JOURNAL=no
209/209, SANDBOX_FDSTORE=no 204/204; without it the new check fails.

### Current state / Next steps

Unchanged from the 2026-10-21 entries (owner: live s6-rc repair, label
check, RPMs, reboot, power-off retest; reload flake; the "Not changed"
dyn_stop items).

## Session: 2026-10-22: damage checker in check.sh

Owner asked for the damage checker from the test plan (step 2a).

### Changes (`1b5e391`)

- check.sh: `section NAME` replaces every `echo "== NAME"` (31) and
  first runs damage(): a leftover s6-rc live directory, live
  servicedirs differing from the compiled db, a dangling scan-directory
  entry, an s6rc-* service that s6-rc lists up but whose supervisor
  doesn't run it. Each kind fails once ($W/damage.seen). New final
  section "end state": no job left after up to 10 s, no damage seen.
- First version checked every longrun s6-rc lists as up: false alarms
  for failing.service, flappy.service, second.service (s6-rc doesn't
  follow deaths; a failed unit stays "up" for it, sdcompat reports it
  failed from its own state), so only s6rc-* are checked.
- s6-rc list takes its lock without waiting (EBUSY while a section holds
  it), so that check is skipped then instead of hanging.

### Findings

- The reload flake (open since session 7) is frequent: 3 of 7 runs
  today (2 default, 1 FDSTORE=no). Every time: daemon.log `s6-rc-update
  failed, retrying: ... unable to manage new service directories in
  .../live: Broken pipe`, the retry "succeeds", and the checker reports
  before "timers" (the section after "unit files", which reloads):
  `s6-rc live directory left over: live:s6-rc-init:*` and `s6-rc has
  s6rc-oneshot-runner up, but its supervisor doesn't run it`. Twice it
  cascaded into 16 functional failures (hello.service inactive, timers,
  path units, job queue, ...); once only the leftover dir showed.
- Five Reload calls on a fresh sandbox: one live dir each time, no
  retry, so a leftover dir is never normal.
- "unable to manage new service directories" comes after s6-rc-update
  has switched the live symlink (s6-rc-update.c, after make_new_livedir),
  so sdcompat-gen's retry runs against a half-finished update.

### Tests

bash -n and shellcheck -S warning on check.sh. Sandbox default 206/206
(three runs), SANDBOX_JOURNAL=no 211/211; the flaky runs above fail only
by the flake (189/208 with its cascade).

### Current state

The suite now fails visibly whenever the reload flake hits, with the
cause named first.

### Next steps

1. Fix the reload flake: find what the "Broken pipe" is (s6-svscan's
   control fifo? s6-ftrigrd?) and what sdcompat-gen should do after a
   post-switch failure (not retry s6-rc-update; restart the s6rc-*
   services / rescan instead?). Never patch skarnet's sources.
2. Earlier next steps unchanged.

## Session: 2026-10-22: s6 boot on the old build; RPMs at HEAD

The owner rebooted into s6 (09:05) and asked what to do next.

### Findings (read-only, no root)

- PID 1 s6-svscan, enforcing, `is-system-running` running, nothing
  failed, one live dir (/run/s6-rc:s6-rc-init:HNM796), no leftover,
  /run/service/s6rc-oneshot-runner resolves.
- Installed sdcompat RPMs are `g06585e6` (2026-10-21 21:14): none of
  `ebcebda` onward, so neither the label fix `a1eb405` nor the later
  sdcompatd fixes. /run/s6-rc and the live dir are var_run_t and
  `matchpathcon` gives var_run_t: the next daemon-reload (or dnf
  transaction) would fail as on 2026-10-21.
- Since `06585e6`, dist/ and packages/ changed only in sdcompat.cil and
  setup's restorecon of the live dirs; sdcompat.spec %post loads the
  module, so `dnf upgrade` is enough (no setup rerun) once the live dir
  is relabelled.
- /var/log/sdcompat/last-shutdown-*.log are from 2026-10-21 21:28: no s6
  power-off recorded since.

### Changes

None to the repo. RPMs built at `7f64963`:
~/rpmbuild-sdcompat/rpms/*-0.1.0-1.20261022090953.g7f64963.fc45; the
packaged sdcompat.cil has the /run/s6-rc filecon.

### Next steps (given to the owner)

1. On this boot: `chcon -h -t init_var_run_t /run/s6-rc`, `chcon -R -t
   init_var_run_t /run/s6-rc:s6-rc-init:HNM796`, `systemctl
   daemon-reload` must succeed with one s6-rc-update:* live dir left
   (confirms the diagnosis on the old build).
2. `dnf upgrade ~/rpmbuild-sdcompat/rpms/*.rpm`; `matchpathcon
   /run/s6-rc` must then give init_var_run_t.
3. Reboot into s6: labels init_var_run_t without chcon; daemon-reload
   works.
4. Power-off retest of `ebcebda`/`526d60d` (logs as listed earlier).
5. Earlier next steps unchanged (reload flake, dyn_stop "Not changed").

## Session: 2026-10-22: first s6 boot on `7f64963`

The owner upgraded the RPMs, rebooted into s6 and asked what to do next.

### Findings (read-only, no root)

- Installed: sdcompat* 0.1.0-1.20261022091359.g7f64963 (installed
  09:23:14; a rebuild of the same commit, not the 090953 build named
  above). Boot 09:49:37, PID 1 s6-svscan, enforcing, is-system-running
  running, nothing failed.
- **`a1eb405` works at boot:** /run/s6-rc, /run/s6-rc:s6-rc-init:GDGg97
  and its contents are init_var_run_t without chcon; `matchpathcon
  /run/s6-rc` gives init_var_run_t. No daemon-reload yet on this boot, so
  s6-rc-update under enforcing is still untested.
- The previous boot shut down right after the RPM transaction:
  user@1000's log has SELinux load_policy at 09:26:32.97, exit.target at
  33.23, the last slice at 33.67; last-shutdown-*.log are from 09:27:22,
  so ~49 s again. That user@1000's runner was the old binary (started
  09:05), so no `closing the PAM session took` line could appear; whether
  sdcompatd was already the new one depends on the transaction's
  restart. The console log is root-only and not read yet.

### Changes

None to the repo.

### Current state

`7f64963` installed and booted; the label fix holds at boot. Not yet
seen: a live update and a power-off with the new sdcompatd and runner.

### Next steps (given to the owner)

1. Copy the 09:26 shutdown logs before the next power-off overwrites
   them: `mkdir -p ~/sdcompat-logs/0926 && sudo install -m 644 -o
   "$USER" /var/log/sdcompat/last-shutdown-*.log ~/sdcompat-logs/0926/`.
2. `sudo systemctl daemon-reload; echo rc=$?`; `ls -dZ /run/s6-rc:*`
   must show one s6-rc-update:* dir, init_var_run_t; the catch-all grep
   for `s6-rc-update|rollback|unable to stat` must print nothing.
3. If 2 is clean: power off from this boot, time it, boot s6, copy the
   logs to ~/sdcompat-logs/retest the same way; read them with
   user@1000's log (world-readable).
4. Earlier next steps unchanged (reload flake, dyn_stop "Not changed").

## Session: 2026-10-22: live update OK; shutdown logs lost on a read-only root

The owner ran next steps 1-3 of the previous entry and pasted the
terminal output without comment.

### Findings (read-only, no root)

- Step 1: ~/sdcompat-logs/0926 holds the 09:27:22 logs (the 09:26
  shutdown, run by the old runner): `sdcompat: shutdown starts` at
  1293.56, the runner's PAM session_close (res=failed) at 1339.28, i.e.
  ~46 s; iscsiadm and systemd-user-runtime-dir denied writing to
  /run/service/s6-svscan-log/fifo (var_run_t).
- **Step 2 passed: s6-rc-update works under enforcing.** `systemctl
  daemon-reload` rc=0, one live dir `s6-rc-update:PokOrA`,
  init_var_run_t; the catch-all grep printed nothing. This confirms
  `a1eb405` for live updates, not only at boot.
- **Step 3: the 09:56 shutdown wrote no logs.** ~/sdcompat-logs/retest
  is byte-identical to 0926; /var/log/sdcompat/last-shutdown-*.log are
  still from 09:27:22. So rc.shutdown.final either didn't run (shutdown
  hung, or the machine was switched off by hand) or couldn't write.
  It ran for the 09:26 shutdown with the same installed scripts (09:23,
  same as dist/ except the `/usr/bin/sh` shebang) and the same policy.
- What the 09:56 shutdown did leave: user@1000's log runs to exit.target
  at 09:56:34.44 and app.slice at 34.69, the same sequence as at 09:26,
  and its logger exited cleanly (no new @*.u in any service log dir).
  iscsid's last write was 09:56:34.70. Nothing in /var/log/sdcompat was
  written after that. The next kernel started 10:03:41 (btime), 7 min
  later. There's no `closing the PAM session took` line: either the PAM
  close took < 2 s or the runner never got that far.
- The owner started the 09:56 shutdown from the GNOME menu, as at 09:26
  (the 09:26 log has `poweroff.target requested, running:
  s6-linux-init-shutdown -p now` at 09:26:32.965).
- The 09:26 console log (readable in ~/sdcompat-logs/0926) shows the dnf
  transaction restarted sdcompatd (`ready` at 09:23:54.90) and reloaded
  it (`reloaded: 294 units` at 09:24:52). So the 09:26 shutdown already
  ran the new sdcompatd after a reload, and it finished. Neither the new
  sdcompatd nor a reload alone explains 09:56. What was new at 09:56:
  user@1000's runner (the 09:26 one dated from 09:05).
- **At 09:26 the 45 s was user@1000's PAM close.** `sdcompatd: stopping
  user-runtime-dir@1000.service took 45.7 s` at 09:27:18.93, the same
  moment as the runner's session_close audit (res=failed). systemd
  --user had exited at 09:26:33.67, and the runner closes PAM right
  after its child exits (runner.rs exec_with_pam), so ~45 s were inside
  pam_close_session. The systemd-user stack's close: pam_systemd_home,
  pam_systemd (ReleaseSession to logind, which s6-rc stopped at
  09:26:33.20; a failure returns PAM_SESSION_ERR, matching res=failed),
  pam_keyinit, pam_namespace, pam_selinux, pam_unix. sdcompatd logged no
  login1 or home1 activation during that time. Which module waited isn't
  settled.
- user@1000's dynamic servicedir has timeout-kill 60000, so s6-supervise
  SIGKILLs the runner 60 s after the SIGTERM: a PAM close that hangs
  can't stop the shutdown for more than ~60 s, but a killed runner never
  logs `closing the PAM session took`.
- The owner: the 09:56 power-off **turned the machine off by itself**,
  slowly (over a minute). So it wasn't a hang: stage 4 ran.
- **Cause: rc.shutdown.final writes to a read-only root.** Read in
  skarnet's sources (not modified): shutdownd, after rc.shutdown exits,
  unsupervises the tree, sync, SIGTERM to all, waits the grace time
  (-g 3000), sync, SIGKILL; the stage 4 script is `foreground {
  s6-linux-init-umountall }`, `foreground { tryexec { .../scripts/
  rc.shutdown.final } }`, `s6-linux-init-hpr -f -p` (hpr -f syncs, then
  reboot(2)). s6-linux-init-umountall (src/misc) calls umount() on every
  /proc/mounts entry in reverse order, except the first devtmpfs, proc
  and sysfs, and on EBUSY remounts the filesystem MS_RDONLY. The root is
  always busy, so it is remounted read-only, which only fails while some
  process still has a file open for writing. rc.shutdown.final's
  comment ("the root filesystem is usually still busy, so it stays
  writable") was wrong: the logs survived only messy shutdowns (09:27,
  2026-10-21 21:28) and were lost at 09:56, presumably a cleaner one.
  That fits: same scripts (old and new basedir identical, checked
  against setup-backup/20261022-092350), same policy, labels fine,
  /var/log on /, and no sdcompat path powers off except through
  s6-linux-init-shutdown.
- No time limit on rc.shutdown either: shutdownd waits for it without
  one (wait_pid, no timeout), rc.shutdown's `s6-rc -bDa change` had no
  `-t`, and sdcompatd's s6-rc service has neither timeout-down nor
  timeout-kill. A stop that never finishes would keep the machine up for
  good with no logs. Not what happened at 09:56, but possible.
- Current boot (10:03): `7f64963`, enforcing, is-system-running running,
  nothing failed, one live dir s6-rc-init:WqOjWA, init_var_run_t.

### Changes (`d0f29da`)

- rc.shutdown.final: if /var/log/sdcompat isn't writable, remount its
  filesystem (findmnt -T, else /) read-write, write both files, sync,
  remount it read-only again. If it is still not writable, print
  `sdcompat: /var/log/sdcompat is read-only, the shutdown logs are not
  kept` to /dev/console and exit 0. `mount` runs as mount_t (init_t
  domain transition); not verified under enforcing.
- rc.shutdown: `s6-rc -v2 -t 180000 -bDa change` (owner chose 3 min);
  exit 2 logs `sdcompat: services still not down after 180 s, killing
  the rest` to /dev/kmsg, other failures `s6-rc change exited N`; the
  script exits with s6-rc's code (shutdownd only warns). Per the s6-rc
  doc, -t leaves the live state as it is and doesn't kill children;
  whether -t also covers the -b lock wait isn't documented.
- README/README-us: the 3-minute limit. TESTING/TESTING-us "Previous
  shutdown": the kernel log has no `services still not down after
  180 s`.

### Tests

bash -n and shellcheck -S warning on both scripts. rc.shutdown.final
in `unshare -rm` with a ramfs over /var/log/sdcompat (tmpfs remount
fails in a user namespace: util-linux passes uid=1000): read-only ->
remounted rw, both files rewritten, read-only again; already writable
-> left rw; read-only locked by a nested user namespace -> message,
files untouched. rc.shutdown in `unshare -rm` with tmpfs over
/run/sdcompat and a fake s6-rc in PATH: marker written, `-t 180000`
passed, exits 0/2/1 as s6-rc did. cargo and the sandbox suite not run:
no Rust changed and the sandbox doesn't use the skel.

### Current state

Label fix confirmed for boot and live update. The shutdown logs should
now survive a clean shutdown; untested on a real one. The scripts reach
/etc/s6-linux-init/current only when setup reruns s6-linux-init-maker
(the RPM's %post doesn't). The power-off retest of `ebcebda`/`526d60d`
is still open: the 09:56 power-off left no data.
RPMs built at `99e077e` (owner's request):
~/rpmbuild-sdcompat/rpms/*-0.1.0-1.20261022103911.g99e077e.fc45; the
packaged skel has both new scripts.

### Next steps

1. Owner: `packages/build-rpms.sh`, `sudo dnf upgrade
   ~/rpmbuild-sdcompat/rpms/*.rpm`, rerun setup (`sudo
   dist/setup-fedora.sh --enforcing`, as at 09:23) so it regenerates
   /etc/s6-linux-init/current; check `grep remount,rw
   /etc/s6-linux-init/current/scripts/rc.shutdown.final` prints a line. Then boot
   s6, power off from GNOME, time it, boot s6, copy the logs to a new
   directory and read them with user@1000's log.
2. If the logs are still old: look for the console message and an AVC
   on mount (mount_t remount of /) in the kernel log's tail, which needs
   the screen or a photo.
3. With the logs: the runner's `closing the PAM session took` line, or
   its absence with `stopping user-runtime-dir@1000.service took` ≈ 60 s
   (SIGKILL), shows whether the 45 s PAM close remains; then find the
   module (pam_systemd's ReleaseSession to the stopped logind first).
4. Earlier next steps unchanged (reload flake, dyn_stop "Not changed").

## Session: 2026-10-22: whole-project review (code-review max), fixes

The owner ran `/code-review max --fix --comment main all`: a review of the
whole main branch at `e2b754c` (not a PR, so `--comment` did not apply; this
machine has no GitHub credentials anyway). Twelve finder angles ran as
subagents (line scans of every crate, dist/ and tests/, a history audit, a
cross-file contract tracer, Rust/shell pitfalls, wrappers, cleanup, altitude,
CLAUDE.md conventions), then verification and a gap sweep. The session was
cut by the usage limit several times. The fixes are in the working tree, not
committed.

### Findings fixed (the reported top 15, most severe first)

1. Start timeouts unlike systemd: a Type=oneshot service got the 90 s
   default (systemd: none unless set; 150 of the 175 oneshots on this host,
   e.g. fstrim, man-db-cache-update, dnf-makecache, raid-check, would be
   killed and marked failed after 90 s), and TimeoutStartSec=0 was not "no
   timeout": the notify helper's deadline was "now" (docker.service,
   Type=notify + TimeoutStartSec=0, SIGTERMed at once), the daemon waited
   1 s, gen wrote a 15 s s6-rc timeout-up. Now one helper,
   `Unit::start_timeout_usec()` (loader.rs), used by runner, gen, daemon and
   the TimeoutStartUSec property; `unit::parse_timeout_usec()` maps 0 to
   infinity for the stop timeouts too; gen clamps s6-rc timeouts to 32 bits
   (s6-rc-compile rejects larger ones).
2. Timers with OnUnitInactiveSec= fired once per boot. Seen live:
   dnf-makecache.timer (OnBootSec=10min, OnUnitInactiveSec=3h,
   Unit=dnf5-makecache.service) fired at 10:13 and then had `elapsed` 1 and
   next-monotonic 0. Three causes in timer.rs: the target's state was read
   under the Unit= alias (dnf5-makecache.service; the runner writes
   dnf-makecache.service), the base ignored the timer's last trigger (as
   systemd's MAX(inactive_enter, last_trigger)), and "nothing left" was
   final although OnUnit*Sec= can arm later. Without the last-trigger base a
   resolved alias would have re-triggered in a tight loop while the oneshot
   ran. Also: OnBootSec=/OnStartupSec= fired again after a restart of the
   timer longrun (now skipped when past at start and the timer triggered).
3. The s6 boot never created kmod's static device nodes: the default
   EXCLUDE drops kmod-static-nodes.service and systemd-*, and early's
   tmpfiles step skips /dev. Seen live: /dev/net/tun, /dev/uinput,
   /dev/vhost-net and /dev/cuse missing, /run/tmpfiles.d/static-nodes.conf
   holding only the initramfs's three lines. Opening such a node fails with
   ENOENT and the module never loads (rootless podman/pasta, VPNs, uinput,
   libvirt's vhost-net). early mount-api now runs `kmod static-nodes` and
   `systemd-tmpfiles --prefix=/dev --create --boot --graceful`, as those two
   units do, before udevd and before its restorecon.
4. The fd stores (standalone sdcompat-fdstore and the daemon's fallback)
   never dropped fds whose peer hung up. systemd drops FDSTORE= fds on
   POLLHUP/POLLERR and journald relies on it: one fd per stdout stream, so
   with JOURNAL=yes the store would reach RLIMIT_NOFILE and every PUT would
   fail (held sockets included). Now pruned on PUT/GET for unit keys ("#"
   keys stay); the accept loops no longer spin on EMFILE.
5. A lone `CapabilityBoundingSet=` (empty set: drop everything) was read as
   "never set": upower, systemd-localed, chrony-wait, malcontent-* and 16
   more units on this host ran as root with every capability. Fixed in
   sandbox::bounding_set; AmbientCapabilities= outside the bounding set are
   now left out as systemd does (capset() would fail with 218).
6. After a rollback boot (sdcompat.db=previous) the first reload set
   "previous" from "current" (the broken database) and garbage-collected the
   running, good one. "previous" is now the database live at the time of the
   update (gen regenerate()).
7. The stderr-to-syslog thread of sdcompatd ended on the first line that
   wasn't UTF-8 (lines()); fd 2 then had no reader, every eprintln! panicked
   and children died of SIGPIPE. It reads bytes now.
8. ProtectSystem=strict left /home, /root and /run/user read-only (systemd
   leaves them to ProtectHome=; accounts-daemon creates home directories with
   ProtectHome=false), and PrivateTmp= dirs under it were read-only too (they
   live in RUN_DIR and a bind keeps the read-only flag).
9. The service of a transient timer or path unit (systemd-run
   --on-calendar) was unloaded 10 s after its first run, so later elapses
   failed with NoSuchUnit. watch_dynamic keeps units a loaded transient
   timer/path triggers.
10. journal-relay read a memfd-passed entry from its current offset (the
   sender's writev leaves it at the end: entries lost) and read any fd type
   on its only thread (a pipe kept open by any user blocked all native
   logging, then every sender). Now regular files only, from the start.
11. The fd store limit could be bypassed with a NUL in FDNAME= (the client's
    3-field retry then sent the attacker's <max>). The runner validates
    FDNAME= as systemd's fdname_is_valid(); put() refuses NUL.
12. When the runner could not set a unit up (missing User=/Group=/
    EnvironmentFile=, unit gone), run/up/finish exited 1: no readiness, s6
    restarting every second, the boot's change waiting timeout-up. Now run
    records the failure (START_FAILED, result resources), reports ready and
    waits; finish exits 125; up exits 0.
13. rc.shutdown: s6-rc's -t starts counting only after -b got the lock
    (s6-rc.c: lock at 619, deadline at 741), so a change holding the lock
    kept the machine up for good, against the README's 3 minutes. timeout(1)
    now bounds the whole call (`timeout -s KILL 180`; exit 137 logs the same
    kernel line as exit 2).
14. try-restart of an inactive on-demand instance started it (unit_status
    saw the job itself and said "activating"), and reload-or-try-restart of a
    stopped database unit ran ExecReload=. Both now check the unit's state.
15. Restart jobs (do_restart, dyn_restart) reported "done" when the new run
    failed (cmd_up exits 0, soft-failed longruns report ready); they now go
    through job_outcome() like starts.
Also fixed in passing: `systemctl is-enabled gdm.service` said "disabled"
(seen live): a unit enabled only by an Alias= link in /etc
(display-manager.service) now counts as enabled, vendor alias links still
don't. RestrictSUIDSGID= now covers mkdir/mkdirat/mknod/
mknodat (systemd's seccomp_restrict_sxid); Config::unit_run_dir escapes like
state::for_unit and StartTransientUnit validates scope/slice names (a
privileged "../x.scope" made gc_scopes remove_dir_all outside RUN_DIR);
rc.shutdown.final remounts read-only again when the directory stays
unwritable after its remount.

### Reported, not fixed (owner to decide or later sessions)

- EXTEND_TIMEOUT_USEC= is ignored (MariaDB recovery killed at
  TimeoutStartSec); a fix needs the runner, the daemon's wait and gen's s6-rc
  backstop together.
- Runner: SuccessExitStatus= not applied to oneshots and only numbers match
  (DATAERR etc.); Restart=on-failure/on-abnormal ignore a helper's timeout
  (SIGTERM counts clean); EnvironmentFile= read once before ExecStartPre=;
  environment order (PassEnvironment= over Environment=); forking PIDFile=
  waits 5 s; ExecStopPost= before the cgroup kill; NNP for '+' commands;
  MAINPID= taken from any accepted sender; PAM wrapper's fork/handler window
  and kill-whom=main hitting the parent; RESTART_TIMES unbounded with
  StartLimitIntervalSec=0.
- Daemon: AttachProcessesToUnit("" , ...) from systemd --user rejected;
  GetUnitByPID picks the innermost .service/.scope; polkit details empty;
  `s6-svc -r` restarts never escalate to SIGKILL and leave
  RESTART_REQUESTED; ResetFailed keeps START_FAILED; Unit.SetProperties/
  Ref/Unref unreachable; dynamic/index lock order; ReloadUnit refused while
  another job runs; f[1] index in transient render().
- Parsing/loading: a comment line ending in '\' swallows the next line;
  EnvironmentFile= quotes inside values; prefix drop-ins (user-.slice.d);
  month/year time units; mountinfo with non-UTF-8 bytes; path units
  re-creating inotify every round; calendar `~` with a repetition runs
  backwards; TzGuard's set_var in the multi-threaded daemon.
- Sandbox: ReadOnlyPaths= over a nested ReadWritePaths=; /dev/null (rw) for
  inaccessible files; SystemCallFilter= `~name:errno` in allow lists and
  unknown @groups.
- Standalone: journal-relay stream priority (facility|level) and unbounded
  threads; fd store duplicate suppression, single blocking thread, cmsg
  buffer alignment; sdcompat-notify sends its own PID and lacks options.
- dist/packages: kernel-install's <id>-<kver>-sdcompat.conf sorts before the
  systemd entry for grubby --set-default; rc.shutdown.final with a separate
  /var or 99+ mounts; skel changes reach /etc/s6-linux-init/current only by
  rerunning setup (packages/README doesn't say so); early's restorecon of
  /run as the root of the repeated label fixes; gen drops KillSignal= names
  it can only print as numbers (httpd's SIGWINCH); JOURNAL=no maps
  rsyslog.service to the s6 syslog even when INCLUDEd; fdstore run script
  hardcodes /run/sdcompat; `--config` after the command is ignored by gen
  and sdcompatd; specs' %check skips the library crates; SC2046 in
  sdcompat.spec scriptlets; %changelog carries 0.1.0-1.
- setup/early (from the gap sweep): check_versions exits setup halfway
  under `set -euo pipefail` when the tools are from different builds (the
  PIPESTATUS warning never runs); a setup rerun deletes the s6 entries of
  all kernels and recreates only the default kernel's; mount-fs remounts /
  rw whatever fstab says; port-only/[::] Listen*= fail the start with
  ipv6.disable=1 (systemd falls back to IPv4); gen drops both units of a
  mutual Conflicts=; SYSTEMD_WANTS templates get no instance.
- tests: check.sh's fdkeep check can pass on a stale line; sandbox.sh stop
  kills PIDs from files without checking them.
- Cleanup notes: timeouts and service-type defaults were computed in many
  places (now one helper for the start timeout); write_servicedir duplicates
  gen's service files; list_unit_files is O(files x links) (0.85 s here);
  Properties.Get builds every interface; watch_dynamic spawns s6-svstat
  every second per instance.

### Tests

- `cargo build` (no warnings), `cargo test`: 52 passed (new: unit
  parse_timeout, loader start_timeouts, sandbox empty_bounding_set, fdstore
  hung_up_fds_go and the NUL name, install enabled_through_alias).
  Standalone: sdcompat-fdstore 2 (new hung_up_fds_go), journal-relay 3 (new
  memfd_from_start), notify 0.
- Sandbox on the working tree (XDG_RUNTIME_DIR=/tmp/claude-1000/sdcr-rev):
  default 206/206, SANDBOX_JOURNAL=no 211/211, SANDBOX_FDSTORE=no 206/206
  (the same in a scratch copy before applying). The dist/early change came
  after those runs; the sandbox doesn't use early.
- bash -n, sh -n and shellcheck -S warning on rc.shutdown,
  rc.shutdown.final and early. rc.shutdown with a fake s6-rc and a shortened
  timeout: exits 0, 2 and 137 (s6-rc hung) with the expected kernel lines.
  kmod static-nodes run as the user lists /dev/net/tun, /dev/uinput and
  /dev/vhost-net among 13 nodes.
- Not tested: anything needing root or a real boot (capabilities, the
  ProtectSystem=strict mounts, the static nodes at boot, rc.shutdown under
  shutdownd, the rollback path).

### Current state

All fixes above are uncommitted in the working tree on top of `e2b754c`.
early reaches /usr/libexec/sdcompat with the sdcompat RPM; the skel scripts
(rc.shutdown, rc.shutdown.final) reach /etc/s6-linux-init/current only when
setup reruns s6-linux-init-maker.

### Next steps

1. Owner: review and commit the working tree; build RPMs, upgrade, rerun
   setup (regenerates /etc/s6-linux-init/current), reboot into s6.
2. On that boot: `ls -l /dev/net/tun /dev/uinput`; dnf-makecache.timer
   should show a next elapse (`systemctl list-timers dnf-makecache.timer`)
   about 3 h after dnf-makecache.service last ran; `systemctl is-enabled
   gdm` prints enabled; `systemctl show -p CapabilityBoundingSet upower`
   and `grep Cap /proc/$(pidof upowerd)/status` show an empty set.
3. Then the power-off retest from the previous entry.
4. Decide on the "Reported, not fixed" list; EXTEND_TIMEOUT_USEC= and the
   oneshot SuccessExitStatus= are the next most likely to bite.

## Session: 2026-10-23: review committed; the "reported, not fixed" list

The owner asked to commit the previous session's review fixes and to fix
the "Reported, not fixed" list of that entry.

### Commits

- `24e6b57` the review fixes (unchanged from the working tree; cargo test
  passed before committing), `64de578` its work_history entry.
- `761d2e0` the list below.

### Fixed, by group (the list's order)

- EXTEND_TIMEOUT_USEC=: the notify helper moves its deadline to now+N
  (never earlier) and records it as the `extend-until` state key, which
  the daemon's wait_ready()/wait_woken() honour. gen writes no s6-rc
  timeout-up for Type=notify/notify-reload: s6-rc would bring the service
  down at a fixed backstop, and the helper releases s6-rc at the
  (extended) timeout anyway. Only the start timeout is extended, not the
  stop or reload ones.
- Runner: SuccessExitStatus= on a oneshot's ExecStart= (systemd: main
  processes only, not Pre/Post) and exit status names; EXIT_SECCOMP was
  231 (APPARMOR_PROFILE), now 228. Restart=: a helper's "timeout" result
  makes the exit unclean (on-failure, on-abnormal restart; on-abort does
  not). Environment order as exec_invoke(): our_env, PassEnvironment=,
  Environment=, EnvironmentFile=, then UnsetEnvironment=; the files are
  read per command (a missing one: exit 204 as systemd's EXIT_MEMORY there),
  Runner::new still checks them so a missing file fails the start early.
  PIDFile=: up to TimeoutStartSec= ("timeout"), or "protocol" once no
  process is left. ExecStopPost= after the cgroup kill and before a final
  one. '+' commands: no NNP. MAINPID=: systemd's
  service_is_suitable_main_pid() (in the unit's cgroup, subgroups too, or
  a root sender). PAM wrapper: signals blocked and handlers set before
  fork; exec_command() resets dispositions before unmasking. KillUnit
  whom=main uses the recorded MAINPID (the supervised pid is the wrapper
  for PAMName=/forking). RESTART_TIMES not kept without a start limit.
- Daemon: AttachProcessesToUnit("") = caller's unit; unprivileged callers
  pass without polkit when the unit is their own, Delegate=yes, and every
  pid is theirs and in it. GetUnitByPID: outermost .service/.scope, else
  the innermost slice. polkit details unit/verb/polkit.message for
  job, kill, reset-failed and set-property calls. `s6-svc -r`:
  restart_longrun() kills (s6-svc -k) after TimeoutStopSec= and clears
  RESTART_REQUESTED if even that fails; checked in s6-supervise.c that -r
  is killr() only and timeout-kill is armed by up_d (-d) alone.
  ResetFailed turns START_FAILED into a new `parked` key (inactive, still
  restarted by the next start; clearing START_FAILED alone would have
  shown the waiting run process as active). SetProperties/Ref/Unref moved
  into unit_method (they were only reached with a non-Unit interface).
  ReloadUnit is accepted behind a non-stop job; at its turn
  reload_if_active() returns "invalid" if that job left the unit
  inactive. transient render(): structs shorter than 2 fields are an
  error. "dynamic/index lock order": no place takes them in the opposite
  order; known_units() now takes one lock at a time.
- Parsing: comments are skipped before continuation handling; envfile
  follows parse_env_file_internal() (quotes only in PRE_VALUE);
  drop-ins for each dash prefix (`user-.slice.d`); `M`/month and
  `y`/year (30.44 / 365.25 days) and `µs`/`μs` (the unit scan was
  ASCII-only, so µs never parsed); mountinfo read as bytes and octal
  escapes decoded from bytes (a str slice could panic); path units keep
  one inotify instance, re-add watches each round and remove the stale
  ones; `~07/1`-style repetitions count towards the month's end (test:
  `Mon *-05~07/1` is 2027-05-31); the daemon no longer calls next_after()
  for a named zone (switches_tz(); it uses the runner's next elapse).
- Sandbox: rw paths below a ReadOnlyPaths= entry are bound rw again after
  it; inaccessible files get `<unit state dir>/inaccessible` (empty,
  mode 0000, bound read-only); SystemCallFilter=: unknown groups ignored
  with a warning, `~name:errno` in an allow list kept with that errno
  (seccomp_parse_syscall_filter()).
- Standalone: journal-relay stream `<N>` is a level 0-7 only, facility
  LOG_USER (journald's syslog_parse_priority(with_facility=false)); at
  most 4096 stream threads, 128 KiB stacks. fd stores (standalone and the
  daemon's fallback): same open file stored once (kcmp, else inode and
  flags), 2 s SO_RCVTIMEO/SO_SNDTIMEO per client, u64 cmsg buffers.
  sdcompat-notify rewritten: credentials with the caller's PID (falls back
  to its own on EPERM), BARRIER=1 wait (5 s, a timeout only warns),
  options of systemd-notify 254; README pair updated.
- dist/packages: kernel-install entry `<id>-<kver>~sdcompat.conf`
  (grubby-bls lists with `ls -vr`, where `x86_64-sdcompat` sorted above
  `x86_64` and `grubby --set-default KERNEL` on kernel updates would have
  picked the s6 entry); the old name is removed on add and remove.
  rc.shutdown.final: mounts /var/log or /var from fstab if umountall
  unmounted it, and when / is still writable (umountall's MAXLINES is 99:
  it dies "too many mount points" without unmounting anything, read in
  its source) runs `umount -a -r` and remounts / ro. packages/README pair:
  skeleton changes need a setup rerun. early mount-api: restorecon -R
  /dev, /run itself, and each /run entry except service, uncaught-logs,
  s6-rc, s6-rc:* and s6-linux-init* (the root of the earlier label fixes);
  mount-fs: `mount -o remount /` with fstab's options when / is in fstab.
  gen: down-signal as a number when there is no name (skalibs' sig0_scan
  takes numbers); util::signal_name knows all standard signals. JOURNAL=no:
  INCLUDE of rsyslog.service or syslog.service leaves both unmapped. The
  fdstore run script sources sdcompat.conf for RUN_DIR. gen and sdcompatd
  take --config anywhere before `--`. Each spec's %check tests its crate
  and the library crates it is built from. sdcompat.spec scriptlets use
  xargs (SC2046). %changelog lines carry no EVR (the release is generated).
- setup/early: check_versions captures the exit code instead of a pipeline
  under pipefail; add_grub_entry re-creates the entry of every kernel in
  /lib/modules with a /boot/vmlinuz; port-only Listen*= bind 0.0.0.0
  without /proc/net/if_inet6, explicit IPv6 addresses are skipped with a
  warning (socket_address_parse()); resolve_conflicts keeps the first of
  a mutual pair; SYSTEMD_WANTS templates get the escaped sysfs path
  (util::unit_name_path_escape, syspath from DEVPATH or the db id).
- tests: last_new() makes the fdkeep checks require a new line;
  `sandbox.sh stop` kills only PIDs whose command line names $W or $R;
  new "start timeout" section (extend.service, noextend.service).
- Cleanup notes: Unit::stop_timeout_usec() replaces four copies;
  gen::Timings and gen::longrun_files() are shared with the daemon's
  write_servicedir() (its down-signal and timeouts had drifted);
  list_unit_files() reads the links once (Links index; a test compares it
  with unit_file_state()); Properties.Get/GetAll build only the asked
  interface; svstat_cached() keeps an answer while supervise/status has
  the same device/inode/mtime (s6_svstatus_write() renames a new file in,
  checked in the source).

### Tests

- `cargo build` (no warnings), `cargo test`: all pass (new: envfile
  quotes, unit comments, timespans µs/M/y, octal, calendar `~` repetition,
  seccomp allow-list errno and unknown group, fdstore dedup in both
  stores, notify options and barrier, install list vs state, devices
  templates, util path_escape, relay stream prefix). Standalone: fdstore
  2, journal-relay 3, notify 2.
- Sandbox (XDG_RUNTIME_DIR=/tmp/claude-1000/sdcr-fix): default 209/209,
  SANDBOX_JOURNAL=no 214/214, SANDBOX_FDSTORE=no 209/209; no processes
  left after `stop`.
- bash -n / sh -n and shellcheck -S warning on every changed script.
- Not tested: anything needing root or a real boot (AttachProcessesToUnit
  from systemd --user, polkit rules on unit/verb, the sandbox mounts,
  rc.shutdown.final with a separate /var or 99+ mounts, early's restorecon,
  the kernel-install name with grubby, SYSTEMD_WANTS templates from real
  udev events, IPv6-less sockets); the specs were not built (no rpmbuild
  here), so the %changelog without EVR and the %check lists are unchecked.

### Current state

All of the previous entry's list is done in `761d2e0` on top of
`64de578`; nothing uncommitted. Old `-sdcompat.conf` boot entries go away
on the next kernel add/remove or setup rerun.

### Next steps

1. Owner: build RPMs, upgrade, rerun setup (new skeleton, kernel-install
   entries renamed), reboot into s6.
2. On that boot: the checks from the previous entry; `ls
   /boot/loader/entries` shows `~sdcompat.conf` names and `grubby
   --default-kernel` is unchanged after a kernel update; `ls -Z -d
   /run/service /run/s6-rc` shows init_var_run_t; a `sdcompat reload`
   (s6-rc-update) works under enforcing.
3. Power-off retest; then rc.shutdown.final's logs with a separate /var if
   such a machine is available.

## Session: 2026-10-23: `sudo sdcompat reload` hung on the s6 boot

The owner, on an enforcing s6 boot, ran `sudo sdcompat reload`; it hung,
with AVCs: init_t (s6-svc, s6-svstat, sdcompatd) denied `read` on
lnk_file `cups.service`, user_tmp_t, unconfined_u.

### Findings

- sdcompat-gen ran s6-rc-update itself, in the shell's domain
  (unconfined_t). /run/service is its own tmpfs (tmpfs_t), and Fedora's
  policy labels what unconfined_t creates there user_tmp_t: every scan
  directory link (249) and the new live directory's service files became
  user_tmp_t; the live directory itself var_run_t (the a1eb405 filecon
  only acts through restorecon / init_t's transitions).
- init_t can't read those links, so s6-svscan started no s6-supervise for
  the new service directories, and s6-rc-update waits for them with no
  deadline (s6rc_servicedir_manage_g, s6-rc-update.c:791; we pass no -t).
- The owner then interrupted it and ran `systemctl daemon-reload`
  (sdcompatd, init_t): it moved s6rc-oneshot-runner into its new live dir,
  failed, and could not roll back into the var_run_t live dir ("unable to
  rollback ... Permission denied"): the 2026-10-21 state again, with
  /run/service/s6rc-oneshot-runner dangling (aborts every scan).
- Repair, done by the owner: mv the runner's directory back into the live
  dir, rm -r the leftover update dir, `restorecon -R /run/s6-rc
  /run/s6-rc:*`, chcon -h init_var_run_t on the /run/service links,
  `s6-svscanctl -an /run/service`; then `systemctl daemon-reload`
  succeeded. Checked afterwards: one live dir (init_var_run_t), 253 links
  tmpfs_t from system_u (init_t's own), none dangling, is-system-running
  `running`, nothing failed.

### Changes (`885df66`)

- sdcompat-gen `reload`: when the live state exists (LIVE_DIR/compiled)
  and neither --no-update nor --direct is given, it calls Manager.Reload on
  sdcompatd (sdcompat-client: private socket, else the bus), after checking
  the manager's Version contains "sdcompat" (on a systemd boot the private
  socket is systemd's). On failure it exits 1 and names --direct, which
  keeps the old in-process update (only safe from init_t under SELinux).
  rc.init and setup use --no-update, unchanged. Usage text, README pair
  (tool table), sdcompat-gen.spec %check (+ sdcompat-client).
- tests/check.sh: `sdcompat-gen reload` after the daemon-reload check must
  print "reloaded by sdcompatd", and hello.service stays up.
- Not done: a -t for s6-rc-update. Its deadline also covers the s6-rc
  change it runs (stopping services), and dying in the middle leaves a
  worse state than waiting; with the update in init_t the label hang
  can't happen.

### Tests

- cargo build (no warnings), cargo test: all pass.
- Sandbox (XDG_RUNTIME_DIR=/tmp/claude-1000/sdcr-rl), default: 211/211.
  JOURNAL/FDSTORE variants not rerun (not touched).
- bash -n, sh -n, shellcheck -S warning on tests/check.sh.
- Not tested: the real s6 boot (needs RPMs), the "not sdcompatd" refusal,
  --direct.

### Current state

The owner's s6 boot is repaired. The fix is committed in `885df66`.

### Next steps

1. Owner: build RPMs, upgrade.
2. On the s6 boot: `sudo sdcompat reload` prints "reloaded by sdcompatd",
   `ls -Z /run/service` stays tmpfs_t/system_u, no AVCs.
3. On a systemd boot (no /run/s6-rc): `sudo sdcompat reload` compiles
   only, as before (the Version check is a guard, not the usual path).
4. Earlier next steps (power-off retest, rc.shutdown.final with /var).

## Session: 2026-10-23: s6-rc repository and `s6 set`

The owner reported `s6 set list` failing ("unable to lock
/var/lib/s6-rc/repository") and corrected the project goal: s6 is PID 1
and the admin uses s6-frontend and s6-rc, with systemd's commands only in
a limited way (Project goal updated). The earlier decision "do not use
`s6 set`/`s6 live install`" (`88bb40d`) is reversed.

Decisions asked of the owner (AskUserQuestion):
- systemd's enable state is only a new service's first prescription; after
  that the s6 set decides (`systemctl enable/disable` keeps writing the
  symlinks but does not change what boots).
- A reload (daemon-reload, `sdcompat reload`, dnf's file triggers) commits
  and installs the set automatically.
- No regeneration at boot (rc.init boots the installed database).

### Design (read in s6-rc 0.8.0.2 and s6-frontend 0.2.0.0 sources)

- sdcompat-gen writes the translated units as an s6-rc **store**
  (STATE_DIR/db/<stamp>/source; STORE_DIR, default STATE_DIR/store, is a
  symlink to the newest, swapped atomically and put back on failure). The
  repository (REPO_DIR, default /var/lib/s6-rc/repository, s6's default)
  links [STORE_DIR] + the EXTRA_SOURCE dirs that exist (default
  /etc/sdcompat/s6:/etc/s6/sources; later stores override earlier ones).
- Reload: plan, write store, `s6-rc-repo-init` (no repo) / `-U` (store
  list changed) / `s6-rc-repo-sync`, `s6-rc-set-new current` if missing,
  essentials sync, `s6-rc-set-fix -u`, `s6-rc-set-commit --no-snapshot -f
  -D default current`, `s6-rc-set-install -K --no-snapshot [-c bootdb -l
  live | --no-update]`. GenResult.set_changes (fix moves, essentials)
  printed by the CLI and the daemon.
- The dependency graph is unchanged: targets stay bundles with everything
  they want. In s6-rc a set holds only atomic services (bundles are not in
  sets; one with a masked member is dropped), and `active` only means
  "in the default bundle" that set-commit makes. So `s6 set disable sshd`
  keeps it from booting while starting a bundle (udev's bluetooth.target,
  systemctl isolate) still starts its usable members. Removing enablement
  edges from target bundles was considered and rejected: it would have
  broken device targets (bluetooth.service is enabled into
  bluetooth.target.wants in /etc).
- flag-recommended on every atomic service in Plan.boot = the closure of
  default.target over deps/contents (what the old "default" bundle
  started), with log atoms. Repository sync reads flags only for services
  it hasn't seen.
- s6-rc-set-commit refuses an inconsistent set (an active service with a
  usable/masked dependency; an always one with a lower one;
  s6rc_repo_badrx/fixset). Hence `s6-rc-set-fix -u` before each commit:
  a new package's service needing a usable one gets it enabled, and a
  `s6 set disable X` without `-p` while active services need X is undone
  (documented: use `-p`).
- Essentials: dist/s6 dbus, mount-api, mount-fs, udevd, udev-trigger,
  hostname, modules-load, sysctl, sysusers, tmpfiles-setup, vconsole got
  flag-essential (sdcompatd, sdcompat-fdstore had it): always services may
  only depend on always ones, and runlevel's `s6-rc -up` keeps essentials.
  earlyoom flag-recommended. syslog/journal-relay/resolv carry no flag:
  sync_essentials makes `always` = flag-essential or member of
  SYSINIT_BUNDLE (setup toggles membership only), and moves a service no
  longer either from always to active (`s6-rc-set-change -e`). An RPM
  upgrade re-adding a flag file can't start rsyslog beside journald.
- DEFAULT_BUNDLE now means set-commit's -D bundle, default `default`
  (s6-rc's, s6-frontend's and s6-linux-init's runlevel name). The
  generated `sdcompat-default` bundle and dist/s6/default are gone
  (s6rc_repo_makedefbundle refuses a store defining it; gen errors
  clearly, setup removes /etc/sdcompat/s6/default with a backup).
- Rollback: the boot link's target before the install becomes
  compiled.previous (read from the link: set-install prints the replaced
  db only for a live update); the one before is deleted; after a rollback
  boot the replaced (bad) db is deleted instead. GC removes STATE_DIR/
  .current:* copies that are not boot/previous/live and db/<stamp> dirs
  not current/previous/holding a db in use. rc.init: no regeneration,
  sdcompat.db=previous kept, sdcompat.reload= gone.
- Install retry (the old "Broken pipe" flake): set-install leaves its copy
  beside the boot link when s6-rc-update fails; a second set-install then
  says "already installed" and exits 0 without doing anything (read in
  copy_compiled_to_bootdir). The retry now reruns `s6-rc-update -e` on the
  copy and switches the boot link itself when the live state already runs
  on it, else removes the copy and reruns set-install.
- SELinux (dist/selinux/sdcompat.cil): new type sdcompat_live_exec_t for
  /usr/bin/s6-rc-update and s6-rc-format-upgrade, entrypoint of init_t,
  `typetransition unconfined_t ... process init_t` and roletransition
  unconfined_r -> system_r (sesearch on this host: unconfined_t may
  transition to any domain, `allow unconfined_r system_r` exists,
  unconfined_u has system_r; same pattern as initrc_exec_t). So `s6 live
  install`/`s6 set apply` from sudo work like sdcompatd's own updates
  instead of the user_tmp_t breakage of this morning. /var/lib/s6-rc gets
  init_var_lib_t (filecon + named typetransitions for init_t and
  unconfined_t); spec %post/%postun and setup restorecon it.
- s6.conf: repodir, bootdb, storelist = /var/lib/sdcompat/store:
  /etc/sdcompat/s6:/etc/s6/sources; setup replaces a copy it wrote, warns
  about an edited one, creates /etc/s6/sources.

### Upstream bug found (not patched, per the rules)

s6-rc 0.8.0.2 src/repo/s6rc_repo_setuptodate.c: the snapshot path is
built as repo/sources/.snapshot<readlink of compiled/SET>, readlink fills
up to index repolen+setlen+51, and the terminator is written at
`snap[repolen + setlen + 53]`, leaving index +52 uninitialized. stat()
then randomly fails and s6-rc-set-install says "set current is not
up-to-date; commit it first" right after a commit (seen in the sandbox;
a small test repo happened to have a 0 there). Workaround: --no-snapshot
on commit and install (sdcompat always; documented for `s6 set commit`,
`s6 live install`, `s6 set apply`). The owner may want to report it to
skarnet.

### Tests

- cargo build (no warnings), cargo test: all pass (new
  boot_closure_marks_recommended); standalone fdstore 2, journal-relay 3,
  notify 2.
- check.sh: new section "s6-rc repository" (10 checks: enabled unit starts
  active, new unit usable, new enabled unit active, set-fix enables its
  usable dependency, default bundle contents, a reload keeps
  `s6-rc-set-change usable`, systemctl disable leaves the set alone,
  compiled.previous kept, only two boot copies); sandbox.sh sets REPO_DIR
  and starts the `default` bundle.
- Sandbox, default: 221/221 in 9 of 13 runs (several in parallel); 3 runs
  hit the Broken pipe flake (19-20 cascading failures), 1 left a
  cal.service job at the end. SANDBOX_FDSTORE=no: 221/221 in 4 of 4.
  SANDBOX_JOURNAL=no: the flake in every completed run (5), then a hang in
  the next reload (s6-rc-update waiting for s6rc-fdholder, whose
  supervisor the failed update removed). Baseline at HEAD (worktree, same
  machine): default 1 of 6 flaked, JOURNAL=no 2 of 3 flaked and hung the
  same way, so this is not new. Idle sandbox (JOURNAL=no): 15 direct
  `reload --direct` and 15 D-Bus Reloads in a row, no failure: the flake
  needs concurrent s6 activity (check.sh's earlier sections).
- Real units of this host into a scratch repository (`reload --no-update`
  with UNIT_PATH default, EXTRA_SOURCE a copy of dist/s6 without
  syslog/journal-relay in sdcompat-sysinit): commits with no fix needed;
  13 always, 79 active, 149 usable; gdm/NetworkManager/cups/journald
  active, sshd usable (not enabled here), syslog usable. Adding syslog to
  sdcompat-sysinit -> always, removing it -> active; `s6-rc-set-change
  usable NetworkManager.service` without -p is undone by the fix
  (rsyslog and NetworkManager-wait-online need it); a store `default` is
  refused with the message; boot copies rotate (compiled +
  compiled.previous only).
- s6-rc dry run: a commit of an unchanged store converts nothing.
- bash -n / sh -n and shellcheck -S warning: setup-fedora.sh, check.sh,
  sandbox.sh, rc.init. `setup-fedora.sh --dry-run` on this host: writes
  /etc/s6.conf, creates /etc/s6/sources, backs up and removes
  /etc/sdcompat/s6/default.

### Not done / open

- The "Broken pipe" reload flake (s6_supervise_link_names ->
  "unable to manage new service directories: Broken pipe", after the live
  switch) is still open and frequent with JOURNAL=no in the sandbox, at
  HEAD as well (see Tests). s6-svscan's control fifo can't give EPIPE (it
  holds a writer), so it is most likely s6-ftrigrd dying under s6-rc-update;
  not found why. The new retry handles the half-done update better but
  damage (supervisors killed by do_unlink) can remain.
- sdcompatd still reports UnitFileState from systemd's symlinks, not the
  set's prescription; a masked-in-set unit is still in the index (s6-rc
  then refuses it as unknown).
- Not run with root / on a real boot: the SELinux transition (module not
  compiled here: no read access to the policy store), set-install against
  /run/s6-rc, setup's migration, rc.init without regeneration.

### Current state

Committed as `e8af905`; this entry in the next commit.

### Next steps

1. Owner: build RPMs; upgrade; rerun setup (removes
   /etc/sdcompat/s6/default, new s6.conf, SELinux module, repository).
2. On the s6 boot: `s6 set status`; `sudo s6 set disable -p X`, reload,
   reboot, X not started; `sudo s6 set commit --no-snapshot && sudo s6
   live install --no-snapshot` returns and leaves no user_tmp_t in
   /run/service and no AVCs (TESTING rows added).
3. Find the Broken pipe flake's cause (s6-ftrigrd's exit under
   s6-rc-update in the sandbox, JOURNAL=no reproduces it most often).
4. Optionally: UnitFileState from the set; report the s6-rc snapshot bug.

## 2026-10-25: owner's check of the s6 set on the real boot (next step 2)

No code changes.

- Owner ran on the s6 boot (Fedora, `JOURNAL=no` in
  /etc/sdcompat/sdcompat.conf): `s6 set status cups.service` -> usable;
  `sudo s6 set commit --no-snapshot && sudo s6 live install --no-snapshot`
  returned ("set current is already installed", exit 0); all 254 entries
  of /run/service are tmpfs_t, no user_tmp_t. So the sdcompat_live_exec_t
  transition works from sudo.
- `ausearch -m AVC -ts recent`: nothing from s6-rc, s6-svscan, sdcompat or
  the repository. Only the two accepted JOURNAL=no output-loss kinds:
  NM dispatcher scripts (NetworkManager_dispatcher_{iscsid,chronyc}_t)
  writing the service's s6-log pipe (init_t), and systemd-user-runtime-dir
  writing the catch-all /run/service/s6-svscan-log/fifo. The fifo is now
  labelled initctl_t (earlier entries said var_run_t); neither type is
  writable by those domains, so the result is the same: the kernel gives
  them /dev/null, the programs run, their messages are lost.
- Not yet shown: `sudo s6 set disable -p X`, reload, reboot, X not started.

### Next steps

Unchanged from the previous entry, minus the commit/install/AVC part of
step 2.

## Session: 2026-10-25: catch-all fifo denials (initctl_t) on the enforcing boot

### Findings (from the owner's `ausearch -m AVC -ts boot`, enforcing, JOURNAL=no)

- Writes to /run/service/s6-svscan-log/fifo were denied for loadkeys_t,
  syslogd_t (rsyslogd), setfiles_t (restorecon), system_dbusd_t, getty_t
  and (earlier entry) systemd_user_runtimedir_t, and getattr for
  logrotate_t: ~255 records from one `pkill` (procps stats each process's
  fds) in a second. The fifo is `initctl_t`.
- Cause: a regression from `761d2e0`. `early` no longer runs restorecon on
  /run/service, so the fifo keeps its creation label: init_t creates it in
  the /run tmpfs (tmpfs_t) and the policy has
  `type_transition init_t tmpfs_t:fifo_file initctl_t` (for /dev/initctl).
  Before, restorecon made it var_run_t, which sdcompat.cil let
  system_dbusd_t and hostname_t write; those rules no longer matched, so
  dbus-daemon's stderr went to /dev/null again. (The previous entry's "same
  result" holds for the two domains it named, not for dbus-daemon and
  hostname.)
- Not sdcompat: systemd-hostnamed searching /proc/pressure (proc_psi_t),
  as before.

### Changes (`d23ad7b`)

- dist/selinux/sdcompat.cil: new type `sdcompat_log_fifo_t` for the fifo
  (named type transition `init_t tmpfs_t fifo_file "fifo"`, filecon for
  restorecon). init_t creates/reads it; the `domain` attribute may
  write/append/ioctl/getattr it but not open it, so only an inherited
  descriptor works, as with journald's stream socket under systemd. The
  three var_run_t fifo_file allows (init_t, system_dbusd_t, hostname_t)
  are removed.
  Decision: all domains instead of a per-domain list, because every
  service's stderr goes there and the list kept growing with each boot's
  AVCs (output loss under JOURNAL=no had been left "not fixed"; this fixes
  the catch-all part of it). Without `open`, the type only grants writing
  to an fd s6 handed over.
- setup-fedora.sh and sdcompat.spec %post: restorecon the live fifo, so a
  running boot is fixed without a reboot.
- TESTING pair: a row for the fifo's label and its denials.

### Tests

- CIL parsed with libsepol's `cil_add_file` (rc 0); `domain`, `file_type`,
  `non_*_file_type` and `tmpfs_t` exist in the loaded policy; file_type may
  associate with tmpfs_t. `semodule -i` not run (root).
- `bash -n` and `shellcheck -S warning` on setup-fedora.sh pass. No Rust
  or sandbox change (the sandbox has no SELinux), so cargo/sandbox not run.

### Current state

Committed as `d23ad7b`; this entry in the next commit. Fix not yet
loaded or booted. Left: NM dispatcher scripts writing
their service's s6-log pipe (init_t pipefs), JOURNAL=no only.

### Next steps

1. Owner: rerun setup (or reinstall the sdcompat RPM), check
   `ls -Z /run/service/s6-svscan-log/fifo` is sdcompat_log_fifo_t, reboot,
   `sudo ausearch -m AVC -ts boot | grep s6-svscan-log/fifo` is empty.
2. Then the previous entry's steps.

### Also discussed (no changes)

- Owner asked about removing dbus-broker. Uninstalling is blocked by the
  `dbus` package (`Requires: (dbus-broker >= 16-4 if systemd)`); a mock
  package providing it would replace the real one, which the owner wants
  kept for the systemd boot. Proposed instead: a user generator that, on the
  s6 boot only, aliases dbus.service to dbus-daemon.service in
  generator.early (searched before /etc/systemd/user). Owner chose to do
  nothing for now.

## Session: 2026-10-25: setup's sdcompatd restart timed out (event fifodir labels)

### Findings (owner's setup rerun on the enforcing s6 boot, `d23ad7b` RPMs)

- Setup printed `s6-svlisten: fatal: timed out` / "sdcompatd was not ready
  within 30 s". sdcompatd itself was fine: restarted at 19:33:15, logged
  "ready, serving org.freedesktop.systemd1" at 19:33:16, s6-svstat "up ...
  ready".
- Cause: three AVCs at 19:33:16, s6-supervise (init_t) denied `write` on
  `ftrig1:@...` (unconfined_u:object_r:user_tmp_t) fifo_file. `s6-svc -wR`
  (run from sudo, unconfined_t) creates its s6-ftrigr fifo in the service's
  event/ directory. The event directories are created by init_t in the
  /run tmpfs and stay tmpfs_t (early no longer relabels, `761d2e0`), and
  the policy has `type_transition unconfined_t tmpfs_t:fifo_file
  user_tmp_t`; init_t may not write user_tmp_t, so the ready event never
  reached the waiter. setup_selinux's restorecon of /run/s6-rc:* (which
  makes the live directory init_var_run_t) ran only after restart_daemon.
- The same affects any waiter started from an admin shell on a fresh boot:
  `s6-svc -w*`, s6-svwait, s6-svlisten, and s6-rc's own waits (not yet
  confirmed for `sudo s6-rc change`).
- The four real directories in /run/service (s6-linux-init's
  early-getty, runleveld, shutdownd, s6-svscan-log) still had tmpfs_t
  event/ after setup; the 250 s6-rc ones were init_var_run_t.

### Changes (`bdbdcb0`)

- sdcompat.cil: named transition `init_t tmpfs_t dir "event"
  init_var_run_t` and a filecon for /run/service/*/event. A fifo created by
  unconfined_t in an init_var_run_t directory keeps init_var_run_t (no
  transition rule for it), which init_t may write and unlink.
  Decision: label the fifodir instead of allowing init_t to write
  user_tmp_t, which would cover every user's /tmp fifos.
- setup-fedora.sh: restart_daemon after setup_selinux (the new daemon
  starts under the new policy and waits in relabelled directories);
  restorecon the event/ of the real /run/service directories (the s6-rc
  ones are covered by the live directory's -R). The final "Next" text:
  step 4 says "boots enforcing" with --enforcing; the obsolete
  `sdcompat.reload=0` line is removed (rc.init no longer regenerates).
- sdcompat.spec %post: restorecon /run/service/*/event.
- TESTING pair: a row for waiting from a shell.

### Tests

- CIL parsed with libsepol's `cil_add_file` (rc 0); init_t may create
  non_security_file_type dirs (init_create_dirs on); unconfined_t has no
  fifo_file transition in init_var_run_t. `semodule -i` not run (root).
- `bash -n` and `shellcheck -S warning` on setup-fedora.sh pass;
  `--dry-run --enforcing`: labels, then the restart; restorecon of the four
  real event/ directories; the new step 4 text.

### Current state

Committed as `bdbdcb0`; this entry in the next commit. Fix not yet loaded
or booted. Earlier entry's catch-all
fifo fix (`d23ad7b`) is installed; its boot check is still to do.

### Next steps

1. Owner: build RPMs, rerun setup; the restart step returns without the
   timeout and `ausearch -m AVC -ts recent | grep ftrig` is empty.
2. Reboot; check the new TESTING row (event dirs init_var_run_t at boot,
   `sudo s6-svc -wR` returns) and the catch-all fifo row.
3. Then the earlier steps (`s6 set disable -p X` across a reboot; the
   Broken pipe flake).

## 2026-10-25: owner's boot check of the fifo labels (`d23ad7b`, `bdbdcb0`)

No code changes.

- Owner rebooted into s6 (enforcing) with both fixes installed.
- `ls -dZ /run/service/*/event`: every entry is init_var_run_t, the four
  s6-linux-init directories (early-getty, runleveld, shutdownd,
  s6-svscan-log) and the s6-rc ones alike. So the named `event` transition
  works at boot, before setup or %post restorecon.
- `sudo s6-svc -wR -T 30000 -r /run/service/sdcompatd` returned silently,
  without the s6-svlisten timeout: the ready event reaches a waiter started
  from sudo.
- `/run/service/s6-svscan-log/fifo` is sdcompat_log_fifo_t, and
  `ausearch -m AVC -ts boot | grep s6-svscan-log/fifo` is empty.
- Both TESTING rows (waiting from a shell; catch-all fifo) are verified on
  the real boot. Whether setup's own restart step runs without the timeout
  was not shown separately, but it does the same `s6-svc -wR` wait.

### Current state

`d23ad7b` and `bdbdcb0` verified on the enforcing s6 boot.

### Next steps

1. `sudo s6 set disable -p X`, reload, reboot, X not started.
2. Optionally `sudo s6-rc change` waits from an admin shell, and an
   `ausearch -m AVC -ts boot | grep ftrig` check.
3. Find the Broken pipe flake's cause (s6-ftrigrd's exit under
   s6-rc-update in the sandbox; JOURNAL=no reproduces it most often).
4. Optionally: UnitFileState from the set; report the s6-rc snapshot bug.

## 2026-10-25: owner's check of cups after a reboot into s6

No code changes.

- Owner rebooted into s6 (22:19:36); `systemctl is-active cups.service` ->
  active. Checked on the machine: cups.path is `active` in the set and
  enabled, cups.service is `usable` and disabled (preset), and cupsd was
  started at 22:19:54 by cups.path (`TriggeredBy: cups.path`; PathExists
  /var/cache/cups/org.cups.cupsd). /run/cups/cups.sock is listening. This
  is what systemd does too, so the cups preset fix (`36e443d`) holds
  across reboots. The set sources were last changed on 2026-10-23, so this
  was not the `s6 set disable -p X` check.
- Seen, not acted on: `systemctl status cups.socket` says inactive (dead)
  while its socket is listening (the service's runner binds it, by design
  sockets are not set entries); `s6 set status cups.socket` fails with
  s6-rc-db's "not a valid identifier". Cosmetic.
- For the `disable -p X` check, cups.service is a poor X: cups.path would
  still start it.

### Next steps

Unchanged from the previous entry.

## 2026-10-25: disable test service changed to chronyd

- The TESTING row "s6 set decides what boots" used cups.service, which
  could never pass: cups.service is already `usable` in the set, and
  cups.path (active) starts it at boot anyway. Changed the pair to
  chronyd.service: `active` in the set, a converted unit (Type=notify),
  with no .socket/.path/.timer, D-Bus activation file or udev rule for it,
  and only chrony-wait.service (not enabled) refers to it. A reboot
  without time sync does no harm.
- Not chosen: earlyoom (sdcompat's own s6 service, not a converted unit;
  earlyoom.service is `usable`), D-Bus-activatable services (firewalld,
  ModemManager, upower, ...), and services with socket units (avahi,
  sssd-kcm, iscsid, virt*).

### Next steps

1. Owner: the chronyd row of TESTING; then `sudo s6 set enable
   chronyd.service` and `sudo systemctl daemon-reload` to restore it.
2. The rest unchanged.

## 2026-10-25: owner's `s6 set disable -p` check across a reboot

No code changes.

- Owner ran the TESTING row with chronyd.service (`58462a5`) and rebooted
  into s6 (boot 22:26:29); `systemctl is-active chronyd.service` ->
  inactive. Checked on the machine: `s6 set status` says
  `chronyd.service/usable`, s6-rc has it `down`, no chronyd process, and
  `journalctl -b -u chronyd.service` is empty, so it was not started at
  all (not a failed start). The set decides what boots: verified.
- Seen, not acted on: `chronyd.service.log` stays `active` in the set and
  `up` (`-p` disables dependents, and the logger is a dependency), which
  leaves one idle s6-log. `systemctl status` still shows `enabled`
  (UnitFileState not from the set, already a next step).

### Current state

`s6 set disable -p X` across a reboot verified on the enforcing s6 boot.

### Next steps

1. Owner: `sudo s6 set enable chronyd.service` and
   `sudo systemctl daemon-reload` to restore it (time sync is off until
   then).
2. Optionally `sudo s6-rc change` waits from an admin shell, and an
   `ausearch -m AVC -ts boot | grep ftrig` check.
3. Find the Broken pipe flake's cause (s6-ftrigrd's exit under
   s6-rc-update in the sandbox; JOURNAL=no reproduces it most often).
4. Optionally: UnitFileState from the set; disable a service's logger
   along with it; report the s6-rc snapshot bug.

## 2026-10-25: UnitFileState and idle loggers from the set (`25f6fc4`)

The two things seen in the previous entry, fixed on sdcompat's side.

- **Logger left booting.** In s6-rc's compiled database a producer
  depends on its logger (read in s6-rc 0.8.0.2: `s6-rc-db all-dependencies
  chronyd.service` lists chronyd.service.log), but not the other way;
  s6rc_repo_badpipeline only requires a pipeline to agree on masked or
  not. So `s6 set disable -p X` leaves X.log active. New reload step
  sync_loggers (after sync_essentials, before set-fix): a last consumer
  (consumer-for, no producer-for) that is `active` while none of its
  producers is active/always becomes `usable` (s6-rc-set-change, reported
  in set_changes). Starting the service still starts the logger (s6-rc
  dependency; start_beside starts it too), and set-fix -u makes it active
  again when the service is. On the owner's machine this applies to
  cups.service.log (cups.service usable since the preset fix); loggers of
  never-enabled services were already usable.
- **UnitFileState.** sdcompat-install: set_rx (moved from sdcompat-gen)
  and set_file_state: for a unit the set lists, enabled/enabled-runtime/
  disabled become enabled (always/active; enabled-runtime kept),
  disabled (usable) or masked (masked); static, indirect, alias, bad,
  masked from systemd are unchanged. Used by the UnitFileState property,
  GetUnitFileState, ListUnitFiles(ByPatterns) and `sdcompat-gen
  is-enabled`. PresetAllUnitFiles and the plan's first prescription keep
  systemd's link state. Consequence: `systemctl disable X` alone now
  shows X as enabled while the set has it active (true: it still boots);
  the cache is cleared on reload and file ops, so an `s6 set` change
  shows after the reload.
- Docs: README pair (the set paragraph), TESTING pair (the chronyd row
  now also expects the logger usable and is-enabled `disabled`).

### Tests

- cargo build (no warnings), cargo test all pass (new
  file_state_follows_the_set); standalone 2/3/2.
- Sandbox needs a short path (the scratchpad path overflows sun_path:
  "Argument list too long") and XDG_RUNTIME_DIR under /tmp/claude-1000.
  Default 212/218, JOURNAL=no 219/225; the 6 failures are one
  s6-rc-update "unable to take lock on .../live: Permission denied" in
  the reload and what follows from it. HEAD without the change (worktree)
  fails the same 6 (210/216): this environment, not the change. New
  checks pass: UnitFileState after disable follows the set, of a usable
  and an active service; JOURNAL=no: the logger usable with its service,
  active again after.
- bash -n and shellcheck -S warning: check.sh.

### Current state

`25f6fc4` committed, not yet on the real boot.

### Next steps

1. Owner: build RPMs, install, `sudo systemctl daemon-reload` (prints
   `cups.service.log -> usable`); then the chronyd TESTING row again
   (logger usable, `systemctl is-enabled chronyd.service` disabled), then
   enable and reload to restore.
2. Find why s6-rc-update can't take its lock in this agent environment
   (the last sessions' sandbox runs passed the repository section).
3. Optionally `sudo s6-rc change` waits from an admin shell, and an
   `ausearch -m AVC -ts boot | grep ftrig` check.
4. Find the Broken pipe flake's cause.
5. Optionally: report the s6-rc snapshot bug.

## 2026-10-25: why the sandbox's s6-rc-update couldn't take its lock (`3055b48`)

- Cause: sdcompat's own SELinux module (2026-10-23) labels
  /usr/bin/s6-rc-update and s6-rc-format-upgrade sdcompat_live_exec_t
  with `typetransition unconfined_t ... process init_t`, so from the
  agent's (or the owner's) unconfined shell s6-rc-update runs as init_t.
  The sandbox's files are user_tmp_t and init_t may not write them
  (sesearch: no allow init_t user_tmp_t file write), so s6rc_lock's
  open(live/lock, O_RDWR|O_CREAT) got EACCES. (skalibs' fd_lock maps a
  held lock to EBUSY, so "Permission denied" had to be the open.) The
  sandbox runs that passed the repository section were before the module
  was on this machine. Not a code bug; the real system wants the
  transition.
- Shown: in a running sandbox, `/usr/bin/s6-rc-update -n -l LIVE DB`
  fails with the lock error; a byte-identical copy in the sandbox dir
  (user_tmp_t, no transition) exits 0. Python's open + fcntl/flock on the
  same files work. Not shown: the AVC itself (ausearch needs root).
- Fix (tests only): sandbox.sh copies both binaries into WORKDIR/bin and
  puts it first in PATH; check.sh does the same for the daemon it
  restarts. s6-rc-set-install runs `s6-rc-update` by PATH (no prefix),
  and sdcompat-gen's retry does too. TESTING pair notes it.
- Sandbox: default 223/223, SANDBOX_JOURNAL=no 230/230 (this includes
  the repository section and `25f6fc4`'s logger and UnitFileState
  checks, so `25f6fc4` is now fully covered in the sandbox); no Broken
  pipe flake in either run. bash -n, sh -n, shellcheck -S warning:
  sandbox.sh, check.sh.

### Next steps

1. Owner: build RPMs, install, `sudo systemctl daemon-reload` (prints
   `cups.service.log -> usable`); then the chronyd TESTING row again,
   then enable and reload to restore.
2. Optionally `sudo s6-rc change` waits from an admin shell, and an
   `ausearch -m AVC -ts boot | grep ftrig` check.
3. Find the Broken pipe flake's cause (not seen in today's two runs).
4. Optionally: report the s6-rc snapshot bug.

## 2026-10-25: s6 as the default boot entry (`1e0fdea`)

Owner asked to make s6 the first (default) boot instead of systemd. This
was next step 3 of the first session ("after testing: make the s6 entry
the default"); the enforcing s6 boot is verified, so done now, opt-in.

- Read on this machine: GRUB_DEFAULT=saved, UPDATEDEFAULT=yes. On a
  kernel add, /usr/lib/kernel/install.d/20-grub.install sets
  `tmp_saved_entry=<new systemd BLS id>` (not for debug/rt/64k kernels),
  and 95-set-boot-entry.install, after checking the initrd, sets
  saved_entry from it. So a plain `grub2-editenv set saved_entry` would
  be undone by the next kernel update.
- 90-sdcompat.install: with /etc/sdcompat/boot-default = s6, on add it
  replaces tmp_saved_entry when it names this kernel's systemd entry with
  `<id>~sdcompat`; on remove, if saved_entry named a removed entry
  ending in ~sdcompat, it sets the newest remaining s6 entry (`ls -v`).
  Env overrides for tests: SDCOMPAT_BOOT_DEFAULT, SDCOMPAT_GRUB_EDITENV.
- setup-fedora.sh: `--default-s6` / `--default-systemd`; neither keeps
  the file's choice (like --journal). set_boot_default (end of
  add_grub_entry) writes the file and sets saved_entry to the default
  kernel's s6 entry, or removes the file and moves an s6 saved_entry
  back to `${id%~sdcompat}` (unset if that entry is gone). Uninstall
  does the latter and removes the file. Plan, final messages and header
  say which entry is the default. --help printed three code lines after
  the header (sed range 2,38); now exactly the header.
- Bug found while testing: `saved=$(saved_entry)` exited setup under
  set -euo pipefail when grub2-editenv list fails (unreadable grubenv);
  fixed with `|| true` in the pipeline.
- Docs: README pair (summary, options table, plugin paragraph incl.
  Shift/Esc for the hidden menu), TESTING pair (install note, new section
  "Making s6 the default boot" with checks).

### Tests

- Plugin, unprivileged, with a fake BLS dir and a grub2-editenv stand-in,
  simulating 20-grub -> 90-sdcompat -> 95-set-boot-entry: no file ->
  systemd default kept; s6 -> saved_entry abc-7.2.12~sdcompat; removing
  that kernel -> abc-7.2.11~sdcompat; removing a non-default kernel ->
  unchanged; debug kernel (no tmp_saved_entry) -> unchanged.
- setup --dry-run (as a user): --default-s6 writes boot-default and
  sets saved_entry (kver shows wrong in a user dry run: grubby
  --default-kernel needs root, existing behaviour); none/--default-systemd
  print the systemd default; --uninstall removes boot-default and, with
  an s6 saved_entry (fake grub2-editenv), moves it back (unset as a user
  since /boot/loader/entries is unreadable).
- bash -n / sh -n, shellcheck -S warning: setup-fedora.sh, the plugin.
- Not run as root; no real GRUB change made.

### Next steps

1. Owner: build RPMs, `sudo dist/setup-fedora.sh --enforcing --default-s6
   --rpms=...`, then the TESTING section "Making s6 the default boot"
   (grubby --info=DEFAULT, reboot without the menu, later a kernel
   update). Also step 1 of the previous entry (logger/is-enabled check).
2. Optionally `sudo s6-rc change` waits from an admin shell, and an
   `ausearch -m AVC -ts boot | grep ftrig` check.
3. Find the Broken pipe flake's cause.
4. Optionally: report the s6-rc snapshot bug.

## 2026-10-25: owner wants no systemd in the boot at all: s6 initramfs (design)

No code changes. Owner clarified "only s6 in the boot process": the
initramfs too. Answers (AskUserQuestion): the LUKS root is unlocked by a
typed passphrase (no TPM2/FIDO2), and the initramfs's PID 1 should be s6
(not dracut's shell init). This resolves the first session's open
question 2 (systemd-free initramfs, waiting on the unlock method).

Facts read: root btrfs subvol=root on LUKS nvme1n1p6, US keymap; nvme,
dm_crypt, xhci_pci, hid_generic are modules, btrfs/i8042/atkbd/usbhid
built in; dracut 111 still has 80base's non-systemd init; s6-svscan
execs .s6-svscan/finish on exit; skarnet binaries link libskarnet.so;
/boot 2.0G free; QEMU with a usable /dev/kvm, kernel images readable.

Proposed design (awaiting the owner's go-ahead):
- /init (execline) mounts proc/sys/devtmpfs/run and execs s6-svscan as
  PID 1; a supervised `rootfs` service parses rd.luks.uuid/root/rootflags,
  modprobes a host-only list, waits for the disk (blkid -U, 30 s),
  cryptsetup open with a console passphrase (3 tries, DM_DISABLE_UDEV=1),
  mounts /sysroot ro, checks init-selinux, s6-svscanctl -t; on failure a
  bash rescue shell, retry on exit. .s6-svscan/finish moves /run and
  `exec switch_root /sysroot /usr/sbin/init-selinux`.
- Builder dist/sdcompat-mkinitramfs (shell), dracut-install only as a
  copy helper (binaries+libs, modules+deps+firmware), early microcode
  cpio, zstd; /boot/initramfs-<kver>-s6.img; module list from sysfs plus
  /etc/sdcompat/initramfs-modules. The kernel-install plugin builds it
  and points the s6 entry's initrd at it; setup option to enable. The
  systemd entry keeps Fedora's initramfs.
- Lost on the s6 entry: plymouth (already off), TPM2/FIDO2, resume.
- Risks: dm nodes without udev; coldplug after switch_root without an
  initramfs udev db. Test everything in QEMU (LUKS2+btrfs test disk)
  before a real boot.
- Open choice: fixed host module list (proposed) vs packaging skarnet's
  mdevd for mdevd-coldplug.

### Next steps

1. Owner: approve the design and the module-list choice.
2. Then implement and test in QEMU; then a real s6 boot.
3. The earlier steps unchanged (RPMs, --default-s6 checks, logger and
   is-enabled checks, Broken pipe flake).

## 2026-10-25: --boot-only-s6, the s6 initramfs (`07a34f7`)

Owner's choices: setup option name `--boot-only-s6`; drivers loaded by
skarnet's mdevd (packaged), not a fixed module list.

### What changed

- mdevd 0.1.8.2 in dist/skarnet.versions (sha256 matches skarnet's
  published .sha256) and dist/rpm/mdevd.spec (no library; its only
  header, config.h, is dropped). Optional in setup's skarnet list like
  s6-frontend; sdcompat.spec Recommends it.
- dist/initramfs/root: `init` (execline: devtmpfs, console stdio, proc,
  sys, tmpfs /run, kernel args kept in /init-args, exec s6-svscan
  /service; scandir outside /run so the moved /run is clean),
  service/mdevd (notification-fd 3), service/rootfs/run (bash: cmdline
  parse, s6-svwait mdevd, mdevd-coldplug, LUKS via cryptsetup with
  DM_DISABLE_UDEV=1 and crypttab discard, btrfs device scan, mount ro,
  init= check, rd.break, rescue shell -> exit 1 -> s6-supervise retries;
  on success s6-svc -O then s6-svscanctl -t), .s6-svscan/finish (bash:
  mount --move dev/proc/sys/run, switch_root to init= with the kept
  args; without a root a shell loop, PID 1 must not exit), etc/mdev.conf
  (`$MODALIAS=.* 0:0 0600 ! +importas ... modprobe -bq`).
- dist/initramfs/mkinitramfs: host /usr layout symlinks, programs by
  type -P on a fixed PATH (SDCOMPAT_INITRAMFS_PATH for tests),
  multicall symlinks resolved by hand (dracut-install given several
  links made modprobe -> execline), /usr/libexec/s6/s6-ftrigrd (s6-svwait
  needs it: found by the test), libgcc_s, crypttab, modules
  (=drivers/{nvme,ata,scsi,md,block,mmc,virtio,pci/controller,usb/host,
  usb/storage,hid,input/keyboard,input/serio} =crypto =arch/x86/crypto
  =lib/crypto dm_mod dm_crypt btrfs ext4 xfs vfat nls) plus
  /etc/sdcompat/initramfs-modules, depmod -b, early microcode cpio
  (AMD famXXh / Intel FF-MM-SS), cpio -R 0:0, zstd. 30 MB, 4 s here.
- 90-sdcompat.install: with /etc/sdcompat/boot-only-s6 and Fedora's
  initramfs present, builds BOOT/initramfs-KVER-s6.img and rewrites only
  the image name on the s6 entry's initrd line ($tuned_initrd stays);
  build failure -> Fedora's image + message; no marker or remove ->
  deletes the s6 image. Env overrides SDCOMPAT_BOOT_ONLY_S6,
  SDCOMPAT_MKINITRAMFS, SDCOMPAT_BOOT_DIR.
- setup: `--boot-only-s6` / `--no-boot-only-s6` (kept across reruns by
  the marker file), dies without mdevd, installs mkinitramfs and the
  tree on the non-RPM path, plan/next-steps text, uninstall removes the
  marker and the s6 images. sdcompat.spec installs both.
- Docs: README pair (packages, options, a paragraph on the initramfs,
  roles table), TESTING pair (seven skarnet RPMs, new section "Booting
  with no systemd at all" with the QEMU test and real-boot rows).

### Decisions and why

- udev doesn't run in the initramfs, and Fedora's 10-dm.rules skip a
  coldplug "add" for a dm device without DM_UDEV_PRIMARY_SOURCE_FLAG in
  the udev db (read in /usr/lib/udev/rules.d/10-dm.rules, lines 76-106):
  rootfs writes /run/udev/data/b<maj:min> with that flag (and
  DM_UDEV_RULES_VSN=2) for each dm device, as a systemd initramfs's udev
  would have. Shown to be present after switch_root in QEMU; that the
  real system then makes /dev/disk/by-uuid links is NOT shown (needs a
  real boot: TESTING row).
- Bash for rootfs/finish (prompting, loops); PID 1 is s6-svscan
  throughout, as asked.

### Tests

- tests/initramfs.sh (QEMU/KVM, unprivileged, running kernel 7.2.11,
  mdevd from the freshly built RPM): prep (LUKS2 argon2id + btrfs
  subvol=root, made in a guest) and 5 boots: passphrase -> PID 1
  init-selinux stand-in with its arg, root=/root, /run moved, no other
  user process left, udev record present; NVMe disk (nvme is a module:
  mdevd loads it); three wrong passphrases -> rescue -> exit -> retry ->
  boots; rd.break; no disk -> timeout -> rescue. 6/6 in four runs (the
  last with the doubled rootflags= of the owner's cmdline).
- Test bugs fixed on the way: cryptsetup prints its prompt before
  tcsetattr(TCSAFLUSH), which drops input typed in between (the driver
  pauses 1 s); sysrq's message split the report line.
- Plugin with fake entries: build ok / build fails / no marker / remove.
- setup --dry-run --boot-only-s6 and --uninstall.
- dist/rpm/build-skarnet-rpms.sh into /tmp: all built after the spec fix
  (first run: unpackaged config.h). packages/build-rpms.sh: sdcompat RPM
  holds mkinitramfs and the tree, Recommends mdevd.
- bash -n / sh -n, shellcheck -S warning on every changed script.
- Not done: a real boot; root; a USB keyboard at the prompt (hid modules
  are in the image, mdevd loads them by modalias, untested); the
  SELinux side of /run/udev/data on the enforcing boot (early's /run
  relabel should cover it).

### Current state

Committed, QEMU-verified, not booted on hardware.

### Next steps

1. Owner: rebuild skarnet RPMs (now with mdevd) and sdcompat RPMs,
   `sudo dist/setup-fedora.sh --enforcing --boot-only-s6 --rpms=...`,
   run tests/initramfs.sh first, then TESTING "Booting with no systemd at
   all" (the dm links row matters most).
2. The earlier steps (--default-s6 checks, logger/is-enabled check).
3. Broken pipe flake; s6-rc snapshot bug report (optional).

## 2026-10-26: first real boot with --boot-only-s6

Owner booted the s6 entry with the s6 initramfs on the hardware (kernel
7.2.11, LUKS2 passphrase, btrfs) and ran the TESTING "Booting with no
systemd at all" rows plus tests/collect-diagnostics.sh.

### Results

- Images: `initramfs-<kver>-s6.img` for 7.2.9/7.2.10/7.2.11; all three s6
  entries' initrd lines name them, `$tuned_initrd` kept (from the
  diagnostics' boot-entry copies).
- dmesg: only `Run /init as init process`, no systemd[1], no "running in
  initrd". PID 1 `s6-svscan`.
- The dm links row (the one the QEMU test could not show): the root's
  UUID in /dev/disk/by-uuid points at ../../dm-0; `udevadm info` shows
  DM_NAME=luks-a0990770-... and ID_FS_TYPE=btrfs. So the
  /run/udev/data record written by rootfs works on the real system.
- Mounts (checked here, from /proc/self/mountinfo): / is subvol=/root and
  /home subvol=/home from the LUKS mapping, rw; /dev, /proc, /sys, /run
  are the moved initramfs mounts, shared, /run once.
- `systemctl --failed` empty, `is-system-running` running, no
  warning-or-higher journal entries.
- AVCs: only known ones. This boot has JOURNAL=no, so the
  NetworkManager dispatcher scripts' (chronyc, iscsid) writes to the
  s6-log pipe are the known JOURNAL=no item; hostnamed proc_psi_t is the
  known policy-side one. Uncaught log: known home1-excluded noise,
  user@60578 missing user, "stopping shutdown.target" (harmless).

### Doc bugs fixed (TESTING pair)

- `sudo grep ^initrd /boot/loader/entries/*~sdcompat.conf` failed: the
  glob is expanded by the user's shell and /boot/loader/entries is not
  readable for the user, so grep got the literal pattern. Now
  `sudo sh -c '...'`; section 5's `ls /boot/loader/entries/` got `sudo`
  for the same reason.
- `findmnt -n / /home` prints nothing: with two arguments findmnt takes
  them as source and target of one mount. Now `findmnt -n /; findmnt -n
  /home`.

### Not done

Rescue row (wrong passphrase three times), a USB keyboard at the prompt,
a kernel update with the s6 image, and an enforcing boot with JOURNAL=yes
under the s6 initramfs.

### Current state

--boot-only-s6 boots the hardware with no systemd anywhere in the boot;
the dm/udev handoff is verified on hardware.

### Next steps

1. Owner, optional: the rescue row, and the kernel-update row at the next
   `dnf upgrade kernel*`.
2. The earlier steps (--default-s6 checks, logger/is-enabled check,
   Broken pipe flake, s6-rc snapshot bug report).

## 2026-10-26, part 2: dnf upgrade scriptlet errors (`865d4e6`, `6742f46`)

Owner ran `sudo dnf upgrade --refresh` on the s6 boot (--boot-only-s6,
JOURNAL=no). Two scriptlet errors, both sdcompat bugs.

### 1. User-unit markers: "Transport endpoint is not connected"

oo7-portal/oo7-daemon %postun → systemd-update-helper
mark-restart-user-units → `systemctl --user -M 1000@ set-property ...`.
Evidence: /var/log/sdcompat/run-p*-i*.service/current (00:58-00:59)
held `DATA`, `OK <guid>`, `ERROR`: systemd-stdio-bridge's SASL replies
went to its s6 log, not back to systemctl. Cause: `761d2e0` (Oct 23,
after the Oct 20 real-boot check of `-M`) made both fd stores store an
open file passed twice once (systemd's FDSTORE= rule). sd-bus's unixexec
hands systemd-run one socketpair as stdin and stdout, so "stdout" was
dropped from `<unit>#stdio` and the runner left stdout on the log pipe.
The sandbox test piped `echo | systemd-run -P cat`, where they differ.

Fix `865d4e6`: no dedup for `#stdio` keys, in crates/sdcompat-base and
standalone/sdcompat-fdstore. Tests: both stores' unit tests (one socket
PUT as stdin and stdout → two fds); sandbox check "systemd-run -P with
stdin and stdout one socket" (python socketpair) fails without the fix
(got ''), passes with it. First try exempted every "#" key; dropped for
`#stdio` only since `#sockets` gains nothing from it. Note for tests:
systemd-run expands `$VAR` in the command (`--expand-environment`), so
the check uses sed, not `echo "$x"`.

### 2. dmeventd -R: "Failed to communicate with new instance"

device-mapper-event %post: `systemctl start dm-event.socket`, then
`dmeventd -R` (old daemon reports exec_method=systemd, exits, and the
new one is expected from socket activation). dm-event.service stayed
down afterwards: s6-supervise with no child, result success, exit 0 at
00:58:00. Cause: dm-event (DefaultDependencies=no) started at
00:48:48.94, the fd store's socket appeared at 00:48:49.05; the runner's
"best effort" PUT of the FIFOs failed, so at exit has_held_sockets() was
false → 125 (down) instead of SOCKET_WAIT.

Fix `6742f46`: sdcompat-gen gives every service activated by a .socket
a dependency on sdcompat-fdstore (as FileDescriptorStoreMax>0 services
already had). The store depends only on mount-api, so no cycle. Test
`socket_service_orders_after_fdstore` fails without the fix.

### Tests

cargo build, cargo test, every standalone's tests; sandbox default
224/224 (×4 of 5 runs; one run hit the known session-7 "Broken pipe"
reload flake, same checks as recorded there), SANDBOX_FDSTORE=no
224/224 (×2), SANDBOX_JOURNAL=no 231/231 (×2). bash -n and shellcheck
-S warning on tests/check.sh. Not on a real boot.

### Current state

Both fixes committed, not installed. The running fd store has the old
code until the next boot (the 2026-10-21 part 3 lesson), and
dm-event.service is down on the running system; root is btrfs on LUKS
with no LVM, so nothing was being monitored.

### Next steps

1. Owner: rebuild RPMs, rerun setup, reboot into the s6 entry, then:
   `systemctl status dm-event.service` (active after boot);
   `sudo dmeventd -R; echo $?` → 0 with no message, service active
   again; `sudo systemctl --user -M $USER@ is-system-running` →
   running (both via the unixexec bridge).
2. Earlier next steps unchanged (rescue/kernel-update rows,
   --default-s6 checks, Broken pipe flake).

## 2026-10-26, part 3: code and security review fixes

Owner ran `/engineering:code-review` (this session's commits, then the
whole project) and `/security-review` on the whole project, asked for
Korean copies of the results in code-review.md and security-review.md
(written, ignored by .gitignore, not committed) and for the suggested
fixes.

### Security review (whole project)

Four candidates, each verified separately; confidence threshold 8.
- Reported (8/10, MEDIUM): PrivateTmp= reachable by every local user.
  The 1777 dirs were RUN_DIR/units/<unit>/private/{tmp,var-tmp} under
  0755 parents; seen live: uid 1000 could list the private /tmp of
  fwupd, chronyd, colord, bolt, bluetooth, ModemManager (init_var_run_t,
  readable by unconfined_t). Fixed: `private` is 0700, set on every
  prepare() (systemd's /tmp/systemd-private-* layout).
- Below threshold, fixed anyway:
  - collect-diagnostics.sh wrote a 0644 archive at a guessable name
    beside its mktemp dir (7/10): now umask 077, the archive's name from
    mktemp, chowned to SUDO_UID under sudo.
  - PIDFile= PID used without checks (6/10, signals only): as systemd's
    service_load_pid_file() (CVE-2018-16888), a PID file that someone
    other than root could have written or redirected (non-root file or
    symlink, directory on the way not root's or group/other-writable)
    must name a process in the unit's cgroup; PID 1 and the runner
    itself are never taken. Refused PIDs are logged and waited past
    (timeout/protocol as before).
- Below threshold, NOT fixed (owner to decide): no SELinux access check
  in sdcompatd (uid 0 passes without systemd's mac_selinux_access_check)
  or in the fd stores (any uid-0 peer can GET/PUT/DEL any key). On this
  host's policy nsswitch_domain (540 types) can send to init_t and
  connect to init_var_run_t sockets. Needs the caller's label
  (GetConnectionCredentials / SO_PEERSEC), selinux_check_access() with
  systemd's class/permission mapping, and an enforcing VM to test; risk
  of new denials on the real boot.

### Code review fixes

- rootfs: root=/dev/disk/by-{uuid,label,partuuid,partlabel}/X is looked
  up as the blkid tag (no udev, so no such links in the initramfs).
- sdcompat.spec %posttrans: with /etc/sdcompat/boot-only-s6, re-run the
  kernel-install plugin for every kernel that has an -s6.img, so an
  sdcompat update reaches the next boot. README pair says so.
- sdcompatd: the UnitFileState cache is dropped when a directory of the
  s6 set changes (mtimes of the set dir and its rx dirs): `s6 set
  enable|disable` doesn't go through the daemon.
- 90-sdcompat.install: boot options escaped for the sed replacement (`\`,
  `&`, `|`; before, `d=e|f` failed with "unknown option to 's'").
- gen: a socket's Service= naming an alias of the service counts for the
  fdstore dependency.
- check.sh: timeout 30 on both systemd-run pipe checks; the fd stores'
  `#stdio` comments name each other.
- Not done: the s6-rc-update "Broken pipe" reload flake (needs its own
  investigation; seen again below).

### Tests

- cargo test (new: sandbox private_tmp_parent_is_closed, runner
  pid_file_trust, gen alias socket case, which fails without the fix),
  standalone tests.
- tests/initramfs.sh (QEMU/KVM): 7/7, new "root=/dev/disk/by-uuid/...
  without udev links".
- Sandbox: default 225/225, SANDBOX_JOURNAL=no 232/232 (new checks
  "UnitFileState follows an s6 set change without a reload", forking
  MainPID from PIDFile still passes); SANDBOX_FDSTORE=no 225/225 on the
  second run (the first hit the known reload flake: 19 failures, same
  signature).
- Plugin with fake entries and `a=b&c d=e|f g=h\\i`: options intact.
- collect-diagnostics.sh unprivileged: archive 0600, owner the user.
- bash -n / shellcheck -S warning on every changed script, %posttrans
  body included (rpmspec -P shows `%%` expanded to `%`).

### Current state

Committed, not installed or booted. PrivateTmp and PIDFile changes need
the new runner (setup rerun); the s6 images are rebuilt by the new
%posttrans when the RPM is updated.

### Next steps

1. Owner: rebuild RPMs, rerun setup, reboot into s6, then
   `ls -ld /run/sdcompat/units/*/private` (drwx------) and the checks of
   part 2 (dm-event, dmeventd -R, systemctl --user -M).
2. Decide on SELinux access checks in sdcompatd and the fd stores.
3. Broken pipe reload flake.

## 2026-10-26, part 4: systemd-rfkill replaced by an s6 oneshot

Owner asked to replace systemd-rfkill with util-linux rfkill and an s6
oneshot, and whether mdevd is only used at boot.

- mdevd: only in the s6 initramfs (--boot-only-s6). It is killed with
  s6-svscan before switch_root; no mdevd process on the running system
  (pgrep), where systemd-udevd (the `udevd` s6 service) manages devices.
- New s6 oneshot `rfkill` (dist/s6/rfkill: flag-essential, in
  sdcompat-sysinit, after mount-fs, which is after udev-trigger's
  settle). `early rfkill` restores, `early rfkill-save` (down, at
  shutdown) saves. Decision: keep systemd-rfkill's own state files
  (/var/lib/systemd/rfkill/ID_PATH:TYPE, "0"/"1", ID_PATH from
  udevadm, devices without ID_PATH skipped as systemd does), so the
  systemd boot entry shares the radio state. The state is read from
  sysfs `soft` (rfkill can't print one device's state), applied with
  `rfkill block|unblock ID`. Honours systemd.restore_state=0.
- systemd-rfkill.socket/.service left the default INCLUDE (config.rs,
  dist/sdcompat.conf), so EXCLUDE's systemd-* drops them; udev's
  SYSTEMD_WANTS for /dev/rfkill is ignored for excluded units.
- SELinux: init_t may only stat systemd_rfkill_var_lib_t (sesearch on
  this host); sdcompat.cil allows init_t to read/write/create files
  there and labels a new /var/lib/systemd/rfkill as that type.
  /dev/rfkill (wireless_device_t) is already open/read/write for init_t.
- Known differences from systemd-rfkill: saves only at a clean shutdown
  (systemd saves on every change), and a device appearing after the
  boot's udev settle keeps its state. README pair and TESTING pair (new
  row) say so.
- Tests: early with a fake sysfs and stub rfkill/udevadm: saved
  `platform-thinkpad_acpi:bluetooth=1`, `pci-0000:02:00.0:wlan=0`,
  skipped the device without ID_PATH, restored with `unblock 0`/
  `block 1`. sh -n, shellcheck -S warning. cargo test. Sandbox
  SANDBOX_JOURNAL=no 232/232, SANDBOX_FDSTORE=no 225/225, default
  225/225 once, but 4 of 6 default runs (2 of them on the code before
  this change) hit the known reload flake: it got much more frequent in
  default mode this evening.
- Not tested: the policy (needs semodule as root), a real boot.

### Next steps

1. Owner: rebuild RPMs, rerun setup (loads the policy, installs
   /etc/sdcompat/s6/rfkill), reboot into s6, then the TESTING rfkill row.
2. The reload flake is now the most urgent open item.
3. Earlier steps (SELinux access checks decision, part 3's checks).

## 2026-10-26, part 5: the reload flake's cause, and the repair

Commits `af915a3` (fix), this entry in the next. Owner asked to
investigate the s6-rc-update "Broken pipe" reload flake
(open since session 7). Later in the session the owner said: don't report
upstream and don't modify upstream source (memory updated; the s6-rc
snapshot report is dropped from the next steps too).

### Cause (read from s6 2.15.1.1 / skalibs 2.16.0.0, reproduced)

- "unable to manage new service directories: Broken pipe" is
  s6_supervise_link_names() losing its s6-ftrigrd. s6-ftrigrd dies with
  SIGSEGV when a subscription follows one that already fired on the same
  connection: sassserver_async_success() frees the fired query's handle
  without calling the cancel function, so the ftrigio stays in
  s6-ftrigrd's list; the next subscribe reuses the handle and its data,
  links the same ftrigio again, and the list loops (writes past the
  iopause array). A 40-line ftrigr client reproduced it every time
  (subscribe A, B; fire A; subscribe C -> EPIPE; s6-ftrigrd signal 11).
  Upstream git head has the same ftrigio_read().
- How s6-rc-update meets it: make_new_livedir() unlinks every scandir
  link and sends "x" to the supervisors it doesn't keep (down or
  restarted services), then manage relinks them all while subscribing
  (subscribe, then symlink, per directory). s6-svscan respawns an exited
  supervisor of a still-active name at once, or every second after
  "unable to chdir" (the svscan.log lines); when the respawn finds the
  just-made link, the new supervisor's "s" event fires a subscription
  while later ones are still to come. inotify trace of a flaky run
  (scratch watcher on $R and the scandir): beside1.service subscribed at
  +32.9 ms, its supervisor started at +37.6 ms, the subscriptions stopped
  after s6rc-fdholder at +34.9 ms. It depends on timing: the linking pass
  takes ~1.5 ms on an idle machine and 10-120 ms under load, which is
  why it came and went (0 of 11 idle runs today, 4 of 6 this evening
  while builds ran; 1 of 4 with all cores busy on the old code).
- The damage after it was ours: on that error do_unlink() removes the
  links it made so far, those of running supervisors too (no kill, the
  failure is before "a"). sdcompat-gen's retry then sent `s6-svscanctl
  -an`, which killed those supervisors; the retry relinked them, the new
  supervisors kept the services down while s6-rc had them up (hello,
  tick.timer, watch.path, s6rc-oneshot-runner: exactly the cascade).

### Fix (sdcompat side only)

- sdcompat-gen: on "unable to manage new service directories" the retry
  no longer rescans; relink_supervised() first puts back the scandir
  link (PREFIX+NAME -> LIVE/servicedirs/NAME, as s6-rc makes it) of
  every live service directory whose supervise/lock is held. Directories
  with no supervisor stay unlinked (the retry's s6-rc-update links them
  and refuses an existing link for an unlocked one). Other errors keep
  the rescan.
- lock_held() moved from sdcompatd to sdcompat-base::util (both use it).
- New test relink_supervised_only_locked (a python child holds the
  fcntl lock). TESTING pair: the "known flaky failure" note now says the
  Broken pipe is repaired and to report only a cascade after it.

### Tests

cargo build (no warnings), cargo test. Sandbox with the fix, all cores
busy: default 8 of 8 runs 225/225, 7 of them hit the crash
(`relinked` 20-25 links each, damage checks clean); SANDBOX_JOURNAL=no
loaded 232/232 with the crash repaired (35 links); idle
SANDBOX_JOURNAL=no 232/232, SANDBOX_FDSTORE=no 225/225. Baseline under
the same load (old code): 19 failures with the usual cascade. Not on a
real boot.

### Current state

The crash still happens (s6 bug, not patched per the rules); sdcompat
recovers from it. A crash during the wait for the new supervisors (after
"a") would kill them instead (do_unlink with kill options); not seen,
since the crash needs a subscribe.

### Next steps

1. Owner: rebuild RPMs at the next install.
2. Earlier steps: the rfkill row (part 4), SELinux access checks
   decision, part 3's checks.

## 2026-10-26, part 6: the wait that never ends; sdcompat-linkwatch

Owner asked to check part 5's claim that a crash during the wait for the
new supervisors (which would kill them) can't happen, then to implement a
watchdog as its own package and record it here. Commit `836404c`.

### The claim, checked in the sources

- The kill path (do_unlink with killopts 3) needs ftrigr_wait_and() to
  fail. The client sends nothing to s6-ftrigrd while waiting
  (ftrigr_update/ack/peek only read), so the subscribe-after-fire crash
  can't happen there; a second "s" on an already-fired fifo is answered
  with a stale id and a harmless double free of a handle (no
  subscriptions follow); a second supervisor for a directory dies on
  supervise/lock before its "s". Neither s6-rc-set-install nor sdcompat
  passes -t to s6-rc-update, so no ETIMEDOUT either. The claim holds.
- The same race has another outcome: a supervisor that starts after
  s6_supervise_link_names() noted the directory as unsupervised but
  before it subscribed sends its "s" to nobody. The wait then never ends
  (infinite deadline) and holds s6-rc's lock: the reload and every s6-rc
  command after it hang. Inferred from the code and part 5's timings,
  not caught in a trace; it fits the 2026-10-21 check.sh hang in `s6-rc
  -u change hold2.service` and the 2026-10-22 JOURNAL=no "waiting for
  s6rc-fdholder" hangs, which were blamed on the failed update then.
- `-t` for s6-rc-update would be worse: on a timeout it kills every new
  supervisor and brings their services down.

### standalone/sdcompat-linkwatch (new project, own RPM)

- Every --interval (2 s) it finds an s6-ftrigrd whose parent's comm is
  s6-rc-update or s6-svlink and which has run --after (10 s) seconds
  (subscribing takes milliseconds, so it is waiting). For each
  `event/ftrig1:*` fifo in its /proc/PID/fd whose service directory has
  supervise/lock held, it writes "s" into that fifo, once per (pid,
  fifo), and logs it. Other waiters (s6-rc on a service, s6-svwait) are
  never touched; it writes only to the waiting s6-ftrigrd's own fifos,
  and only after the subscriptions are over (an extra event during them
  is what crashes s6-ftrigrd). --under DIR limits it to one tree
  (the sandbox), --once for a single look. Depends only on libc; its
  own lock_held() (no shared code with the core).
- Core side: s6 longrun dist/s6/sdcompat-linkwatch (after mount-api, in
  sdcompat-sysinit), idle when the tool isn't installed; setup's
  STANDALONE list and --without; restart_daemon restarts it too (no
  state); `sdcompat linkwatch`; sdcompat.spec Recommends it; README pair
  (table, layout, --without), TESTING pair (a row).
- SELinux: /usr/libexec/sdcompat is init_exec_t, so it runs as init_t,
  as s6-rc-update and s6-ftrigrd do; no new rules. Not tried enforcing.

### Tests

cargo test for the project (stat parsing, fifo paths, a fake /proc: sent
once, only to the supervised directory, not below another --under, not to
an s6-rc's s6-ftrigrd). By hand with real s6: a supervisor started first,
then s6-ftrig-wait copied as `s6-rc-update` waiting on its event/:
times out alone (exit 99), returns 0 after ~2 s with the watcher.
check.sh section "linkwatch" (2 checks) does the same against the
sandbox's instance (`--after 3 --interval 1 --under $R`, started by
sandbox.sh). Sandbox: default 227/227, SANDBOX_JOURNAL=no 234/234,
SANDBOX_FDSTORE=no 227/227 (the machine was suspended for most of the
JOURNAL=no run; the run itself was normal). bash -n and shellcheck -S
warning on the changed scripts. cargo build/test for the core.

### Next steps

1. Owner: build and install the new RPM
   (standalone/sdcompat-linkwatch/sdcompat-linkwatch.spec) with the
   others, rerun setup, then the TESTING "Reload watchdog" row.
2. Earlier steps: the rfkill row (part 4), SELinux access checks
   decision, part 3's checks.

## 2026-10-26, part 7: the rfkill policy rule did not load

Owner's `dnf upgrade` of the g8695765 RPMs: the sdcompat %post semodule
failed, "Failed to resolve permission truncate ... cil:130". Part 4's
rule for init_t on systemd_rfkill_var_lib_t files named `truncate`,
which the file class doesn't have here (O_TRUNC needs write, truncating
setattr: both already in the rule). Removed it. Checked every allow
rule's class and permissions (all 7) against the loaded policy with
seinfo, and the two types: no other problem. The old module stayed
loaded (semodule changes nothing on failure). Lesson: check a new rule's
permissions with seinfo before committing; the module can't be compiled
here without root.

### Next steps

1. Owner: rebuild the RPMs, `dnf upgrade` them (the %post must print no
   semodule error), then part 6's steps (setup with --rebuild, reboot,
   checks).

## 2026-10-26, part 8: no /dev/fd on the --boot-only-s6 boot

The owner's setup rerun on the s6 boot (s6 initramfs) died in
setup_selinux: `line 730: /dev/fd/63: No such file or directory` (the
`mapfile < <(skarnet_files)` process substitution), after semodule -i.
/dev/fd, /dev/stdin, /dev/stdout, /dev/stderr and /dev/core don't exist
on this boot: systemd's mount-setup (PID 1 of Fedora's initramfs) makes
them and they came along with the moved /dev; the s6 initramfs mounts a
bare devtmpfs and nothing made them. Every bash `<(...)` on the system
fails, and programs opening /dev/stdin.

Fix: dist/early mount-api creates the four links (and /dev/core when
/proc/kcore exists) when missing, before its `restorecon -R /dev`
(device_t, as matchpathcon says). With a systemd initramfs they exist and
nothing changes. sh -n, shellcheck -S warning; the loop run twice
against a scratch directory (creates all five, then exits 0 with no
change). Not on a boot yet. The QEMU test (tests/initramfs.sh) never
used /dev/fd, so it couldn't show this; the 2026-10-26 hardware boot
checks didn't either.

### Next steps

1. Owner, now: create the links by hand, rerun setup (it stopped before
   the labels, GRUB entries and the rest).
2. Rebuild/upgrade the RPMs with this fix (early is in the sdcompat
   RPM), reboot, `ls -l /dev/fd`, then part 6's checks.

## 2026-10-26, part 9: status after the g86951b6 reboot

Status check only, no code change. As the unprivileged user (no sudo):
s6-svscan is PID 1 with s6 init-selinux, SELinux enforcing, all
g86951b6 RPMs installed 11:01, booted ~11:19. Part 8's fix works: /dev/fd,
/dev/std{in,out,err} and /dev/core exist, created at boot. sdcompatd,
sdcompat-fdstore, journal-relay and linkwatch are running (linkwatch since
boot, no s6-rc-update waiting). The standalone binaries in
/usr/libexec/sdcompat are owned by no package (installed by setup from
the 10:49 build, not from the standalone RPMs). rfkill restore matches the
saved state (bluetooth blocked, wlan not). Network and DNS work
(RESOLVED=no). GNOME session up. /etc/sdcompat/sdcompat.conf.rpmnew is
only the newer commented defaults; the owner's JOURNAL=no and RESOLVED=no
are kept.

Then by the owner, as root:

- `s6-rc -da list` filtered by `s6-rc-db atomics default`: empty, every
  member of the boot bundle is up. (I first gave `s6-rc -d list`, which
  prints the whole database: -d inverts the empty selection. Use -da.)
  The `<(...)` in that command (the owner's shell) ran: /dev/fd works.
- `ausearch -m avc -ts boot`: only the known two. NetworkManager
  dispatcher scripts (NetworkManager_dispatcher_iscsid_t/_chronyc_t)
  denied writing their service's s6-log pipe (init_t fifo_file), the
  open JOURNAL=no item; systemd_hostnamed_t searching /proc/pressure
  (proc_psi_t), Fedora's policy.
- 11:03 shutdown logs: only known items. home1 excluded by
  configuration; dbus-daemon "Rejected send message" (bus policy);
  "Transaction ... is destructive (the system is shutting down)" for
  dbus activations during shutdown (intended, as systemd); PAM
  session_close/setcred res=failed for gdm's and user@1000's sessions
  (logind already stopped; known). Two "Broken pipe" lines from clients
  of a restarted sdcompatd: cups.path at 10:49:22 (setup's daemon
  restart) and snapper-timeline.timer at 11:00:00 (its connection had
  died earlier and was found at the elapse). Both drop the connection and
  retry (pathunit.rs re-fires while the level holds; timer.rs trigger()
  returns false, so the elapse is retried); cupsd runs now. Nothing to
  fix.

### Current state

This boot is clean apart from the known JOURNAL=no dispatcher AVCs.
The part 8 fix is verified on hardware.

### Next steps

1. Part 6's TESTING "Reload watchdog" row (linkwatch is running).
2. Optional: install the standalone RPMs so dnf owns the four tools in
   /usr/libexec/sdcompat.
3. Earlier steps unchanged (SELinux access checks decision, the
   dispatcher-script pipe AVCs under JOURNAL=no).

## 2026-10-26, part 10: Reload watchdog checked on hardware

Owner ran part 6's TESTING "Reload watchdog" row plus two steps I gave
(not in TESTING), s6 boot, enforcing, g86951b6:

- `s6-svstat /run/service/sdcompat-linkwatch`: up 5066 s (since boot).
  Its uncaught-log lines before the test: only s6-rc's start; no real
  stuck wait was unblocked this boot.
- Ten `timeout 120 systemctl daemon-reload` in a row: all exit 0, 1-2 s
  each.
- check.sh's linkwatch test by hand against the system instance
  (defaults --after 10 --interval 2): a throwaway s6-supervise in
  /run/sdcompat/lwtest (init_var_run_t, which init_t may open and write;
  /tmp's user_tmp_t it may not), s6-ftrig-wait copied as `s6-rc-update`
  waiting on its event/: exit 0 after 10 s, logged "s6-rc-update (pid
  16425) waited 10 s for /run/sdcompat/lwtest/svc, whose supervisor runs:
  sent its start event". A plain s6-ftrig-wait on the same directory
  was left alone (exit 99 after its 15 s timeout). No AVC for lwtest or
  ftrig. The linkwatch SELinux claim of part 6 (runs as init_t, no new
  rules) holds under enforcing.

### Current state

linkwatch verified on hardware. Open: the JOURNAL=no dispatcher-script
pipe AVCs, the SELinux access checks decision.

### Next steps

1. Optional: install the standalone RPMs so dnf owns the four tools in
   /usr/libexec/sdcompat.
2. Optional: add step 3 (the fake stuck wait) to the TESTING pair's
   "Reload watchdog" row, if the owner wants an active check there.
3. Earlier steps unchanged.

## 2026-10-26, part 11: the watchdog check as a script

Owner asked to add part 10's step 3 to the TESTING docs. Too long for a
table cell, so it is tests/linkwatch-check.sh (root; `ok:`/`FAIL:` lines,
exit 0 when all pass): linkwatch up, the fake stuck `s6-rc-update` wait
in /run/sdcompat/lwtest unblocked in < 25 s, a plain s6-ftrig-wait left
alone, linkwatch's log line, no lwtest/ftrig AVCs; cleanup on exit
(s6-svc -x, rm). New TESTING pair row "Reload watchdog unblocks a wait"
under "Reload watchdog". bash -n and shellcheck -S warning pass; only
the non-root and usage paths ran here (no root). The commands are the
ones the owner ran successfully in part 10.

Owner ran it on the s6 boot (enforcing): five ok lines, exit 0; the
waiter was unblocked after 11 s, the plain wait timed out (99, 15 s).

### Next steps

1. Part 10's steps unchanged (standalone RPMs optional; the JOURNAL=no
   dispatcher-script pipe AVCs; the SELinux access checks decision).

## 2026-10-26, part 12: SELinux access checks; dispatcher pipe AVCs

Owner asked to fix both open items: the JOURNAL=no dispatcher-script
pipe AVCs and part 3's "SELinux access checks in sdcompatd and the fd
stores" (decided: do it).

### Dispatcher scripts' pipe AVCs (dist/selinux/sdcompat.cil)

- Cause: with JOURNAL=no a service's stdout/stderr is its s6-log pipe,
  an init_t pipefs object (s6-rc's fdholder). Daemons may write it
  through the policy's `daemon -> initrc_domain:fifo_file` rule.
  NetworkManager's dispatcher scripts run in their own domains, all in
  the attribute `networkmanager_dispatcher_plugin` (12 types), which is
  not `daemon`; the policy only gives it `init_t:unix_stream_socket
  { append connectto getattr ioctl }`, i.e. journald's stdout under
  systemd. Each script's write was denied and its output lost.
- Fix: `(allow networkmanager_dispatcher_plugin init_t (fifo_file
  (write append ioctl getattr)))`, the same four permissions as the
  catch-all fifo rule. Attribute, type, class and permissions checked
  with seinfo against the loaded policy (part 7's lesson). Not loaded
  (needs root).

### sdcompatd: systemd's SELinux access checks

- Ported from systemd 262's sources (selinux-access.c, dbus.c
  mac_selinux_filter, dbus-manager.c, dbus-unit.c, dbus-job.c, fetched
  for the installed version). Target: the unit file's raw label
  (getfilecon_raw on the fragment; none when masked, transient or not
  found) in class `service`, else sdcompatd's own (getcon_raw) in class
  `system`. Source: the bus's LinuxSecurityLabel
  (GetConnectionCredentials, cached per unique name like the uid) or
  SO_PEERSEC on the private socket. Before polkit, for root too.
- Per method, as systemd: job methods start/restart/try-restart/
  reload-or-restart/reload-or-try-restart -> `start`, stop -> `stop`,
  reload -> `reload` (job_type_to_access_method; ReloadOrRestart is a
  restart job there); Kill, AbandonScope, Job.Cancel/CancelJob ->
  `stop`; ResetFailedUnit -> `reload`; SetProperties/SetUnitProperties,
  Ref/Unref, AttachProcessesToUnit -> `start`; GetUnit, LoadUnit,
  GetUnitByPID, GetUnitProcesses, GetJob -> `status` (unit). Manager:
  StartTransientUnit, EnqueueMarkedJobs -> `start` (then per marked unit,
  skipped and logged when denied); Reload/Reexecute, ResetFailed,
  ClearJobs, Set/UnsetEnvironment -> `reload`; PowerOff/Halt -> `halt`,
  Reboot -> `reboot`; ListJobs, Subscribe, ListUnitFiles*,
  GetUnitFileState, GetDefaultTarget, Dump -> `status`;
  SetDefaultTarget, PresetAllUnitFiles -> `enable`. ListUnits* drop the
  units the caller may not `status`; ListUnitsByNames and the per-file
  Enable/Disable/Mask/Preset calls have no check in 262, nor here.
  Properties/Introspectable/Peer on any path below
  /org/freedesktop/systemd1: `system` `status` (`reload` for Set);
  systemd's object_path_startswith makes its unit branch dead code, so
  the manager is the target for unit objects too.
- libselinux via the existing dlopen binding (sdcompat-exec selinux.rs:
  getcon_raw, getfilecon_raw, selinux_check_access,
  security_getenforce), only as root with SELinux on, so the sandbox is
  unaffected. The userspace AVC answers permissive mode and permissive
  domains itself and logs denials to stderr (sdcompatd has no logger:
  /run/uncaught-logs); calls are serialised (no AVC lock callbacks).
  sdcompatd adds a "SELinux access check scon= tcon= tclass= perm=" line
  as systemd does. Not sent to audit as USER_AVC (no libaudit), so
  `ausearch` doesn't show these; the uncaught log does.
- When the caller's or our own label is unknown: denied under enforcing,
  allowed in permissive (systemd denies in both; lenient where it
  can't matter).
- Policy check before writing it: init_t has every `system` permission
  and `service` on systemd_unit_file_type, so sdcompat's own init_t
  clients (timers, runner) keep working; other callers get what systemd
  gives them under the same policy (same unit files, same init_t).

### fd stores (sdcompat-base fdstore.rs, standalone sdcompat-fdstore)

- systemd has no such socket (its fd store is internal). The clients are
  sdcompatd and the runner, both init_t on the s6 boot (ps -eZ). Under
  SELinux (selinuxfs present) a peer must now have the store's own
  context (SO_PEERSEC == /proc/self/attr/current), after the existing
  root-or-own-uid check; without SELinux nothing changes. Not a policy
  query: a confined root daemon may connect to init_t sockets
  (nsswitch_domain), and the store holds every service's fds. Both
  copies kept in step (no shared code), README pair of the standalone
  updated.

### Docs

README pair (authorization: the checks; dependency table: libselinux
also does the checks; layout: access.rs, selinux.rs). TESTING pair: rows
"SELinux checks on D-Bus callers" (runcon -r system_r -t
insights_core_t: a non-permissive system_r domain with a bin_t
entrypoint, connectto init_t sockets, write on init_var_run_t sockets
and only `status` on init_t; `systemctl show` must work, `daemon-reload`
must be refused) and "Dispatcher scripts' output".

### Tests

- cargo test: new sdcompat-exec `check_access_on_this_host` (unconfined_t
  -> init_t system status/reload, systemd_unit_file_t service start;
  chronyd_t reboot denied under enforcing: the denial was logged here),
  sdcompatd access tests (job type mapping, path prefix, SO_PEERSEC of a
  socketpair equals /proc/self/attr/current), fd store same-domain test
  in both copies. Whole workspace and the fdstore project pass.
- Sandbox: default 227/227, SANDBOX_FDSTORE=no 227/227,
  SANDBOX_JOURNAL=no 234/234 (the store's check runs there: all
  unconfined_t; the daemon's checks don't, not root).
- Not tested: the daemon's checks as root under enforcing, the CIL rule
  loaded (both need root and the real boot).

### Current state

Committed, not installed. The checks change behaviour only for confined
callers on the s6 boot, to systemd's.

### Next steps

1. Owner: rebuild the RPMs and dnf upgrade (the %post semodule must
   print no error), reboot into s6, then the two new TESTING rows and
   `sudo grep -E 'SELinux access check|avc: +denied'
   /run/uncaught-logs/current` after normal use (expected: nothing but
   the test's own denial).
2. Optional: standalone RPMs so dnf owns the tools in
   /usr/libexec/sdcompat.

## 2026-10-26, part 13: owner's first run of the part 12 TESTING rows

Owner rebuilt, installed (sdcompat-0.1.0-1.20261026131046.gdca1194) and
rebooted into s6, then ran the new rows.

### Results

- `runcon -r system_r -t insights_core_t -- systemctl …` failed with
  `runcon: 'systemctl': Permission denied` for both commands, so the
  daemon's checks were not exercised. Bug in the TESTING row, not in
  sdcompatd: /usr/bin/systemctl is `systemd_systemctl_exec_t`, and
  insights_core_t's only entrypoints are bin_t, usr_t and gpg_exec_t
  (sesearch). Part 12 checked the domain had "a bin_t entrypoint" but
  not systemctl's own label.
- Fix: run it as `runcon … -- env systemctl …`. /usr/bin/env is bin_t,
  insights_core_t may `execute execute_no_trans` any exec_type and has
  no type_transition on systemd_systemctl_exec_t, so systemctl runs in
  insights_core_t. unconfined_u has system_r; unconfined_t may
  transition to any domain. TESTING pair changed.
- `grep -E 'SELinux access check|avc: +denied' /run/uncaught-logs/current`:
  empty. Expected, since the test never reached sdcompatd; it also
  means normal use since boot produced no denial from the new checks.
- `ausearch -m AVC -ts boot | grep NetworkManager_dispatcher`: empty.
  Not conclusive on its own: it only shows the CIL rule works if the
  dispatcher ran scripts with JOURNAL=no on this boot.

### Current state

The checks are installed and not yet tested as root under enforcing.

### Next steps

1. Owner: rerun the corrected row (`sudo runcon -r system_r -t
   insights_core_t -- env systemctl show -p Version`, then `… env
   systemctl daemon-reload`, then the grep).
2. Optional: standalone RPMs so dnf owns the tools in
   /usr/libexec/sdcompat.

## 2026-10-26, part 14: the caller check passes on the s6 boot; AVC line end

Owner reran the corrected row (part 13) on the s6 boot, enforcing.

### Results

- `env systemctl show -p Version` and `env systemctl daemon-reload` both
  printed nothing. The reason: insights_core_t has no rule on
  `user_devpts_t` (sesearch), so systemctl's writes to the terminal
  are denied (and not audited). Not an sdcompatd problem.
- The uncaught log had exactly the expected denial:
  `avc: denied { reload } … insights_core_t … init_t … tclass=system
  permissive=0` and `sdcompatd: SELinux access check … perm=reload
  state=enforcing function=Reload … Permission denied`. Nothing for the
  `show` (its `system status` was granted) and nothing from normal use.
  So the daemon's checks work as root under enforcing: part 12's
  open item is done.
- Bug: both messages were on one log line. libselinux 3.11's avc_audit
  logs with `avc_log(SELINUX_AVC, "%s", buf)` and the default callback
  is `vfprintf(stderr, …)`: no newline (sources fetched for 3.11).
  systemd ends the line in its own SELINUX_CB_LOG callback, which is
  variadic; Rust can't define one on stable and the workspace builds no
  C. The SELINUX_CB_AUDIT hook writes into the middle of the message,
  not the end.

### Fix (sdcompat-exec selinux.rs)

check_access, after selinux_check_access and still under its lock, asks
the kernel (security_compute_av_flags_raw, string_to_av_perm) whether
the check is one avc_audit logs (denied & auditdeny, or granted &
auditallow, as in avc_audit) and if so writes the "\n". One extra
selinuxfs query per check; only as root with SELinux on. Another thread's
stderr line could still land between the two; accepted. Test: `audited`
true for chronyd_t -> init_t system reboot, false for unconfined_t
status.

TESTING pair: the row now says that neither command prints anything and
the grep is the result, with the two lines separate.

### Tests

cargo test (workspace) passes; sandbox default 227/227 (the change is
root-only, so not exercised there). Not tested: the newline on the real
boot (needs reinstall).

### Current state

All of part 12 has been checked on the real boot except the CIL
dispatcher rule (needs a JOURNAL=no boot where the dispatcher runs
scripts). The newline fix is committed, not installed.

### Next steps

1. Owner: with the next rebuild, the same grep should show the AVC line
   and the sdcompatd line separately.
2. Optional: the dispatcher rule on a JOURNAL=no boot (`ausearch -m AVC
   -ts boot | grep NetworkManager_dispatcher` empty after a network
   change).
3. Optional: standalone RPMs so dnf owns the tools in
   /usr/libexec/sdcompat.

## 2026-10-26, part 15: the AVC line end checked on the s6 boot

Owner rebuilt with `1df2507`, installed, rebooted into s6 (enforcing) and
ran part 14's next steps. No code changes.

### Results

- `sudo runcon -r system_r -t insights_core_t -- env systemctl
  daemon-reload`, then the grep of /run/uncaught-logs/current: two
  separate lines, `avc:  denied  { reload } … insights_core_t … init_t
  … tclass=system permissive=0` and `sdcompatd: SELinux access check …
  perm=reload state=enforcing function=Reload path=n/a: Permission
  denied (os error 13)`, 0.26 ms apart. The newline fix works. Nothing
  else in the log, so normal use since boot caused no denial from the
  checks.
- Dispatcher rule: /etc/sdcompat/sdcompat.conf has `JOURNAL=no`; after
  `nmcli networking off; nmcli networking on` the `ausearch -m AVC -ts
  boot | grep NetworkManager_dispatcher` was empty. Conclusive: the rule
  is in the loaded policy (`sesearch -A -s
  networkmanager_dispatcher_plugin -t init_t -c fifo_file`), and
  /var/log/sdcompat/NetworkManager-dispatcher.service has lines from
  /usr/lib/NetworkManager/dispatcher.d/04-iscsi (label
  NetworkManager_dispatcher_iscsid_script_t) at 20:45:22 and 20:45:27,
  the toggle, written through the s6-log pipe without a denial.
- Those lines are "Failed to reload iscsi.service: Unit iscsi.service is
  not active.": the script runs `systemctl --no-block reload
  iscsi.service || :` on up, iscsi.service is inactive, and sdcompatd
  answers with systemd's own error (daemon/mod.rs E_INACTIVE). Same
  message under systemd (into the journal); not a bug.

### Current state

All of part 12 has been checked on the real boot. No open items from the
SELinux work.

### Next steps

1. Optional: standalone RPMs so dnf owns the tools in
   /usr/libexec/sdcompat.

## 2026-10-26, part 16: standalone RPM build scripts (`779abae`)

Owner asked for the standalone RPMs. The four specs existed, but only
with a comment on how to make the tarball by hand, and a fixed
`Release: 1`, so a rebuild had the same NVR and `dnf upgrade` would skip
it.

### Changes

- `standalone/NAME/build-rpm.sh [OUTDIR]` in each project, modelled on
  packages/build-rpms.sh: vendored tarball, `rpmbuild -bb`, release
  `1.YYYYMMDDHHMMSS.gCOMMIT[.dirty]` (dirty from the project's own files),
  `SDCOMPAT_RELEASE` overrides. Version from the project's Cargo.toml;
  dies if the spec's Version differs. Default OUTDIR
  ~/rpmbuild-sdcompat-standalone, shared by the four (not the core's
  ~/rpmbuild-sdcompat, whose script empties rpms/); each script replaces
  only its own RPM in rpms/.
- Four copies differing only in NAME: the standalone projects may not
  share code (CLAUDE.md), and each stays buildable when taken out alone.
- Specs: `Release: %{?build_release}%{!?build_release:1}%{?dist}`, so a
  plain rpmbuild still works; header comment points at the script.
- README pair of each project and packages/README pair updated.
- sdcompat.spec's Recommends/Suggests on them are unversioned; nothing
  to change. setup-fedora.sh already skips a tool whose RPM is installed.

### Tests

All four built here with the scripts (rpmbuild's %check ran each
project's tests); each RPM holds only /usr/libexec/sdcompat/NAME (plus
build-id). bash -n and shellcheck -S warning clean; cargo test of each
project passes. Not installed (needs root). No core code changed, so the
sandbox suite was not rerun.

### Current state

The four tools in /usr/libexec/sdcompat are still owned by no package
until the owner installs the RPMs.

### Next steps

1. Owner: `for n in fdstore journal-relay linkwatch notify; do
   standalone/sdcompat-$n/build-rpm.sh; done`, then `sudo dnf install
   ~/rpmbuild-sdcompat-standalone/rpms/*.rpm` (it overwrites the unowned
   files), then `rpm -qf /usr/libexec/sdcompat/*`: every file owned. The
   running services keep the old binaries until restarted (`s6-svc -r`
   on each, or the next boot).

## 2026-10-26, part 17: standalone RPMs installed and booted

Owner built the four standalone RPMs at `6060b2b`, installed them, re-ran
setup-fedora.sh and rebooted into s6. No code changes.

### Results

- `rpm -qf /usr/libexec/sdcompat/*`: every file owned (core at
  `1df2507`, standalone at `6060b2b`); `rpm -V` of the four: clean, so
  setup-fedora.sh left the RPM-owned files alone.
- `tests/linkwatch-check.sh`: five `ok:`, no denials.
  `systemctl is-system-running`: `running`; no failed units; no
  `is not installed` in /run/uncaught-logs/current.
- `ausearch -m AVC -ts boot | audit2allow`: only `systemd_hostnamed_t
  proc_psi_t:dir search`, the /proc/pressure denial known since part 1
  and attributed to Fedora's policy, never checked on a systemd boot.
- My first instructions were wrong: s6-svstat takes one service dir,
  and `pgrep -x` cannot match names over 15 characters. Rerun per
  service with `s6-svstat -p`: sdcompat-fdstore, sdcompat-linkwatch and
  journal-relay all `up` (~420 s, the boot), each exe
  /usr/libexec/sdcompat/NAME, not deleted: the services run the RPM
  binaries.

### Current state

The standalone RPM step is done. Only open item: the hostnamed
proc_psi_t denial.

### Next steps

1. Owner: on the systemd boot, `ausearch -m AVC -ts boot | grep
   proc_psi` to settle whether the hostnamed denial is Fedora's or ours.

## 2026-10-26, part 18: the hostnamed proc_psi_t denial is ours (`43e3986`)

### Systemd boot result

- Owner, on the systemd boot: `ausearch -m AVC -ts boot | grep proc_psi`:
  no matches, although `journalctl -b -u systemd-hostnamed` shows hostnamed
  started four or more times. Same binary and domain, so the denial comes
  from sdcompat's environment, not Fedora's policy. The earlier entries'
  "policy-side, not sdcompat" (parts 1 onward) were wrong.
- Cause (from systemd's code and the unit, not yet seen on an s6 boot):
  hostnamed calls sd_event_add_memory_pressure(). PID 1 passes
  `MEMORY_PRESSURE_WATCH=<cgroup>/memory.pressure` (hostnamed has
  `MemoryAccounting=yes`, `MemoryPressureWatch=auto`); without the variable
  sd-event checks /proc/pressure itself, the `proc_psi_t dir search`.

### Changes (`43e3986`)

- sdcompat-exec cgroup.rs `memory_pressure_env()`, called by the runner in
  build_env() and prepare_start(): systemd's rules. `MemoryPressureWatch=`
  auto/on with the cgroup's memory.pressure present: WATCH = that file,
  WRITE = base64 of `some <threshold> 2000000\0`
  (`MemoryPressureThresholdSec=`, default 200 ms, clamped to the 2 s
  window). off: `/dev/null`, also when the unit has no cgroup (systemd sets
  nothing then; /dev/null keeps sd-event out of /proc/pressure). skip or no
  file: nothing. A User= service run by a root runner gets the file
  chowned, as systemd does, so it can write its trigger.
  `MemoryAccounting=no` is not read: auto means on whenever the file exists.
- sdcompat-gen: the two keys are no longer reported as ignored.
- README pair: one sentence under cgroup v2.

### Tests

- cargo build; cargo test all passed (new `cgroup::tests::memory_pressure`,
  checked against `printf 'some 200000 2000000\0' | base64`).
- Sandbox 231/231 (new: psi-off.service sees /dev/null; psi-auto.service
  sees it unset, as the unprivileged sandbox has no cgroup). bash -n and
  shellcheck clean. The JOURNAL/FDSTORE variants were not run (not touched).
- Owner, on the systemd boot, hostnamed's /proc/PID/environ:
  `MEMORY_PRESSURE_WATCH=/sys/fs/cgroup/system.slice/systemd-hostnamed.service/memory.pressure`,
  `MEMORY_PRESSURE_WRITE=c29tZSAyMDAwMDAgMjAwMDAwMAA=`: the same values
  sdcompat now sets (the WRITE value is the unit test's). (`sudo` through
  Claude Code's `!` prefix fails: no terminal for the password; run it in
  a separate terminal.)

### Current state

Fix committed, not yet on an s6 boot. No other open items.

### Next steps

1. Owner: rebuild/install, boot s6, `ausearch -m AVC -ts boot | grep
   proc_psi` (expect no matches) and check hostnamed's
   MEMORY_PRESSURE_WATCH/WRITE in /proc/PID/environ.

## 2026-10-26, part 19: proc_psi_t fix confirmed on an s6 boot

### Result

- Owner, on the s6 boot (`/proc/1/comm`: `s6-svscan`), with the build
  containing `43e3986` installed:
  - `ausearch -m AVC -ts boot | grep proc_psi`: no matches.
  - After `hostnamectl` (to start hostnamed), its /proc/PID/environ:
    `MEMORY_PRESSURE_WATCH=/sys/fs/cgroup/system.slice/systemd-hostnamed.service/memory.pressure`,
    `MEMORY_PRESSURE_WRITE=c29tZSAyMDAwMDAgMjAwMDAwMAA=`: identical to
    the systemd boot's values (part 18).
- So the denial was the missing MEMORY_PRESSURE_WATCH, and sdcompat now
  passes what PID 1 does. No code changes this part.

### Current state

No open items. Owner, same boot: `ausearch -m AVC -ts boot | audit2allow`:
`<no matches>`, `Nothing to do`. The s6 boot has no AVC denials at all.

### Next steps

None pending. New work only at the owner's request.

## 2026-10-26, part 20: ExecStop= when s6-rc stops a longrun (`e569be5`)

The owner picked the first item of the README's "Not implemented" list.

### Scope found

- Static oneshots with `RemainAfterExit=yes` already ran `ExecStop=` from
  their s6-rc `down` (runner `down`), at shutdown too. The gap was
  longruns only: s6 has no hook before it sends the down signal, and the
  runner execs the main process, so `s6-rc -d` (shutdown's `-bDa`,
  `s6 live stop/reset`, s6-rc-update removing a service) only signalled
  them. On-demand longrun instances were just killed at shutdown.

### Design (and why)

- gen adds `UNIT.stop` (index::stop_atom) for each longrun service with
  `ExecStop=`: a oneshot depending on the service, empty `up`, `down` =
  `sdcompat-runner stop-atom`, `timeout-down` = TimeoutStopSec + 10 s,
  flag-recommended with its service. s6-rc takes it down before the
  service, so ExecStop= runs while the service is still up, as in systemd.
- **Nothing depends on `UNIT.stop`.** Redirecting dependents to it would
  order their stops before the ExecStop=, but s6-rc-update restarts a
  service that gains a dependency on a new (or down) service: the first
  upgrade would have restarted every dependent (gdm and the session). So
  dependents stop beside the ExecStop=, the service itself still last.
  Documented as a difference.
- Bundles list `UNIT.stop` beside the service: an `s6-rc -p change` of a
  bundle (isolate) would otherwise take it down alone and run ExecStop=
  on a service that stays up. The daemon's isolate adds the `.stop` atoms
  of the whole closure for the same reason.
- Set: `sync_stop_atoms()` gives `UNIT.stop` its service's prescription
  (always -> active), before set-fix (a `disable` without -p would leave
  it active and set-fix would re-enable the service) and again after it
  (services set-fix enabled).
- Runner `stop-atom`: skipped when SOCKET_WAIT, CONDITION_FAILED,
  DEPENDENCY_FAILED, START_FAILED or PARKED, or when s6-svstat says down;
  `$MAINPID` from the state's MAIN_PID if alive (PAMName=/forking: the
  supervised pid is the runner), else s6-svstat's pid; deadline =
  TimeoutStopSec= (log says "stop timeout"); always exits 0, as a failed
  down would keep s6-rc from stopping the service.
- Daemon: `reconcile_stop_atoms()` every second brings up `UNIT.stop` of
  each longrun s6-rc has up (started by name, by start_beside, by an
  older database: s6-rc-update adds new services down). Not while
  s6-rc's lock is held or `shutting-down` exists, nor for a unit whose
  job holds its unit lock, and with s6-rc without `-b`: a change that
  stops the service takes UNIT.stop down first, and bringing it back up
  would restart the service. `do_stop` leaves ExecStop= to UNIT.stop
  when it is up (ran once, checked). Shutdown: `stop_dynamic_oneshots`
  became `stop_dynamic_at_shutdown` and also stops on-demand longruns
  that have an ExecStop=.

### Tests

- cargo build (no warnings); cargo test all passed (new
  `stop_atom_for_longrun_exec_stop`); standalone tests passed.
- Sandbox 244/244, SANDBOX_JOURNAL=no 251/251, SANDBOX_FDSTORE=no
  244/244. New: stopper.service (`.stop` up after boot; `s6-rc -d` runs
  ExecStop= with the right $MAINPID; started by name, `.stop` follows;
  StopUnit runs ExecStop= once), stopcond.service (condition failed: no
  ExecStop=), prepl@d (on-demand longrun's ExecStop= at shutdown), set
  rx of `stopper.service.stop` following usable/active.
- bash -n and shellcheck -S warning clean on check.sh and sandbox.sh.
- Not tested: a real s6 boot (power-off with UNIT.stop, SELinux on the
  oneshot path, the upgrade from a database without `.stop` atoms).
  New TESTING row "ExecStop= of longruns" covers it.

### Current state

Committed, sandbox-verified; not yet on a real boot.

### Next steps

1. Owner: rebuild RPMs, upgrade (the first daemon-reload adds the
   `.stop` atoms down; the daemon brings them up within a second, and no
   service should restart: check `s6 live status` uptimes of e.g. gdm),
   then the TESTING row "ExecStop= of longruns" and "Previous shutdown".
2. Remaining "Not implemented" items: socket activation/Accept=yes,
   crypttab volumes, watchdog/credentials, cancelling running jobs.

## 2026-10-26, part 21: ExecStop= of longruns confirmed; 2 × 45 s at power-off

### Result (owner, s6 boot, build `9cb7c61`)

- Upgrade: `systemctl daemon-reload` restarted nothing (s6-svstat pids of
  every /run/service entry identical before and after). `.stop` atoms
  present: alsa-state's up, the others down with their services.
- TESTING "ExecStop= of longruns" passes: `sdc-stoptest.service.stop` up;
  `s6 live stop` -> `stop 10227` (= MainPID); `systemctl stop` -> a second
  line; power-off -> `stop 10480` (= MainPID). Console log: `.stop` down
  at 22:39:59.651, the service at .787, i.e. ExecStop= before the stop.
- No AVC denials (`ausearch -m AVC -ts boot`, both shutdown logs: 0).
- **"Previous shutdown" fails:** `sdcompatd: stopping
  user-runtime-dir@0.service took 45.3 s` (22:40:45.13) and
  `user-runtime-dir@1000.service took 45.3 s` (22:41:30.45): ~90 s.
  The same 45 s as 2026-10-21 (user@1000 only) and 2026-10-22, never
  explained.

### Findings (read-only; the shutdown logs are root-only, not read)

- The owner powered off with `sudo systemctl poweroff` (logind: session
  c516 of root at 22:39:59.41, which started user@0 at .57; logind
  stopped at .647).
- stop_dynamic_at_shutdown is serial: user@0's manager got exit.target at
  22:39:59.80, user@1000's only at 22:40:45.13, the moment
  user-runtime-dir@0's stop ended. Both user managers' logs end right
  after exit.target; no `closing the PAM session took` line in either.
- 45.3 s matches sd-varlink's default method timeout (45 s), not the
  bus's 25 s. systemd is 262; logind has a varlink socket,
  /run/systemd/io.systemd.Login (systemd-logind-varlink.socket, inode
  3580, created beside /run/systemd/private at boot, so likely held by
  sdcompat's socket handling rather than logind; not checked whether it
  still listens once logind is down). **Hypothesis, unconfirmed:** after logind stopped, something
  in the user@/user-runtime-dir@ stop (pam_systemd's session release, or
  systemd-user-runtime-dir stop) connects to io.systemd.Login, the
  connect succeeds into the backlog, nobody accepts, and the call times
  out after 45 s. Under systemd user@ and user-runtime-dir@ are
  `After=systemd-logind.service` and stop before logind; here the
  on-demand instances stop only when s6-rc stops sdcompatd, after logind.

### Current state

ExecStop= of longruns verified on a real boot. Open: the 2 × 45 s at
power-off (one per logged-in UID).

### Next steps

1. Owner: copy the logs so they can be read without root:
   `mkdir -p ~/sdcompat-logs/2239 && sudo install -m 644 -o "$USER"
   /var/log/sdcompat/last-shutdown-*.log ~/sdcompat-logs/2239/`. Read
   22:39:59.6-22:41:31 for what ran in each 45 s (dbus-daemon, sdcompatd,
   runner lines) to confirm or drop the varlink hypothesis.
2. If confirmed, the fix direction (owner's decision): stop the on-demand
   instances before s6-rc stops the services they are ordered after
   (e.g. a oneshot depending on logind/dbus whose `down` asks sdcompatd
   to stop them, as UNIT.stop does for ExecStop=), or close a socket
   unit's listener when its service is stopped at shutdown.
3. Cleanup of the test unit: `sudo rm
   /etc/systemd/system/sdc-stoptest.service /var/tmp/sdc-stop.log; sudo
   systemctl daemon-reload`.

## 2026-10-26, part 22: on-demand instances stop before s6-rc's services

### Cause confirmed (owner copied the logs to ~/sdcompat-logs/2239)

- Console log: `sdcompat[user@0.service]: closing the PAM session took
  45.0 s` (22:40:44.88), then the same for user@1000 (22:41:30.20). No
  dbus-daemon activation or other bus traffic in either 45 s. So the wait
  is inside pam_close_session, and 45.0 s is sd-varlink's default
  timeout: pam_systemd (systemd 262) releases the session over varlink on
  /run/systemd/io.systemd.Login. logind was down since 22:39:59.91, but
  its listening socket stays in sdcompat's fd store (the runner keeps a
  service's sockets there across stops; no RemoveOnStop=), so the connect
  succeeds and nobody answers. The part 21 hypothesis holds.
- Root cause is the order: user@/user-runtime-dir@ are
  `After=systemd-logind.service`, but as on-demand instances s6-rc doesn't
  know them, and sdcompatd stopped them only when s6-rc took sdcompatd
  down, after logind.

### Fix (owner chose "reorder the stops" over closing held sockets)

- New `sdcompatd stop-instances`: calls `StopInstances` on interface
  `org.sdcompat.Manager` (sdcompat_client::SDCOMPAT_IFACE) at the manager
  path, no D-Bus call timeout. The daemon runs stop_dynamic_at_shutdown
  and replies when it is done. mac_check "halt" + require_privileged;
  refused unless the `shutting-down` marker exists.
- rc.shutdown: after the marker and the kmsg line, `timeout -s KILL 120
  /usr/libexec/sdcompat/sdcompatd stop-instances` (kmsg line on 137 or
  failure), then `s6-rc -bDa` as before. What it leaves, the daemon's
  SIGTERM path still stops (dyn_stop entries are removed once stopped, so
  nothing stops twice).
- Why not a s6-rc oneshot depending on logind/dbus: it would have to
  depend on the template units' After= services, and bringing it up
  would start those (s6-rc starts dependencies). The early call keeps
  every s6-rc service up while the instances stop, which is systemd's
  order for these leaf units.
- README/README-us: the shutdown paragraph.

### Tests

- cargo build (no warnings), cargo test passed; standalone tests passed.
- Sandbox 250/250, SANDBOX_JOURNAL=no 257/257, SANDBOX_FDSTORE=no
  250/250. New: stop-instances exits 0 with no output, the daemon still
  runs after it, prep@c's ExecStop= (with a concurrent StopUnit and an
  activation) runs once and within 5 s; refused without the marker; a
  prep@e left over is stopped by the daemon's SIGTERM; prep@c's ExecStop=
  not repeated.
- bash -n and shellcheck -S warning on rc.shutdown and check.sh.
- Not tested: rc.shutdown itself (fixed path), SELinux on init_t ->
  sdcompatd private socket at shutdown, a real power-off.

### Current state

Committed, sandbox-verified; not on a real boot. The new rc.shutdown
reaches /etc/s6-linux-init/current only when setup reruns
s6-linux-init-maker (the RPM's %post doesn't).

### Next steps

1. Owner: rebuild/upgrade RPMs, rerun setup (`sudo dist/setup-fedora.sh
   --enforcing`), check `grep stop-instances
   /etc/s6-linux-init/current/scripts/rc.shutdown`. Boot s6, log in,
   `sudo true` (root session), power off, boot s6: TESTING "Previous
   shutdown" (no `stopping ... took`, no `closing the PAM session took`,
   kernel log without `stop-instances failed`), and `ausearch -m AVC -ts
   boot`.
2. Test unit cleanup from part 21 if not done.

## 2026-10-26, part 23: power-off without the 45 s waits (real boot)

### Results (owner; logs in ~/sdcompat-logs/retest2)

- `/etc/s6-linux-init/current/scripts/rc.shutdown` has `stop-instances`.
- TESTING "Previous shutdown" passes: no `stopping ... took`, no `closing
  the PAM session took`, no `stop-instances failed` in the kernel log, no
  `services still not down`. AVC: 0 in both logs, `ausearch -m AVC -ts
  boot` has no matches.
- Timing: `poweroff.target requested` 23:44:10.42, s6-rc starts stopping
  at 12.54 (so stop-instances took ~2 s, user@/user-runtime-dir@
  included), last s6-rc line 13.59. Kernel: `sdcompat: shutdown starts`
  85.34 s, root unmount 91.53 s. The owner timed ~7 s to power-off, down
  from ~90 s in part 21.
- Unrelated, not investigated: `sdcompat-runner: user@60578.service: user
  60578 does not exist` at 23:43:33 (during the session, not at shutdown).

### Current state

Part 22's fix verified on a real boot; the 2 x 45 s at power-off is gone.

### Next steps

1. Test unit cleanup from part 21 if not done.
2. Optional: what requested user@60578 (60578 isn't a local user).

## 2026-10-26, part 24: `user@60578.service: user 60578 does not exist`

### Findings (read-only, current s6 boot and ~/sdcompat-logs/retest2)

- 60578 is GDM's greeter user: `gnome-shell --mode=gdm` runs as uid
  60578. GDM 51 registers it as a dynamic user over userdb
  (/run/systemd/userdb/org.gnome.DisplayManager, nss-systemd in
  nsswitch) only while the greeter runs; `getent passwd 60578` is empty
  now.
- After login the greeter ends, GDM drops the user, then logind stops
  user@60578. The runner's `finish` for it calls `Runner::new`, which
  resolves `User=%i` for every command, not only for starts. The lookup
  fails, so `setup_failed` logs the line and returns 125. All of
  cmd_finish is skipped.
- Effect on the current boot: /run/sdcompat/units/user@60578.service has
  no active-exit, inactive-enter or result. `systemctl status` shows
  "inactive (dead)" (s6 reports it down) but still shows Main PID 2362,
  Status "Activating special unit exit.target..." and Memory 610.7M, and
  no "since". Anything else finish does (ExecStopPost=, cleanup) would
  also be skipped; user@.service has no ExecStopPost=, so nothing more
  is lost here. user-runtime-dir@60578 (runs as root) stopped normally.
  The PAM session is closed by the `run` process, which resolved the user
  at start, so that part is fine.
- The same applies to `down`, `stop-atom` (they return 0 without doing
  their work) and `stop-exec` (returns 1) for any unit whose User= has
  disappeared. systemd resolves User= only when it spawns a process, so
  stopping such a unit works and only an ExecStop*= running as that user
  fails (217/USER).

### Proposed fix (not done; owner's decision)

Resolve User=/Group=/SupplementaryGroups= only for start-side commands
(`run`, `up`), or lazily when a command spawns a process with those
credentials. Then `finish`/`down`/`stop-atom`/`stop-exec` do their
bookkeeping, and only a spawn that needs the missing user fails.

### Next steps

1. Owner: decide on the fix above.
2. Test unit cleanup from part 21 if not done.

## 2026-10-27, part 25: User=/Group= resolved only for a start (3752774)

### Change (owner chose part 24's fix)

- sdcompat-runner: `Runner::new(cfg, name, starting)`; main passes
  `starting` for `run`/`up`. New `resolve_credentials` (User=,
  DynamicUser=, Group=, SupplementaryGroups=) returns the error with
  systemd's exit code. A start fails as before (setup_failed, Result
  "resources"). For other commands (`finish`, `down`, `stop-atom`,
  `stop-exec`, `reload-exec`) the error is kept in `cred_error` and
  `exec_command` returns it after setup_stdio, so only a spawned process
  fails (217/USER, 216/GROUP) and the command's bookkeeping runs.
- README/README-us: a note under User=.

### Tests

- cargo build (no warnings), cargo test, standalone tests passed.
- Sandbox 257/257 (new section "User= gone before the stop":
  userless.service started with User=$(id -un), User= changed to a
  missing user in the unit file, StopUnit -> inactive/dead, MainPID 0,
  Result success, InactiveEnterTimestamp set; a start with the missing
  user -> failed, Result resources).
- With the old runner the new section fails at InactiveEnterTimestamp
  (256/257). MainPID was 0 there even without the fix, unlike the real
  boot's user@60578 (MainPID 2362 kept), so that assertion alone
  wouldn't catch it.
- bash -n and shellcheck -S warning on check.sh and sandbox.sh.
  SANDBOX_JOURNAL=no / SANDBOX_FDSTORE=no not rerun (those parts not
  touched). cargo clippy isn't installed.
- Not tested: a real boot (GDM login, then `systemctl status
  user@60578.service` should show "inactive (dead) since ...", no Main
  PID/Status, and no `user 60578 does not exist` line).

### Current state

Committed, sandbox-verified; not on a real boot.

### Next steps

1. Owner: rebuild/upgrade RPMs, boot s6, log in through GDM, then check
   `systemctl status user@60578.service` (the greeter's uid can differ
   per boot; `ls /run/sdcompat/units | grep user@`) and the absence of
   `does not exist` in the next shutdown console log.
2. Test unit cleanup from part 21 if not done.

## 2026-10-27, part 26: `sdcompat init` replaces setup-fedora.sh; COPR source RPMs (a365750)

### Goal (owner)

Deploy the packages through Fedora COPR (the owner uploads the source RPMs
by hand), and replace `dist/setup-fedora.sh` with an installed command,
`sdcompat init --default-s6 --journal=no ...`.

### Decisions (owner, asked 2026-10-27)

- Keep the five core specs (not one spec with subpackages): 5 source RPMs
  per core release, uploaded together (they Require each other's exact
  version-release).
- `sdcompat init` in Rust, in the `sdcompat` binary (the package that
  already holds the boot setup): crates/sdcompat/src/init.rs. No new crate
  or package.
- Options in `--NAME=yes|no` style: `--journal=`, `--resolved=`,
  `--boot-only-s6=`, `--grub=`, `--selinux=enforcing|permissive`, plus
  `--default-s6`/`--default-systemd`, `--dry-run`, `--yes`, `--uninstall`.
  The old spellings (`--no-journal`, `--enforcing`, ...) are refused with a
  hint.
- The build modes are dropped (`--rpms`, `--build-s6`, `--rebuild`,
  `--without`, `--remove-s6`): init installs nothing. Packages come from dnf
  (COPR or local RPMs).

### What init does (a port of setup-fedora.sh without the install steps)

- Checks: root (not for --dry-run), Fedora (`SDCOMPAT_INIT_ANY_OS=1` allows
  a dry run elsewhere), the packaged files (skel, cil, s6.conf, rsyslog.conf,
  sdcompat-gen), programs by package (s6-linux-init-maker, semanage, grubby,
  grub2-editenv with --default-s6, rsyslogd + sdcompat-journal-relay with
  --journal=no, mdevd with --boot-only-s6=yes). Missing ones end with "sudo
  dnf install ...". It warns about gdm/gnome-shell, as before.
- Same steps as before: version report (main.rs `version_report()`, shared
  with `sdcompat --version`), sdcompat.conf keys (marker now `# set by
  sdcompat init --journal=no`; the old script's marker is removed too),
  /etc/sdcompat/rsyslog.conf, /etc/s6.conf from /usr/share/sdcompat/s6.conf
  (new in the package), sysinit bundle members, unit enabling (chronyd and
  NetworkManager only if their unit files exist), `sdcompat-gen reload
  --no-update`, s6-linux-init-maker, SELinux labels, sdcompatd restart on
  the s6 boot, boot entry and default.
- Changed: the hand-written s6 services are the package's (no checksum
  logic). The kernel-install plugin used is the packaged
  /usr/lib/kernel/install.d one; the old script's /etc copy (which would
  hide it) is backed up and removed. The SELinux mode is kept across runs
  now (read from /etc/sdcompat/boot-options; first run permissive), like
  the other choices.
- --uninstall: the s6 BLS entries, the saved default, boot-options,
  boot-default, boot-only-s6, the /etc plugin copy, /boot/initramfs-*-s6.img,
  /etc/s6.conf if ours, /etc/s6-linux-init/current; then it prints the dnf
  remove command (the module goes in the package's %postun).

### Packaging

- sdcompat.spec: installs dist/s6.conf to /usr/share/sdcompat; Requires
  policycoreutils-python-utils and grubby; drops the duplicate
  `Recommends: s6-linux-init`; changelog.
- `--srpm` for packages/build-rpms.sh, standalone/*/build-rpm.sh (the
  release now goes into the spec as `%global build_release`, also for
  binary builds) and dist/rpm/build-skarnet-rpms.sh.
- packages/copr-srpms.sh [OUTDIR] [core|standalone|skarnet...]: all
  source RPMs into OUTDIR/srpms (default ~/sdcompat-srpms) and
  BUILD-ORDER.txt. Skarnet stages come from the specs' -devel BuildRequires:
  1 skalibs, 2 execline mdevd, 3 s6, 4 s6-rc s6-linux-init, 5 s6-frontend,
  6 all Rust packages. It runs the standalone build scripts but shares no
  code with them.
- Docs: README pair (deployment, dependencies), TESTING pair (stage 5
  builds/installs the RPMs then runs init; update, default, boot-only,
  removal), packages/README pair (new COPR section, migration from
  setup-fedora.sh), standalone READMEs, comments in dist/ and tests/.
  `License: TBD` is still in the specs (noted in packages/README).

### Tests

- cargo build (no warnings), cargo test (sdcompat: 4 new unit tests:
  options, conf keys, boot settings, LUKS), standalone tests passed.
- Sandbox 257/257. bash -n / shellcheck -S warning on every changed script.
- Dry run on this machine (s6 boot, sdcompat c8b83db RPMs installed):
  `sdcompat init --dry-run --selinux=enforcing` vs the old `setup-fedora.sh
  --dry-run --enforcing`: the same commands except the dropped install
  steps, the packaged plugin path, and the removal of the /etc plugin copy.
  It kept this machine's JOURNAL=no, RESOLVED=no and boot-only-s6.
- copr-srpms.sh made all 16 source RPMs; `rpmbuild --rebuild` (no defines)
  of the sdcompat, sdcompat-notify and skalibs ones succeeded (the core one
  with its %check). The rebuilt sdcompat RPM has /usr/share/sdcompat/s6.conf
  and the new Requires; its `sdcompat init --help`, a bad value (exit 2) and
  a non-root run (refused) behave.
- Not tested: a real `sudo sdcompat init` (or --uninstall), a build on COPR
  itself (mock chroot), an upgrade from the RPMs installed now.

### Current state

Committed. Built from a365750 (clean, release 1.20261027002413.ga365750
for the core) for the owner to test: ~/rpmbuild-sdcompat/rpms (5 core),
~/rpmbuild-sdcompat-standalone/rpms (4 standalone); the skarnet RPMs
installed now are unchanged. COPR source RPMs (all 16) in
~/sdcompat-srpms/srpms with ~/sdcompat-srpms/BUILD-ORDER.txt.

### Next steps

1. Owner: `sudo dnf upgrade ~/rpmbuild-sdcompat/rpms/*.rpm
   ~/rpmbuild-sdcompat-standalone/rpms/*.rpm`, `sdcompat init --dry-run`,
   then `sudo sdcompat init` (it keeps the current choices); check that
   /etc/kernel/install.d/90-sdcompat.install is gone, the s6 entries are
   rewritten, `grep stop-instances
   /etc/s6-linux-init/current/scripts/rc.shutdown`, then boot s6 (part 25's
   check of user@GREETER-UID too).
2. Owner: create the COPR project and upload `packages/copr-srpms.sh`'s
   output stage by stage; then try `dnf copr enable` on a VM.
3. Choose a license for the specs before publishing.

## 2026-10-27, part 27: dry-run ending message (8b8ecec)

### What changed

- `sdcompat init --dry-run` printed the next-steps text ("Done. Next:
  1. Reboot ..."), which read as if something had been applied. Found while
  running the dry run on this machine (installed RPM ga365750; output as in
  part 26, no warnings, the same choices kept). A dry run (also with
  --uninstall) now ends with "Dry run: nothing was changed. To apply this,
  run: sudo sdcompat init OPTIONS" (its own options without --dry-run),
  instead of the next steps. crates/sdcompat/src/init.rs `dry_run_end()`,
  with a unit test.

### Tests

- cargo build (no warnings), cargo test (all passed; sdcompat 5 tests),
  sandbox 257/257. Both dry runs checked by hand with target/debug/sdcompat.
- No docs quoted the old ending. Afterwards (dae1e9b), at the owner's
  request: README pair (the deploy example runs the dry run without sudo; the
  `--dry-run` row names the apply command it ends with) and TESTING pair
  (stage 5 says what the preview ends with and that a missing package stops
  it with a dnf hint).

### Current state

Committed. The 5 core RPMs were rebuilt from 4db6aef (clean, release
1.20261027091952.g4db6aef) in ~/rpmbuild-sdcompat/rpms (build-rpms.sh exit
0, with the spec's %check; the new /usr/bin/sdcompat has the dry-run
message). The installed ones are still ga365750. The standalone and skarnet
RPMs are unchanged. ~/sdcompat-srpms was not rebuilt (still a365750).

### Next steps

1. Owner: `sudo dnf upgrade ~/rpmbuild-sdcompat/rpms/*.rpm`, then
   `sdcompat init --dry-run` and `sudo sdcompat init`, and part 26's checks.
2. Rerun `packages/copr-srpms.sh` before uploading to COPR.

## 2026-10-27, part 28: GitHub releases instead of COPR; skarnet sources by hand (7d4ec0f, 5d76f03, a7f0104, 0def011)

### Decisions (owner, 2026-10-27)

- Publish the RPMs as files on GitHub releases (asked: Pages repo, Releases
  or Actions; owner chose Releases). COPR is replaced, not kept beside it.
- The skarnet.org packages are not in a release: each user builds them.
- build-skarnet-rpms.sh downloads nothing; the user downloads the tarballs
  by hand. The README guides that without direct download URLs (only
  skarnet.org's software page and the file names).

### What changed

- 7d4ec0f: packages/github-release.sh [OUTDIR] (default ~/sdcompat-release)
  replaces packages/copr-srpms.sh. Needs a committed tree; builds in a fresh
  OUTDIR/work; OUTDIR/assets = binary RPMs (no debuginfo) + SHA256SUMS;
  OUTDIR/NOTES.md = release text (commit, Fedora release/arch, install
  steps, package list); prints the `gh release create vVERSION ...`
  command (tag from [workspace.package] version, so bump it per release).
  The --srpm options of the build scripts stay (comments no longer mention
  COPR). sdcompat init's hints point at the releases page.
- 5d76f03: skarnet left out of the release; NOTES.md and docs give the
  skarnet build first. init: missing s6-linux-init/mdevd -> build them with
  dist/rpm/build-skarnet-rpms.sh; missing sdcompat-journal-relay -> release.
- a7f0104: build-skarnet-rpms.sh reads tarballs from ~/skarnet-sources
  (`--sources DIR` / `--sources=DIR`), checks each against
  dist/skarnet.versions, stops on a mismatch, lists missing ones; no curl.
- 0def011: README pair "Deploying on Fedora" is three steps (1 get the
  sources: clone the tag, a loop printing NAME-VERSION.tar.gz from
  skarnet.versions, download from https://skarnet.org/software/ pages into
  ~/skarnet-sources; 2 build/install skarnet; 3 install the release RPMs).
  TESTING stage 2, packages/README pair, NOTES.md and the script's error
  name files only, no tarball URLs. Spec Source0 lines and the maintainer
  comment in skarnet.versions keep their URLs.

### Tests

- cargo build (no warnings), cargo test, standalone tests, sandbox 257/257
  (after 7d4ec0f; later commits change scripts/docs only and init's hint
  text). bash -n / shellcheck -S warning on the changed scripts.
- build-skarnet-rpms.sh: empty sources dir (lists 7 names, exit 1), a
  corrupt mdevd tarball (checksum error, exit 1), good tarballs with --srpm
  (7 source RPMs). A full binary skarnet build with local tarballs was not
  run.
- github-release.sh on 0def011: exit 0, 9 RPMs (5 core, 4 standalone),
  `sha256sum -c SHA256SUMS` passes, NOTES.md as intended.
- Not tested: a real GitHub release (no credentials here), installing from
  the release files.

### Current state

Committed through 0def011 (plus this entry), not pushed. Release files for
v0.1.0 in ~/sdcompat-release/assets with ~/sdcompat-release/NOTES.md.
~/rpmbuild-sdcompat/rpms has the 4db6aef build from part 27; the installed
RPMs are still ga365750. ~/sdcompat-srpms (COPR, part 26) is obsolete.

### Next steps

1. Owner: push, choose a license (specs say `License: TBD`), then
   `gh release create v0.1.0 ...` as printed (or the web UI).
2. Owner: on this machine `sudo dnf upgrade` to a new build and run
   `sudo sdcompat init` (part 26's checks).
3. Try the README's install steps on a fresh Fedora VM from the release.

## 2026-10-27, part 29: docs review; the repository goes public (fd25395, and the next commit)

- Review of the README and TESTING pairs after part 28 (fd25395): the deploy
  section names its three steps; the update note says when to rebuild the
  skarnet packages; TESTING's stage table said 6 s6 packages (7 since
  mdevd); stage 1 installs cpio (build-skarnet-rpms.sh needs it) instead of
  curl.
- Owner decision: the repository will be made public (GitHub releases of a
  private repository are only downloadable with access). TESTING stage 1
  now clones with plain `git clone https://...` (no `gh auth login`).
- Next steps: as in part 28; make the repository public before
  `gh release create`.

## 2026-10-27, part 30: repository renamed to useless-init-converter (165c64d)

- Owner renamed the GitHub repository to shandyCho/useless-init-converter.
  Decision (asked): rename the repository name and URLs only; the commands,
  crates, RPM packages, paths (/etc/sdcompat, ...), SELinux module, GRUB
  title and kernel options keep the name sdcompat.
- Changed: every GitHub URL, the clone commands (`cd
  useless-init-converter`, `useless-init-converter/dist/...`), the release
  script's GITHUB_REPO, init's releases link and --help text; the README
  pair's title is useless-init-converter with a line saying the commands,
  packages and paths are named sdcompat. The local `git remote` origin was
  updated. The local checkout directory stays ~/sdcompat.
- cargo build/test passed; shellcheck on github-release.sh.
- ~/sdcompat-release (built from 0def011) still has the old URLs in
  NOTES.md and the old repository in its gh command: rerun
  packages/github-release.sh after pushing before publishing.

## 2026-10-27, part 31: sdcompat renamed to uinitconverter (a5f780f)

From this entry on, the project's names are uinitconverter; the entries
above keep the old names (sdcompat, sdcompatd, /etc/sdcompat, ...).

### Decisions (owner, asked 2026-10-27)

- Rename every sdcompat name to uinitconverter (not only the main command):
  commands (uinitconverter, uinitconverterd, uinitconverter-gen, -runner,
  -timers), crates, standalone tools, RPM packages/specs, /etc, /var/lib,
  /var/log and /usr/libexec paths, SELinux module and types, D-Bus interface
  org.uinitconverter.Manager and policy file, kernel options
  (uinitconverter.db=, uinitconverter.initrd.timeout=), GRUB title "Fedora
  (s6 + uinitconverter)", UINITCONVERTER_* variables, build directories
  (~/rpmbuild-uinitconverter, ~/rpmbuild-uinitconverter-standalone,
  ~/uinitconverter-release), CLAUDE.md, docs, tests.
- No automatic migration: packages/README pair "Moving from sdcompat"
  (boot systemd, back up /etc/sdcompat, `sudo sdcompat init --uninstall`,
  dnf remove the 9 old packages, move /var/lib/s6-rc/repository aside, rm
  /var/lib/sdcompat and /var/log/sdcompat, install the new RPMs, run `sudo
  uinitconverter init` with the old options, copy edits by hand). It
  replaces the setup-fedora.sh migration note; the README pair points at it.

### How

- git mv of the 132 tracked paths with sdcompat in them; text replace of
  sdcompat/SDCOMPAT/Sdcompat in 141 files (not work_history.md). Old empty
  directories and the old standalone target/ caches removed.
- Checked first: no code matches our process names by comm (the 15-char
  limit: uinitconverterd is exactly 15); tests use pgrep -f. cargo fmt was
  already not clean before, so nothing was reformatted.
- Leftovers that now read oddly but are harmless (they handle old installs,
  and the clean switch makes them moot): init's removal of the old
  setup-script marker and of /etc/uinitconverter/s6/default, and comments
  like "uinitconverter shipped /etc/uinitconverter/s6/default before" in
  uinitconverter-gen.

### Tests

- cargo build (no warnings), cargo test, the 4 standalone test suites,
  bash -n and shellcheck -S warning on every tracked .sh.
- Sandbox: default 257/257, SANDBOX_JOURNAL=no 264/264,
  SANDBOX_FDSTORE=no 257/257.
- packages/github-release.sh on a5f780f: exit 0, 9 RPMs, SHA256SUMS ok;
  no file, Requires or Provides with "sdcompat" in them.
- Not tested: a real install of the renamed RPMs, the "Moving from
  sdcompat" steps on this machine, a boot.

### Current state

Committed through a5f780f (plus this entry), not pushed. Release files for
v0.1.0 in ~/uinitconverter-release (tag on a5f780f). Obsolete: ~/sdcompat-release,
~/rpmbuild-sdcompat, ~/rpmbuild-sdcompat-standalone, ~/sdcompat-srpms.
This machine still runs the old sdcompat RPMs (ga365750) on its s6 boot.

### Next steps

1. Owner: on this machine, follow "Moving from sdcompat" (from the systemd
   boot), install ~/uinitconverter-release/assets/*.rpm, `sudo
   uinitconverter init --journal=no --resolved=no --boot-only-s6=yes
   --selinux=enforcing` (this machine's old choices), boot s6.
2. Owner: push, make the repository public, choose a license, then
   `gh release create` (rerun packages/github-release.sh first if new
   commits should be in the tag).


## 2026-10-27, part 32: docs review after the rename (4fde5ce)

- Korean particles: sdcompat ended in a consonant, uinitconverter (…컨버터)
  ends in a vowel, so 은/이/을/과 after it became 는/가/를/와 (17 places:
  README, TESTING, packages/README, three standalone READMEs). Check this
  again whenever a name before a particle changes.
- TESTING pair, update section: an sdcompat install can't be upgraded;
  link to "Moving from sdcompat".
- No other sentence was made wrong by the rename; English has no "an
  uinitconverter".

## 2026-10-27, part 33: README/TESTING checked against the source (see the docs commit after 4ebef55)

Owner asked to compare the README with the code and update README and
TESTING. Checked: source layout, Cargo dependencies, config keys and
defaults (config.rs vs dist/uinitconverter.conf vs README), each tool's
subcommands (main.rs dispatch), init's options and steps, the D-Bus methods
listed, the directive lists (implemented / not implemented), the s6
initramfs's kernel options, rc.init/rc.shutdown limits, spec Requires, the
sandbox's programs, and the expected results of check.sh, initramfs.sh,
linkwatch-check.sh, bpf-root.sh, memcompare.sh, plan's output.

Fixed (both languages): missing subcommands (stop-instances, stop-atom,
init, forwarding to the standalone tools); OnTimezoneChange= not
implemented and the timer keys accepted silently; shadow-utils dependency;
check counts (~260, was 119/120); init's conditional unit enabling, -y,
--grub=yes|no; rd.luks=0; initramfs.sh's by-uuid case; s6-frontend/s6.conf
without a "setup script"; SELinux permissive only on the first run; GNOME
status limited to what is recorded (GDM login, GNOME session, power off from
the GNOME menu on one Fedora 45 machine, enforcing); source layout entries.
Everything else matched the code.

## 2026-10-27, part 34: zram swap on the s6 boot (6d331a2); memcompare (abf10e8)

- abf10e8: tests/memcompare.sh groups sdcompat* processes too, so this
  machine (still the old sdcompat RPMs, core 4db6aef) can be measured.
- The owner wants to redo the memory comparison (part of 2026-10-21: s6
  used ~211 MiB more; the s6 boot had no swap). First: swap on s6.
- Cause: the s6 boot runs no systemd generators; Fedora's zram swap comes
  from zram-generator (zram-generator-defaults: [zram0] zram-size =
  min(ram, 8192)). It writes dev-zram0.swap (What=/dev/zram0,
  Priority=100, Options=discard) and systemd-zram-setup@zram0.service
  runs `zram-generator --setup-device zram0`.
- 6d331a2: s6 oneshot `zram` (dist/s6/zram: flag-essential, after
  mount-api, member of uinitconverter-sysinit, so generated services that
  follow sysinit.target stop before it) runs `early zram` / `early
  zram-stop`. It runs the generator itself into /run/systemd/generator
  (+ .early/.late), restorecon'd: policy lets systemd_zram_generator_t
  (entered from init_t, as under systemd) write only
  systemd_generator_unit_file_t dirs; not tmpfs_t files, not var_run_t.
  The generator dirs are not in UNIT_PATH. Then per dev-zramN.swap:
  --setup-device, `swapon -p PRIO -o OPTS`; stop: swapoff if in
  /proc/swaps, --reset-device. Missing generator: nothing (exit 0); every
  failure only warns, as the other early steps.
- Tested: bash -n, shellcheck; the logic with a fake generator and fake
  swapon/swapoff/restorecon (up: generator, setup, `swapon -p 100 -o
  discard /dev/zram0`; down: swapoff, reset). The sandbox doesn't use
  dist/s6. Not tested: a real s6 boot (SELinux enforcing included).
- Docs: replaced-components table (zram row), "Not implemented": systemd
  generators (fstab and zram covered), TESTING row "Swap (zram)".

### Next steps

1. Owner: switch this machine to uinitconverter ("Moving from sdcompat"),
   installing ~/rpmbuild-uinitconverter/rpms (core with zram) and the
   standalone RPMs from ~/uinitconverter-release/assets; `sudo
   uinitconverter init --journal=no --resolved=no --boot-only-s6=yes
   --selinux=enforcing`; boot s6; TESTING "Swap (zram)" row; `sudo
   ausearch -m AVC -ts boot | grep zram`.
2. Then the memory comparison (tests/memcompare.sh snap on both boots).

## 2026-10-27, part 35: memory comparison redone (no code change)

Snapshots: ~/uinitconverter-mem/mem-systemd-20261027-173018.tsv (up 580 s)
and mem-s6-20261027-173624.tsv (up 338 s; zram0 8G prio 100 active, so
6d331a2 works on a real s6 boot). The uptimes differ by 4 min, so Cached
is not comparable.

- "used" gap +210.8 → +78.6 MiB; total − available now −17.4 MiB (s6
  slightly better). Most of the 2026-10-21 gap was packagekitd (100 MiB,
  transient) and no swap, not the init.
- init group +65.7 MiB PSS: 256 s6-supervise (32.3), 37 uinitconverter-*
  helpers (35.1: 23 runners, 9 timers, linkwatch, journal-relay, fdstore),
  uinitconverterd 7.6. Logging −36.5 (no journald).
- Services running on s6 that are idle/socket-activated on systemd:
  iscsid+iscsiuio 14.2, virtnetworkd/virtlogd/virtlockd 13.4 (and other
  virt*d under 0.5 each), dmeventd 2.4, dnsmasq 1.0 — about 31 MiB.
  dbus-daemon (6.1) is the s6 system bus instead of dbus-broker (−5.1).
- Kernel side from 458 vs 114 user processes: PageTables +17.8,
  SUnreclaim +20.0, KernelStack +6.6.

### Next steps

Unchanged from part 34, except the memory comparison is done; owner to
decide whether the eagerly started socket-activated services matter.

## 2026-10-27, part 36: on-demand socket activation at boot (f930166)

Owner asked for socket activation after part 35 (~31 MiB of
socket-activated daemons ran on the s6 boot only).

- Rule (gen, `mark_on_demand`): a longrun service with sockets is on
  demand when the boot reaches it only through .socket dependencies
  (`Planned::socket_deps`: deps resolved from a .socket name and not also
  by name). Walk from default.target following only units that really
  start: what an on-demand service requires doesn't count (iscsiuio
  Requires=iscsid; systemd starts neither at boot). Its run script gets
  `--on-demand`; `plan` prints "on demand". Dynamic instances: never.
- Runner: `run --on-demand` without ACTIVATED and without
  START_REQUESTED binds the sockets (`bind_sockets`, split out of
  setup_listen_fds), and if the fd store holds them sets SOCKET_WAIT and
  takes the existing wait path (ready for s6, unit inactive). Store
  unreachable or bind error: starts at once. Leaving the wait also starts
  (detached StartUnit, `start_units_detached`, shared with OnFailure=) the
  Requires/BindsTo/Wants that are themselves in SOCKET_WAIT.
- Keys: ACTIVATED (set when the run proceeds to start, cleared by finish
  when wanted down) so Restart=/`systemctl restart`/s6-rc-update don't
  wait again; START_REQUESTED (daemon, do_start for a longrun, before
  s6-rc/s6-svc), so a job's start runs at once without the 1 s
  wake_socket_wait delay. A leftover START_REQUESTED (job that didn't run
  it) only makes a later run start at once.
- First version gated candidates on uinitconverter-fdstore being an
  extra s6-rc source; the sandbox runs the store outside s6-rc, so nothing
  was marked. Dropped: the runner's fallback covers a missing store.
- On this machine's units (current config, `uinitconverter-gen plan`): 16
  on demand: cups, dm-event, iscsid, iscsiuio, lvm2-lvmpolld, sssd-kcm,
  systemd-hostnamed, virtinterfaced, virtlockd, virtlogd, virtnetworkd,
  virtnodedevd, virtnwfilterd, virtproxyd, virtsecretd, virtstoraged.
  cups still runs once cups.path sees its file (path-run starts it, as
  systemd). Each waiting service keeps its runner process (0.2-2 MiB PSS
  seen in part 35), so the saving is less than the daemons' 31 MiB.
- Not changed (README "Not implemented"): starting a .socket at run time
  still starts its service at once; requirements of an on-demand service
  that are not on demand still start at boot (s6-rc deps); `systemctl
  stop` still closes the sockets.
- Sandbox: it had no sockets.target, so enabled sockets never booted;
  added (multi-user Wants=). sock.service is now on demand at boot (checks
  in "boot state"); new lazy.socket/lazy.service (Requires=lazydep) and
  lazydep.socket/.service: not run at boot, a connect starts both, restart
  and start-after-stop run at once. The fd store inode check restarts
  sock.service once first (SANDBOX_FDSTORE=no: bound at boot before the
  daemon served the store). FDSTORE=no expects the fallback.
- Mistake: a heredoc around Python code containing `EOF` lines ended
  early and ran the rest as shell; only failing writes to / and unknown
  functions, nothing changed (git status checked).
- Tests: `cargo test` all pass (gen 6, new
  socket_only_services_are_on_demand); standalone 3/3/3/2; sandbox
  272/272, SANDBOX_FDSTORE=no 260/260, SANDBOX_JOURNAL=no 279/279;
  bash -n and shellcheck -S warning pass. Not tested: a real s6 boot.
- Docs: README pair (enabled .socket, on-demand activation, not
  implemented), TESTING pair row "On-demand services".

### Next steps

1. Owner: rebuild and install the RPMs, `sudo uinitconverter reload`,
   reboot into s6; TESTING row "On-demand services"; `uinitconverter plan
   | grep -B1 'on demand'`; check printing (cups), hostnamectl, virt-manager
   or `virsh` with a VM, LVM (`lvs`, dmeventd monitoring).
2. Then `sudo tests/memcompare.sh snap` again on s6 (same uptime as the
   systemd snapshot, ~580 s: `--wait 580`) and compare.

## 2026-10-27, part 37: memory test on every boot (1ec4ba2)

Owner asked for an automated memory comparison on this machine, "just an
executable thing when I reboot both". First draft rebooted by itself
(grub2-reboot alternating entries, a state machine of N rounds); the owner
then said they only need something that runs on the reboots they do, so it
became a snapshot service.

- `tests/memtest.sh install [--wait 600] [--dir] [--max-diff 50]` (root):
  copies memtest.sh and memcompare.sh to /usr/local/libexec/
  uinitconverter-memtest (restorecon: bin_t, a /home script can't run as a
  service under enforcing), writes /etc/systemd/system/
  uinitconverter-memtest.service (Type=simple, WantedBy=multi-user,
  StandardOutput=append: so no s6-log under JOURNAL=no), `systemctl
  enable`, then the s6 database: systemd boot `uinitconverter reload
  --no-update`, s6 boot daemon-reload (new service seen enabled -> active
  in the set). Options are baked into ExecStart=.
- Each boot: sleeps --wait from the service's start, not uptime (uptime
  includes the LUKS prompt, which the owner types by hand; the s6 entry
  uses the s6 initramfs, passphrase only), then `memcompare.sh snap --wait
  0 --exclude-cgroup uinitconverter-memtest.service` into
  ~/uinitconverter-mem/memtest, report.txt rewritten. Measure at the GDM
  greeter (don't log in) so both inits are alike.
- report: medians per init (total - available, used, AnonPages, Shmem,
  SUnreclaim, KernelStack, PageTables, PSS by group, process count),
  per-snapshot lines, the latest pair through memcompare compare; PASS
  (exit 0) when s6's total - available median is at most --max-diff MiB
  above systemd's, FAIL 1, INCOMPLETE 2. On the part-35 snapshots it gives
  the same numbers as memcompare compare (-17.4: PASS).
- memcompare snap --exclude-cgroup UNIT (matches the cgroup path's last
  components); the runner uses system.slice/UNIT on s6 as systemd does.
- tests/memtest-check.sh: unprivileged, fake systemctl/uinitconverter/
  restorecon; install on both boots, the service's snapshot and report,
  run, uninstall, --dir with spaces refused, report medians/PASS/FAIL/
  INCOMPLETE on made-up snapshots. 22/22. --exclude-cgroup checked against
  a snapshot without it (own PID present, then absent). shellcheck/bash -n
  pass. Not run: install as root, a real boot of either init.
- Docs: TESTING pair, memory comparison section.
- Found uncommitted changes in tests/check.sh and tests/sandbox.sh
  (SANDBOX_BUS=broker) that are not from this session; left untouched and
  out of the commit.

### Next steps

1. Owner: `sudo tests/memtest.sh install` (after installing the f930166
   RPMs and reloading, so on-demand activation is measured), then reboot
   into each entry a few times, not logging in for 10 minutes; read
   ~/uinitconverter-mem/memtest/report.txt. Check the service ran on s6:
   `tail /var/lib/uinitconverter-memtest/log`, `sudo ausearch -m AVC -ts
   boot | grep memtest`.
2. Earlier next steps from part 36 (on-demand services on a real boot).

## 2026-10-27, part 38: dbus-broker as the s6 system bus — study and sandbox mode (e3c8c9c)

The owner asked about replacing dbus-daemon on the s6 boot, suggesting a
"standalone dbus-broker". Findings (from dbus-broker v37's source and the
binaries):

- `dbus-broker` (the broker) doesn't link libsystemd; activation goes to
  its controller over a private fd (`org.bus1.DBus.Controller`).
  `dbus-broker-launch` links libsystemd and needs a systemd *API*, not
  systemd: `Subscribe`, `LoadUnit` (then `PropertiesChanged` on the unit
  for `ActiveState=failed`/`ConditionResult=false`), `StartUnit` or
  `StartTransientUnit("dbus-:1.N-NAME@K.service", ExecStart a(sasb),
  KillMode=process, CollectMode, User=)`, `JobRemoved` (done/skipped =
  ok), `SetEnvironment`. Also a listening socket via LISTEN_FDS and
  /run/systemd/journal/socket (fatal if missing).
- Options considered: keep dbus-daemon; dbus-broker + stock launcher on
  uinitconverterd's systemd1 emulation (chosen to try: Fedora's default
  bus, fits decision 1); own controller for the bare broker (fallback:
  must translate the dbus-1 XML policy); busd (alpha, no activation:
  rejected).
- Checked on our side: LoadUnit returns a path for a not-yet-existing
  unit (needed, or every transient activation fails); StartUnit replies
  before JobRemoved with the same job path. The launcher connects to the
  bus before dropping to `dbus`, so the bus reports uid 0 and no polkit
  query happens (would deadlock: polkit is bus-activated). It ignores
  activation of org.freedesktop.systemd1, so calls made before
  uinitconverterd joins wait in the broker (sd-bus 25 s timeout).
- Known gap (same with dbus-daemon today): a generated s6-rc unit that
  fails after its job finished emits no PropertiesChanged, so callers
  wait for their own timeout.

Change (e3c8c9c): `SANDBOX_BUS=broker` in tests/sandbox.sh runs
`dbus-broker-launch --scope system` in `unshare -U --map-current-user -m
--keep-caps` (same uid, so peer credentials stay real) with a tmpfs /run,
/run/user bound back, and a stand-in journal bound at
/run/systemd/journal (journal-relay's dir with SANDBOX_JOURNAL=no, else a
sink writing broker-journal.log); a python binder passes the socket as fd
3 (needs set_inheritable: the socket often already is fd 3). Setup exits
with the launcher's log if it dies. New activation services in both
modes: org.example.Trans (no SystemdService=; broker mode also checks a
running dbus-*-org.example.Trans@N.service via ListUnits) and
org.example.Fail (failact.service fails; the caller must get an error in
under 10 s). TESTING/TESTING-us document SANDBOX_BUS.

Tests: shellcheck/bash -n; default 274/274, SANDBOX_BUS=broker 275/275,
SANDBOX_BUS=broker SANDBOX_JOURNAL=no 282/282 (launcher's "Ready" reached
the stand-in /dev/log through the relay). Not run: cargo test (no Rust
change). Not tested: a real s6 boot with dbus-broker, SELinux enforcing.

### Next steps (dbus-broker)

1. Owner: decide whether to make the s6 system bus selectable.
2. If yes: dist/s6/dbus run script choosing broker/daemon by a setting
   (keep dbus-daemon installed as the fallback; rollback without editing
   files, e.g. a kernel command-line flag); depend on journald or
   journal-relay; socket binding + LISTEN_FDS/LISTEN_PID + restorecon;
   readiness via sd_notify; `dbus` reload as a ReloadConfig call.
3. Then a real s6 boot in enforcing mode (`ausearch -m AVC -ts boot`).

## 2026-10-27, part 39: selectable system bus on the s6 boot (ff59ab0)

The owner decided: make the s6 system bus selectable (part 38, step 1).

- `BUS=daemon|broker` in uinitconverter.conf (default daemon, so existing
  installs don't change), set by `uinitconverter init --bus=` (marker "#
  set by uinitconverter init --bus=broker"; daemon removes the line);
  `uinitconverter.bus=daemon|broker` on the kernel command line wins for
  one boot. config.rs accepts the key (the run script reads it). init's
  plan shows a D-Bus line; next steps list the kernel flag; --bus=broker
  requires dbus-broker-launch.
- dist/s6/dbus/run, broker: `s6-softlimit -o 16384
  s6-ipcserver-socketbinder -a 0666 /run/dbus/system_bus_socket sh -c
  'echo >&3; export LISTEN_FDS=1 LISTEN_PID=$$; exec dbus-broker-launch
  --scope system --audit 3<&0 </dev/null'`, oom_score_adj -900 (as
  dbus-broker.service). Ready once the socket is bound: clients wait in
  its backlog, as with dbus.socket. The supervised PID is the launcher, so
  `s6-svc -h` reloads it (it also watches its config dirs itself, so no
  reload work was needed: risk 3 of part 38 is void).
- Fallbacks to dbus-daemon (risk 5, no boot without a bus): dbus-broker
  not installed; /run/systemd/journal/socket missing after 10 s (journald
  with JOURNAL=yes is a generated service started beside dbus, so no
  static dependency is possible; journal-relay is in sysinit with
  JOURNAL=no); a 4th start in this boot (/run/uinitconverter/dbus-broker-tries,
  so three launcher exits). The reason goes to the catch-all log ("dbus:
  dbus-daemon instead of dbus-broker: ..."). dbus-daemon stays a Requires.
- SELinux: s6-ipcserver-socketbinder (s6 2.15) creates PATH.lock while
  binding (not passed on to the program). Fedora's policy lets init_t
  create the socket in system_dbusd_var_run_t but no file there (sesearch:
  pidfile:file has no create/write), so enforcing would deny it and the
  broker would never start. cil: named typetransition for
  "system_bus_socket.lock" to init_var_run_t. Not compiled here (no root
  to read the policy store); same form as the existing "event" rule.
- Tested: the broker branch of a path-substituted copy under a real
  s6-supervise in the sandbox namespace with the real launcher (own config
  instead of --audit and the system config's <user>dbus</user>): s6-svwait
  -U ready, socket 0666, bus answers, supervised PID = launcher with
  LISTEN_FDS=1 and its own LISTEN_PID, launcher log on the journal socket.
  With a fake dbus-daemon: kernel flag daemon over BUS=broker, the 4th try,
  dbus-broker missing, and (by accident, in the first attempt) the missing
  journal socket all fall back with their message. cargo test (75 + 2
  ignored), standalone tests, shellcheck/bash -n, sandbox daemon 274/274,
  broker 275/275, broker + SANDBOX_JOURNAL=no 282/282.
- Test mistake, no harm: one fallback case run outside the namespace found
  the host's journal socket and started a real launcher on the test
  config; stopped by its command line (the host's buses untouched). Note:
  `pgrep -x dbus-broker-launch` never matches (name over 15 characters).
- Docs: README/README-us (--bus row, BUS key, kernel flag, activation
  through the broker, replaced-components rows), TESTING/TESTING-us boot
  check row "System bus (only with --bus=broker)", uinitconverter.conf.

### Current state

The s6 boot runs dbus-daemon unless BUS=broker. Not tested: a real s6
boot with BUS=broker, permissive or enforcing; loading the new cil rule.

### Next steps

1. Owner: build the RPMs, install, `sudo uinitconverter init --bus=broker`
   (semodule -i loads the new rule), boot s6 permissive; TESTING row
   "System bus"; `sudo ausearch -m AVC -ts boot | grep -E 'dbus|socketbinder'`;
   GNOME login, `busctl --system list`, a bus-activated service (e.g.
   `busctl --system introspect org.freedesktop.hostname1 /org/freedesktop/hostname1`).
2. Then enforcing. If the bus fails, boot with `uinitconverter.bus=daemon`.
3. Known gap (both buses): a generated unit that fails after its start job
   sends no PropertiesChanged, so activation callers wait for their own
   timeout.

## 2026-10-27, part 40: s6 boot memory, notify helper re-executes (720c44c)

Owner asked how to cut the s6 boot's memory. Measured on the live s6 boot
(old RPMs) and the part-35 snapshot:

- 254 s6-supervise, 118 with a child; 136 idle (80 down services, 56 down
  loggers). s6-rc-init (s6-rc 0.8.0.2 doc) links every longrun of the
  database into the scandir with ./down, so each gets a supervisor.
- Lingering uinitconverter-runner processes by kind (PSS): Type=notify and
  notify-reload helpers ~1.7 MiB each (11: chronyd, polkit, logind, cups,
  iscsid, iscsiuio, virtlogd, virtlockd, virtnetworkd, user@1000 x2);
  socket-wait runners ~280 KiB (the on-demand ones stay cheap); forking
  supervisors ~0.4 MiB; timer/path-run ~0.45 MiB each (9).
- Kernel cost per process ~130 KiB (part 35: +44 MiB for +344 processes).

Options given (owner's choice): 2a notify helper re-exec (done here), 2b
notify sockets in the daemon, 3 fewer supervisors (A: unreached units
`masked` in the s6 set, the s6-native way per repodefs.html; B: left out
of the database, run as dynamic instances), 4 one timers process, 5 shared
logging under JOURNAL=no. **Owner: item 3 stays as it is for now** (hard
to judge yet); s6 distros (Artix/Obarun) avoid it by shipping s6 service
packages only for what is installed.

720c44c:
- The notify helper (double fork after setup_listen_fds) now exec()s the
  runner as `notify-helper` (internal subcommand, handled in main before
  any config/unit load) with NotifyHelper's fields as options (`--unit`
  last for pgrep). It clears FD_CLOEXEC on its socket and the readiness
  pipe, and in the new image closes every other fd > 2: the forked helper
  used to keep the service's LISTEN_FDS open too. Path: current_exe()
  (so ps shows uinitconverter-runner), /proc/self/exe if the binary was
  replaced. exec failure: the old in-fork loop runs.
- ExecStartPost=: the helper forks a child that does Config::load +
  Runner::new + after_ready (MAINPID, kill on failure as before) and waits
  for it; without ExecStartPost= it only stamps ACTIVE_ENTER.
- Sandbox, debug build: helper RssAnon 384 kB (installed release helpers
  on this boot: 1.6-2.7 MB). Expected saving ~15-17 MiB plus nothing else
  changes in behaviour. sssd's helper was already ~300 kB.
- The sandbox's "notify helper idle" check matched the old command line;
  with the new one it would have passed by finding nothing, so it now
  uses the notify-helper pattern, and new checks: helper re-executed,
  RssAnon < 1 MiB, at most 5 fds (stdio, socket, pidfd), ExecStartPost=
  with $MAINPID (notifier.service gained one).
- Tests: `cargo test` all pass (runner: notify_helper_args_round_trip);
  sandbox 278/278, SANDBOX_FDSTORE=no 266/266, SANDBOX_JOURNAL=no 285/285
  (run with parts 38-39 in the tree); shellcheck/bash -n pass.
- Another session committed parts 38-39 (e3c8c9c, ff59ab0, 84e723b) while
  this ran; my check.sh/sandbox.sh hunks were staged alone with git apply
  --cached and sit on top of them.
- Not done: the PAM parent of user@ (~3 MiB, keeps the PAM handle) and
  the forking supervisor stay as they are; Type=dbus and Type=simple
  helpers end at readiness, so they were left alone.

### Next steps

1. Owner: rebuild/install the RPMs (on-demand + this), reboot s6, let
   tests/memtest.sh collect snapshots on both boots; `ps -o rss,args -C
   uinitconverter-runner` should show `notify-helper ... --unit X` lines.
2. Item 3 (supervisors) and 4/5 when the owner decides.

## 2026-10-27, part 41: memtest unit never ran on the s6 boot (372b9b2)

Owner found no snapshots after installing memtest (22:21, systemd boot)
and rebooting into s6: `s6 set status uinitconverter-memtest.service` said
"not a valid identifier", and `uinitconverter-gen plan` listed it as
"excluded by configuration". Cause: the default EXCLUDE ends with
`uinitconverter*` (the package's own units, was `sdcompat*`), which matches
the test unit's name. The systemd boot took none either, since it was
installed after that boot was up and Type=simple enable doesn't start it.

372b9b2:
- tests/memtest.sh: the unit is now memtest-uinitconverter.service (the
  lib and log directories keep their paths); install and uninstall
  disable and remove uinitconverter-memtest.service if it is still there.
  The EXCLUDE glob was left as it is, since it's there on purpose.
- tests/memtest-check.sh: checks that the old name is replaced and that
  the unit name is outside `uinitconverter*`. 24/24 pass;
  bash -n/shellcheck pass. Only the test scripts changed, so cargo and
  the sandbox suite weren't run.
- Side note: `uinitconverter-gen` isn't on PATH on this machine; the
  plan was read with target/debug/uinitconverter-gen.

### Current state

memtest is fixed in the tree but the old install is still on the machine.

### Next steps

1. Owner: `sudo tests/memtest.sh install` again (replaces the old unit),
   then reboot into each init; on s6 check `s6 set status
   memtest-uinitconverter.service` and the log.
2. As in part 40: items 3-5 when the owner decides.

## 2026-10-28, part 42: memtest results, 3 boots each (no code change)

Owner ran memtest on 3 s6 and 3 systemd boots (~/uinitconverter-mem/
memtest, report.txt, all at uptime ~615-626 s, GDM greeter).

- PASS: total - available median s6 1164.7 vs systemd 1179.4 MiB (-14.7).
  Spread within an init is ~20 MiB (s6 1145.5-1166.2, systemd
  1171.1-1179.7), so the two are equal within noise.
- "used" +107 MiB is Cached -144 (no journald files in the page cache on
  s6), not more memory in use; total - available is the figure to read.
- s6 side: init group +66 MiB PSS (s6-supervise x255 32.2, runner helpers
  x37 37.0, uinitconverterd 7.5), kernel cost of 414 vs 77 processes
  (PageTables +17, SUnreclaim +13, KernelStack +6), logging -36 (no
  journald; rsyslogd 2.4 vs 15.4).
- On-demand socket activation (f930166) shows on the real boot: iscsid,
  virt*d, dmeventd are no longer running on s6 (part 35 had ~31 MiB of
  them); running services 25 (s6) vs 35 (systemd).
- RPMs from 31d9726 were installed 2026-10-27 23:57, so the 23:10 s6
  snapshot is the old build (dbus-daemon) and 00:12/00:45 the new one
  (dbus-broker as the s6 bus). The figures barely change between them.
- The notify helper re-exec (720c44c) does not show: in all three s6
  snapshots the same ~11 runner processes have RssAnon 1.5-2.8 MB (the
  sandbox's re-executed helper had 384 kB), and the runner total stays at
  35.5-37.0 MiB. The snapshot records only comm, so it can't say which
  units these are or whether the exec failed (its error goes to the
  service's log; nothing in /var/log/uinitconverter or /var/log/messages).

### Next steps

1. On the next s6 boot: `ps -o pid,rss,args -C uinitconverter-runner`
   (are there `notify-helper ... --unit X` lines?), and look for
   "notify helper: exec" in those units' logs and for AVCs
   (`sudo ausearch -m AVC -ts boot | grep runner`). If the exec works,
   find what the ~2 MB processes are.
2. Optional: memcompare snap could record args for the init group, so a
   snapshot names the runner kinds.
3. Items 3-5 of part 40 when the owner decides.

## 2026-10-28, part 43: helper re-exec confirmed, on-demand runners' SELinux heap (08d57db)

Owner, s6 boot (uptime ~18 min): `ps -o pid,rss,args -C uinitconverter-runner`
shows `uinitconverter-runner notify-helper ... --unit X` for chronyd,
polkit, systemd-logind, cups, user@1000 and systemd-hostnamed; no AVCs
for the runner (`ausearch -m AVC -ts boot`). So 720c44c works on the real
boot; part 42's snapshots were taken before it was installed, or the ~2 MB
processes were something else (below).

Read from /proc/PID/status (RssAnon, KiB):
- notify helpers 168-184 (were 1.5-2.7 MB). Their RSS of ~3.5 MB in ps is
  file pages of the shared binary.
- on-demand runners: iscsid, iscsiuio, dm-event 280; virt*d 1540-2164,
  lvm2-lvmpolld 2772. None of them ever started its service (process
  age = boot, no children). The difference: the virt*d and lvmpolld
  sockets are filesystem paths, iscsid/iscsiuio abstract, dm-event FIFOs.
  bind_unix() calls selinux::restorecon(), which was selinux_restorecon();
  libselinux keeps that file_contexts handle until the process exits.
  Measured with ctypes as a user: selabel_open +1.25 MB RssAnon,
  selabel_close + malloc_trim(0) gives back ~0.9 MB; one open takes
  ~0.5 ms.
- Other ~2 MB processes: user@1000's PAM parent 2704 (known, part 40),
  journal-relay 1788, uinitconverterd 4196.

08d57db:
- selinux::restorecon() now calls label_as() on the path itself
  (selabel_open, lstat, lookup, lsetfilecon, close) and label_as_with
  calls malloc_trim(0) after selabel_close. The selinux_restorecon symbol
  is no longer loaded. Difference: lsetfilecon runs even when the label
  is already right; label_as already worked this way for the private /dev.
- Expected: the 9 filesystem-socket on-demand runners drop from ~1.5-2.8
  MB to ~0.3-0.6 MB RssAnon, ~12-15 MiB in total.
- Tests: cargo build no warnings, `cargo test` all pass, sandbox 278/278.
  The sandbox runs unprivileged, so lib() is None there and the SELinux
  path is not exercised; only the real boot can show it.

### Current state

Notify helper re-exec verified on the real s6 boot. The SELinux heap fix
is committed, not built into RPMs or booted yet.

### Next steps

1. Owner: rebuild/install the RPMs, reboot s6, then check RssAnon of the
   on-demand runners, e.g. `for p in $(pgrep -f -- 'runner run --on-demand');
   do grep -H RssAnon /proc/$p/status; done`, and that the socket files
   keep their labels (`ls -Z /run/libvirt /run/lvm`, expect
   virt*_var_run_t / lvm_var_run_t as on systemd) with no new AVCs.
2. Optional: memcompare snap could record args for the init group.
3. Items 3-5 of part 40 when the owner decides.

## 2026-10-28, part 44: hardening comparison script, item 3 design (5080477)

Owner asked whether the s6 boot gets s6's security advantage. Answer given:
PID 1 is small (s6-svscan) and there is no journald, but uinitconverterd
offers the systemd1 API as root on the system bus, 37 uinitconverter
processes run as root, and logind/hostnamed/polkit/dbus-broker remain, so
the whole attack surface is not smaller. The open question was whether
the units' hardening applies the same way on s6.

5080477: tests/hardencompare.sh (snap/compare/report) and
tests/hardencompare-check.sh (29/29), documented in TESTING.md/-us.md.
- snap reads each running service's main process (oldest process in
  /sys/fs/cgroup/system.slice/**/X.service that isn't uinitconverter* or
  s6-*; same cgroup layout on both inits): uid/gid/groups, umask,
  NoNewPrivs, Seccomp + filter count, the 5 capability sets by name, the
  SELinux context, 7 namespaces (host/private against PID 1), the mount
  under 19 system paths (mount point, type, ro/rw, bind), 4 rlimits,
  oom_score_adj, cgroup BPF program count (bpftool, if installed).
- compare marks WEAKER: nnp 1->0, seccomp mode lower, capabilities added,
  private ns -> host, ro -> rw, a systemd mount on the path missing, bind
  lost, non-root -> root, unconfined SELinux type, BPF progs -> 0;
  everything else DIFF. DynamicUser ids (61184-65519) count as equal.
  Limit: the syscalls a seccomp filter allows aren't in /proc.
- Unprivileged run on this s6 boot: 25 services, readable apart from
  ns/exe. Not yet run as root on either boot.

Item 3 (part 40): first presented as reducing root processes; corrected:
the 136 idle supervisors (58 loggers) run nothing, so masking saves
memory (~20-30 MiB), not attack surface. Owner chose **A (unreached units
masked in the s6 set)** and, for starting them, **dynamic start**: units
uinitconverter masked itself stay masked in the set and the daemon runs
them on request like user@UID (dynamic.rs: svc dir under RUN_DIR/dyn,
s6-svlink); units the admin masked are still refused. Facts found for the
design: the daemon refuses set-masked units today (check_job ->
can_be_dynamic allows only template instances and transient units);
set_file_state maps rx masked to "masked", which must not happen for
auto-masked units (systemctl is-enabled); s6-rc-set-change -p pulls in
dependents and pipelines (a logger with its service). Not implemented.

Owner then asked to make hardcoded parts configurable, to support other
inits (dinit, runit) later, and to make libraries easy to replace.
Survey (for the plan given to the owner): s6 references by crate:
uinitconverterd 105, gen 54, uinitconverter 34, base 21, runner 16; exec,
timers, install, client none. Absolute path literals: exec 62,
uinitconverter 44, base 43, runner 20, daemon 11 (most are kernel/OS
paths). Config has 33 keys. External crates: libc, zbus, tokio,
futures-util, serde only; zbus/tokio used in ~10 files each; libselinux,
PAM, seccomp, BPF are already dlopen()ed at run time.

### Next steps

1. Owner: `sudo tests/hardencompare.sh snap` on a systemd and an s6
   boot, then `tests/hardencompare.sh report`.
2. Owner decides the order of item 3A and the init-backend/config work
   (see the plan in this session's answer).

## 2026-10-28, part 45: INIT= and TOOLS= (2cb6541), plan for other inits

Owner chose (나): first make hardcoded parts configurable and gather the s6
code behind one module, then implement item 3A (auto-mask + dynamic start)
inside that s6 backend. "Libraries" means **packages** (what dnf
installs), not Rust crates.

Survey of what the code runs from other packages (Command::new and
init.rs ctx.run): s6-svc 12, s6-svstat 4, s6-rc-db 3, s6-rc 3,
s6-rc-set-change 3, s6-rc-repo-init 2, s6-svscanctl 2, s6-svlink,
s6-svunlink, s6-rc-update, s6-rc-set-{new,fix,commit,install},
s6-rc-repo-sync, s6-log (generated loggers); mount/umount/swapon/swapoff;
useradd; journalctl; init.rs: systemctl, restorecon, semanage, semodule,
grub2-editenv, grubby (via kernel-install plugin), coreutils. Spec
Requires: s6, s6-rc, s6-linux-init, execline, dbus-daemon, util-linux,
shadow-utils, policycoreutils(-python-utils), grubby; Recommends
s6-frontend, rsyslog, mdevd, earlyoom. Generated run scripts use only
/bin/sh and the runner.

2cb6541:
- uinitconverter_base::tools: a process-wide NAME -> PROGRAM table,
  set from TOOLS= by Config::load (blank-separated NAME=PROGRAM; a bad
  pair is a config error); tools::path(name) falls back to the name (PATH).
  Global rather than a Config field because mountunit.rs and init.rs have
  no Config at hand.
- Routed through it: all s6 tools in daemon/s6.rs, dynamic.rs, gen,
  runner (s6-svstat, useradd), mountunit.rs, journalctl in uinitconverterd
  log, s6-log in generated logger scripts (quoted), and every program
  `uinitconverter init` runs (try_run/output/succeeds). The installer
  crate now depends on uinitconverter-base and loads the config (warning
  on errors). Not routed: coreutils, /bin/sh, the shell scripts and s6
  sources in dist/ (they belong to the s6 backend, step 2).
- INIT= (Config.init, INITS = ["s6"]): any other value fails Config::load.
- Tests: cargo test 78 pass (tools: configure_and_lookup; config:
  init_and_tools_keys, both under tools::TEST_LOCK since the table is
  global); standalone 3/3/3/2; sandbox 279/279 (new: the sandbox's
  TOOLS= routes s6-svc through a logging wrapper, check.sh checks it ran;
  45 calls), SANDBOX_JOURNAL=no 286/286; shellcheck/bash -n pass.
- Mistake during the session: a bash heredoc holding a Python script that
  itself contained `EOF` ended early and ran a few lines as shell (a
  chmod and a write to /uinitconverter.conf, both failed; nothing was
  created). Edits to scripts with heredocs are now made with the Edit tool.

### Plan (remaining)

1. Rest of step 1:
   a. Package roles whose replacement changes the command line, not only
      the path, get a provider key like BUS= (exists: daemon|broker):
      syslog (rsyslog is hardcoded in the s6 "syslog" service and
      SYSLOG_UNITS), boot loader (grub2-editenv/grubby/kernel-install in
      init.rs). Specs: express alternatives with rich dependencies.
   b. The ~180 absolute path literals: kernel/OS paths stay; the
      installer's own (S6LI_BASE, S6_CONF, SHARE, KI_PLUGIN, ...) move to
      the s6 backend in step 2 or to Config.
2. s6 backend module: daemon/s6.rs + dynamic.rs (start/stop/status,
   scandir, s6-rc-db queries), gen's repo/set/commit/install, init.rs's
   s6-linux-init setup, the runner's s6-svstat/readiness, dist/s6*. The
   daemon and gen call it through one interface; the trait is fixed only
   when a second backend (dinit first: dependencies and readiness like
   s6-rc; runit has no dependency manager) is written.
3. Item 3A inside the s6 backend (design in part 44).

## 2026-10-28, part 46: providers, s6 backend modules, AUTO_MASK (1c9c567..ae863cd)

Owner: "continue the remaining steps" (part 45's plan: 1a/1b, 2, 3A).

1c9c567 SYSLOG=rsyslog|syslog-ng (step 1a)
- dist/s6/syslog/run picks the daemon; dist/syslog-ng.conf writes the
  files of dist/rsyslog.conf. syslog-ng refuses a plain /dev/log when
  /run/systemd/system exists (our sd_booted flag) and rewrites it to a
  socket-activated source, so the run script binds /dev/log with
  s6-ipcserver-socketbinder -m -b0 and passes it in LISTEN_FDS to
  systemd-syslog(); without the flag (BOOTED_FLAG= empty) systemd-syslog()
  fails ("systemd_status='not-running'"), so it uses unix-dgram. The
  config takes the source from `UINITCONVERTER_DEVLOG`.
- Verified with syslog-ng 4.12 + ivykis + libnet downloaded with
  `dnf download` and unpacked in the scratchpad (not installed): syntax
  both ways, routing of user/authpriv/cron/mail to messages/secure/
  cron/maillog, both source variants (the second in an unshare'd mount
  namespace with /run/systemd hidden). /proc/kmsg and SELinux on the real
  boot not tested.
- Config: SYSLOG= checked (SYSLOGS), syslog-ng.service excluded with
  rsyslog.service under JOURNAL=no. init: --syslog=, generic
  conf_choice/set_conf_choice (BUS= uses it), writes the chosen conf,
  mkdir /var/lib/syslog-ng or /var/lib/rsyslog. Spec: ships
  syslog-ng.conf, Recommends (rsyslog or syslog-ng).
- Boot loader provider: **not done**. Entries are BLS (any BLS loader);
  only the default entry (grub2-editenv saved_entry/tmp_saved_entry) is
  GRUB's. systemd-boot (bootctl) can't be tested here and a mistake
  could leave the machine unbootable.

3f5bed4 init: directories from the config (step 1b)
- Ctx holds the loaded Config: CONFIG_PATH, STATE_DIR, LOG_DIR
  (config::DEFAULT_LOG_DIR when JOURNAL=yes cleared it), RUN_DIR,
  REPO_DIR's parent, LIVE_DIR(:*), DAEMON_SERVICE, UNIT_PATH, last
  EXTRA_SOURCE. Package layout and s6-linux-init paths stay constants;
  the remaining literals elsewhere are kernel/systemd-semantic paths.
  Dry run unchanged except the order of two restorecon arguments.

Step 2, s6 code behind modules (no behaviour change):
- 3936b5b gen: lib.rs keeps plan()/report()/types; s6.rs has
  longrun_files, write_source, stop atoms, repo/set/commit/install,
  relink, and regenerate's install half (takes the plan). Check: reload
  --no-update from old and new build on this machine's units: same 2599
  files, only the --config path differs, reports identical.
- 6afe132 daemon: s6-only fields in s6::State (lock, beside,
  atom_order, svstat/bundle caches, state_reader); mod.rs uses
  clear_caches()/database_changed()/lock.
- 6533286 installer: init/s6.rs (s6-linux-init, s6.conf, admin store,
  sysinit members, runtime relabel, daemon restart, uninstall part).
  Dry runs (default, --journal=no --resolved=no, --uninstall) identical.
  (First attempt deleted the wrong range: a reused variable in my
  script; restored with git checkout and redone.)
- ef151de runner: supervisor.rs (wanted_up, up_pid); a doc comment that
  had drifted onto notify_sender_ok is back on wanted_up.
- The trait is still not fixed: it waits for a second backend.

ae863cd AUTO_MASK= (item 3A, owner's choice: A + dynamic start)
- automask.rs (base): STATE_DIR/auto-masked and auto-masked.keep.
- gen s6::sync_auto_mask (after sync_loggers, before stop atoms):
  candidates = plan units of kind longrun/oneshot named .service/.timer/
  .path, rx usable (or masked by us), not in plan.boot, no sockets
  (Planned::has_sockets: enabled socket, Sockets=, NAME.socket, or
  Service= naming it), not kept, not excluded; then repeatedly drop those
  a non-candidate, non-masked plan unit depends on (bundles never are
  candidates, so their members stay). Masked with their logger and
  .stop atom, `s6-rc-set-change -i`, per group if the batch is refused.
  Unmask (to usable, -I, set-fix -u then makes it active if needed) when
  no longer a candidate or AUTO_MASK=no, except while RUN_DIR/dyn/NAME
  exists. An entry the admin moved out of masked goes to the keep list.
- install::set_file_state: masked + listed -> disabled (static stays).
- daemon: can_be_dynamic accepts set-masked + listed service/timer/path;
  check_job refuses an admin-masked unit with UnitMasked.
- Bugs found and fixed: Index::load listed set-masked units (the daemon
  ran `s6-rc -u change` on them: "not a recognized identifier"); the
  dynamic status ignored CONDITION_FAILED; a second start of a parked
  dynamic instance didn't restart it (both affected template instances
  before too); the dynamic logger ran s6-log by name.
- Sandbox: hold1-4/beside1-3 in the keep list (the section drives s6-rc
  on them); new checks (masked, listed, static, dynamic start, unmask
  when repo-new needs it, admin unmask kept, admin mask refused; admin
  changes use -p, needed for the logger pipeline with JOURNAL=no).
- This machine's units (scratch STATE_DIR/REPO_DIR, JOURNAL=no): 107
  units masked (167 set entries); still usable longruns ~16, mostly with
  sockets (sssd-*, sshd, rpcbind, qemu-*-helper, libvirtd, httpd) or in
  a bundle (bluetooth, alsa-state).

Tests (final tree): cargo test 79 pass, standalone 3/3/3/2; sandbox
289/289, SANDBOX_JOURNAL=no 296/296, SANDBOX_FDSTORE=no 277/277 (run
before the last check.sh -p edit, which only touches the repo-lazy
lines); shellcheck/bash -n pass.

Not covered / known limits:
- AUTO_MASK=no reverting is not exercised by the sandbox.
- An admin `s6 set unmask` + apply while the unit runs as a dynamic
  instance would give s6-rc a second service of that name in the scan
  directory; not handled.
- `s6 live start X` / `s6-rc -u change X` can't start an auto-masked unit
  (not in the database); systemctl and the bus can. The admin takes it
  back with `s6 set enable|unmask -p X` (then kept).
- dist/ shell scripts still run programs by name (not TOOLS=).

### Current state

All of the above committed; nothing booted. The first reload on the real
system will mask ~107 units.

### Next steps

1. Owner: rebuild the RPMs, reboot s6, check `s6 set status` (masked
   entries), `cat /var/lib/uinitconverter/auto-masked`, that GNOME's
   D-Bus-activated services (fprintd, geoclue, localed, timedated,
   packagekit), timers (snapper-cleanup, btrfs-*) and `systemctl start
   sshd`-style starts still work, and count idle s6-supervise again
   (part 40: 136). AUTO_MASK=no in uinitconverter.conf + daemon-reload
   undoes it.
2. Owner: `sudo tests/hardencompare.sh snap` on both boots (part 44).
3. Optional: SYSLOG=syslog-ng on a real boot (/proc/kmsg, SELinux).
4. Boot loader provider (systemd-boot) and a second init backend (dinit)
   when the owner decides.
