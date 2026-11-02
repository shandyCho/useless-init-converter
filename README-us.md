# useless-init-converter

> **This software is AI slop: you install and run it at your own risk.**

**English** | [한국어](README.md)

## Project summary

uinitconverter runs Fedora with **s6 as PID 1** instead of systemd. It translates
Fedora's unit files into s6-rc services, and you manage them with s6's own tools.
`systemctl`, libsystemd and D-Bus clients keep working, so dnf, SELinux enforcing
and GNOME still work. The commands, packages and paths are all named
`uinitconverter`.

**Read this before installing:**

- **The main gain is control, not speed or memory.** If you don't specifically
  want s6, plain systemd is the better choice.
- **Real boots have only been checked on one Fedora 45 Workstation machine.**
  Try it in a virtual machine first.
- **Some systemd features are missing**, such as generators and
  credentials. See [Not implemented](docs/etc-us.md#not-implemented-yet).
- **Your normal systemd boot stays.** Installing adds a GRUB entry, and systemd
  remains the default until you change it.

### What gets installed

| Group | Packages | Needed? | Comes from |
|---|---|---|---|
| s6 and related tools | skalibs, execline, s6, s6-rc, s6-linux-init, s6-frontend, mdevd | Yes | You build them (step 2) |
| TCP super-server | s6-dns, s6-networking | Optional: only for `Accept=yes` TCP sockets such as `sshd.socket` | Built in step 2; to leave them out, delete their `.rpm` files before `dnf install` |
| Core | `uinitconverter`, `uinitconverterd`, `uinitconverter-gen`, `uinitconverter-runner`, `uinitconverter-timers` | Yes | Release or source (step 3) |
| Standalone tools | The four below | Optional | Release or source (step 3) |

The standalone tools are separate projects. Each one builds, installs and runs
on its own, and the core works without them, but you lose what the last column
says:

| Tool | What it does | Without it |
|---|---|---|
| [`uinitconverter-linkwatch`](standalone/uinitconverter-linkwatch/README-us.md) | Unblocks a daemon-reload stuck on an s6 race | Such a reload hangs, and so does every s6-rc command after it, until the stuck `s6-rc-update` is killed |
| [`uinitconverter-fdstore`](standalone/uinitconverter-fdstore/README-us.md) | Holds services' stored file descriptors and listening sockets | They are lost when `uinitconverterd` restarts, and on-demand services start at boot |
| [`uinitconverter-notify`](standalone/uinitconverter-notify/README-us.md) | `systemd-notify` replacement | Programs use `systemd-notify`, which stays installed |
| [`uinitconverter-journal-relay`](standalone/uinitconverter-journal-relay/README-us.md) | Stands in for journald's client sockets with `--journal=no` | `--journal=no` can't be used; nothing changes with the default journald |

Installing all four is the simplest choice, and it's what step 3 does.

## Requirements

**Machine**

- Fedora 45 Workstation on x86_64. The release RPMs are built for exactly that.
  For another Fedora release or architecture, build from source (step 3, option B),
  but real boots have only been checked on Fedora 45.
- A virtual machine with at least 4 GB RAM and 30 GB disk is recommended.
- A user that can use `sudo`, and internet access.

**To build and install**

| Needed for | Fedora packages |
|---|---|
| Steps 1 and 2 (always) | `git curl rpm-build gcc make cpio` |
| Step 3, option B (source build only) | `cargo rust` |

**To run**

- The s6 packages from step 2. Fedora doesn't package them.
- `systemd` must stay installed. udevd, logind, journald, `systemd --user` and
  `systemctl` still come from it.
- `pam`, `libselinux` and `libseccomp`. Fedora Workstation already has them, and
  GNOME can't start without `pam`.
- dnf installs everything else (dbus-daemon, grubby, earlyoom and so on).

Full list: [Dependencies](docs/etc-us.md#dependencies).

## Installation

Run these commands in order, in one terminal (step 3 uses `VERSION` from step 1).
Steps 1 to 3 only install packages; the boot menu doesn't change until step 4.

### 0. Prepare

Take a snapshot if you're in a VM. Then set a root password, because the rescue
console needs one, and keep the GRUB menu visible so you can choose the boot
entry:

```sh
sudo passwd root
sudo grub2-editenv - unset menu_auto_hide
```

### 1. Get the source

Use the release you want to install as `VERSION`. See the
[releases](https://github.com/shandyCho/useless-init-converter/releases).

```sh
sudo dnf install -y git curl rpm-build gcc make cpio
VERSION=0.2.2
git clone --branch "v$VERSION" https://github.com/shandyCho/useless-init-converter.git
cd useless-init-converter
```

### 2. Build and install the s6 packages

This downloads the exact versions listed in `dist/skarnet.versions`. The build
script checks each file's SHA-256 and stops on a mismatch. It downloads nothing
itself, and reads `~/skarnet-sources` unless you give it `--sources DIR`.

```sh
mkdir -p ~/skarnet-sources
grep -v '^#' dist/skarnet.versions | while read -r name ver _; do
    [ -n "$name" ] || continue
    curl -fL -o ~/skarnet-sources/"$name-$ver.tar.gz" \
        "https://skarnet.org/software/$name/$name-$ver.tar.gz"
done
dist/rpm/build-skarnet-rpms.sh
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm
```

### 3. Install uinitconverter and the standalone tools

Pick one option. Both install the core and all four standalone tools. To leave
out a standalone tool, delete its `.rpm` file before the `dnf install` line.

**Option A: from the release (Fedora 45, x86_64)**

```sh
mkdir ~/uinitconverter-rpms && cd ~/uinitconverter-rpms
curl -fsSL "https://api.github.com/repos/shandyCho/useless-init-converter/releases/tags/v$VERSION" \
    | grep browser_download_url | cut -d'"' -f4 | xargs -n1 curl -fLO
sha256sum -c SHA256SUMS
sudo dnf install ./*.rpm
```

**Option B: build from source**

```sh
sudo dnf install -y cargo rust
packages/build-rpms.sh
for script in standalone/*/build-rpm.sh; do "$script"; done
sudo dnf install ~/rpmbuild-uinitconverter/rpms/*.rpm \
    ~/rpmbuild-uinitconverter-standalone/rpms/*.rpm
```

### 4. Set up the boot

The first command only shows what would happen. The second asks before it
starts, then adds the GRUB entry "Fedora (s6 + uinitconverter)". If a package is
missing, it stops and prints the `dnf install` command.

```sh
uinitconverter init --dry-run
sudo uinitconverter init
```

The first run boots s6 with SELinux in permissive mode. Common options are
below; all of them are in
[What `uinitconverter init` does](docs/etc-us.md#what-uinitconverter-init-does).

| Option | Effect |
|---|---|
| `--selinux=enforcing` | Boot s6 with SELinux enforcing |
| `--default-s6` | Make s6 the default boot entry |
| `--journal=no` | Use rsyslog and plain-text logs instead of journald (needs `uinitconverter-journal-relay`) |
| `--boot-only-s6=yes` | No systemd in the initramfs either (a root on LVM or MD can't use this) |

### 5. Boot with s6

```sh
sudo reboot
```

Choose **Fedora (s6 + uinitconverter)** in the GRUB menu.

### 6. Check that it worked

Log in, then run:

```sh
cat /proc/1/comm                  # should print s6-svscan
uinitconverter --version          # all core tools from one build
s6 live status | head             # the services s6 manages
sudo ausearch -m AVC -ts boot     # SELinux denials on this boot
```

If there are no denials you need to worry about, switch to enforcing for the next
boot:

```sh
sudo uinitconverter init --selinux=enforcing
```

### If the boot fails

Reboot and choose the normal Fedora entry, which is still in the menu. If
the s6 boot starts but doesn't finish, press Ctrl+Alt+F9, log in as root on the
rescue console and read `/run/uncaught-logs/current`. To report a problem, see
[TESTING-us.md](TESTING-us.md#reporting-a-problem).

### Update

With option A, download the new release as in step 3, in a new empty directory,
then:

```sh
sudo dnf upgrade ./*.rpm
sudo uinitconverter init --yes
```

With option B, pull the new source and rerun the two build commands from step 3,
then:

```sh
sudo dnf upgrade ~/rpmbuild-uinitconverter/rpms/*.rpm \
    ~/rpmbuild-uinitconverter-standalone/rpms/*.rpm
sudo uinitconverter init --yes
```

On the s6 boot, `init` restarts `uinitconverterd` so the new build takes over; reboot
to finish the update. Rebuild the s6 packages only if `dist/skarnet.versions` changed.

### Remove

Do this from the normal systemd boot:

```sh
sudo uinitconverter init --uninstall
sudo dnf remove 'uinitconverter*'
```

## More

- [docs/etc-us.md](docs/etc-us.md): how it works, implemented features, the `s6`
  command, configuration, source layout
- [TESTING-us.md](TESTING-us.md): a step-by-step test from a fresh clone to a
  GNOME session
- [packages/README-us.md](packages/README-us.md): the RPM packages, and moving
  from the project's old name, sdcompat
