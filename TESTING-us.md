# Testing sdcompat on Fedora

**English** | [한국어](TESTING.md)

This manual takes you from a fresh clone of
`https://github.com/shandyCho/sdcompat` to a GNOME session on Fedora running
with s6 as PID 1. It is split into stages that go from harmless to invasive.
Stop at the first stage that fails and report it (see
[Reporting a problem](#reporting-a-problem)).

| Stage | Needs root | Changes the system | Time |
|---|---|---|---|
| 1. Get the code and tools | yes (dnf) | installs build tools | 10 min |
| 2. Build the s6 RPMs | yes (dnf) | installs 6 packages | 10 min |
| 3. Sandbox tests | no | no | 5 min |
| 4. Translation preview | yes (read-only) | no | 2 min |
| 5. Install | yes | adds a boot entry, `/etc/sdcompat` | 10 min |
| 6. Boot with s6 | yes | boots the new entry | 15 min |

## 0. Before you start

- Use a **virtual machine** with **Fedora 44 or 45 Workstation** (GNOME; the s6
  boot has been tested on Fedora 45), at least
  4 GB RAM and 30 GB disk, and **take a snapshot** before stage 5. That is the
  quickest way back if a boot goes wrong.
- Log in as a normal user that can use `sudo`.
- **Set a root password**, otherwise the rescue console in stage 6 cannot be
  used (Fedora Workstation locks root by default):
  ```sh
  sudo passwd root
  ```
- Make the GRUB menu always visible, so you can choose the boot entry:
  ```sh
  sudo grub2-editenv - unset menu_auto_hide
  ```

## 1. Get the code and the build tools

The repository is private, so log in to GitHub first. The GitHub CLI is the
simplest way:

```sh
sudo dnf install -y git gh
gh auth login                      # choose GitHub.com, HTTPS, log in with a browser
gh repo clone shandyCho/sdcompat
cd sdcompat
git log -1 --oneline               # note this commit hash for reports
```

(With SSH keys set up on GitHub, `git clone git@github.com:shandyCho/sdcompat.git`
works too.)

Install everything needed to build and test:

```sh
sudo dnf install -y gcc make rpm-build cargo rust dbus-daemon python3 curl
```

The full list of dependencies is in
[README-us.md](README-us.md#dependencies). sdcompat loads libpam, libselinux
and libseccomp at run time instead of linking them. Fedora Workstation already
has them, but check that all four are installed:

```sh
rpm -q pam libselinux libseccomp policycoreutils
```

## 2. Build and install the s6 packages

Fedora does not package skalibs, execline, s6, s6-rc, s6-linux-init, s6-frontend or mdevd. The
build script downloads them, checks them against pinned SHA-256 sums and builds
RPMs in dependency order:

```sh
dist/rpm/build-skarnet-rpms.sh
```

- Without mock, it uses `rpmbuild`. Each package is unpacked into
  `~/rpmbuild-skarnet/sysroot`, and the next one builds against that. Nothing
  is installed and no `sudo` is needed.
- With mock (`sudo dnf install mock && sudo usermod -aG mock $USER`, then log
  out and back in), everything is built in clean chroots instead.

The RPMs end up in `~/rpmbuild-skarnet/rpms`. Install all of them:

```sh
sudo dnf install -y ~/rpmbuild-skarnet/rpms/*.rpm
rpm -q skalibs execline s6 s6-rc s6-linux-init s6-frontend mdevd   # all seven must be listed
```

**If a package fails to build**, the log is in `~/rpmbuild-skarnet/<name>.build.log`.
Send the last 50 lines.

## 3. Sandbox tests (no root, nothing changes)

The sandbox runs its own s6-svscan, s6-rc and D-Bus bus as your user. It never
touches the running system, even though the machine is booted with systemd.

```sh
cargo build
tests/sandbox.sh ~/sdc
tests/check.sh ~/sdc 2>&1 | tee ~/sdc/check.out
```

**Expected:** the last line is `passed: N  failed: 0` (N is about 120). These
lines are normal and not failures:
- `SKIP transient scope (needs a writable cgroup …)`. Creating cgroups needs root.
- `Call failed: Job type reload is not applicable …`. That one is a check that an error is returned.
- `sdcompat[bad.service]: ExecStart=/bin/sh failed: exit status 1`, printed by `tests/sandbox.sh`.
  `bad.service` fails on purpose, to test how its dependents are handled.

**Known flaky failure:** about half of the runs fail the same 14 checks
(`hello.service: expected active/running, got inactive/dead`, `tick.timer: …`,
`job not queued`, `CancelJob failed`, …). This is a known bug when the
database is reloaded (`s6-rc-update: … Broken pipe`). Recreate the sandbox and
run it again. Report it only if other checks fail, or if these 14 fail every
time.

`check.sh` expects the state right after the sandbox starts, so it runs once per
sandbox. To run it again, recreate the sandbox:
`tests/sandbox.sh ~/sdc stop && tests/sandbox.sh ~/sdc`.

To test a `JOURNAL=no` install (per-service s6-log instead of journald), set up
the sandbox with `SANDBOX_JOURNAL=no tests/sandbox.sh ~/sdc`. `check.sh` then
looks for service output under `~/sdc/logs/` instead of the journal stream, and
sends native and stream messages through `sdcompat-journal-relay` (sockets in
`$XDG_RUNTIME_DIR/sdc-test/jrelay`, forwarded to a stand-in `/dev/log` that
writes `devlog.out` next to them).

To check that the core works without the standalone `sdcompat-fdstore`, run
`sandbox.sh` and `check.sh` with `SANDBOX_FDSTORE=no` in front. The daemon then
holds the fds itself, so `check.sh` checks that they are gone after the daemon
restarts. `tests/sandbox.sh` builds the standalone tools from `standalone/`.

On a machine with sdcompat's SELinux module installed, `/usr/bin/s6-rc-update`
runs as `init_t` when you start it (that is what lets `sudo s6 live install`
work), and `init_t` can't open the sandbox's files. So `sandbox.sh` copies
`s6-rc-update` and `s6-rc-format-upgrade` into `WORKDIR/bin`, where they keep
the directory's label, and both scripts put that directory first in `PATH`.

Then try Fedora's real `systemctl` against the sandbox:

```sh
tests/systemctl.sh status hello.service
tests/systemctl.sh list-units --all
tests/systemctl.sh list-timers
tests/sandbox.sh ~/sdc stop
```

**If a check fails**, collect the logs before stopping the sandbox:

```sh
tests/collect-diagnostics.sh ~/sdc
```

The cgroup BPF programs (`DevicePolicy=`, `IPAddressDeny=`) can only be checked
in the kernel as root. This one test asks for your sudo password, uses a
scratch cgroup `/sys/fs/cgroup/sdcompat-bpf-test-PID` that it removes again,
and touches no unit:

```sh
tests/bpf-root.sh
```

**Expected:** `test result: ok. 1 passed`.

## 4. Translation preview (read-only)

This shows how sdcompat would translate this machine's own units, without
starting or writing anything:

```sh
cargo build --release
sudo ./target/release/sdcompat plan > ~/sdcompat-plan.txt
head -3 ~/sdcompat-plan.txt
grep -A3 -E '^(gdm|NetworkManager|systemd-logind|chronyd|accounts-daemon)\.service ' ~/sdcompat-plan.txt
```

Look for:
- the first lines: how many units were translated and skipped;
- `warning:` lines on the services above. Most are sandboxing directives that
  are not enforced, which is expected;
- the `skipped units:` list at the end. `systemd-*` units are skipped on purpose.

## 5. Install (take the VM snapshot first)

Preview, then install:

```sh
dist/setup-fedora.sh --dry-run --rpms=$HOME/rpmbuild-skarnet/rpms
sudo dist/setup-fedora.sh --rpms=$HOME/rpmbuild-skarnet/rpms
```

The script prints a plan and asks before changing anything. It adds a boot
entry **"Fedora (s6 + sdcompat)"** and keeps the normal systemd boot as the
default (once the s6 boot works for you, `--default-s6` makes it the default;
see the end of this file). That entry boots with SELinux in permissive mode.

Check the result:

```sh
sudo ls /boot/loader/entries/ | grep sdcompat         # the new boot entry
head -3 /var/lib/sdcompat/current/report.txt       # translation summary
```

## 6. Boot with s6

```sh
sudo reboot
```

In the GRUB menu choose **Fedora (s6 + sdcompat) …**. You will see text
boot messages instead of the splash screen, then the GDM login screen.

**With LUKS encryption**, the passphrase prompt is plain text
(`Please enter passphrase for disk …`) among the boot messages, because the s6
entry disables plymouth. Type the passphrase there as usual. The root disk is
unlocked by the initramfs exactly as on a normal boot. Additional encrypted
volumes that the initramfs does not unlock are not opened under s6; the setup
script warns about them in stage 5.

**If it never reaches GDM,** see [If the s6 boot fails](#if-the-s6-boot-fails).

After logging in to GNOME, open a terminal and go through this list:

| Check | Command | Expected |
|---|---|---|
| s6 is PID 1 | `cat /proc/1/comm` | `s6-svscan` |
| SELinux domains | `ps -eZ \| grep -E ' (s6-svscan\|chronyd\|gdm\|systemd-logind)$'` | `init_t`, `chronyd_t`, `xdm_t`, `systemd_logind_t` (not `unconfined_service_t`) |
| SELinux denials | `sudo ausearch -m AVC -ts boot \| audit2allow` | ideally nothing; send the output otherwise |
| fwupd | `fwupdmgr get-devices` | a device list (fwupd starts with its syscall filter) |
| No busy runner | `top -b -n1 -o %CPU \| head -15` | no `sdcompat` process near 100 % CPU |
| earlyoom | `sudo s6-svstat /run/service/earlyoom` (twice, a few seconds apart) | `up (pid …) N seconds`, with N growing (not restarting) |
| Radio state (rfkill) | Turn Bluetooth off in GNOME Settings, reboot into the s6 entry, then `rfkill` and `s6-rc -a list \| grep -x rfkill` | Bluetooth still `blocked` after the reboot; `rfkill` is listed (the oneshot, not systemd-rfkill) |
| Restart on package update | `systemctl show -p MainPID chronyd`, then `sudo dnf reinstall -y chrony`, then the first command again | a different MainPID (the update marked chronyd needs-restart and `systemctl enqueue-marked` restarted it) |
| User managers on package update | `sudo systemctl --user -M $USER@ is-system-running`, then `sudo /usr/lib/systemd/systemd-update-helper user-reload-restart; echo $?` | `running` or `degraded` (reached through `systemd-run --pipe`), then `0` |
| Shared mount propagation | `findmnt -no PROPAGATION /` | `shared` (as systemd sets it; `private` means mounts made later, e.g. a USB stick, don't reach units, flatpak apps or containers) |
| Runtime directory mounted once | `findmnt -n /run/user/$UID \| wc -l` | `1` (`2` means user-runtime-dir@UID ran twice) |
| Stopped users are dropped | `loginctl list-users` a minute after logging in (and after a `sudo`) | no `root` or `gdm-greeter` left in `closing` (`closing` that stays means logind still thinks the stop job runs) |
| /run mounted once | `findmnt -n /run \| wc -l` | `1` (the initramfs's /run, kept as systemd does; `2` means s6-linux-init stacked a second tmpfs on it) |
| cgroup BPF programs | `sudo dnf install -y bpftool`, then `sudo bpftool cgroup show /sys/fs/cgroup/system.slice/polkit.service; sudo bpftool cgroup show /sys/fs/cgroup/system.slice/systemd-logind.service` | polkit: one `cgroup_device` line `sd_devices`; logind: `sd_devices`, `sd_fw_ingress` and `sd_fw_egress`, one each |
| BPF programs refused | `journalctl -b -g 'not enforced'` (with `JOURNAL=no`: `sudo grep 'not enforced' /var/log/messages`) | nothing |
| polkit under `DevicePolicy=strict` | `pkaction \| head -3` | three action IDs (polkitd answers) |
| The `s6` command | `s6 live status \| head` | services with `up`, e.g. `NetworkManager.service/longrun//up/explicit` |
| s6 set | `s6 set status gdm.service sshd.service dbus` | `gdm.service/active`, `sshd.service/usable` (unless you enabled sshd), `dbus/always` |
| s6 set decides what boots | `sudo s6 set disable -p chronyd.service`, `sudo systemctl daemon-reload`, `s6 set status chronyd.service`, reboot into s6, `systemctl is-active chronyd.service`; then `sudo s6 set enable chronyd.service` and `sudo systemctl daemon-reload` | `chronyd.service/usable` and `chronyd.service.log/usable` after the reload (`s6 set status chronyd.service chronyd.service.log`), `systemctl is-enabled chronyd.service` says `disabled`; `inactive` after the reboot; after the enable and reload both `active` and `enabled` |
| `s6 live install` from a shell | `sudo s6 set commit --no-snapshot && sudo s6 live install --no-snapshot`, then `ls -Z /run/service \| awk '{print $1}' \| sort \| uniq -c` and `sudo ausearch -m AVC -ts recent` | it returns; the links are `tmpfs_t` or `init_var_run_t`, no `user_tmp_t`; no denials |
| Previous shutdown | after an s6 power-off and the next s6 boot: `sudo ls -l /var/log/sdcompat/last-shutdown-*.log; sudo grep -ci denied /var/log/sdcompat/last-shutdown-*.log` | both files from the power-off time, and `0` denials (send the files otherwise); the power-off took a few seconds, not 25 s or more; the console log is not empty and has no `unable to rename`, no `Failed to activate service ... timed out` and no `sdcompatd: stopping ... took`; the kernel log has no `services still not down after 180 s` |
| Bus and userdb labels | `ls -dZ /run/dbus/system_bus_socket /run/systemd/userdb` | `system_dbusd_var_run_t` and `systemd_userdbd_runtime_t` |
| Catch-all fifo label | `ls -Z /run/service/s6-svscan-log/fifo` and `sudo ausearch -m AVC -ts boot \| grep s6-svscan-log/fifo` | `sdcompat_log_fifo_t`, no denials on that fifo (as `initctl_t` the services' stderr became /dev/null) |
| Waiting from a shell | after a reboot, `ls -dZ /run/service/s6-svscan-log/event /run/service/sdcompatd/event`; `sudo s6-svc -wR -T 30000 -r /run/service/sdcompatd`; `sudo ausearch -m AVC -ts recent \| grep ftrig` | both `init_var_run_t`; s6-svc returns at once with 0; no denials (as `tmpfs_t` the waiter's fifo was `user_tmp_t`, s6-supervise could not write it, and the wait timed out) |
| Overall state | `systemctl is-system-running` | `running` (`degraded` is OK; see next line) |
| Failed units | `systemctl --failed` | ideally none; note any that are listed |
| Journal | `journalctl -b \| head` | log lines from this boot |
| Logs in status | `systemctl status NetworkManager` | the service's last log lines are shown |
| Login session | `loginctl` | a session for your user |
| User manager | `systemctl --user status` | a tree of your user services |
| Network | `ping -c 3 fedoraproject.org` | replies |
| DNS without resolved (only with `--no-resolved`) | `pgrep -x systemd-resolve; cat /etc/resolv.conf` | no PID; `# Generated by NetworkManager` and your network's name servers |
| Time sync | `chronyc tracking` | a reference time source |
| Timers | `systemctl list-timers` | timers with next run times |
| systemd-run | `sudo systemd-run --wait true` | `Finished with result: success` |
| Mounts | `systemctl status tmp.mount` | `active (mounted)` |
| GNOME | open Settings, Files, a terminal; lock and unlock the screen | all work |
| Power | log out and in again; then power off from the GNOME menu | both work |

Then collect the results (do this even if everything works):

```sh
sudo tests/collect-diagnostics.sh
```

**Optional: compare memory use with the systemd boot.** Run this once on the
s6 boot and once on the normal systemd boot, logged in to GNOME with the same
apps open each time:

```sh
sudo tests/memcompare.sh snap        # waits until 5 minutes after boot
```

Each run writes `~/sdcompat-mem/mem-<s6|systemd>-<date>.tsv`. Then, on either
boot:

```sh
tests/memcompare.sh compare ~/sdcompat-mem/mem-systemd-*.tsv ~/sdcompat-mem/mem-s6-*.tsv
```

It prints the system totals (as `free`), memory per group (init and
supervision, logging, user manager, everything else) and the commands whose
memory differs most. With root it compares PSS (shared pages split between
the processes that use them); without root it falls back to private
anonymous memory.

## If the s6 boot fails

1. Press **Ctrl+Alt+F9**. A rescue login prompt runs there even when the rest
   of the boot fails. Log in as root, then:
   ```sh
   tail -50 /run/uncaught-logs/current      # what s6 and the services printed
   s6-rc -a list                            # what is running
   ```
2. If the newest s6 database is the problem, reboot, press **e** on the s6
   entry in GRUB and add one of these to the `linux` line:
   - `sdcompat.db=previous` boots the database installed before the last one.
3. To get out, reboot and choose the normal Fedora entry. Logs from the failed
   s6 boot are then visible with `journalctl -b -1`, if journald was running.
   sdcompat's own messages (the daemon, and the runner's errors such as a
   failed PAM session or exec) are there too:
   `journalctl -b -1 -t sdcompatd -t sdcompat`.
4. Run `sudo tests/collect-diagnostics.sh` from whichever boot you can reach
   and send the archive. If you can only use the rescue console, send
   `/run/uncaught-logs/current`.

Reverting the VM snapshot undoes everything.

## Updating to a newer version

```sh
cd sdcompat
git pull
cargo build --release
sudo dist/setup-fedora.sh --rpms=$HOME/rpmbuild-skarnet/rpms --yes
```

Running the setup script again is safe. It keeps your `/etc/sdcompat`
configuration, builds and installs the new binaries (the core tools and the
standalone ones), regenerates the database and the
boot entry, and backs up what it replaces. The s6 services in
`/etc/sdcompat/s6` are updated to the new version unless you edited them;
for an edited one it prints "keeping your modified …" and you merge the new
version from `dist/s6` yourself. Run on the s6 boot, it restarts sdcompatd
so the new binary takes over without a reboot (sdcompat-fdstore and
journal-relay keep the old one until the next boot). It also reloads the
SELinux policy module; check that it is loaded:

```sh
sudo semodule -l | grep -x sdcompat
```

If `dist/skarnet.versions` or `dist/rpm/` changed in the update, rebuild and
reinstall the s6 packages first (stage 2). Do that from the **normal systemd
boot**: the new s6 and s6-rc programs then never act on a running s6 tree
started by the old ones, and the next s6 boot compiles its database with the
new s6-rc.

## Making s6 the default boot

Once the s6 boot works for you (the checks above pass, ideally enforcing),
make its entry the default. From the source tree, with the same options as
your install:

```sh
sudo dist/setup-fedora.sh --enforcing --default-s6 --rpms=$HOME/rpmbuild-skarnet/rpms
```

Check:

| What | Command | Expected |
|---|---|---|
| The default | `sudo grubby --info=DEFAULT \| grep title` | `title="Fedora (s6 + sdcompat) <kernel>"` |
| The choice is kept | `cat /etc/sdcompat/boot-default` | `s6` |
| It boots | reboot without touching the menu; `cat /proc/1/comm` | `s6-svscan` |
| After a kernel update | `sudo dnf upgrade kernel*`, then the first row again | the s6 entry of the new kernel |

To reach the systemd entry once, hold Shift or press Esc while GRUB starts
and pick it. To make systemd the default again:
`sudo dist/setup-fedora.sh --default-systemd ...` (the same options as above).

## Booting with no systemd at all

`--boot-only-s6` gives the s6 entry sdcompat's own initramfs: s6-svscan is
its PID 1 and mdevd loads the drivers, so systemd never runs, not even in
the initramfs. The LUKS root must be unlocked by typing the passphrase.

First test it without touching the machine, in QEMU (needs mdevd installed,
or `MDEVD_BIN=DIR` holding `mdevd` and `mdevd-coldplug`; no root):

```sh
tests/initramfs.sh /tmp/sdc-initramfs
```

It makes a LUKS2 disk with a btrfs root in `subvol=root`, boots the running
kernel with the s6 initramfs and types the passphrase on the serial console.
Expected: `passed: 6  failed: 0` (passphrase, an NVMe disk whose driver mdevd
must load, three wrong passphrases then the rescue shell and a retry,
`rd.break`, no disk).

Then on the machine, with the same options as your install:

```sh
sudo dist/setup-fedora.sh --enforcing --boot-only-s6 --rpms=$HOME/rpmbuild-skarnet/rpms
```

| What | Command | Expected |
|---|---|---|
| The image | `ls -l /boot/initramfs-*-s6.img` | one per installed kernel, built just now |
| The entry uses it | `sudo sh -c 'grep ^initrd /boot/loader/entries/*~sdcompat.conf'` | `initramfs-<kernel>-s6.img`; the systemd entries keep `initramfs-<kernel>.img` |
| The prompt | boot the s6 entry | a plain `Enter passphrase for /dev/nvme…:` prompt; the boot goes on after it |
| No systemd ran | `sudo dmesg \| grep -E 'Run /init\|systemd\[1\]\|running in initrd'` | only `Run /init as init process` |
| PID 1 | `cat /proc/1/comm` | `s6-svscan` |
| The LUKS mapping's links | `ls -l /dev/disk/by-uuid/ \| grep dm-` and `udevadm info /dev/dm-0 \| grep -E 'DM_NAME\|ID_FS_TYPE'` | the root filesystem's UUID points at `../../dm-0`; `DM_NAME=luks-…`, `ID_FS_TYPE=btrfs` |
| Mounts | `findmnt -n /; findmnt -n /home` | both from the LUKS mapping, as before |
| Rescue (optional) | type a wrong passphrase three times | `initramfs: rescue shell …`; `exit` asks again |
| A kernel update | `sudo dnf upgrade kernel*`, then the first two rows | the new kernel has its `-s6.img` and its s6 entry uses it |

If the prompt never comes or the disk is not found, the rescue shell is on
the console: `dmesg`, `ls /dev`, `cat /proc/cmdline` show what is missing
(send them). To boot once with Fedora's initramfs, press `e` on the s6 entry
and change `initramfs-<kernel>-s6.img` to `initramfs-<kernel>.img`. To go
back for good: `sudo dist/setup-fedora.sh --no-boot-only-s6 ...`.

## Removing it

Boot the normal Fedora entry, then:

```sh
sudo dist/setup-fedora.sh --uninstall
sudo dnf remove skalibs execline s6 s6-rc s6-linux-init s6-frontend mdevd \
    skalibs-devel execline-devel s6-devel s6-rc-devel s6-linux-init-devel
```

`/etc/sdcompat`, `/var/lib/sdcompat` and the enabled units stay. They do
nothing on a systemd boot.

## Reporting a problem

Please include:
1. the commit hash (`git log -1 --oneline`);
2. the stage and the exact command that failed;
3. what you expected and what happened;
4. the diagnostics archive from `tests/collect-diagnostics.sh` (stage 3:
   `tests/collect-diagnostics.sh ~/sdc`; stage 6: `sudo tests/collect-diagnostics.sh`).
   Check what it contains first (`tar -tzf FILE`): it has log lines, unit
   names and boot settings of the test machine.
