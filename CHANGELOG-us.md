# Changelog

[한국어](CHANGELOG.md)

Versions apply to the core packages. The standalone tools have their own
versions. Release notes: [packages/release-notes](packages/release-notes).

## Unreleased

Changes since 0.2.2. Details per step in `work_history.md`, parts 130-157.

### Added
- `Accept=yes` sockets: on unix paths through `s6-ipcserverd`, on TCP ports
  through `s6-tcpserverd` (optional s6-networking package, e.g. `sshd.socket`).
  Each connection runs a template instance named as systemd names it, and
  while it runs it is a unit: listed, shown and stoppable.
- `GetUnitByPIDFD` (polkitd asks it for every caller), and
  `PropertiesChanged` for every state change, not only a job's own unit.
- **The s6 user manager, opt-in.** `USER_MANAGER=s6` in
  `uinitconverter.conf`, or `uinitconverter.user_manager=s6` on the kernel
  command line, runs `initconvert-user-manager` (package
  `uinitconverter-user`) for `user@UID.service`
  instead of `systemd --user`; the default stays `systemd`. In the test VM it
  runs GDM's greeter and GNOME sessions unmodified: logout, a new login, a
  user switch, lingering users, the RPM update helper, a reboot, timers and
  paths, sockets (also `Accept=yes`), systemd-oomd reporting, podman quadlet
  units and flatpak apps. Its transactions start units in parallel. If it
  fails a user (GNOME's failure path, a crash), it hands over to
  `systemd --user` in place for the rest of the boot (`USER_FALLBACK_ON=`).
  See [the s6 user manager](docs/etc-us.md#the-s6-user-manager-in-progress).
- **The s6 user manager on a normal systemd boot**, opt-in:
  `sudo initconvert-user-setup on` (package `uinitconverter-user`, which no
  longer needs any s6 boot package) runs it for every `user@`; `off` goes
  back to `systemd --user`, `status` shows what each user runs. The kernel
  flag `uinitconverter.user_manager=s6` is no longer needed there;
  `uinitconverter.user_manager=systemd` still turns it off for one boot.
- `UINITCONVERTER_LOG_JOBS=1` in the daemon's environment logs every job
  request with the caller's process name.
- `packages/build-rpms.sh --test` (`--shared-target`, `--nocheck`): faster
  RPM builds for testing; see [Building](packages/README-us.md#building).
- `tests/vm-s6-boot.sh`: the RPMs, `initconvert init` and the s6 boot in
  the GNOME test VM. `BOOT=s6` runs `tests/vm-user-manager.sh`'s session
  and lifecycle checks on the s6 boot.
- Tests for the formats the tools share (the `result` values, the runner's
  commands, the fd store protocol against the standalone store, the config
  keys, the index); see
  [Contracts between the tools](docs/etc-us.md#contracts-between-the-tools).

### Changed
- `uinitconverterd` no longer requires s6-rc and dbus-daemon; the
  `uinitconverter` package, which the s6 boot needs, still does.
- **The commands are named `initconvert*`**: `initconvert` (was
  `uinitconverter`), `initconvertd`, `initconvert-gen`, `initconvert-runner`,
  `initconvert-timers`, `initconvert-user-manager`, and the standalone
  `initconvert-fdstore`, `-notify`, `-journal-relay`, `-linkwatch`. Packages,
  paths, s6 services, the SELinux module, the D-Bus interface, kernel options
  and variables keep the `uinitconverter` names. Moving an installed system:
  [Moving to the initconvert commands](packages/README-us.md#moving-to-the-initconvert-commands).
- Job side effects as with systemd: units that list a starting unit in
  `Conflicts=` are stopped; stopping a unit stops the running units with
  `PartOf=`, `BindsTo=` or `Requires=` on it, restarting it restarts those with
  `PartOf=`; `StopWhenUnneeded=` is honoured; a target's state is its own; a
  `Type=notify` service that sends `STOPPING=1` is stopped.
- A start of a unit while another start of it is pending merges into that one,
  as systemd merges jobs.
- A `Type=notify` service with `NotifyAccess=all` may hand its main process
  over with `MAINPID=` (podman's quadlet units): the runner then follows the
  announced process.
- `AttachProcessesToUnit` follows systemd's rule for unprivileged callers, and
  `Delegate=` with a controller list counts as delegation.
- New package `uinitconverter-user` (noarch): the s6 user manager's entry point
  `initconvert-user-manager` (a link to `initconvertd`) and the
  tmpfiles.d entry for `/run/uinitconverter-user` (its fallback record).
  Without it, `user@` keeps `systemd --user` whatever `USER_MANAGER=` says.

### Fixed
- The s6 user manager answers `/org/freedesktop/systemd1/unit/self` (the
  caller's own unit), and `GetUnitByPID` there names the user unit (it named
  `user@UID.service` for every process). `systemd-run --user --scope`
  failed with "Failed to request invocation ID for unit: Unknown object", so
  Ptyxis printed that line for each new terminal and started the shell
  without its scope.
- At shutdown the users' `user@` and `user-runtime-dir@` stop at the same
  time, as with systemd. They stopped one user after another, so with the s6
  user manager three users took 8-11 s.
- An `Accept=yes` instance gets `LISTEN_FDNAMES=` from the socket's
  `FileDescriptorName=` (it was always `connection`), so systemd's varlink
  services answer.
- `Symlinks=` of a socket creates the link's parent directories.
- The s6 boot works with `/var` (or `/var/lib`) on a filesystem of its own,
  such as Fedora Cloud's btrfs `var` subvolume: stage 2 mounts it from fstab
  before it reads the database. It stopped in stage 2 with
  `s6-rc-init: fatal: unable to open /var/lib/uinitconverter/compiled`.
- `initconvert init --default-s6` makes the s6 entry the default when
  `/boot` is a btrfs subvolume and not a partition (grubby then names the
  kernel `/boot/boot/vmlinuz-…`). It added the entry, but left the default
  unchanged with `no s6 entry for …`.

### Known issues
- The s6 user manager has not run for SELinux-confined users; keep
  `USER_MANAGER=systemd` there. Under the s6 boot's own system manager it has
  run in the test VM only (`tests/vm-s6-boot.sh`), not on real hardware.
- On the s6 boot with the s6 user manager, a reboot with GNOME sessions open
  can take 6-9 s for the users: gnome-shell sometimes doesn't exit until its
  unit's `TimeoutStopSec=5`, and each user's stop ends with about 2 s for
  `user-runtime-dir@`. Under `systemd --user` on the same boot it takes under
  1 s. Not explained yet.
- The system sandbox failed once in a few runs on a dangling scan directory
  entry of an on-demand instance (`quitter@0.service`); not reproduced.

## 0.2.2 (2026-11-02)

### Fixed
- Updating `selinux-policy-targeted` with a package that has its own SELinux
  module (such as `passt-selinux`) on the s6 boot no longer breaks the running
  system. The update restores the labels under `/run/service`, which made
  s6-svscan's and the s6-linux-init services' fifos unwritable for `init_t`, so
  every reload then failed with
  `unable to manage new service directories ... Permission denied`. The SELinux
  module now gives them their own labels; updating the package and running
  `uinitconverter init` repair what an earlier update left.
- A reload's last failed attempt puts back the links of running services in the
  scan directory, as the earlier attempts did.
- A transient service whose start fails before it runs is unloaded, and its
  stored stdio released. It used to hold the caller's pipe, which left `dnf`
  waiting forever on a scriptlet that ran `systemctl --user -M`.

## 0.2.1 (2026-11-02)

### Fixed
- `PrivateNetwork=` now implies `PrivateMounts=` unless that is turned off, and
  remounts `/sys` for the new network namespace, as systemd does. Before, such a
  service stayed in the host's mount namespace and `/sys/class/net` listed the
  host's devices. If the remount fails, the service still starts and logs it.
- Shutdown asks the display manager (`display-manager.service`) to stop along
  with the session scopes, as systemd stops them together. Before, gdm saw the
  session end and tried to start a new greeter up to six times, each refused,
  leaving six `gnome-session-init-worker` core files per shutdown.

### Changed
- `.path` units' inotify and the fd store's socket handling use rustix instead
  of raw libc calls. No behaviour change intended. Received credentials
  (`SCM_CREDENTIALS`, `SO_PEERCRED`) stay on libc: the kernel can report pid 0,
  which rustix's `UCred` can't hold.
- The s6 initramfs no longer needs `cryptsetup` when nothing is encrypted, and
  loads `dm_crypt` only for LUKS. The build fails if cryptsetup is missing while
  `/etc/crypttab` or the command line names encrypted volumes.
- At shutdown, uinitconverterd logs how long it waited for the session scopes.
- The README covers first installs only; everything else moved to
  [docs/etc-us.md](docs/etc-us.md).
- `uinitconverter init --boot-only-s6=yes` warns about a root on LVM or MD, which
  that initramfs can't assemble.

## 0.2.0 (2026-11-01)

Changes since the first published release (tag `initial`, 2fac273).

### Added
- zram swap on the s6 boot, through a oneshot that runs zram-generator.
- On-demand socket activation at boot.
- `BUS=daemon|broker` picks the system bus: dbus-daemon or dbus-broker.
- `SYSLOG=rsyslog|syslog-ng` picks the s6 boot's syslog daemon.
- `INIT=` names the init backend and `TOOLS=` maps programs of other packages.
- s6 service `coredump`: systemd-coredump's listener, so core dumps are kept.
- `tests/hardencompare.sh` and `tests/memtest.sh`.

### Changed
- rsyslog, earlyoom, udevd and the system bus run under their units' sandbox.
- `PrivateUsers=` is implemented (colord, upower and others that set it get their own user namespace).
- Read-only mounts match systemd's.
- `/var/log/uinitconverter` is 0750 root:wheel.
- Hard `RLIMIT_NOFILE` is 524288 and soft `RLIMIT_CORE` is unlimited, as on systemd.
- The Type=notify helper re-executes instead of staying a fork.
- Time zones in `OnCalendar=` use jiff instead of `TZ` switching, so
  `uinitconverterd` computes the next elapse of named zones itself.
- Source: formatted with rustfmt; large files split by role. No logic change.

### Fixed
- Shutdown stops session scopes before `user@UID`, as systemd. Aimed at
  gnome-shell's crash at shutdown; see Known issues.
- `OnCalendar=` around DST changes now matches systemd: `minutely` no longer
  skips the second pass of a repeated hour, a repeated time no longer elapses
  twice, and a date a zone skipped no longer moves to another day.
- Soft `RLIMIT_CORE` survives the kernel_t to init_t transition.
- rsyslog's `/run/user` exists before it is confined.
- `selinux`: restorecon loads file_contexts per call and frees it.
- Input checks from a security review: transient units, fd stores, runner;
  linkwatch only acts for waiters of its own user.

### Removed
- `AUTO_MASK=`.

### Known issues
- Whether the shutdown order fixes gnome-shell's crash is not proven: 2 clean
  shutdowns so far, none from the login screen.
- `tests/hardencompare.sh` still reports rtkit as weaker on s6 (`ns_mnt`): its
  unit only sets `PrivateNetwork=`, so this isn't a file system restriction lost.
