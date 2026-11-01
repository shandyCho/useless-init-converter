# Changelog

[한국어](CHANGELOG.md)

Versions apply to the core packages. The standalone tools have their own
versions. Release notes: [packages/release-notes](packages/release-notes).

## Unreleased

### Fixed
- A `selinux-policy` update on the s6 boot no longer breaks the running system.
  Its scriptlet relabels `/run`, which made s6-svscan's and the s6-linux-init
  services' fifos unwritable for `init_t`, so every reload then failed with
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
