# sdcompat

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
| `sdcompat notify` | `systemd-notify` replacement. |
| `sdcompat log UNIT [-f]` | Shows a service's s6-log output (there is no journal). |
| `sdcompat enable/disable/mask/preset/is-enabled` | Offline unit file management. |

## What is implemented

**Translation** (`.service` → longrun/oneshot, `.target` → bundle)
- `Requires=`, `Requisite=`, `BindsTo=`, and `Wants=` combined with `After=`/`Before=`, become s6-rc dependencies. Targets pull in all of their `Wants=`.
- `.wants/`, `.requires/` and `.upholds/` directories, aliases, templates/instances, masking, drop-ins, specifiers.
- Implicit dependencies: `DefaultDependencies=` → `sysinit.target`; `Type=dbus` → `dbus.socket`.
- Dependency cycles are broken (and reported) instead of failing the compile.
- `DEP_MAP` maps systemd units to hand-written s6 services. For example, `dbus.service` → `dbus`.
- An enabled `.socket` pulls in its service. The runner binds the sockets.

**Execution** (runner)
- `Type=simple|exec|notify|notify-reload|dbus|forking|oneshot|idle`. Readiness for each type is mapped onto s6's notification fd.
- `ExecStartPre/Post`, `ExecCondition`, `ExecStop`, `ExecStopPost`, `ExecReload`, with the `-@+!:` prefixes and `$VAR`/`${VAR}` expansion.
- `User=`, `Group=`, `SupplementaryGroups=`. `DynamicUser=` is approximated by a persistent system user.
- `Environment=`, `EnvironmentFile=`, `PassEnvironment=`, `UnsetEnvironment=`, `WorkingDirectory=`, `RootDirectory=`, `UMask=`, `Nice=`, `OOMScoreAdjust=`, `Limit*=`.
- `Runtime/State/Cache/Logs/ConfigurationDirectory=`.
- `StandardInput/Output/Error=` (null, tty, file:, append:, truncate:), `TTYPath=` and `TTYVHangup=`, which are enough for `getty@.service`.
- Socket activation for `Accept=no`: `ListenStream/Datagram/SequentialPacket/FIFO`, with `LISTEN_FDS`, `LISTEN_PID` and `LISTEN_FDNAMES` set.
- `Restart=` (all modes), `RestartSec=`, `SuccessExitStatus=`, `RestartPrevent/ForceExitStatus=`, `StartLimitBurst/IntervalSec=`.
- Conditions and asserts: `Condition*=`/`Assert*=` for paths, the kernel command line, virtualization, security and more.
- cgroup v2 at systemd's paths (`system.slice/<unit>`), `KillMode=`, and `MemoryMax/High`, `CPUQuota/Weight`, `TasksMax`, `IOWeight`.
- Sandboxing subset: `PrivateTmp`, `ProtectSystem`, `ProtectHome`, `Read{Write,Only}Paths`, `InaccessiblePaths`, `NoNewPrivileges`, `CapabilityBoundingSet`, `AmbientCapabilities`. Every other directive is listed as "ignored" in the report.

**D-Bus API** (what `systemctl` and libraries call)
- Jobs: `Start/Stop/Restart/TryRestart/Reload/ReloadOrRestart…Unit`, with `JobNew`/`JobRemoved` signals and systemd's job results (`done`, `failed`, `dependency`, `timeout`). Isolate is supported.
- Queries: `GetUnit`, `LoadUnit`, `GetUnitByPID`, `ListUnits*`, `ListJobs`, `GetUnitProcesses`. Unit, Service, Target, Scope and Job properties are available, including `GetAll("")`.
- Unit files: `Enable/Disable/Reenable/Mask/Unmask/Preset*UnitFiles`, `GetUnitFileState`, `ListUnitFiles*`, `Get/SetDefaultTarget`.
- Manager: `Reload`/`Reexecute` (regenerate and update the s6 database), `KillUnit`, `ResetFailed*`, `Set/UnsetEnvironment`, `PowerOff/Reboot/Halt`.
- Transient units: `StartTransientUnit` for `.scope` units only (used by podman, flatpak and KDE).
- D-Bus activation: answers dbus-daemon's `ActivationRequest` and replies with `ActivationFailure` when a start fails.
- Authorization: mutating calls require root, or a uid listed in `ALLOW_UIDS`.

## Not implemented (yet)

- `.timer`, `.path`, `.mount`, `.automount`, `.swap` units. They are skipped and listed in the report. Mounts come from `/etc/fstab` (see `dist/early`).
- `Accept=yes` sockets, and on-demand socket activation: services are started eagerly, with their sockets.
- Transient `.service` units (`systemd-run`) and the per-user manager (`systemd --user`). GNOME needs the user manager; KDE, Xfce and Sway don't.
- The journal. Logs are in `/var/log/sdcompat/<unit>/current` (s6-log), so `journalctl` shows nothing.
- `dnf system-upgrade` and offline updates, because they depend on systemd's offline update target.
- Watchdog, file descriptor store, credentials (`LoadCredential=`), `PrivateDevices/Network/Users`, seccomp, `ProtectKernel*`.
- `ExecStop=` only runs when a unit is stopped through the daemon (`systemctl stop`), not when s6-rc stops services directly, for example at shutdown.

## Try it without root (any distro with s6, s6-rc and dbus-daemon)

```sh
cargo build
tests/sandbox.sh ~/sdc-sandbox        # private s6-svscan, s6-rc and D-Bus bus
tests/check.sh ~/sdc-sandbox          # 37 scenario checks over D-Bus
tests/systemctl.sh status hello.service   # the real systemctl, in a namespace
tests/systemctl.sh list-units --all
tests/sandbox.sh ~/sdc-sandbox stop
```

`tests/systemctl.sh` uses the host's `systemctl`. On a distro without one, set
`SYSTEMD_ROOT=` to an extracted Fedora `systemd` + `systemd-shared` +
`systemd-libs` RPM. This setup was tested on Artix with Fedora 44's systemctl 259.5.

## Deploying on Fedora (do it in a VM first)

The recommended setup **keeps the `systemd` package installed**. It still
provides `systemctl`, `systemd-udevd`, `systemd-tmpfiles`, `systemd-sysusers`,
`kernel-install` and libsystemd. Only PID 1 changes. Because the package stays,
dnf's dependencies and protected packages aren't affected. The systemd
package's own RPM file triggers (`systemctl daemon-reload`, `systemctl preset`)
reach sdcompat, so `dnf install` behaves as before.

1. **Install the pieces:** `s6`, `s6-rc`, `execline`, `s6-linux-init` (build from skarnet.org if your Fedora release lacks one), `dbus-daemon`, `elogind`, `rsyslog`, and sdcompat itself (`dist/sdcompat.spec`).
2. **Logging:** in `/etc/rsyslog.conf`, replace `imjournal` with `imuxsock`, because there is no journald.
3. **Early boot:** review `/etc/sdcompat/s6/` (the `sdcompat-sysinit` bundle: API mounts, udev, fstab, tmpfiles, sysusers, sysctl, modules, hostname, syslog). It replaces the systemd units that only work under systemd as PID 1, which are excluded by default (`systemd-*`).
4. **Login:** elogind provides `org.freedesktop.login1`. Add it as an s6 service in `/etc/sdcompat/s6`, and switch `pam_systemd.so` to `pam_elogind.so` with a custom authselect profile.
5. **Build the database once:** `sdcompat reload --no-update`, then read `/var/lib/sdcompat/current/report.txt`.
6. **Set up s6-linux-init:** copy `/usr/share/sdcompat/s6-linux-init/rc.init` into your s6-linux-init skel directory and run `s6-linux-init-maker` (boot bundle `default`).
7. **SELinux:** boot with `init=/usr/sbin/init-selinux` (it loads the policy, then execs s6-linux-init). Start in permissive mode, collect AVCs with `ausearch -m AVC -ts boot | audit2allow -M sdcompat_local`, and review them before loading.
8. **GRUB:** add an entry with that `init=` and keep the normal systemd entry as a fallback:
   ```sh
   grubby --add-kernel=/boot/vmlinuz-$(uname -r) --initrd=/boot/initramfs-$(uname -r).img \
          --copy-default --title="Fedora (s6 + sdcompat)" --args="init=/usr/sbin/init-selinux"
   ```

After boot, `systemctl status/start/stop/enable/list-units` work as usual,
`systemctl daemon-reload` regenerates the s6 database, and logs are read with
`sdcompat log sshd.service -f`.

### Running the tests on Fedora

All tests work unchanged on Fedora with the host `systemctl`. On a host booted
with systemd, they run in a namespace and never touch the real systemd:

```sh
sudo dnf install s6 s6-rc execline dbus-daemon   # if packaged; otherwise build them
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

## Configuration

`/etc/sdcompat/sdcompat.conf` (`KEY=VALUE`; see `dist/sdcompat.conf` for all keys
and defaults). The most important keys:

- `EXCLUDE`, `EXCLUDE_ADD`, `INCLUDE`: which units are translated.
- `DEP_MAP`, `DEP_MAP_ADD`: systemd unit → s6 service from `EXTRA_SOURCE`.
- `EXTRA_SOURCE`: hand-written s6-rc sources compiled into the same database.
- `LOG_DIR`: one s6-log pipeline per service (empty disables).
- `ALLOW_UIDS`: non-root users allowed to manage units.

## Source layout

```
src/unit.rs        unit file parser, word splitting, time spans
src/loader.rs      search path, aliases, templates, drop-ins, specifiers
src/gen.rs         plan → s6-rc source, compile, s6-rc-update, report
src/runner.rs      exec environment, readiness, restart policy
src/install.rs     enable/disable/mask/preset, unit file state
src/socket.rs      socket units (sd_listen_fds)
src/sandbox.rs     namespaces and capabilities subset
src/cgroup.rs      systemd-compatible cgroup layout and resource control
src/conditions.rs  Condition*/Assert*
src/daemon/        D-Bus API: dispatch, properties, s6 backend, private socket
dist/              D-Bus policy, config, s6 services, s6-linux-init, RPM spec
tests/             unprivileged sandbox, scenario checks, systemctl wrapper
```
