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
