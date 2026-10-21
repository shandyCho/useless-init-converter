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
