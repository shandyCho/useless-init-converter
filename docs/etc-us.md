# uinitconverter: technical reference

**English** | [한국어](etc.md)

Everything that isn't needed for a first install: how it works, what is and isn't implemented, what `initconvert init` does, configuration and the source layout. To install, start with the [README](../README-us.md).

## Architecture

```
 systemctl / dnf scriptlets / libsystemd clients / dbus-daemon activation
              │ org.freedesktop.systemd1 (system bus + /run/systemd/private)
              ▼
   initconvertd ── jobs, unit state, enable/disable/preset, scopes
              │ s6-rc -u/-d, s6-svc, supervise/status
              ▼
   s6-rc database  ◄── initconvert-gen reload: unit files → s6-rc store →
              │          repository set "current" (s6 set ...) → commit, install
              │
   s6-supervise ── run/finish scripts call `initconvert-runner run|finish|up|down`
                   (User=, Environment=, sockets, NOTIFY_SOCKET, Restart=, ...)
                   and `initconvert-timers timer-run|path-run`
```

It works at the protocol level, not by replacing libraries: the real `systemctl`
and `libsystemd` talk to it exactly as they would talk to systemd.

## Scope

The project's rules, in this order:

1. Use s6 and its ecosystem's features first.
2. A systemd feature another package can replace is replaced by it (earlyoom
   for systemd-oomd, rsyslog and s6-log for logging without journald,
   util-linux `rfkill` for systemd-rfkill).
3. Only what nothing but systemd can do is done by systemd itself (logind,
   `systemd --user`, journald, udevd, run as s6 services).
4. s6 is PID 1.
5. Follow the Unix philosophy: small tools that each do one thing, composed.
6. Shell code that Rust can replace is replaced by Rust (what s6 itself
   expects as a script, such as a service's `run`, stays one).

uinitconverter is a bridge from systemd units to s6, not a second systemd.
It implements only:

1. what the real systemd programs that still run need from PID 1
   (systemd-logind, `systemd --user`, systemd-journald): the
   `org.freedesktop.systemd1` calls they make, transient slices and scopes,
   on-demand `user@UID.service`, cgroup delegation, the shutdown targets;
2. what it takes to run Fedora's unit files under s6-rc.

Anything else is out of scope, even when it would be easy to add: "systemd
does it too" is not a reason. A feature is added only for a real caller
(a program or a Fedora unit that needs it), and that caller is named in
`work_history.md`. This is why the s6 user manager was removed (see
[below](#the-s6-user-manager-removed)): it reimplemented `systemd --user`
itself, which no caller needs.

Both large systemd-shaped parts are frozen at what they do now (audit of
2026-11-11, `work_history.md` part 216):

- **The D-Bus surface** (`org.freedesktop.systemd1`): no method or property
  is added without a program that calls it. Seen calling in the VM (GNOME
  session, logout and login, user switch, lingering, RPM transactions):

  | Caller | Calls |
  |---|---|
  | systemd-logind | `StartTransientUnit`, `Scope.Abandon`, `StartUnit`, `StopUnit`, `Subscribe`; `ActiveState`, `LoadState`, `Job.State` |
  | polkitd | `GetUnitByPIDFD`; `Service.NoNewPrivileges` |
  | uresourced | `SetUnitProperties` |
  | gsd-power | `Manager.Virtualization` |
  | RPM scriptlets (`systemd-update-helper`, `%systemd_*` through systemctl) | `Reload`, `ReloadUnit`, `EnqueueMarkedJobs`, `SetUnitProperties`, `ListUnitsByPatterns`, `PresetUnitFiles`, `EnableUnitFiles`, `DisableUnitFilesWithFlagsAndInstallInfo`, `GetUnit`, `NeedDaemonReload` |
  | systemd-run (logind's and GNOME's scopes, `--pipe`) | `StartTransientUnit`, `GetAll`, `InvocationID` |
  | uinitconverter's runner and timers | `LoadUnit`, `StartUnit`, `ActiveState` |
  | `systemd --user` | `AttachProcessesToUnit` |

  The other methods are the systemctl verbs an admin uses. One method is
  called and not implemented, on purpose: `EnqueueUnitJobMany` (systemctl
  from systemd-update-helper), which carries on without it.
  Methods no file in Fedora's `/usr` names were removed (part 217:
  `Manager.AbandonScope`, `GetJob`, `UnrefUnit`, `Unit.TryRestart`,
  `ReloadOrRestart`, `ReloadOrTryRestart`). Properties only `systemctl
  show` prints stay: it is how an admin reads a unit's settings.
- **The sandbox** (`uinitconverter-exec`): Fedora's own services use it
  (of 248 non-systemd services, 47 set `ProtectSystem=`, 32
  `SystemCallFilter=`, 15 `IPAddressDeny=`, 13 `DeviceAllow=`), and no
  systemd program can apply it outside PID 1, so it stays. No directive is
  added without a Fedora unit that needs it; directives it doesn't know are
  ignored and listed in the report.

## Compared with booting systemd

**There is little measurable benefit over booting with systemd. The main gain
is control:** you get Fedora and GNOME with s6 as PID 1, managed with s6's
tools. If you don't specifically want s6, plain systemd is the better choice.
The numbers and problems below are as of 2026-11-02, from one Fedora 45
Workstation machine. The systemd numbers are from boots up to 2026-10-31.

**Real benefits**

- **s6 is the init and the admin interface.** PID 1 is
  s6-linux-init/s6-svscan, not systemd. You manage services with `s6 set`,
  `s6 live` and the s6-rc repository.
- **Predictable boot.** The boot runs a compiled s6-rc database, and nothing
  is regenerated at boot. If a change goes wrong, `uinitconverter.db=previous`
  boots the database from before the last install.
- **More of systemd can be replaced.** uinitconverter's own initramfs removes
  systemd from the initramfs wherever it can boot the root (`--boot-only-s6=auto`,
  the default), journald is replaced by rsyslog and s6-log's
  plain-text logs (the default; `--journal=yes` runs journald), and NetworkManager handles DNS alone (the default;
  `--resolved=yes` runs systemd-resolved). The system bus is dbus-daemon with the default `--bus=daemon`.
- **Normal Fedora still works.** dnf, SELinux enforcing, GNOME and the real
  `systemctl` keep working. This is the advantage over a non-systemd distro
  like Artix, which isn't Fedora.

**Not real benefits**

| Area | Result on s6 compared with systemd |
|---|---|
| Memory | About the same: the median (total - available) is 1154 vs 1176 MiB (-23 MiB; 21 s6 and 6 systemd snapshots), about the normal 20-25 MiB variation between boots. |
| Boot speed | Perhaps faster, but not shown by a controlled test: from kernel start to the login screen's session opening, the median in the logs is about 19 vs 28 s (23 s6 and 11 systemd boots, excluding firmware and boot loader). Those boots ran on different days, with different builds and initramfs images, and the session opens before the login screen appears. |
| Security | Not better overall. PID 1 is smaller, but `initconvertd` and about 37 helper processes run as root, and the latest hardening comparison of 28 services shows no setting weaker on s6 (98 differ). It still flags bolt's seccomp filter count (9 vs 11): one filter here covers the three ABIs where systemd loads one per ABI. |
| Less systemd | Only partly. udevd, logind and `systemd --user` still run (GNOME needs logind and a user manager), and the `systemd` package stays installed. uinitconverter doesn't replace `systemd --user` (see [Scope](#scope)). |

**Costs compared with systemd**

- **Missing features:** generators, real autofs
  (automounts are mounted at boot instead), watchdog, credentials,
  and encrypted volumes in `/etc/crypttab` that the initramfs
  doesn't unlock. Details: [Not implemented (yet)](#not-implemented-yet).
- **Limited testing:** real boots have only been checked on one machine.
- **Known problems:** gnome-shell crashed during shutdown in 4 of 28 s6
  shutdowns (none of the 34 shutdown requests in the systemd journal). None has
  crashed since the shutdown order changed on 2026-10-31, in at least 14
  shutdowns, which is too few to call it fixed. One of those shutdowns had
  gnome-shell hang for 5 s until it was aborted (0 of 61 under systemd), cause
  unknown. `s6-ftrigrd` can crash during a reload that adds services, which is
  upstream behaviour: the reload retries and recovers, and a core file is left. With `JOURNAL=no`, the service logs under `/var/log/uinitconverter`
  are readable by root and the `wheel` group.
- **More to maintain:** about 29,000 lines of Rust, much of it running as
  root, plus the skarnet packages you have to build yourself.

## Components

uinitconverter is split by role. The **core** is one Cargo workspace (`crates/`):
its parts work together and may depend on each other. The **standalone
tools** (`standalone/`) are separate projects: each builds, installs and runs
on its own, and the core works without them.

**Core tools** (installed in `/usr/libexec/uinitconverter`):

| Tool | Commands | Role |
|---|---|---|
| `initconvert-gen` | `reload`, `plan` | Translates every unit into an s6-rc store, syncs the s6-rc repository with it and the hand-written stores (`EXTRA_SOURCE`), then commits the set `current` and installs it as the boot and live database (`s6-rc-set-install`). Writes a coverage report. On a running s6 boot `reload` asks initconvertd to do it, as `systemctl daemon-reload` does. See [The `s6` command](#the-s6-command-s6-frontend). |
| | `enable/disable/mask/unmask/preset/is-enabled` | Offline unit file management. |
| `initconvertd` | `daemon` | Serves `org.freedesktop.systemd1` on the system bus **and** on `/run/systemd/private` (systemd ≥ 259's `systemctl` uses only that socket when run as root). |
| `initconvert-runner` | `run/finish/up/down`, `stop-exec`, `reload-exec`, `stop-atom`, `confine`, `accept/accept-finish`, `connection` | Called by the generated s6 scripts and the daemon; does what systemd does between `fork()` and `exec()`. `stop-atom` is the `down` of a longrun's `UNIT.stop` oneshot (its `ExecStop=`). `confine UNIT -- COMMAND` gives a program s6 starts itself the unit's cgroup, limits and sandbox (the system bus with `--bus=broker`). `accept`/`accept-finish` are the `run`/`finish` of an `Accept=yes` socket (they exec s6-ipcserverd or s6-tcpserverd), which runs `connection` for each client. |
| `initconvert-timers` | `timer-run`, `path-run` | Runs one `.timer` (computes elapses) or `.path` unit (inotify) as an s6 longrun and asks the daemon to start the unit. |
| `initconvert` | any of the above (not `stop-atom`/`confine`/`accept`/`accept-finish`/`connection`) | The command users know (`/usr/bin/initconvert`): runs the tool that handles the command, so `initconvert reload` still works. `fdstore`, `journal-relay`, `linkwatch` and `notify` run the standalone tools when they are installed. |
| | `log UNIT [-f]` | A service's log, wherever this install keeps it: `journalctl -u`, or `tail` on its s6-log directory with `JOURNAL=no`. To start or stop a unit use `systemctl` or `s6-rc`; `initconvertd` itself is only the daemon. |
| | `rc stop-instances` | What `rc.shutdown` does first (`initconvert rc shutdown`): one `dbus-send` call asking the daemon to stop the on-demand instances (`user@UID`, ...) before s6-rc stops the services they are ordered after. |
| | `init` | Sets up the s6 boot entry (see [What `initconvert init` does](#what-initconvert-init-does)). |
| | `--version` | This build and each installed tool's build. Says so and exits 1 when the core tools are from different builds. Each tool also has `--version`. |

Each core tool is its own RPM package, and the packages depend on each other
with `Requires`. The source stays in the one workspace; the library crates
(`uinitconverter-base`, `-install`, `-exec`, `-client`) are used only to build and
are not packaged. The packages and how to build them:
[packages/README-us.md](../packages/README-us.md).

**Standalone tools** (optional, each its own project and RPM):

| Project | Role | Without it |
|---|---|---|
| `initconvert-fdstore` | Holds `FDSTORE=` fds and listening sockets in a process of its own. Reads no configuration or unit files. | initconvertd holds them itself; they are lost when initconvertd restarts. |
| `initconvert-journal-relay` | With `JOURNAL=no`: listens on journald's client sockets (`/run/systemd/journal/socket` and `stdout`) and forwards to rsyslog. dbus-broker, the session bus of `systemd --user`, refuses to start without them. | Needed for `JOURNAL=no`; nothing with `JOURNAL=yes` (the default). |
| `initconvert-linkwatch` | Unblocks a daemon-reload (`s6-rc-update`) or an `s6-svlink` that waits for a new supervisor's start event which came before it listened (an s6 race; the wait has no end and holds s6-rc's lock). Looks at `/proc` every 2 s and acts on a wait older than 10 s. | Such a reload hangs, and every s6-rc command after it, until the stuck `s6-rc-update` is killed. |
| `initconvert-notify` | `systemd-notify` replacement; works under any service manager that sets `$NOTIFY_SOCKET`. | Use `systemd-notify`, which stays installed. |

How each one is built, installed and run: [standalone/uinitconverter-fdstore](../standalone/uinitconverter-fdstore/README-us.md),
[standalone/uinitconverter-journal-relay](../standalone/uinitconverter-journal-relay/README-us.md),
[standalone/uinitconverter-linkwatch](../standalone/uinitconverter-linkwatch/README-us.md),
[standalone/uinitconverter-notify](../standalone/uinitconverter-notify/README-us.md).

## What is implemented

**Translation** (`.service` → longrun/oneshot, `.target` → bundle, `.timer`/`.path` → longrun, `.mount`/`.swap` → oneshot)
- `Requires=`, `Requisite=`, `BindsTo=`, and `Wants=` combined with `After=`/`Before=`, become s6-rc dependencies. Targets pull in all of their `Wants=`.
- `.wants/`, `.requires/` and `.upholds/` directories, aliases, templates/instances, masking, drop-ins, specifiers.
- Implicit dependencies: `DefaultDependencies=` → `sysinit.target`; `Type=dbus` → `dbus.socket`. Services with `BusName=` (and another `Type=`) are also ordered after `dbus.socket`: systemd starts them by bus activation, but here a socket unit may start them at boot.
- Dependency cycles are broken (and reported) instead of failing the compile.
- `DEP_MAP` maps systemd units to hand-written s6 services. For example, `dbus.service` → `dbus`.
- An enabled `.socket` pulls in its service. The runner binds the sockets, matched by name or by `Service=`. A service the boot pulls in only through its sockets (sockets.target wants `virtlogd.socket`, nothing wants `virtlogd.service`) is on demand: its runner only binds the sockets and waits (`run --on-demand`, "on demand" in `initconvert plan`).
- `Conflicts=`: if both units would start at boot, the conflicted one is dropped from the targets (for example `gdm.service` over `getty@tty1.service`). At runtime, starting or restarting a unit stops the units it conflicts with, and those that list it in their own `Conflicts=`.
- Job side effects as with systemd: stopping a unit also stops the running units that have it in `PartOf=`, `BindsTo=` or `Requires=` (database units and on-demand instances alike), and restarting it restarts those with `PartOf=`. A running unit with `StopWhenUnneeded=yes` is stopped once no running unit needs it (`Requires=`, `Requisite=`, `Wants=`, `BindsTo=`), also right after a job started it directly. A target's state is its own: a stopped target is inactive even while units it pulled in still run, and starting it again starts its wanted units that are no longer active (a oneshot that already ran). A `Type=notify` service that sends `STOPPING=1` is stopped, with what is bound to it (gnome-session's monitor ends a session this way). A `Type=notify` service with `NotifyAccess=all` may hand its main process over with `MAINPID=`, as podman's quadlet units do (`podman run -d` exits once conmon runs the container): its runner then stays as s6's process, forwards signals to the current main process, and the service ends when that one does. A start of a unit while another start of it is pending merges into that one, as systemd merges jobs: it gets the same result without running again (the GNOME leader and two `gnome-session-ctl` start `gnome-session-shutdown.target` at once). Unlike systemd, a restart does not propagate along `Requires=` (restarting dbus would restart everything that needs it).

**Mounts, swap, path units**
- `.mount` and `.swap` units run `mount(8)`/`swapon(8)`. Their state comes from the kernel (`mountinfo`, `/proc/swaps`), so `systemctl status tmp.mount` is correct whoever mounted it. `RequiresMountsFor=` becomes a dependency on the covering mount units.
- `.automount` units are mounted eagerly at boot (there is no autofs support).
- `/etc/fstab` is still mounted by the early `mount-fs` service, which `local-fs.target` contains.
- `.path` units: `PathExists=`, `PathExistsGlob=`, `DirectoryNotEmpty=` (re-checked while the unit is inactive), `PathChanged=`, `PathModified=` (inotify), `MakeDirectory=`, `TriggerLimitBurst=`.

**Timers**
- `OnCalendar=` with the full `systemd.time(7)` syntax: weekdays, lists, ranges, `/step` repetitions, `~` (days from the end of the month), the shorthands, and `UTC` or IANA time zones. Around a DST change it elapses as systemd does (a skipped time never, a repeated time once).
- `OnBootSec=`, `OnStartupSec=`, `OnActiveSec=`, `OnUnitActiveSec=`, `OnUnitInactiveSec=`.
- `Persistent=` (catches up a missed run), `RandomizedDelaySec=`, `FixedRandomDelay=`, `Unit=`.
- `systemctl list-timers` and `systemctl status x.timer` show next and last elapse.

**Failure semantics.** s6-rc has no soft dependencies: depending on a bundle
means depending on every service in it, so one failing wanted unit would stop
everything ordered after it. uinitconverter therefore uses s6-rc only for ordering:
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
  As in systemd, they are resolved for a start; a user that is gone by the stop (GDM's greeter user)
  only fails the ExecStop*= commands it would run (217/USER), not the stop.
- `PAMName=`: opens a PAM session, runs the service in a child process, and closes the session when it exits, like systemd. libpam is loaded at runtime.
- `Delegate=` and `DelegateSubgroup=`: the unit's cgroup is handed to the service user (needed by `systemd --user`).
- `Environment=`, `EnvironmentFile=`, `PassEnvironment=`, `UnsetEnvironment=`, `WorkingDirectory=`, `RootDirectory=`, `UMask=`, `Nice=`, `OOMScoreAdjust=`, `Limit*=`.
- `Runtime/State/Cache/Logs/ConfigurationDirectory=`.
- `StandardInput/Output/Error=` (null, tty, file:, append:, truncate:), `TTYPath=` and `TTYVHangup=`, which are enough for `getty@.service`.
- Socket activation for `Accept=no`: `ListenStream/Datagram/SequentialPacket/FIFO/Special`, with `LISTEN_FDS`, `LISTEN_PID` and `LISTEN_FDNAMES` set. A service that exits cleanly while it has sockets is started again on the next activity on them, as with systemd (systemd-rfkill exits when idle). Starting a `.socket` (`systemctl start`, udev's `SYSTEMD_WANTS`) starts its service with the socket.
- `Accept=yes` with one `ListenStream=`, a unix path (e.g. `polkit-agent-helper.socket`) or an IP port (e.g. `sshd.socket`): the socket is an s6 service of its own, where `s6-ipcserverd` (unix) or `s6-tcpserverd` (TCP, from the optional s6-networking package) accepts and starts one instance of the template per connection, named as systemd names it (`NAME@N-PID-UID.service`, or `NAME@N-LOCALIP:PORT-REMOTEIP:PORT.service`). TCP instances also get `REMOTE_ADDR` and `REMOTE_PORT`. The instance gets the connection on fd 3 (`LISTEN_FDS=1`, and `LISTEN_FDNAMES=` the socket's `FileDescriptorName=` or `connection`: systemd's varlink services such as `journalctl` serve only on an fd named `varlink`) and, with `StandardInput=`/`StandardOutput=socket`, on stdin/stdout. It runs `ExecStartPre=`, `ExecStart=`, `ExecStartPost=`, `ExecStop=` and `ExecStopPost=` with the template's user, sandbox and cgroup. Templates may be `Type=simple`, `exec` or `oneshot`. `MaxConnections=` (default 64) and `MaxConnectionsPerSource=` become the server's `-c`/`-C`, at most 1000 for unix sockets and 16384 for TCP. Without `s6-tcpserverd` a TCP socket is not generated, and `initconvert plan` says so. `systemctl status` shows `Accepted:` and `Connected:`. While a connection runs, its instance is a unit like any other: `systemctl list-units` lists it, `systemctl status` shows its main process, and `systemctl stop` sends it `KillSignal=` (SIGKILL after `TimeoutStopSec=`) and then runs `ExecStopPost=`. A main process ended by SIGTERM, SIGHUP, SIGINT or SIGPIPE counts as success, as with systemd. Stopping the socket leaves running connections alone, as with systemd. A socket that can't be bound (`sshd.socket` while `sshd.service` holds port 22: `Conflicts=` isn't enforced) is `failed` and isn't retried until it is started again.
- On-demand activation at boot: an on-demand service is `inactive` until the first activity on its sockets, as under systemd (iscsid, the libvirt daemons, cupsd, dmeventd, hostnamed). It then also starts the on-demand services it requires (`Requires=`/`BindsTo=`/`Wants=`) that are still waiting. A start by a job (`systemctl start`, a dependency, D-Bus activation, a `.path` unit) runs it at once; s6 restarting one that ran (`Restart=`, `systemctl restart`) doesn't wait either. After `systemctl stop` the sockets are closed, as before. The sockets are held by `initconvert-fdstore`; without it nothing can hold them before initconvertd is up, so the service starts at once.
- `Restart=` (all modes), `RestartSec=`, `SuccessExitStatus=`, `RestartPrevent/ForceExitStatus=`, `StartLimitBurst/IntervalSec=`.
- `OnFailure=`, `OnSuccess=`, `SuccessAction=`, `FailureAction=` (reboot/poweroff/halt).
- `FDSTORE=1` / `FileDescriptorStoreMax=`: stored fds are held and handed back
  after a restart (journald and logind use this). Listening sockets are held
  the same way, so they stay open across service restarts. The store is its
  own s6 service running the standalone `initconvert-fdstore` tool, so the fds
  also survive a restart of initconvertd; when that tool is not installed the
  daemon holds them itself.
  Services with `FileDescriptorStoreMax=` start after it.
- `TimeoutStartSec=` is enforced by the runner: on timeout the start fails and the service is terminated.
- Output goes to **systemd-journald** through its stdout stream protocol (as PID 1 does). journald attributes it to the unit from the cgroup, so `journalctl -u UNIT` and the log lines in `systemctl status` work. With `JOURNAL=no`, each service gets an s6-log directory instead.
- Conditions and asserts: `Condition*=`/`Assert*=` for paths, the kernel command line, virtualization, security and more.
- cgroup v2 at systemd's paths (`system.slice/<unit>`), `KillMode=`, and `MemoryMax/High`, `CPUQuota/Weight`, `TasksMax`, `IOWeight`. `MemoryPressureWatch=`/`MemoryPressureThresholdSec=` give the services `MEMORY_PRESSURE_WATCH`/`MEMORY_PRESSURE_WRITE` as systemd does (the cgroup's `memory.pressure`, `/dev/null` for `off`); without them sd-event daemons search /proc/pressure, which SELinux denies for some (systemd-hostnamed).
- Sandboxing: `PrivateTmp`, `ProtectSystem`, `ProtectHome`, `Read{Write,Only}Paths`, `InaccessiblePaths`, `NoNewPrivileges`, `CapabilityBoundingSet`, `AmbientCapabilities`, `PrivateDevices`, `PrivateNetwork`, `PrivateIPC`, `PrivateMounts`, `ProtectKernelTunables`, `ProtectKernelModules`, `ProtectKernelLogs`, `ProtectControlGroups`, `ProtectClock`, `ProtectHostname`, `ProtectProc`, `ProcSubset`, `PrivateUsers` (`self`, `identity`, `full`; `managed` needs systemd-nsresourced, so it gets the `self` mapping; if the namespace can't be set up the unit doesn't start, except on a kernel without user namespaces), with the same mounts, capability drops, syscall filters and implied `NoNewPrivileges=` as systemd. `PrivateNetwork=` also implies `PrivateMounts=` unless that is turned off and remounts `/sys` for the new network namespace, keeping the mounts below it; if the remount fails, the service still starts and logs it. Differences: each process of a unit (`ExecStartPre=`, main, `ExecStop=`) gets its own network/IPC/UTS namespace, where systemd shares one per unit; `ProtectControlGroups=private|strict` only make the cgroup tree read-only (no cgroup namespace). Every other directive is listed as "ignored" in the report.
- seccomp: `SystemCallFilter=`, `SystemCallErrorNumber=` and `SystemCallArchitectures=`, with systemd's syscall groups (`@system-service`, ...; `crates/uinitconverter-exec/src/syscall-groups.txt`, from `systemd-analyze syscall-filter`), and `RestrictAddressFamilies=`, `RestrictNamespaces=`, `MemoryDenyWriteExecute=` (the kernel's `PR_SET_MDWE` when it has it, as systemd), `LockPersonality=`, `RestrictRealtime=`, `RestrictSUIDSGID=`, with systemd's rules and errnos. As in systemd, `RestrictAddressFamilies=` doesn't cover the i386 ABI (its `socket()` goes through `socketcall()`). libseccomp is loaded at run time.
- cgroup BPF: `DevicePolicy=`, `DeviceAllow=` (`/dev/` nodes and `char-`/`block-` classes from /proc/devices, globs included; `PrivateDevices=` implies `DevicePolicy=closed`), `IPAddressDeny=` and `IPAddressAllow=` (addresses, prefixes, `any`, `localhost`, `link-local`, `multicast`). Like systemd, the runner attaches hand-assembled eBPF programs (`sd_devices`, `sd_fw_ingress`, `sd_fw_egress`) to the unit's cgroup before anything runs there, and a program the kernel refuses is logged (`... not enforced: ...`) without failing the start. Differences: the address lists are compiled into the program instead of BPF maps, and lists set on slices are not applied.
- SELinux as under systemd: the s6 tree runs in `init_t` (policy module `dist/selinux/uinitconverter.cil`), so services enter their own domains (`chronyd_t`, `xdm_t`, ...) and `user@.service` can start `systemd --user` in the user's domain. `RuntimeDirectory=` and the other directories, and socket files, get their file_contexts label; the sockets of socket units carry the label of the service that uses them.

**D-Bus API** (what `systemctl` and libraries call)
- Jobs: `Start/Stop/Restart/TryRestart/Reload/ReloadOrRestart…Unit`, with `JobNew`/`JobRemoved` signals and systemd's job results (`done`, `failed`, `dependency`, `timeout`). Isolate is supported (without `AllowIsolate=` and `IgnoreOnIsolate=`, see [Not implemented](#not-implemented-yet)).
- Queries: `GetUnit`, `LoadUnit`, `GetUnitByPID`, `GetUnitByPIDFD` (polkitd asks it for every caller), `ListUnits*`, `ListJobs`, `GetUnitProcesses`. Unit, Service, Target, Scope and Job properties are available, including `GetAll("")`, in systemd 262's order (the Unit interface first, then each interface's own; `systemctl show -p A,B` prints in that order). `PropertiesChanged` (`ActiveState`, `SubState`) is sent for every unit whose state changes, also without a job on it (a service that exits, a stop through `Conflicts=` or `PartOf=`), within about a second.
- Unit files: `Enable/Disable/Reenable/Mask/Unmask/Preset*UnitFiles`, `GetUnitFileState`, `ListUnitFiles*`, `Get/SetDefaultTarget`.
- Manager: `Reload`/`Reexecute` (regenerate and update the s6 database), `KillUnit`, `ResetFailed*`, `Set/UnsetEnvironment`, `PowerOff/Reboot/Halt`.
- The units a service runs are those its s6-rc database was generated from, as systemd runs the units it loaded until `daemon-reload`: gen writes each unit into the database (a longrun's `data/unit`; every other unit and the index in the `data/` of the `uinitconverter-units` service, which is `usable` in the set and never runs), and the runner, the timers and initconvertd read them from the live database. A unit file edited without `systemctl daemon-reload` doesn't change what the next start runs, nor what `systemctl show` reports, and `systemctl status` warns that the unit file changed on disk (`NeedDaemonReload`, from the loaded files' paths and mtimes); units the database doesn't carry are loaded from disk on demand, as systemd loads any unit, and kept until the next reload. A rollback boot (`uinitconverter.db=previous`) runs the older database's units and reads its index. Units the daemon starts outside the database (template instances such as `user@UID.service`, `systemd-run` units, a socket started by hand) are written when it starts them, to `RUN_DIR/units/ID/snapshot`, and loaded again at each reload. The `confine` command of the hand-written s6 services (`dist/s6`) reads its unit file, which isn't in the database. Without a live database, or with one generated by an older version, the unit files are read as before.
- Package installs and updates: the systemd RPM's file triggers run `systemctl daemon-reload`, which regenerates the s6 database, so a newly installed unit gets its s6 service right away (with dnf, GNOME Software or PackageKit alike); the new service starts as enabled or disabled in the s6 set as systemd's preset made it. On an update, the package's scriptlets mark its units with `Markers=+needs-restart`/`+needs-reload` (`SetUnitProperties`), and `systemctl enqueue-marked` (`EnqueueMarkedJobs`) then restarts the running ones or reloads them, as with systemd. For user units, the file trigger reloads `user@*.service` (the user manager re-executes on `ReloadSignal=`; the reload waits for its `RELOADING=1`/`READY=1`, as with `Type=notify-reload` everywhere) and runs `systemctl --user -M UID@ enqueue-marked` in each user manager, which goes through `systemd-run --pipe` (stdio passed as file descriptors) and `systemd-stdio-bridge`.
- Transient units: `StartTransientUnit` for `.scope` and `.slice` units, with `PIDs` or `PIDFDs` (used by logind, podman and flatpak). Also `AttachProcessesToUnit`, `Scope.Abandon`, and the reference-counting calls (accepted). Scopes and slices are recorded in `/run/uinitconverter/units`, so a restarted daemon takes them over (session scopes keep working).
- **`systemd-run`**: transient `.service`, `.timer` and `.path` units, including auxiliary units (`--on-calendar`/`--on-active` send a timer and its service). Properties become a unit file in `/run/systemd/transient/`. `systemd-run --wait` gets `PropertiesChanged` and the exit status. Finished transient units are unloaded.
- Template instances are created on demand without recompiling the database, for example `user@1000.service` and `user-runtime-dir@1000.service`. Their dependencies start first, and units that require a stopped unit are stopped with it. At shutdown, the active oneshot instances are stopped, so their `ExecStop=` runs (systemd-backlight@ saves the brightness); they are recorded in `/run/uinitconverter/dyn-oneshot`, so this also holds after the daemon was restarted. A restarted daemon also takes over the running instances (their service directories in `/run/uinitconverter/dyn`), so it reports and stops them as before. An instance whose service ended on its own (`Restart=` done with it) loses its `s6-supervise` and logger after 10 s: its result (`inactive`/`failed`) still shows, units that require it keep running, as under systemd, and the next start creates it again.
- `ExecStop=` of a long-running service also runs when s6-rc stops it directly: at shutdown, `s6 live stop`, `s6 live reset`, a package update removing it. Each such service gets an s6-rc oneshot `UNIT.stop` that depends on it (so s6-rc takes it down first) and runs the `ExecStop=` with `$MAINPID` in its `down`, within `TimeoutStopSec=`. The daemon brings `UNIT.stop` up whenever s6-rc has the service up, and `systemctl stop` lets it run the `ExecStop=` (once). Oneshots with `RemainAfterExit=yes` always ran theirs from their own `down`. Differences: units that depend on the service are stopped beside its `ExecStop=`, not before it (nothing depends on `UNIT.stop`: on an upgrade s6-rc-update would restart every unit that gained a dependency on a new service); and `s6 process restart`/`s6-svc` act on the process alone, so no `ExecStop=` runs then. On-demand instances that have an `ExecStop=` (longruns too) are stopped by the daemon at shutdown.
- Starting `poweroff.target`, `reboot.target` or `halt.target` (which is how logind powers off) runs the configured shutdown command.
- D-Bus activation: answers dbus-daemon's `ActivationRequest` and replies with `ActivationFailure` when a start fails. With `--bus=broker`, dbus-broker-launch starts services through `StartUnit` (with `SystemdService=`) or `StartTransientUnit` (a transient `dbus-:1.N-NAME@K.service` for the others) and watches `JobRemoved` and the unit's `ActiveState`, as with systemd.
- Authorization: mutating calls need root, a uid in `ALLOW_UIDS`, or a polkit
  grant for systemd's action IDs (`manage-units`, `manage-unit-files`,
  `reload-daemon`, `set-environment`; the policy ships with systemd). Without
  polkit, non-root callers are denied. `ALLOW_UIDS` is root-equivalent.
  Calls on one unit pass polkit the `unit` and `verb` details as systemd does,
  so rules that match on them work. As in systemd, `AttachProcessesToUnit`
  with an empty unit name means the caller's own unit, and an unprivileged
  caller in a delegated unit (`Delegate=yes` or a controller list) that runs
  as the caller may move its own processes into it, also from outside it
  (a user manager moving an app started from an ssh login into its app
  scope).
- SELinux access checks as systemd 262 makes them, before polkit and for
  root too: the caller's label (from the bus, or `SO_PEERSEC` on the private
  socket) needs the method's permission in class `service` on the unit
  file's label (`start`, `stop`, `reload`, `status`), or in class `system` on
  initconvertd's own label (`reload`, `status`, `halt`, `reboot`, `enable`,
  `start`) for manager calls and units without a unit file. `ListUnits`
  leaves out the units the caller may not see. Permissive mode and
  permissive domains only log the denial (to initconvertd's stderr).
- D-Bus activation is accepted only from the bus daemon itself, and never makes
  the daemon regenerate its database.
- Jobs on the same unit run one after another; queued jobs can be cancelled.
- Units wanted by devices (udev's `SYSTEMD_WANTS`, e.g. `bluetooth.target` for a
  Bluetooth adapter, `sound.target`, `bolt.service`) start when the device
  appears, from udevd's events and, at startup, from udev's database. Excluded
  units among them (`systemd-backlight@`, `systemd-rfkill`,
  `systemd-vconsole-setup`) are skipped.

## Not implemented (yet)

- Encrypted volumes in `/etc/crypttab` that the initramfs does not unlock (systemd opens them with `systemd-cryptsetup@` units). The LUKS root disk is unlocked by the initramfs and works; `initconvert init` warns about the others.
- `Accept=yes` with more than one listener, anything but `ListenStream=` (or `vsock:`), and with a `Type=notify`, `forking` or `dbus` template: the socket is skipped, and `initconvert plan` gives the reason. An instance disappears when its connection ends, also when it failed: systemd keeps failed ones until `systemctl reset-failed`, so here they don't count in `systemctl --failed` (their result is in the socket's log and `ExecStopPost=`). Restarting or reloading a running connection fails. The template's `Wants=` and `Requires=` are started with the socket rather than per connection.
- `Accept=no`: starting a `.socket` at run time still starts its service at once, and an on-demand service's requirements that are not on demand themselves are started at boot (s6-rc dependencies).
- autofs: `.automount` units mount eagerly.
- systemd generators: none of the system's runs on the s6 boot (the user ones run in `systemd --user`). `/etc/fstab` is mounted by the `mount-fs` s6 service and the zram swap set up by the `zram` one; units that other generators would make (e.g. `systemd-gpt-auto-generator`'s, `rc-local`) don't exist there.
- `AccuracySec=` (timers fire on time instead of coalescing), `WakeSystem=`, `OnClockChange=`, `OnTimezoneChange=`. These timer keys are accepted without effect, so the report doesn't list them as ignored.
- `ReadWriteOnly=` in `.mount` units: accepted without effect, so the report doesn't list it as ignored. A mount that can't be read-write falls back to read-only, util-linux `mount`'s default (systemd's `ReadWriteOnly=no`). No Fedora unit sets it.
- The console settings of getty-style units (`getty@`, `serial-getty@`, `kmsconvt@`, `console-getty`, `container-getty@`): `TTYReset=` (the terminal isn't reset before the program starts), `TTYVTDisallocate=` (a VT's scrollback isn't cleared when the program exits), and `UtmpIdentifier=`/`UtmpMode=` (the service manager writes no utmp/wtmp records; agetty and login write their own, but the logout record systemd writes when the program exits is missing for a text-console login). GDM writes its own records, so graphical logins are not affected. Accepted without effect, so the report doesn't list them as ignored.
- `AllowIsolate=` and `IgnoreOnIsolate=`: `systemctl isolate` works on any target and stops every s6-rc service outside it, also one with `IgnoreOnIsolate=yes` (`rpc-statd.service`, for one). systemd refuses a target without `AllowIsolate=yes` and leaves those running.
- `RestartMode=` (Fedora's malcontent services set `RestartMode=debug`), `SyslogFacility=` and `TimeoutAbortSec=`: accepted without effect.
- Watchdog, credentials (`LoadCredential=`, `ImportCredential=`), `PrivatePIDs=`, `RemoveIPC=`, `KeyringMode=`, `RestrictFileSystems=` (a BPF LSM program in systemd), and IP accounting (`IPAccounting=`).
- Cancelling a job that is already running.
- Job modes: jobs on a unit run one after another, so `replace` cancels only a conflicting job that is still waiting, not a running one (systemd stops a running start). `ignore-dependencies` and `ignore-requirements` start a unit without its dependencies only when it runs outside s6-rc (instances, transient and user units); a unit in the s6-rc database still gets the dependencies s6-rc compiled in.
- `PrivateNetwork=` in `[Socket]`, `JoinsNamespaceOf=` and `NetworkNamespacePath=`.
- `KillMode=control-group` at the s6 boot's shutdown: s6-rc stops the database's services itself there, so a service's other processes get the signal only after its main process has exited (stops through the daemon signal them together as systemd does).
- Unit settings of the services with their own s6 scripts in `dist/s6` (`dbus`, `udevd`, `earlyoom`, `syslog`, `coredump`): they take the cgroup, limits, sandbox and user from the unit, but the command line and the restart and stop behaviour are in the script. So drop-ins that change `ExecStart=`, `Restart=`, `KillMode=`, `TimeoutStopSec=` or `Type=` don't reach them.

## Offline updates and kernel updates

- **`dnf system-upgrade` / `dnf offline`:** they create `/system-update` and reboot. rc.init then
  boots the `system-update` bundle instead of `default`: early boot, D-Bus,
  uinitconverter, logind, and `system-update.target`, whose wanted units run the
  transaction. This is the same switch systemd's system-update generator makes.
  `OnFailure=`/`SuccessAction=` in those units are honored, and the
  reboot at the end goes through uinitconverter's `Reboot`.
- **New kernels:** the kernel-install plugin
  `/usr/lib/kernel/install.d/90-uinitconverter.install` copies the BLS
  entry that Fedora's grub plugin writes for each kernel and adds the s6
  options from `/etc/uinitconverter/boot-options`. The s6 entry follows kernel
  updates, and removed kernels lose theirs.

## Dependencies

**Required** (the `uinitconverter` packages require these, so dnf installs them):

| Package | Used for |
|---|---|
| skalibs, execline, s6, s6-rc, s6-linux-init | PID 1, supervision, the service database. Fedora doesn't package them, and uinitconverter's releases don't include them: build them with `dist/rpm/build-skarnet-rpms.sh` |
| s6-frontend (recommended) | The `s6` command (see [The `s6` command](#the-s6-command-s6-frontend)); built with the others |
| mdevd (recommended) | Loads the drivers in the s6 initramfs (`--boot-only-s6=yes`); built with the others |
| systemd | Not a package dependency (Fedora's base system has it), but it must stay installed: systemctl, udevd, journald, logind, `systemd --user`, tmpfiles, sysusers, libsystemd |
| dbus-daemon | The system bus (`--systemd-activation`), or dbus-broker with `--bus=broker`; dbus-broker stays for the user bus |
| util-linux | `mount`, `swapon`, `mountpoint` for mount units and early boot; `rfkill` for the radio state |
| shadow-utils | `useradd` for the system users that stand in for `DynamicUser=` |
| policycoreutils, policycoreutils-python-utils | `restorecon` and `semodule` for the SELinux module; `semanage` for `initconvert init` |
| grubby | The boot entry (`initconvert init`) |

**Libraries loaded at run time (recommended).** uinitconverter `dlopen()`s them, so
none is needed to build. Without one, the matching feature is missing:

| Library (Fedora package) | Without it |
|---|---|
| `libpam.so.0` (pam) | Units with `PAMName=` fail (exit 224), so `user@.service` and GNOME do not start |
| `libselinux.so.1` (libselinux) | No labelling of `RuntimeDirectory=` & co., socket files and socket-unit sockets; services may get wrong labels. No SELinux access checks on D-Bus callers |
| `libseccomp.so.2` (libseccomp) | `SystemCallFilter=` is skipped with a message, as in systemd without seccomp. fwupd refuses to start |

**Recommended packages:** earlyoom (replaces systemd-oomd), chrony,
NetworkManager, rsyslog or syslog-ng (only with `JOURNAL=no`).

**Build:** cargo and rust. The core uses zbus, tokio, futures-util, libc,
rustix, jiff and serde (`Cargo.toml`); the standalone tools libc and rustix
(`initconvert-linkwatch` only rustix).

## Testing on Fedora

Step-by-step instructions from a fresh clone to a GNOME session on s6, with
what to check and how to report problems: [TESTING-us.md](../TESTING-us.md).

## Try it without root

Needs s6, s6-rc, execline, dbus-daemon, busctl and python3 (on Fedora, build
the skarnet RPMs first; see the [README](../README-us.md#installation)). Nothing runs as root and the
host's init system is never touched.

```sh
cargo build                           # the core; sandbox.sh builds the standalone tools
tests/sandbox.sh ~/sdc-sandbox        # private s6-svscan, s6-rc and D-Bus bus
tests/check.sh ~/sdc-sandbox          # about 310 scenario checks, ending with an unprivileged-caller phase (once per sandbox)
tests/systemctl.sh status hello.service   # the real systemctl, in a namespace
tests/systemctl.sh list-units --all
tests/sandbox.sh ~/sdc-sandbox stop
```

`tests/systemctl.sh` uses the host's `systemctl`. On a distro without one, set
`SYSTEMD_ROOT=` to an extracted Fedora `systemd` + `systemd-shared` +
`systemd-libs` RPM. This setup was tested on Artix with Fedora 44's systemctl 259.5.

## What `initconvert init` does

The project was called **sdcompat** before. To move an sdcompat install to
uinitconverter (remove, then install as new), see "Moving from sdcompat" in
[packages/README-us.md](../packages/README-us.md#moving-from-sdcompat).

`initconvert init` **adds** a GRUB entry, "Fedora (s6 + uinitconverter)", and keeps the
normal systemd boot as the default unless you pass `--default-s6`. It installs
nothing: when a setting needs a package that is missing, it stops and prints
the `dnf install` command. Backups of anything it overwrites go to
`/var/lib/uinitconverter/setup-backup/`. It shows its plan, asks, and then:

1. Shows the installed builds (`initconvert --version`): the core packages must
   come from one build.
2. Writes the settings in `/etc/uinitconverter/` (`JOURNAL=`, `RESOLVED=` in
   `uinitconverter.conf`, the s6 services in the `uinitconverter-sysinit` bundle) and
   `/etc/s6.conf` for the `s6` command. The package installs the s6 services
   (early boot, dbus-daemon, the daemon, earlyoom). By default (`JOURNAL=no`)
   an s6-only rsyslog config is used instead of systemd-journald, so the systemd boot, where journald owns `/dev/log`, is
   unaffected; the s6 service `journal-relay` then serves journald's client
   sockets and forwards to rsyslog (only MESSAGE, priority and identifier are
   kept).
3. Enables chronyd and NetworkManager when they are installed (a missing one
   is only a warning) and, when gdm is installed, enables gdm and sets
   `graphical.target` as the default.
4. Runs `initconvert reload --no-update`, which creates the s6-rc repository and its set `current` (from systemd's enable state) and installs the boot database, and prints the translation summary. It also creates `/etc/s6/sources` for your own s6-rc services.
5. Generates `/etc/s6-linux-init/current` with a rescue getty on tty9.
6. SELinux: labels `bin/init` as `init_exec_t` and loads the policy module
   (`/usr/share/uinitconverter/selinux/uinitconverter.cil`, from `dist/selinux/`), which labels the s6, s6-rc, s6-linux-init and
   execline programs and uinitconverter's binaries `init_exec_t`, so that the whole
   s6 tree stays in `init_t` as systemd does. It also allows the few things
   s6 does that systemd does not (the catch-all logger's fifo, links in
   `/run/service`, the run scripts of `user@UID.service` in /run/uinitconverter).
   The package loads the module when it is installed and removes it when it is removed.
7. On the s6 boot, restarts initconvertd so the installed build takes over.
8. Runs the package's kernel-install plugin for every installed kernel, which copies the boot entry with `init=/usr/sbin/init-selinux` added. It removes the
   splash (`rhgb quiet`, `plymouth.enable=0`), because plymouth's quit unit is
   systemd-only. On the first run it also adds `enforcing=0`; `--selinux=enforcing` leaves that out.

Options:

| Option | Effect |
|---|---|
| `--dry-run` | Print every command, change nothing (works without root). It ends with the command that applies it: `sudo initconvert init` with the same options minus `--dry-run` |
| `-y`, `--yes` | Don't ask before starting |
| `--journal=yes\|no` | `no`: no systemd-journald, a syslog daemon plus one s6-log directory per service (needs the `--syslog` package and `uinitconverter-journal-relay`) / `yes`: back to journald |
| `--syslog=rsyslog\|syslog-ng` | The syslog daemon with `--journal=no` (default rsyslog). Its configuration goes to `/etc/uinitconverter/rsyslog.conf` or `syslog-ng.conf`; the distribution's stays as it is. Both write the files of Fedora's rsyslog defaults (`/var/log/messages`, `secure`, `maillog`, `cron`, …) |
| `--resolved=yes\|no` | `no`: no systemd-resolved on the s6 boot, NetworkManager handles DNS itself / `yes`: back to resolved |
| `--bus=daemon\|broker` | The s6 boot's system bus: dbus-daemon (default) or dbus-broker as on the systemd boot. dbus-daemon stays installed: the broker falls back to it when dbus-broker isn't installed, journald's socket (journald, or journal-relay with `JOURNAL=no`) isn't there after 10 s, or `dbus-broker-launch` has exited three times in this boot |
| `--selinux=enforcing\|permissive` | SELinux mode of the s6 entry (`permissive` adds `enforcing=0`). The first run uses permissive |
| `--boot-only-s6=yes\|no\|auto` | `yes`: boot the s6 entry with uinitconverter's own initramfs instead of Fedora's, so no systemd runs at all (see below; needs `mdevd`) / `no`: Fedora's / `auto` (the default, rules 2-4): `yes` unless the root needs what that initramfs can't do (LVM, MD, multipath, a network root, a separate `/usr`, `resume=`, a TPM2/FIDO2/PKCS#11 LUKS key, no `mdevd`); init says why when it keeps Fedora's |
| `--default-s6` / `--default-systemd` | Make the s6 entry the default boot entry, also for kernels installed later / make the systemd entry the default again. The systemd entry stays in the menu either way |
| `--grub=yes\|no` | `no`: don't add or update the boot entry (not kept for the next run) |
| `--uninstall` | Remove the boot entries, the s6 initramfs images, `/etc/s6-linux-init/current`, `/etc/s6.conf` (if init wrote it) and the boot settings; keeps `/etc/uinitconverter`. Then `dnf remove` the packages |

An option left out keeps what the last run set: `JOURNAL=`/`RESOLVED=` in
`/etc/uinitconverter/uinitconverter.conf`, the SELinux mode in
`/etc/uinitconverter/boot-options`, `/etc/uinitconverter/boot-only-s6` (or `boot-only-s6.off` for an explicit `no`; neither: `auto`) and
`/etc/uinitconverter/boot-default`. So a plain `sudo initconvert init` after a package
upgrade redoes the setup with the same choices. init writes `JOURNAL=` and `RESOLVED=` with either value, `no` too: a key that is missing is what the package's update takes for an install from before `no` became the default, and it writes `yes`.

The journal and resolved options also switch an existing install. With `--resolved=no`, the s6 oneshot `resolv` writes `/run/NetworkManager/conf.d/99-uinitconverter-no-resolved.conf` (`dns=default`, `rc-manager=symlink`) at boot and points the resolved stub file that `/etc/resolv.conf` links to at `/run/NetworkManager/resolv.conf`. Everything is in `/run`, so the systemd boot keeps using resolved. A `dns=` setting in `/etc/NetworkManager` overrides it (NetworkManager reads `/run` first). nss-resolve in `/etc/nsswitch.conf` finds no resolved and falls through to `dns`.

The kernel-install plugin adds the s6 entry for every new kernel. With
`--default-s6` it also keeps the s6 entry the default: on a kernel update
Fedora's `20-grub.install` names the new systemd entry as the next default
(`tmp_saved_entry`) and `95-set-boot-entry.install` saves it; the plugin runs
between them and names the new s6 entry instead. When the default s6 entry's
kernel is removed, it picks the newest s6 entry left. `--uninstall` and
`--default-systemd` move a saved s6 default back to the same kernel's systemd
entry. To reach the systemd entry when the menu is hidden, hold Shift or
press Esc while GRUB starts.

**`--boot-only-s6=yes`: no systemd at all.** Fedora's initramfs is built by dracut
and runs systemd as its PID 1 until it switches to the root. With this
option the s6 entry boots with uinitconverter's own initramfs instead
(`/boot/initramfs-KVER-s6.img`, built by `/usr/libexec/uinitconverter/mkinitramfs`, a link to
`initconvert` (rule 6), for every kernel by the kernel-install plugin, and again for every kernel
that has one when the uinitconverter package is updated; the systemd entry keeps
Fedora's). Its `/init` (execline) execs **s6-svscan as PID 1**, which
supervises **mdevd** (skarnet's uevent daemon: `mdevd-coldplug` makes it load
the drivers for the hardware found, from each device's modalias) and a
`rootfs` service. That service reads `root=`, `rootflags=`, `rootfstype=`,
`ro`/`rw`, `init=`, `rd.luks.uuid=`, `rd.luks.name=` and `rd.luks=0` from the kernel
command line, asks for the passphrase of any LUKS volume named there on the console (3
tries; `discard` from `/etc/crypttab` is honoured), mounts the root on `/sysroot`
and stops s6-svscan, whose `.s6-svscan/finish` moves `/dev`, `/proc`, `/sys`
and `/run` and runs `switch_root` to `init=` (`/usr/sbin/init-selinux`). The `rootfs`
service and the finish are `initconvert initramfs root|switch`: the image
carries `initconvert`; bash is there only for the rescue shell.
On a failure it opens a rescue shell; exiting it tries again. `rd.break`
gives a shell before the switch, `uinitconverter.initrd.timeout=SECONDS` sets how
long to wait for the disks (30). dracut is used only as a tool when the
image is built (`dracut-install` copies programs with their libraries and
modules with their dependencies); nothing of it runs at boot. A root with no
encryption just mounts (checked in QEMU with ext4, xfs and btrfs), and
`cryptsetup` is only needed for LUKS: without it the image is built with no
LUKS support, unless `/etc/crypttab` has an entry or the command line has
`rd.luks.uuid=` or `rd.luks.name=`, when the build fails. Limits: an encrypted
root must be opened by a typed passphrase (no TPM2/FIDO2/PKCS#11 unlock), a
root on LVM or MD can't be assembled (`initconvert init` warns; use
`--boot-only-s6=no`), no
plymouth (the s6 entry has none anyway), no hibernation resume, no
non-US keymap for the prompt. Extra modules go in
`/etc/uinitconverter/initramfs-modules` (one name or `=DIR` per line). Because
udev does not run in it, the initramfs leaves udev's activation record for
the LUKS mapping in `/run/udev/data`, as a systemd initramfs would, so the
real system's coldplug gives it its `/dev/disk/by-*` links.

What systemd components are replaced, and why logind and `systemd --user` stay
(GNOME needs them; see the next section):

| Role | Under s6 + uinitconverter |
|---|---|
| Initramfs | uinitconverter's where it can boot the root (`--boot-only-s6=auto`): s6-svscan, mdevd, and cryptsetup if installed; else Fedora's (dracut, systemd as its PID 1) |
| PID 1 / services | s6-linux-init, s6-rc, uinitconverter |
| System bus | dbus-daemon, or dbus-broker with `--bus=broker` (dbus-broker stays for the user bus) |
| Logging | rsyslog + s6-log (`--journal=yes`: systemd-journald as an s6 service) |
| Network / DNS / time | NetworkManager (DNS too; `--resolved=yes`: systemd-resolved), chrony |
| OOM handling | earlyoom instead of systemd-oomd |
| Radio state (rfkill) | util-linux `rfkill` from an s6 oneshot instead of systemd-rfkill |
| Console keymap and font | kbd's `loadkeys`/`setfont` from the `vconsole` s6 oneshot instead of systemd-vconsole-setup |
| Brightness across boots | the `backlight` s6 oneshot instead of systemd-backlight@ |
| Login gate (`/run/nologin`) | the tmpfiles step and `rc.shutdown` instead of systemd-user-sessions |
| Machine ID | dbus's `dbus-uuidgen` instead of systemd-machine-id-setup |
| Compressed swap (zram) | Fedora's zram-generator, run by the s6 oneshot `zram` instead of as a systemd generator (same `zram-generator.conf`, size and priority) |
| Devices | systemd-udevd as a plain service |
| Login, user manager | systemd-logind and `systemd --user`, as supervised services |

If stage 2 fails, log in on tty9 (Ctrl+Alt+F9) and read
`/run/uncaught-logs/current`. To boot the s6 database from before the last
install, add `uinitconverter.db=previous` to the kernel command line. Nothing is
regenerated at boot. `uinitconverter.bus=daemon` (or `=broker`) picks the system
bus for one boot, over `BUS=`. udevd runs with the sandbox of
`systemd-udevd.service`; `uinitconverter.udev=plain` runs it without for one
boot, and it also falls back by itself after three failed starts in a row.
rsyslog and earlyoom run with their units' sandboxes too, with no fallback: if
one can't be applied, the service stays down and s6 keeps trying.

With `JOURNAL=no`, services' output is in `/var/log/uinitconverter`, readable by
root and the `wheel` group only (0750), like the journal.

Core dumps are kept as on systemd: the `coredump` s6 service runs
`systemd-coredump --manager` with its unit's sandbox and points
`kernel.core_pattern` at its socket, and services start with an unlimited soft
`RLIMIT_CORE`. The files are in `/var/lib/systemd/coredump`. With `JOURNAL=no`
`coredumpctl` lists nothing, as it reads the journal; the summary goes to syslog.

What an s6 shutdown printed on the console is kept for the next boot in
`/var/log/uinitconverter/last-shutdown-console.log` (s6's catch-all log) and
`last-shutdown-kernel.log` (the kernel log from the start of the shutdown,
including SELinux denials after auditd stops); both are root-only. Once shutdown begins, initconvertd refuses
new unit starts and D-Bus activations at once, as systemd does. It first asks the
display manager (`display-manager.service`) to stop, so it doesn't try new
greeters when its session ends, then stops the scopes (logind's sessions) and
waits for them, up to 11 s, so a desktop session ends by itself before its user
manager is stopped, as on systemd. Then it
stops the on-demand instances (user@UID, user-runtime-dir@UID, systemd-backlight@, ...),
while logind and D-Bus still run, as their `After=` asks; only then does
s6-rc stop its services (this first step has its own limit of 2 minutes).
Services still not down 3 minutes after s6-rc began are killed, so a stop that
never finishes can't keep the machine up.

The skarnet versions and the SHA-256 sums of their tarballs (taken from the
releases when each was pinned, last changed 2026-10-25) are pinned in one file, `dist/skarnet.versions`,
which the RPM build script reads: to move to new releases, edit that file
only. execline is built as one multicall binary, `/usr/bin/execline`, so the
SELinux module does not list its commands.

### Running the tests on Fedora

All tests work unchanged on Fedora with the host `systemctl` and `systemd-run`.
On a host booted with systemd, they run in a namespace and never touch the real
systemd:

```sh
dist/rpm/build-skarnet-rpms.sh && sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm   # tarballs: see README-us.md
sudo dnf install dbus-daemon python3 cargo
cargo build && tests/sandbox.sh ~/sdc && tests/check.sh ~/sdc
tests/systemctl.sh status notifier.service
```

To exercise what the unprivileged sandbox can't test (`User=`, cgroups,
sandboxing, transient scopes), run `initconvert reload` with a config file in a
root-owned VM. The report and `initconvert plan` show how Fedora's own units are
translated without starting anything:

```sh
sudo initconvert --config /etc/uinitconverter/uinitconverter.conf plan | less
```

## GNOME and the user manager

GNOME 49 starts the whole session through a user service manager, and gnome-shell
places applications in transient scopes there. uinitconverter therefore doesn't
reimplement `systemd --user`: it runs the **real user manager as an ordinary
supervised service**, and provides the PID 1 API that logind and the user
manager use. systemd is still not PID 1. What happens at login:

1. GDM (or `login`) runs the PAM stack. `pam_systemd` asks **systemd-logind** (generated from Fedora's unit) to create a session.
2. logind asks PID 1, which is uinitconverter, to create `user-1000.slice` and `session-N.scope` (`StartTransientUnit` with `PIDFDs`), then to start `user@1000.service`.
3. uinitconverter creates `user@1000.service` on demand. Its `BindsTo=user-runtime-dir@1000.service` dependency runs first. The runner opens the `systemd-user` PAM session (`PAMName=`), delegates `user.slice/user-1000.slice/user@1000.service` to UID 1000 with the process in `init.scope` (`Delegate=`, `DelegateSubgroup=`), and execs `/usr/lib/systemd/systemd --user`. Readiness comes from `Type=notify-reload`.
4. The user manager starts the session bus (dbus-broker user units) and GNOME's user targets. `systemctl --user`, `systemd-run --user` and app scopes all work inside the session, because that part is real systemd.
5. On logout, logind stops the scope and later `user@1000.service`. Power off and reboot from the GNOME menu go through logind, which starts `poweroff.target` or `reboot.target`; uinitconverter runs the shutdown command for those.

Setup on top of the [installation steps](../README-us.md#installation):

```sh
dnf install gdm gnome-shell gnome-session   # Workstation already has these; keep dbus-broker installed for the user bus
systemctl enable gdm.service                 # display-manager.service alias
systemctl set-default graphical.target
```

**Status:** the D-Bus side of this flow (on-demand instances with `BindsTo=`,
conflicts, `Service=` sockets, shutdown targets) is covered by the sandbox
tests. The parts that need root (cgroup delegation, PAM sessions,
`StartTransientUnit` creating cgroups, the real logind and `systemd --user`)
are not in the sandbox tests; they have been checked on real boots of one
Fedora 45 Workstation machine with SELinux enforcing: GDM login, a GNOME
session with its user manager, and power off from the GNOME menu. Other
machines are untested, so try it in a VM first. If login fails, check `journalctl -u
systemd-logind` and `journalctl -u user@1000.service` (or `uinitconverter
log UNIT`, which also works with `JOURNAL=no`).

### The s6 user manager (removed)

Up to 0.3.0 the `uinitconverter-user` package offered an s6 replacement for
`systemd --user` (opt-in since part 203 of `work_history.md`). It is removed:
it reimplemented `systemd --user` as GNOME uses it, which is outside the
[scope](#scope), and it couldn't be kept in step with systemd's and GNOME's
changes. `user@UID.service` runs `systemd --user` on both boots.

Updating removes the package (`uinitconverterd` Obsoletes it), and its
removal deletes the `user@` drop-in that `initconvert-user-setup on` wrote
on a systemd boot. `USER_MANAGER=`, `USER_EXCLUDE=` and `USER_FALLBACK_ON=`
left in `uinitconverter.conf` are ignored, and so is
`uinitconverter.user_manager=` on the kernel command line.
`tests/vm-user-manager.sh` checks `systemd --user` under GNOME on both
boots (see the [TESTING](../TESTING-us.md) guide).

## The `s6` command (s6-frontend)

The system is managed with s6's own tools.
[s6-frontend](https://skarnet.org/software/s6-frontend/) (version in
`dist/skarnet.versions`, built with the other skarnet packages) provides the
`s6` command, and `initconvert init` writes `/etc/s6.conf` (from
`dist/s6.conf`) with uinitconverter's repository, stores and boot database. Service names are the s6-rc names: `sshd.service`, `gdm.service`,
… and uinitconverter's own s6 services (`dbus`, `uinitconverterd`, `earlyoom`, `udevd`, …).

How it fits together:

- initconvert-gen translates the unit files into an s6-rc **store**
  (`/var/lib/uinitconverter/store`). The s6-rc **repository**
  (`/var/lib/s6-rc/repository`) links three stores, in this order: the
  translated units, uinitconverter's early-boot services (`/etc/uinitconverter/s6`) and
  your own s6-rc services (`/etc/s6/sources`; a service there overrides one
  of the same name in the earlier stores).
- The repository's set **`current`** decides what boots: every service is
  `always` (early boot, can't be stopped by `s6 live stop`), `active`
  (enabled: in the `default` bundle the boot starts), `usable` (disabled) or
  `masked` (not in the database at all).
- A service seen for the first time starts from systemd's state: what
  `default.target` pulls in (the units systemd enables) becomes `active`, the
  rest `usable`. **From then on the set decides**: `systemctl
  enable/disable` still writes systemd's symlinks but no longer changes what
  boots. `systemctl is-enabled`, `systemctl status` and `list-unit-files`
  show the set: `enabled` for `always`/`active`, `disabled` for `usable`,
  `masked` for `masked` (`static`, `indirect` and `alias` stay as systemd
  has them).
- Automatic masking (`AUTO_MASK=`) is gone. The services an earlier
  version masked (`/var/lib/uinitconverter/auto-masked`) go back to
  `usable` at the next reload, and `auto-masked` and `auto-masked.keep`
  are removed once the list is empty; one running as a dynamic service
  then is stopped and started again from the database. Until then they
  are still started dynamically on request. `AUTO_MASK=` left in the
  configuration is ignored.
- `systemctl daemon-reload` and `initconvert reload` (also run by dnf's file
  triggers when packages change) regenerate the store, sync the repository,
  then commit `current` and install it as the boot and live database. Before
  committing they run `s6-rc-set-fix -u`, which enables what an `active`
  service depends on (the commit refuses an inconsistent set), and print
  what they moved. A service's logger whose service is no longer `active`
  (`s6 set disable -p` leaves it, being a dependency) becomes `usable`; it
  still starts with its service, and enabling the service makes it
  `active` again. A service's `UNIT.stop` oneshot takes its service's
  prescription (`active` for `always`), also when the service was moved
  without `-p`.
- The boot starts the installed database's `default` bundle as it is;
  nothing is regenerated at boot. Units changed from a systemd boot are
  picked up by the next reload.

| Command | What it does |
|---|---|
| `s6 set status [NAME...]` | each service's prescription in `current` (`always`, `active`, `usable`, `masked`) |
| `sudo s6 set enable\|disable -p NAME...` | change what boots; `-p` also moves what depends on it (without it the set stays inconsistent and the next reload's fix undoes a `disable`) |
| `sudo s6 set mask -p NAME...` | leave a service out of the database; a target (bundle) containing it disappears too, so prefer `disable` |
| `sudo s6 set commit --no-snapshot` then `sudo s6 live install --no-snapshot` | compile `current` and make it the boot and live database |
| `sudo s6 set apply --no-snapshot` | commit, install and bring the live state to the set at once |
| `s6 live status [NAME...]` | every (or the listed) service with its type and state (no root needed) |
| `sudo s6 live start\|stop NAME` | `s6-rc -u\|-d change NAME` |
| `sudo s6 process status\|restart NAME` | the supervised process (`s6-svstat`, `s6-svc -r`: the process only, not what depends on it) |
| `sudo s6 process kill -s HUP NAME` | send a signal to the process |
| `sudo s6 system reboot\|poweroff` | the same s6-linux-init shutdown as `systemctl reboot\|poweroff` |

Notes:

- **`--no-snapshot`**: s6-rc 0.8.0.2's `s6-rc-set-install` checks the
  set's snapshot through a path with one uninitialized byte at its end
  (`s6rc_repo_setuptodate()` terminates it one byte too late), so without
  the option it randomly refuses a just-committed set as "not up-to-date".
  uinitconverter always passes it. This is an upstream bug; uinitconverter doesn't patch
  skarnet's sources.
- **Early-boot services** are `always` exactly when their store says so
  (`flag-essential`, or membership in `uinitconverter-sysinit`, which
  `initconvert init` changes for syslog, journal-relay and resolv); a reload
  puts them back if a set moves them.
- **A reload never restarts a running service**, as systemd's daemon-reload
  restarts nothing. Before installing the new database live, gen asks
  `s6-rc-update -n` (its dry run) what the update would stop. If a service
  that is up is among it, the database is installed for the next boot only,
  and the reload says so ("not applied to the running system: the update
  would restart ..."). s6-rc-update restarts every service that gains a
  dependency on a new service: turning journald on would restart nearly
  every service, logind and gdm among them; and initconvertd runs the
  update, so stopping it or dbus would leave it waiting on its own stop.
  Most reloads restart nothing and apply live (a new unit, a changed
  command line: s6 uses a longrun's new `run` at its next start). Until the
  next boot, the running database stays the old one: a unit only the new
  one has can't be started (its start fails with "it is only in the s6
  database for the next boot"), and every later reload is held back the
  same way. The live state keeps the database it runs:
  rc.init gives `s6-rc-init` the database's directory, not the boot link,
  and on a system booted before that, the reload first points the live
  state at its database with an `s6-rc-update` to that same database (which
  restarts nothing). The package ships none of the bundle members
  `initconvert init` adds or removes, so an RPM update doesn't change the
  bundle behind its back.
- `s6 live start/stop` and `s6 process restart` act below uinitconverter: there is
  no systemd job, so units are better started and stopped with `systemctl`.
  `s6 live stop` runs `ExecStop=` (through `UNIT.stop`, see above);
  `s6 process restart` doesn't.
- Starting a target (a bundle) starts all of it, `usable` members included,
  as `s6-rc` does; udev's device targets (`bluetooth.target`, …) rely on it.
- SELinux: `s6-rc-update` and `s6-rc-format-upgrade` enter `init_t` when run
  from an admin's shell (`dist/selinux/uinitconverter.cil`), so `s6 live install`
  and `s6 set apply` work under enforcing. Without that, the scan directory
  got labels `init_t` can't read and the install hung.
- To roll back, boot with `uinitconverter.db=previous` (the database installed
  before the last one).

## Configuration

`/etc/uinitconverter/uinitconverter.conf` (`KEY=VALUE`; see `dist/uinitconverter.conf` for all keys
and defaults). The most important keys:

- `EXCLUDE`, `EXCLUDE_ADD`, `INCLUDE`: which units are translated.
- `DEP_MAP`, `DEP_MAP_ADD`: systemd unit → s6 service from `EXTRA_SOURCE`.
- `EXTRA_SOURCE`: hand-written s6-rc stores linked into the repository after the generated one (default `/etc/uinitconverter/s6:/etc/s6/sources`).
- `REPO_DIR`, `STORE_DIR`, `SET`, `DEFAULT_BUNDLE`: the repository (s6.conf's `repodir`), the generated store, the set a reload commits and installs, and the bundle its commit makes for the boot (`default`).
- `JOURNAL`: `yes` runs systemd-journald and sends service output to it; `no` (the default since rule 2, part 220; an update keeps an existing install's journald with `JOURNAL=yes`) uses per-service s6-log directories under `LOG_DIR` and excludes the distribution's `rsyslog.service` and `syslog-ng.service`, since the s6 `syslog` service is the syslog daemon (`SYSLOG`; `INCLUDE` brings them back).
- `LOG_DIR`: where those s6-log directories go when `JOURNAL=no`.
- `SYSLOG`: the daemon the s6 `syslog` service runs with `JOURNAL=no`: `rsyslog` (default) or `syslog-ng` (`initconvert init --syslog=`). Either way the distribution's `rsyslog.service` and `syslog-ng.service` are excluded. syslog-ng won't open `/dev/log` itself when `/run/systemd/system` exists (uinitconverter creates it for `sd_booted()`), so the run script binds it and passes it in `LISTEN_FDS`.
- `BUS`: `daemon` (default) or `broker`, the system bus of the s6 boot (`initconvert init --bus=`); read by the `dbus` s6 service (`initconvert svc dbus`).
- `RESOLVED`: `yes` runs systemd-resolved; `no` (the default since rule 2, part 220; an update keeps an existing install's resolved with `RESOLVED=yes`) leaves its units out of `INCLUDE` (put the `resolv` s6 service into `uinitconverter-sysinit` too, as `initconvert init --resolved=no` does, or DNS breaks).
- `INIT`: the init system units are translated for. Only `s6` exists yet; any other value stops every tool with a configuration error.
- `TOOLS`: replaces, by name, the programs from other packages that uinitconverter's programs run (`TOOLS="s6-svc=/opt/s6/bin/s6-svc mount=/usr/local/bin/mount"`), so a package can be swapped or installed elsewhere without a rebuild. Names not listed are looked up in `PATH` as before. It covers the s6/s6-rc tools, `mount`/`umount`/`swapon`/`swapoff`, `useradd`, `journalctl`, the programs `initconvert init` runs (`restorecon`, `semanage`, `semodule`, `grub2-editenv`, `systemctl`, …) and `s6-log` in generated loggers. A service's state is read from its `supervise/status` (s6 2.15's format) without starting `s6-svstat`, so `s6-svstat=` only applies when that file has another format. The shell scripts and s6 service definitions in `dist/` still run them by name.
- `ALLOW_UIDS`: non-root users allowed to manage units.
- `USER_MANAGER`, `USER_EXCLUDE`, `USER_FALLBACK_ON`: removed with [the s6 user manager](#the-s6-user-manager-removed); still accepted and ignored.
- `UDEV_DIR`, `UDEV_MONITOR`: where udev's database is (`/run/udev`; empty turns device-wanted units off) and whether to follow udevd's events.
- `INCLUDE`: by default lets through the systemd services that work under uinitconverter: logind, hostnamed, localed, timedated, resolved (DNS) and the sleep services. Replaced by s6 oneshots (rule 2): the console keymap and font (`vconsole`: kbd's `loadkeys` and `setfont` with `/etc/vconsole.conf`, in place of systemd-vconsole-setup), the screen and keyboard brightness kept across boots (`backlight`, in systemd-backlight's own files `/var/lib/systemd/backlight`), and unprivileged logins allowed once tmpfiles has run (the tmpfiles step removes `/run/nologin`, `rc.shutdown` writes it again, in place of systemd-user-sessions); `/etc/machine-id` is created by dbus's `dbus-uuidgen` when it's missing. The early-boot steps are `initconvert`'s own (`/usr/libexec/uinitconverter/early STEP` links to it), and so is what the hand-written s6 services (dbus, syslog, udevd, earlyoom, journal-relay, the fd store, linkwatch) do before their program runs: each `run` is one line of execline, `initconvert svc NAME`, which ends in an exec of the program (rule 6). s6-linux-init's `rc.init`, `rc.shutdown` and `rc.shutdown.final` are one line of execline each too (`initconvert rc init|shutdown|shutdown-final`), and `/usr/sbin/init-selinux` and the kernel-install plugin (`/usr/lib/kernel/install.d/90-uinitconverter.install`) are links to `initconvert`. The radio state (rfkill) is kept by the `rfkill` s6 oneshot instead of systemd-rfkill: util-linux `rfkill` restores each device's soft block at boot and the state is saved at shutdown, in systemd-rfkill's own files (`/var/lib/systemd/rfkill/ID_PATH:TYPE`), so the systemd boot entry shares it. Unlike systemd-rfkill it saves only at a clean shutdown, and a device that appears after the boot's udev settle keeps its state.

## Source layout

```
crates/                    the core (one Cargo workspace)
  uinitconverter-base/           shared by all core crates
    unit.rs                unit file parser, word splitting, time spans
    loader.rs              search path, aliases, templates, drop-ins, specifiers
    config.rs, envfile.rs  uinitconverter.conf, EnvironmentFile= format
    util.rs                small helpers (glob matching, ...)
    tools.rs               TOOLS=: the programs run from other packages
    automask.rs            cleans up what the removed AUTO_MASK= left behind
    state.rs               per-unit state files (RUN_DIR/units/<unit>/)
    index.rs               the generated database's index (unit → s6-rc service)
    fdstore.rs             fd store client and the daemon's fallback store
    calendar.rs            OnCalendar= parser and next-elapse search
    timerspec.rs, pathspec.rs, mountunit.rs   what .timer/.path/.mount units say
    acceptspec.rs          what an Accept=yes .socket says, and what of it is supported
    snapshot.rs            the units a database was generated from, carried in it
  uinitconverter-install/        enable/disable/mask/preset, unit file state
  uinitconverter-exec/           how a service process is set up:
    sandbox.rs             namespaces, mounts and capabilities
    seccomp.rs             SystemCallFilter= and the other seccomp-based settings (libseccomp)
    bpf.rs                 DevicePolicy=/DeviceAllow= and IPAddressDeny=/IPAddressAllow= (cgroup eBPF)
    cgroup.rs              systemd-compatible cgroup layout and resource control
    selinux.rs, pam.rs     labels and access checks (libselinux), PAMName= (libpam), both via dlopen
    socket.rs              socket units (sd_listen_fds)
    execcmd.rs, conditions.rs   Exec*= command lines, Condition*/Assert*
  uinitconverter-client/         small D-Bus client for the daemon
  uinitconverter-version/        the build's version for --version (RPM version-release or git commit)
  uinitconverter-gen/            plan → s6-rc source, compile, s6-rc-update, report (tool initconvert-gen)
  uinitconverter-runner/         exec environment, readiness, restart policy (tool initconvert-runner)
    runner.rs              the Runner type and the run, finish, up and down commands
    runner/                exec.rs, environment.rs, stdio.rs, sockets.rs, forking.rs,
                           notify.rs (readiness helpers), status.rs (exit results),
                           connection.rs (Accept=yes: accept and connection)
    supervisor.rs          what the runner asks of s6-supervise
  uinitconverter-timers/         timer and path unit runners (tool initconvert-timers)
  uinitconverterd/               D-Bus API: dispatch, properties, s6 backend, private socket,
                           on-demand instances (dynamic.rs), systemd-run (transient.rs),
                           units wanted by devices (devices.rs), SELinux access
                           checks (access.rs) (tool initconvertd)
    daemon/mod.rs          the Daemon type, unit loading, the main loop
    daemon/                dispatch.rs, manager.rs (Manager methods), jobs.rs,
                           scopes.rs (StartTransientUnit), accept.rs (Accept=yes
                           connection instances), caller.rs, errors.rs, ...
  uinitconverter/                the initconvert command, which runs the right tool, and `initconvert init`;
                                 the boot's own steps (rule 6): early.rs (early boot), svc.rs (the
                                 hand-written s6 services' start), rc.rs (stage 2, shutdown,
                                 init-selinux), kinstall.rs (the kernel-install plugin),
                                 initramfs.rs (the s6 initramfs and its steps)
standalone/                separate projects, each with its own README and spec
  uinitconverter-fdstore/        file descriptor store server
  uinitconverter-journal-relay/  journald's client sockets to syslog (JOURNAL=no)
  uinitconverter-linkwatch/      unblocks s6-rc-update waits that missed a start event
  uinitconverter-notify/         systemd-notify replacement
dist/                      config, D-Bus policy, s6 services (one-line execline runs),
                           s6-linux-init skeleton (one-line execline scripts),
                           SELinux module (selinux/), the s6 initramfs's tree (initramfs/)
packages/                  RPM specs for the core tools (one per tool), their build script, github-release.sh
dist/rpm/                  specs and build script for the skarnet packages in dist/skarnet.versions
tests/                     unprivileged sandbox, scenario checks, systemctl wrapper,
                           root/QEMU checks (bpf-root, linkwatch-check, initramfs, confine-vm),
                           diagnostics, memory and hardening comparison,
                           update-syscall-groups
```

### Contracts between the tools

The tools run as separate programs and agree on these formats at run time. Each
has one definition and tests that fail when one side drifts from it, so a fix
in one tool can't silently break another. Change the definition and its tests
together.

| Contract | Defined in | Between | Tests |
|---|---|---|---|
| The `result` state values (`success`, `exit-code`, ... and our `running`, shown as `success`) | `base/state.rs`, `state::result` | the runner writes, the daemon reads and exports `Result` | `state::tests`: no literal value beside `keys::RESULT` in the runner, the daemon or the timers |
| The runner's and the timers' commands (`run`, `finish`, `up`, `down`, `stop-atom`, `accept`, ..., `timer-run`, `path-run`) | `base/config.rs`, `runner_cmd`, `timers_cmd` | gen's scripts, the daemon, `dist/s6` run scripts and the runners | the runner's `Cmd` enum (an unhandled command doesn't build); `cmd_tests` (the list, `dist/s6`); `config::contract_tests` (no literal command in gen or the daemon) |
| The fd store protocol | `base/fdstore.rs` (documented at the top) | the core's client and fallback store, and the standalone `initconvert-fdstore` (no shared code) | `fdstore::tests::protocol_conformance_*`: the same cases through the core's client against both servers (the standalone one if built, else skipped with a note) |
| The keys of `uinitconverter.conf` | `base/config.rs`, `KEYS`, `REMOVED_KEYS` | the parser, `dist/uinitconverter.conf`, `initconvert init` | `config::contract_tests` (the parser's keys, the dist file); `init::tests::init_writes_known_keys` |
| The unit snapshot: the units a database was generated from (a longrun's `data/unit`; the others and the index in the `uinitconverter-units` holder's `data/`) | `base/snapshot.rs` (format at the top) | gen writes it; the runner, the timers and the daemon read it | `snapshot::tests` (read back, it is the unit as loaded); gen's `database_carries_its_units`; `check.sh` (the holder in the live database, kept `usable`) |
| The index of a generated database | `base/index.rs`, `Index::render` and `Index::load` | gen writes, the runner and the daemon read | `index::tests::render_and_load_round_trip` |

The daemon's D-Bus methods that the tools call (`LoadUnit`, `StartUnit`,
`Reload`, `StopInstances`, ...) have no test of their own: the sandbox suite
runs the real daemon with the real clients.

