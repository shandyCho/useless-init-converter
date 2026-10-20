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
| 2. Build the s6 RPMs | yes (dnf) | installs 5 packages | 10 min |
| 3. Sandbox tests | no | no | 5 min |
| 4. Translation preview | yes (read-only) | no | 2 min |
| 5. Install | yes | adds a boot entry, `/etc/sdcompat` | 10 min |
| 6. Boot with s6 | yes | boots the new entry | 15 min |

## 0. Before you start

- Use a **virtual machine** with **Fedora 44 Workstation** (GNOME), at least
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

## 2. Build and install the s6 packages

Fedora does not package skalibs, execline, s6, s6-rc or s6-linux-init. The
build script downloads them, checks them against pinned SHA-256 sums and builds
RPMs in dependency order:

```sh
dist/rpm/build-skarnet-rpms.sh
```

- Without mock, it uses `rpmbuild` and asks for your `sudo` password to install
  each `-devel` package, because the next package builds against it.
- With mock (`sudo dnf install mock && sudo usermod -aG mock $USER`, then log
  out and back in), everything is built in clean chroots instead.

The RPMs end up in `~/rpmbuild-skarnet/rpms`. Install all of them:

```sh
sudo dnf install -y ~/rpmbuild-skarnet/rpms/*.rpm
rpm -q skalibs execline s6 s6-rc s6-linux-init     # all five must be listed
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

**Expected:** the last line is `passed: N  failed: 0` (N is about 100). These
lines are normal and not failures:
- `SKIP transient scope (needs a writable cgroup …)`. Creating cgroups needs root.
- `Call failed: Job type reload is not applicable …`. That one is a check that an error is returned.
- `sdcompat[bad.service]: ExecStart=/bin/sh failed: exit status 1`, printed by `tests/sandbox.sh`.
  `bad.service` fails on purpose, to test how its dependents are handled.

`check.sh` expects the state right after the sandbox starts, so it runs once per
sandbox. To run it again, recreate the sandbox:
`tests/sandbox.sh ~/sdc stop && tests/sandbox.sh ~/sdc`.

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

The script prints a plan and asks before changing anything. It keeps the
normal systemd boot as the default and adds a boot entry
**"Fedora (s6 + sdcompat)"**. That entry boots with SELinux in permissive mode.

Check the result:

```sh
ls /boot/loader/entries/ | grep sdcompat           # the new boot entry
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
| Overall state | `systemctl is-system-running` | `running` (`degraded` is OK; see next line) |
| Failed units | `systemctl --failed` | ideally none; note any that are listed |
| Journal | `journalctl -b \| head` | log lines from this boot |
| Logs in status | `systemctl status NetworkManager` | the service's last log lines are shown |
| Login session | `loginctl` | a session for your user |
| User manager | `systemctl --user status` | a tree of your user services |
| Network | `ping -c 3 fedoraproject.org` | replies |
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

## If the s6 boot fails

1. Press **Ctrl+Alt+F9**. A rescue login prompt runs there even when the rest
   of the boot fails. Log in as root, then:
   ```sh
   tail -50 /run/uncaught-logs/current      # what s6 and the services printed
   s6-rc -a list                            # what is running
   ```
2. If the newest s6 database is the problem, reboot, press **e** on the s6
   entry in GRUB and add one of these to the `linux` line:
   - `sdcompat.db=previous` boots the database from before the last change.
   - `sdcompat.reload=0` skips regenerating the database at boot.
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
configuration, installs the new binary, regenerates the database and the
boot entry, and backs up what it replaces.

If `dist/rpm/` changed in the update, rebuild and reinstall the s6 packages
first (stage 2).

## Removing it

Boot the normal Fedora entry, then:

```sh
sudo dist/setup-fedora.sh --uninstall
sudo dnf remove skalibs execline s6 s6-rc s6-linux-init \
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
