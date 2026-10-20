# sdcompat

**English** | [한국어](README.md)

A systemd compatibility layer for s6. Fedora keeps its unit files, `systemctl`,
libsystemd and D-Bus clients, while **s6-linux-init + s6-rc** run the system.

```
 systemctl / dnf scriptlets / libsystemd clients / dbus-daemon activation
              │ org.freedesktop.systemd1 (system bus + /run/systemd/private)
              ▼
   sdcompat daemon ── jobs, unit state, enable/disable/preset, scopes
              │ s6-rc -u/-d, s6-svc, s6-svstat
              ▼
   s6-rc database  ◄── sdcompat reload: unit files → s6-rc source → compile
              │
   s6-supervise ── run/finish scripts call `sdcompat run|finish|up|down`
                   (User=, Environment=, sockets, NOTIFY_SOCKET, Restart=, ...)
```

It works at the protocol level, not by replacing libraries: the real `systemctl`
and `libsystemd` talk to it exactly as they would talk to systemd.

## Components (one binary, `sdcompat`)

| Command | Role |
|---|---|
| `sdcompat reload` | Translates every unit into an s6-rc source tree, compiles it together with your hand-written s6 services (`EXTRA_SOURCE`) and switches the live state (`s6-rc-update`). Writes a coverage report. |
| `sdcompat daemon` | Serves `org.freedesktop.systemd1` on the system bus **and** on `/run/systemd/private` (systemd ≥ 259's `systemctl` uses only that socket when run as root). |
| `sdcompat run/finish/up/down` | Called by the generated s6 scripts; does what systemd does between `fork()` and `exec()`. |
| `sdcompat timer-run` | Runs one `.timer` as an s6 longrun: computes elapses and asks the daemon to start the unit. |
| `sdcompat path-run` | Runs one `.path` unit as an s6 longrun, watching with inotify. |
| `sdcompat start/stop/restart UNIT` | Minimal client for the daemon (private socket or bus). |
| `sdcompat notify` | `systemd-notify` replacement. |
| `sdcompat log UNIT [-f]` | Shows a service's log (`journalctl -u`, or the s6-log directory with `JOURNAL=no`). |
| `sdcompat enable/disable/mask/preset/is-enabled` | Offline unit file management. |

## What is implemented

**Translation** (`.service` → longrun/oneshot, `.target` → bundle, `.timer`/`.path` → longrun, `.mount`/`.swap` → oneshot)
- `Requires=`, `Requisite=`, `BindsTo=`, and `Wants=` combined with `After=`/`Before=`, become s6-rc dependencies. Targets pull in all of their `Wants=`.
- `.wants/`, `.requires/` and `.upholds/` directories, aliases, templates/instances, masking, drop-ins, specifiers.
- Implicit dependencies: `DefaultDependencies=` → `sysinit.target`; `Type=dbus` → `dbus.socket`.
- Dependency cycles are broken (and reported) instead of failing the compile.
- `DEP_MAP` maps systemd units to hand-written s6 services. For example, `dbus.service` → `dbus`.
- An enabled `.socket` pulls in its service. The runner binds the sockets, matched by name or by `Service=`.
- `Conflicts=`: if both units would start at boot, the conflicted one is dropped from the targets (for example `gdm.service` over `getty@tty1.service`). At runtime, starting or restarting a unit stops the units it conflicts with.

**Mounts, swap, path units**
- `.mount` and `.swap` units run `mount(8)`/`swapon(8)`. Their state comes from the kernel (`mountinfo`, `/proc/swaps`), so `systemctl status tmp.mount` is correct whoever mounted it. `RequiresMountsFor=` becomes a dependency on the covering mount units.
- `.automount` units are mounted eagerly at boot (there is no autofs support).
- `/etc/fstab` is still mounted by the early `mount-fs` service, which `local-fs.target` contains.
- `.path` units: `PathExists=`, `PathExistsGlob=`, `DirectoryNotEmpty=` (re-checked while the unit is inactive), `PathChanged=`, `PathModified=` (inotify), `MakeDirectory=`, `TriggerLimitBurst=`.

**Timers**
- `OnCalendar=` with the full `systemd.time(7)` syntax: weekdays, lists, ranges, `/step` repetitions, `~` (days from the end of the month), the shorthands, and `UTC` or IANA time zones.
- `OnBootSec=`, `OnStartupSec=`, `OnActiveSec=`, `OnUnitActiveSec=`, `OnUnitInactiveSec=`.
- `Persistent=` (catches up a missed run), `RandomizedDelaySec=`, `FixedRandomDelay=`, `Unit=`.
- `systemctl list-timers` and `systemctl status x.timer` show next and last elapse.

**Failure semantics.** s6-rc has no soft dependencies: depending on a bundle
means depending on every service in it, so one failing wanted unit would stop
everything ordered after it. sdcompat therefore uses s6-rc only for ordering:
- every start is reported to s6-rc as successful, and the real result is
  recorded (`failed`, `timeout`, ...);
- a unit whose `Requires=`, `BindsTo=` or `Requisite=` failed is not started,
  and its job result is `dependency`, as with systemd;
- starting a unit retries its failed requirements, and re-checks conditions of
  a unit that was skipped earlier.

**Execution** (runner)
- `Type=simple|exec|notify|notify-reload|dbus|forking|oneshot|idle`. Readiness for each type is mapped onto s6's notification fd.
- `ExecStartPre/Post`, `ExecCondition`, `ExecStop`, `ExecStopPost`, `ExecReload`, with the `-@+!:` prefixes and `$VAR`/`${VAR}` expansion.
- `User=`, `Group=`, `SupplementaryGroups=`. `DynamicUser=` is approximated by a persistent system user.
- `PAMName=`: opens a PAM session, runs the service in a child process, and closes the session when it exits, like systemd. libpam is loaded at runtime.
- `Delegate=` and `DelegateSubgroup=`: the unit's cgroup is handed to the service user (needed by `systemd --user`).
- `Environment=`, `EnvironmentFile=`, `PassEnvironment=`, `UnsetEnvironment=`, `WorkingDirectory=`, `RootDirectory=`, `UMask=`, `Nice=`, `OOMScoreAdjust=`, `Limit*=`.
- `Runtime/State/Cache/Logs/ConfigurationDirectory=`.
- `StandardInput/Output/Error=` (null, tty, file:, append:, truncate:), `TTYPath=` and `TTYVHangup=`, which are enough for `getty@.service`.
- Socket activation for `Accept=no`: `ListenStream/Datagram/SequentialPacket/FIFO`, with `LISTEN_FDS`, `LISTEN_PID` and `LISTEN_FDNAMES` set.
- `Restart=` (all modes), `RestartSec=`, `SuccessExitStatus=`, `RestartPrevent/ForceExitStatus=`, `StartLimitBurst/IntervalSec=`.
- `OnFailure=`, `OnSuccess=`, `SuccessAction=`, `FailureAction=` (reboot/poweroff/halt).
- `FDSTORE=1` / `FileDescriptorStoreMax=`: the daemon holds stored fds and hands
  them back after a restart (journald and logind use this). Listening sockets
  are held the same way, so they stay open across service restarts.
- `TimeoutStartSec=` is enforced by the runner: on timeout the start fails and the service is terminated.
- Output goes to **systemd-journald** through its stdout stream protocol (as PID 1 does). journald attributes it to the unit from the cgroup, so `journalctl -u UNIT` and the log lines in `systemctl status` work. With `JOURNAL=no`, each service gets an s6-log directory instead.
- Conditions and asserts: `Condition*=`/`Assert*=` for paths, the kernel command line, virtualization, security and more.
- cgroup v2 at systemd's paths (`system.slice/<unit>`), `KillMode=`, and `MemoryMax/High`, `CPUQuota/Weight`, `TasksMax`, `IOWeight`.
- Sandboxing subset: `PrivateTmp`, `ProtectSystem`, `ProtectHome`, `Read{Write,Only}Paths`, `InaccessiblePaths`, `NoNewPrivileges`, `CapabilityBoundingSet`, `AmbientCapabilities`. Every other directive is listed as "ignored" in the report.
- seccomp: `SystemCallFilter=`, `SystemCallErrorNumber=` and `SystemCallArchitectures=`, with systemd's syscall groups (`@system-service`, ...; `src/syscall-groups.txt`, from `systemd-analyze syscall-filter`). libseccomp is loaded at run time.
- SELinux as under systemd: the s6 tree runs in `init_t` (policy module `dist/selinux/sdcompat.cil`), so services enter their own domains (`chronyd_t`, `xdm_t`, ...) and `user@.service` can start `systemd --user` in the user's domain. `RuntimeDirectory=` and the other directories, and socket files, get their file_contexts label; the sockets of socket units carry the label of the service that uses them.

**D-Bus API** (what `systemctl` and libraries call)
- Jobs: `Start/Stop/Restart/TryRestart/Reload/ReloadOrRestart…Unit`, with `JobNew`/`JobRemoved` signals and systemd's job results (`done`, `failed`, `dependency`, `timeout`). Isolate is supported.
- Queries: `GetUnit`, `LoadUnit`, `GetUnitByPID`, `ListUnits*`, `ListJobs`, `GetUnitProcesses`. Unit, Service, Target, Scope and Job properties are available, including `GetAll("")`.
- Unit files: `Enable/Disable/Reenable/Mask/Unmask/Preset*UnitFiles`, `GetUnitFileState`, `ListUnitFiles*`, `Get/SetDefaultTarget`.
- Manager: `Reload`/`Reexecute` (regenerate and update the s6 database), `KillUnit`, `ResetFailed*`, `Set/UnsetEnvironment`, `PowerOff/Reboot/Halt`.
- Transient units: `StartTransientUnit` for `.scope` and `.slice` units, with `PIDs` or `PIDFDs` (used by logind, podman and flatpak). Also `AttachProcessesToUnit`, `Scope.Abandon`, and the reference-counting calls (accepted).
- **`systemd-run`**: transient `.service`, `.timer` and `.path` units, including auxiliary units (`--on-calendar`/`--on-active` send a timer and its service). Properties become a unit file in `/run/systemd/transient/`. `systemd-run --wait` gets `PropertiesChanged` and the exit status. Finished transient units are unloaded.
- Template instances are created on demand without recompiling the database, for example `user@1000.service` and `user-runtime-dir@1000.service`. Their dependencies start first, and units that require a stopped unit are stopped with it.
- Starting `poweroff.target`, `reboot.target` or `halt.target` (which is how logind powers off) runs the configured shutdown command.
- D-Bus activation: answers dbus-daemon's `ActivationRequest` and replies with `ActivationFailure` when a start fails.
- Authorization: mutating calls need root, a uid in `ALLOW_UIDS`, or a polkit
  grant for systemd's action IDs (`manage-units`, `manage-unit-files`,
  `reload-daemon`, `set-environment`; the policy ships with systemd). Without
  polkit, non-root callers are denied. `ALLOW_UIDS` is root-equivalent.
- D-Bus activation is accepted only from the bus daemon itself, and never makes
  the daemon regenerate its database.
- Jobs on the same unit run one after another; queued jobs can be cancelled.

## Not implemented (yet)

- Encrypted volumes in `/etc/crypttab` that the initramfs does not unlock (systemd opens them with `systemd-cryptsetup@` units). The LUKS root disk is unlocked by the initramfs and works; `setup-fedora.sh` warns about the others.
- `Accept=yes` sockets, and on-demand socket activation: services are started eagerly, with their sockets.
- `systemd-run --pipe`/`--stdio` (passing file descriptors) on the system manager. `--pty`, `--wait`, `--scope`, `--on-*` and `--user` work.
- autofs: `.automount` units mount eagerly.
- `AccuracySec=` (timers fire on time instead of coalescing), `WakeSystem=`, `OnClockChange=`.
- Watchdog, credentials (`LoadCredential=`), `PrivateDevices/Network/Users`, `ProtectKernel*`, and the other seccomp-based settings (`RestrictAddressFamilies=`, `RestrictNamespaces=`, `MemoryDenyWriteExecute=`, `LockPersonality=`, `RestrictRealtime=`).
- Cancelling a job that is already running.
- `ExecStop=` only runs when a unit is stopped through the daemon (`systemctl stop`), not when s6-rc stops services directly, for example at shutdown.

## Offline updates and kernel updates

- **`dnf system-upgrade` / `dnf offline`:** they create `/system-update` and reboot. rc.init then
  boots the `system-update` bundle instead of `default`: early boot, D-Bus,
  sdcompat, logind, and `system-update.target`, whose wanted units run the
  transaction. This is the same switch systemd's system-update generator makes.
  `OnFailure=`/`SuccessAction=` in those units are honored, and the
  reboot at the end goes through sdcompat's `Reboot`.
- **New kernels:** `/etc/kernel/install.d/90-sdcompat.install` copies the BLS
  entry that Fedora's grub plugin writes for each kernel and adds the s6
  options from `/etc/sdcompat/boot-options`. The s6 entry follows kernel
  updates, and removed kernels lose theirs.

## Dependencies

**Required** (`dist/setup-fedora.sh` installs these):

| Package | Used for |
|---|---|
| skalibs, execline, s6, s6-rc, s6-linux-init | PID 1, supervision, the service database. Fedora doesn't package them: `--rpms=DIR` or `--build-s6` |
| s6-frontend (optional) | The `s6` command (see [The `s6` command](#the-s6-command-s6-frontend)); built with the others |
| systemd | Stays installed: systemctl, udevd, journald, logind, `systemd --user`, tmpfiles, sysusers, libsystemd |
| dbus-daemon | The system bus (`--systemd-activation`); dbus-broker stays for the user bus |
| util-linux | `mount`, `swapon`, `mountpoint` for mount units and early boot |
| policycoreutils | `restorecon` and `semodule` for the SELinux module |

**Libraries loaded at run time (recommended).** sdcompat `dlopen()`s them, so
none is needed to build. Without one, the matching feature is missing:

| Library (Fedora package) | Without it |
|---|---|
| `libpam.so.0` (pam) | Units with `PAMName=` fail (exit 224), so `user@.service` and GNOME do not start |
| `libselinux.so.1` (libselinux) | No labelling of `RuntimeDirectory=` & co., socket files and socket-unit sockets; services may get wrong labels |
| `libseccomp.so.2` (libseccomp) | `SystemCallFilter=` is skipped with a message, as in systemd without seccomp. fwupd refuses to start |

**Recommended packages:** earlyoom (replaces systemd-oomd), chrony,
NetworkManager, rsyslog (only with `JOURNAL=no`).

**Build:** cargo and rust. The crates are zbus, tokio, futures-util, libc
and serde (`Cargo.toml`).

## Testing on Fedora

Step-by-step instructions from a fresh clone to a GNOME session on s6, with
what to check and how to report problems: [TESTING-us.md](TESTING-us.md).

## Try it without root

Needs s6, s6-rc, execline, dbus-daemon, busctl and python3 (on Fedora, build
the skarnet RPMs first; see the next section). Nothing runs as root and the
host's init system is never touched.

```sh
cargo build
tests/sandbox.sh ~/sdc-sandbox        # private s6-svscan, s6-rc and D-Bus bus
tests/check.sh ~/sdc-sandbox          # 119 scenario checks, ending with an unprivileged-caller phase (once per sandbox)
tests/systemctl.sh status hello.service   # the real systemctl, in a namespace
tests/systemctl.sh list-units --all
tests/sandbox.sh ~/sdc-sandbox stop
```

`tests/systemctl.sh` uses the host's `systemctl`. On a distro without one, set
`SYSTEMD_ROOT=` to an extracted Fedora `systemd` + `systemd-shared` +
`systemd-libs` RPM. This setup was tested on Artix with Fedora 44's systemctl 259.5.

## Deploying on Fedora (do it in a VM first)

Fedora doesn't package skalibs, execline, s6, s6-rc, s6-linux-init or s6-frontend, so build
RPMs for them first (specs in `dist/rpm/`, sources checked against pinned SHA-256 sums):

```sh
dist/rpm/build-skarnet-rpms.sh                        # uses mock if you're in the mock group
dist/setup-fedora.sh --dry-run --rpms=$HOME/rpmbuild-skarnet/rpms
sudo dist/setup-fedora.sh --rpms=$HOME/rpmbuild-skarnet/rpms
```

`--build-s6` installs them from source into `/usr` instead (not tracked by rpm).

The script keeps the normal systemd boot as the default and **adds** a GRUB
entry, "Fedora (s6 + sdcompat)". Backups of anything it overwrites go to
`/var/lib/sdcompat/setup-backup/`. It does the following:

1. Installs `dbus-daemon`, `earlyoom`, `chrony`, `NetworkManager`, and the
   skarnet tools from your RPMs (`--rpms=DIR`) or from source (`--build-s6`).
   Their versions and checksums are pinned in one file, `dist/skarnet.versions`,
   which `build-skarnet-rpms.sh` and `--build-s6` both read: to move to new
   releases, edit that file only. execline is built as one multicall binary,
   `/usr/bin/execline`, so the SELinux module does not list its commands.
2. Builds and installs sdcompat. If the sdcompat RPM from `dist/sdcompat.spec` is installed, it uses that instead.
3. Writes `/etc/sdcompat/`: the config, the s6 services (early boot, dbus-daemon,
   the daemon, earlyoom) and the boot options, and `/etc/s6.conf` for the `s6` command. systemd-journald runs as an s6
   service. With `--no-journal`, an s6-only rsyslog config is used instead, so the
   systemd boot, where journald owns `/dev/log`, is unaffected.
4. Enables chronyd, NetworkManager and gdm, and sets `graphical.target` as the default.
5. Runs `sdcompat reload --no-update` and prints the translation summary.
6. Generates `/etc/s6-linux-init/current` with a rescue getty on tty9.
7. SELinux: labels `bin/init` as `init_exec_t` and loads the policy module
   `dist/selinux/sdcompat.cil`, which labels the s6, s6-rc, s6-linux-init and
   execline programs and sdcompat's binaries `init_exec_t`, so that the whole
   s6 tree stays in `init_t` as systemd does. It also allows the few things
   s6 does that systemd does not (the catch-all logger's fifo, links in
   `/run/service`, the run scripts of `user@UID.service` in /run/sdcompat).
   `--uninstall` removes the module.
8. Installs the kernel-install plugin and uses it to copy the default boot entry with `init=/usr/sbin/init-selinux` added. It removes the
   splash (`rhgb quiet`, `plymouth.enable=0`), because plymouth's quit unit is
   systemd-only. It also adds `enforcing=0`; pass `--enforcing` to leave that out.

Other options:

| Option | Effect |
|---|---|
| `--dry-run` | Print every command, change nothing (works without root) |
| `--yes` | Don't ask before starting |
| `--rpms=DIR` | Install the skarnet tools from RPMs built with `dist/rpm/build-skarnet-rpms.sh` |
| `--build-s6` | Build the skarnet tools from source into `/usr` instead (not tracked by rpm) |
| `--rebuild` | Rebuild sdcompat even if `target/release/sdcompat` exists |
| `--no-journal` | No systemd-journald: rsyslog plus one s6-log directory per service |
| `--enforcing` | Don't add `enforcing=0` to the s6 boot entry |
| `--no-grub` | Don't add the boot entry |
| `--uninstall [--remove-s6]` | Remove the entry and installed files (and the source-built skarnet tools); keeps your config |

A kernel-install plugin adds the s6 entry for every new kernel.

What systemd components are replaced, and why logind and `systemd --user` stay
(GNOME needs them; see the next section):

| Role | Under s6 + sdcompat |
|---|---|
| PID 1 / services | s6-linux-init, s6-rc, sdcompat |
| System bus | dbus-daemon (dbus-broker stays for the user bus) |
| Logging | systemd-journald as an s6 service (`--no-journal`: rsyslog + s6-log) |
| Network / DNS / time | NetworkManager, chrony |
| OOM handling | earlyoom instead of systemd-oomd |
| Devices | systemd-udevd as a plain service |
| Login, user manager | systemd-logind and `systemd --user`, as supervised services |

If stage 2 fails, log in on tty9 (Ctrl+Alt+F9) and read
`/run/uncaught-logs/current`. To boot the s6 database from before the last
change, add `sdcompat.db=previous` to the kernel command line; to skip
regenerating it at boot, add `sdcompat.reload=0`.

The skarnet tarballs are verified against pinned SHA-256 sums (taken from the
releases on 2026-10-18), both by the RPM build script and by `--build-s6`. When
built from source, the installed files are recorded in
`/var/lib/sdcompat/skarnet-files.txt` for `--uninstall --remove-s6`; RPM
installs are removed with `dnf remove` as usual.

### Running the tests on Fedora

All tests work unchanged on Fedora with the host `systemctl` and `systemd-run`.
On a host booted with systemd, they run in a namespace and never touch the real
systemd:

```sh
dist/rpm/build-skarnet-rpms.sh && sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm
sudo dnf install dbus-daemon python3 cargo
cargo build && tests/sandbox.sh ~/sdc && tests/check.sh ~/sdc
tests/systemctl.sh status notifier.service
```

To exercise what the unprivileged sandbox can't test (`User=`, cgroups,
sandboxing, transient scopes), run `sdcompat reload` with a config file in a
root-owned VM. The report and `sdcompat plan` show how Fedora's own units are
translated without starting anything:

```sh
sudo sdcompat --config /etc/sdcompat/sdcompat.conf plan | less
```

## GNOME and the user manager

GNOME 49 starts the whole session through a user service manager, and gnome-shell
places applications in transient scopes there. sdcompat therefore doesn't
reimplement `systemd --user`: it runs the **real user manager as an ordinary
supervised service**, and provides the PID 1 API that logind and the user
manager use. systemd is still not PID 1. What happens at login:

1. GDM (or `login`) runs the PAM stack. `pam_systemd` asks **systemd-logind** (generated from Fedora's unit) to create a session.
2. logind asks PID 1, which is sdcompat, to create `user-1000.slice` and `session-N.scope` (`StartTransientUnit` with `PIDFDs`), then to start `user@1000.service`.
3. sdcompat creates `user@1000.service` on demand. Its `BindsTo=user-runtime-dir@1000.service` dependency runs first. The runner opens the `systemd-user` PAM session (`PAMName=`), delegates `user.slice/user-1000.slice/user@1000.service` to UID 1000 with the process in `init.scope` (`Delegate=`, `DelegateSubgroup=`), and execs `/usr/lib/systemd/systemd --user`. Readiness comes from `Type=notify-reload`.
4. The user manager starts the session bus (dbus-broker user units) and GNOME's user targets. `systemctl --user`, `systemd-run --user` and app scopes all work inside the session, because that part is real systemd.
5. On logout, logind stops the scope and later `user@1000.service`. Power off and reboot from the GNOME menu go through logind, which starts `poweroff.target` or `reboot.target`; sdcompat runs the shutdown command for those.

Setup on top of the steps above:

```sh
dnf install gdm gnome-shell gnome-session   # Workstation already has these; keep dbus-broker installed for the user bus
systemctl enable gdm.service                 # display-manager.service alias
systemctl set-default graphical.target
```

**Status:** the D-Bus side of this flow (on-demand instances with `BindsTo=`,
conflicts, `Service=` sockets, shutdown targets) is covered by the sandbox
tests. The parts that need root (cgroup delegation, PAM sessions,
`StartTransientUnit` creating cgroups, the real logind and `systemd --user`)
are implemented but **untested**. Try them on Fedora in a VM. If login fails,
check `journalctl -u systemd-logind` and `journalctl -u user@1000.service`
(or `sdcompat log UNIT`, which also works with `JOURNAL=no`).

## The `s6` command (s6-frontend)

The setup script installs [s6-frontend](https://skarnet.org/software/s6-frontend/)
(version in `dist/skarnet.versions`), which provides the `s6` command, and
writes `/etc/s6.conf` (`dist/s6.conf`) so that it uses sdcompat's database
(`bootdb = /var/lib/sdcompat/compiled`). Service names
are the s6-rc names: `sshd.service`, `gdm.service`, … and sdcompat's own s6
services (`dbus`, `sdcompatd`, `earlyoom`, `udevd`, …).

| Command | What it does |
|---|---|
| `s6 live status` | every service with its type and state (no root needed) |
| `s6 live status sshd.service` | the state of the listed services |
| `sudo s6 process status sshd.service` | the supervised process (`s6-svstat`) |
| `sudo s6 process restart sshd.service` | restart the process only, not what depends on it (`s6-svc -r`) |
| `sudo s6 process kill -s HUP sshd.service` | send a signal to the process |
| `sudo s6 live start\|stop X` | `s6-rc -u\|-d change X`; for units, prefer `systemctl` (see below) |
| `sudo s6 system reboot\|poweroff` | the same s6-linux-init shutdown as `systemctl reboot\|poweroff` |

`s6 live start/stop` and `s6 process restart` act below sdcompat: there is no
systemd job, and `ExecStop=` does not run (as with `s6-rc` itself), so use
`systemctl` to start, stop and restart units.

**Do not use** `s6 repository …`, `s6 set …`, `s6 live install`,
`s6 live start-everything`/`stop-everything` or `s6 system boot`. They manage
s6-rc repositories and replace or restart the live database, while sdcompat
compiles that database from unit files itself (`systemctl daemon-reload`,
`sdcompat reload`).

## Configuration

`/etc/sdcompat/sdcompat.conf` (`KEY=VALUE`; see `dist/sdcompat.conf` for all keys
and defaults). The most important keys:

- `EXCLUDE`, `EXCLUDE_ADD`, `INCLUDE`: which units are translated.
- `DEP_MAP`, `DEP_MAP_ADD`: systemd unit → s6 service from `EXTRA_SOURCE`.
- `EXTRA_SOURCE`: hand-written s6-rc sources compiled into the same database.
- `JOURNAL`: `yes` (default) runs systemd-journald and sends service output to it; `no` uses per-service s6-log directories under `LOG_DIR`.
- `LOG_DIR`: where those s6-log directories go when `JOURNAL=no`.
- `ALLOW_UIDS`: non-root users allowed to manage units.
- `INCLUDE`: by default lets through the systemd services that work under sdcompat: logind, user-sessions, hostnamed, localed, timedated, resolved (DNS) and the sleep services.

## Source layout

```
src/unit.rs        unit file parser, word splitting, time spans
src/loader.rs      search path, aliases, templates, drop-ins, specifiers
src/gen.rs         plan → s6-rc source, compile, s6-rc-update, report
src/runner.rs      exec environment, readiness, restart policy
src/install.rs     enable/disable/mask/preset, unit file state
src/socket.rs      socket units (sd_listen_fds)
src/sandbox.rs     namespaces and capabilities subset
src/seccomp.rs     SystemCallFilter= (libseccomp, systemd's syscall groups)
src/selinux.rs     file and socket labels (libselinux)
src/cgroup.rs      systemd-compatible cgroup layout and resource control
src/conditions.rs  Condition*/Assert*
src/calendar.rs    OnCalendar= parser and next-elapse search
src/timer.rs       timer runner (one s6 longrun per .timer)
src/pathunit.rs    path unit runner (inotify)
src/mountunit.rs   .mount/.swap units
src/pam.rs         PAMName= via dlopen(libpam)
src/client.rs      small client for the daemon
src/fdstore.rs     file descriptor store (FDSTORE=1, held listening sockets)
src/daemon/        D-Bus API: dispatch, properties, s6 backend, private socket,
                   on-demand instances (dynamic.rs), systemd-run (transient.rs)
dist/              setup-fedora.sh, config, D-Bus policy, s6 services, early-boot helper,
                   s6-linux-init skeleton, kernel-install plugin, sdcompat.spec
dist/rpm/          specs and build script for the skarnet packages in dist/skarnet.versions
tests/             unprivileged sandbox, scenario checks, systemctl wrapper
```
