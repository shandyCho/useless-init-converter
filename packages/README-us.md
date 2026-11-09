# uinitconverter RPM packages

[한국어](README.md)

Each executable of the core (`crates/`) is its own RPM package. The source
stays in the one Cargo workspace, and all five packages build from the same source tarball. The library crates (`uinitconverter-base`, `uinitconverter-install`,
`uinitconverter-exec`, `uinitconverter-client`) are linked statically into the
executables, so they are used only to build and are not packaged.

| Package | Contains | Needs at run time (`Requires`) |
|---|---|---|
| `uinitconverter-runner` | `initconvert-runner` | s6, shadow-utils. Recommends `uinitconverterd` (`OnFailure=`/`OnSuccess=` units are started through the daemon) |
| `uinitconverterd` | `initconvertd`, the D-Bus policy (`uinitconverter.conf`) | s6, `uinitconverter-runner`. Recommends `uinitconverter-timers` (`systemd-run --on-calendar`). Obsoletes `uinitconverter-user` (the s6 user manager, removed after 0.3.0; see the docs/etc pair) |
| `uinitconverter-timers` | `initconvert-timers` | `uinitconverterd` (timers start units through the daemon) |
| `uinitconverter-gen` | `initconvert-gen`, `/var/lib/uinitconverter` | s6-rc, `uinitconverter-runner`, `uinitconverter-timers` (the generated services run them) |
| `uinitconverter` | the `initconvert` command (with `initconvert init`, the boot setup), `early`, configuration, s6 services, s6-linux-init skeleton, SELinux module, kernel-install plugin, `/var/log/uinitconverter` | s6, s6-linux-init, s6-rc, execline, dbus-daemon, util-linux, policycoreutils-python-utils, grubby, and the four tool packages above |

- `dnf install uinitconverter` installs everything. The boot configuration and the
  SELinux module are in the `uinitconverter` package, so booting with s6 needs it.
- The tool packages can be updated on their own. They are tied to the same
  version (`= version-release`).
- The standalone tools (`standalone/`) are not here. Each directory has its
  own spec and `build-rpm.sh` (result: `~/rpmbuild-uinitconverter-standalone/rpms`), and
  `uinitconverter` only points at them with `Recommends`/`Suggests`.

## Building

Step by step, with the test build and the release build side by side:
[docs/PACKAGEBUILD-us.md](../docs/PACKAGEBUILD-us.md).

```sh
packages/build-rpms.sh            # result: ~/rpmbuild-uinitconverter/rpms
packages/build-rpms.sh DIR        # somewhere else
packages/build-rpms.sh --srpm     # source RPMs only: ~/rpmbuild-uinitconverter/srpms
packages/build-rpms.sh --test     # a faster build for testing (see below)
```

The script packs the workspace (`Cargo.toml`, `Cargo.lock`, `crates`, `dist`)
and the crates fetched with `cargo vendor` into one `uinitconverter-VERSION.tar.gz`,
then runs `rpmbuild -bb` for each spec. rpmbuild builds without network
(`--offline`). Nothing is installed and no sudo is used. Needs `rpm-build`,
`cargo` and `rust`.

**Faster builds for testing.** Each spec builds in its own tree, so a
normal build compiles every crate five times over and runs `cargo test` in each
spec's `%check`. For RPMs you only test (in a VM, for example), two opt-in
options cut that down:

| Option | Effect |
|---|---|
| `--shared-target` | Every spec builds from one copy of the source at a fixed path into one cargo target directory, both under `OUTDIR/cargo-target`, so cargo compiles each crate once instead of once per spec. It's kept, so the next build recompiles only what changed. No debuginfo or debugsource packages |
| `--nocheck` | rpmbuild skips `%check` (no `cargo test`; run it in the tree yourself) |
| `--test` | Both |

Don't use them for RPMs you publish: those are built with neither, as
`github-release.sh` does. The packages are the same either way, but each
spec's build is no longer separate from the others, and the tests didn't run.

## Versions

The specs carry no version. The script sets it, so build the specs with
this script (plain `rpmbuild` fails for lack of a version).

- **Version** is read from one place, `[workspace.package] version` in
  `Cargo.toml`. The crates use it too, so a version bump is that one line.
- **Release** is new for every build: `1.YYYYMMDDHHMMSS.gCOMMIT`, with
  `.dirty` appended when there are uncommitted changes. Example:
  `uinitconverter-gen-0.1.0-1.20261021071849.g0023111.fc45`. The build time comes
  first, so when only the Rust code changed and the version did not, dnf
  still sees the new build as an upgrade.
- The five packages are tied to one version-release, and the script always
  builds all five together. `rpms/` holds only the last build's five.
- The tools built as RPMs print the package's version-release for
  `--version` (e.g. `initconvert-gen 0.1.0-1.20261021073517.gc8aabf8.fc45`);
  built from source they print the commit, like `0.1.0 (git c8aabf8)`.
- To set the release yourself, e.g. for a published release:
  `UINITCONVERTER_RELEASE=1 packages/build-rpms.sh`.

## Changelog and release notes

Before a release, add its section to `CHANGELOG.md`/`CHANGELOG-us.md` and write
`packages/release-notes/VERSION.md`/`VERSION-us.md`. `github-release.sh` puts the
notes for the workspace's version in `NOTES.md`.

## GitHub releases

The packages are published as files on the project's
[GitHub releases](https://github.com/shandyCho/useless-init-converter/releases):

```sh
packages/github-release.sh             # result: ~/uinitconverter-release
packages/github-release.sh DIR         # somewhere else
```

It needs a committed tree, then runs `packages/build-rpms.sh` and
`standalone/*/build-rpm.sh` in a fresh `DIR/work` and collects:

- `DIR/assets/`: the binary RPMs (the five core packages and the standalone
  tools; no debuginfo packages) and `SHA256SUMS`. The skarnet.org packages
  are not in a release: each user builds them with
  `dist/rpm/build-skarnet-rpms.sh`.
- `DIR/NOTES.md`: the release text, with the commit, the Fedora release and
  architecture the RPMs are built for, the install commands and the list of
  packages.

At the end it prints the `gh release create` command (tag
`vVERSION`, from `[workspace.package] version`) to run where the GitHub CLI
is logged in. Without it: on GitHub, Releases > Draft a new release, the
tag `vVERSION` on that commit, NOTES.md as the text, and every file in
`DIR/assets` attached. A tag can only be used once, so bump the version in
`Cargo.toml` before each release.

The RPMs are built for the Fedora release and architecture of the machine
that builds them (e.g. `fc45`, `x86_64`); build on each Fedora release you
want to support. Users first build and install the skarnet.org packages
from the release's tag:

```sh
git clone --branch vVERSION https://github.com/shandyCho/useless-init-converter.git
sudo dnf install git rpm-build gcc make cpio
useless-init-converter/dist/rpm/build-skarnet-rpms.sh            # tarballs downloaded into ~/skarnet-sources first (README-us.md)
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm
```

then, in an empty directory:

```sh
gh release download vVERSION -R shandyCho/useless-init-converter    # or download the files in a browser
sha256sum -c SHA256SUMS
sudo dnf install ./*.rpm
initconvert init --dry-run
sudo initconvert init
```

dnf takes rsyslog, earlyoom and the other dependencies from Fedora's
repositories. There is no repository on GitHub, so `dnf upgrade` alone
does not see new releases: download the next release's files and run
`sudo dnf upgrade ./*.rpm`, then `sudo initconvert init --yes`. The skarnet
packages need rebuilding only when `dist/skarnet.versions` changes.

The specs still say `License: TBD`; choose a license before publishing.

## Installing local builds

```sh
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm      # skarnet tools (dist/rpm/build-skarnet-rpms.sh)
sudo dnf install ~/rpmbuild-uinitconverter/rpms/*.rpm
sudo dnf install ~/rpmbuild-uinitconverter-standalone/rpms/*.rpm   # optional (standalone/*/build-rpm.sh)
sudo initconvert init
```

After the Rust code changed, rebuild and upgrade:

```sh
git pull                                            # or make your change and commit it
packages/build-rpms.sh
sudo dnf upgrade ~/rpmbuild-uinitconverter/rpms/*.rpm
initconvert --version                                  # which build is installed
```

On a running s6 boot, the new initconvertd is used once the daemon restarts
(`s6-svc -r /run/service/uinitconverterd`). The runner and timers are used the next
time a service starts, and initconvert-gen at the next `initconvert reload`
(`systemctl daemon-reload` runs initconvertd's copy, so it changes once the
daemon restarts). Nothing is regenerated at boot.

Moving from a version without the s6-rc repository (before the `s6 set`
change): rerun `sudo initconvert init`. It removes `/etc/uinitconverter/s6/default`
(the set's commit makes the `default` bundle now; a reload refuses to run
while it exists), writes the new `/etc/s6.conf`, creates `/etc/s6/sources`,
loads the new SELinux module and creates the repository with the set
`current` from systemd's enable state.

The s6-linux-init skeleton (`rc.init`, `rc.shutdown`, `rc.shutdown.final`)
is different: the package installs the copy init works from, but the boot
uses `/etc/s6-linux-init/current`, which `s6-linux-init-maker` makes from it.
When an upgrade changes the skeleton, rerun `sudo initconvert init` (it keeps the
choices of the last run) to rebuild it; `early` and the rest take effect at
the next boot without that.

### Moving to the initconvert commands

The commands were renamed from `uinitconverter*` to `initconvert*`:
`initconvert` (`/usr/bin/initconvert`), `initconvertd`, `initconvert-gen`,
`initconvert-runner`, `initconvert-timers`, `initconvert-user-manager` in
`/usr/libexec/uinitconverter/`, and the standalone `initconvert-fdstore`,
`initconvert-notify`, `initconvert-journal-relay` and `initconvert-linkwatch`.
Everything else keeps its name: the packages, `/etc/uinitconverter`,
`/var/lib/uinitconverter`, `/run/uinitconverter`, the s6 services
(`uinitconverterd`, `uinitconverter-fdstore`, ...), the SELinux module, the
D-Bus interface, the kernel options and `UINITCONVERTER_*` variables. There is
no automatic migration. The s6-rc database and the s6-linux-init tree name
the programs by path, so after the upgrade they are made again. From the
**normal systemd boot**:

```sh
sudo dnf upgrade ~/rpmbuild-uinitconverter/rpms/*.rpm \
    ~/rpmbuild-uinitconverter-standalone/rpms/*.rpm   # or a release's RPMs
sudo initconvert init          # same choices as the last run: regenerates
                               # the database, rebuilds the s6-linux-init tree
```

If you edited a service under `/etc/uinitconverter/s6` (it is
`%config(noreplace)`), dnf keeps your copy, which still names the old
path, and puts the new one beside it as `.rpmnew`: carry the new path over
before running `initconvert init`
(`sudo find /etc/uinitconverter -name '*.rpmnew'`).

Then boot the s6 entry. Scripts of your own that call `uinitconverter`,
`uinitconverterd` or another old name need the new one; settings and the
s6-rc set are kept. Don't upgrade on a running s6 boot: until `initconvert init`
has run, a service that (re)starts still runs the old path, which is gone.

### Moving from sdcompat

The project was called sdcompat before; every name changed (commands,
packages, `/etc/sdcompat` → `/etc/uinitconverter`, `/var/lib/sdcompat`,
the SELinux module, the D-Bus interface, `sdcompat.db=` →
`uinitconverter.db=`, the GRUB entry). There is no automatic migration:
remove sdcompat, then install uinitconverter as new. From the **normal
systemd boot**:

```sh
sudo cp -a /etc/sdcompat ~/sdcompat-etc     # your settings, to copy over by hand
cat /etc/sdcompat/boot-options              # the choices of the last init run
sudo sdcompat init --uninstall              # s6 boot entries, initramfs, s6-linux-init tree
sudo dnf remove sdcompat sdcompat-gen sdcompat-runner sdcompat-timers sdcompatd \
    sdcompat-fdstore sdcompat-journal-relay sdcompat-linkwatch sdcompat-notify
sudo mv /var/lib/s6-rc/repository /var/lib/s6-rc/repository.sdcompat
sudo rm -rf /var/lib/sdcompat /var/log/sdcompat
```

Removing the packages also unloads the old SELinux module. The s6-rc
repository is moved aside because its set still names the old services;
`initconvert init` creates a new one from systemd's enable state (`s6
set` changes made by hand have to be made again). Then install the
uinitconverter RPMs and run `sudo initconvert init` with the options you
used before (e.g. `--journal=no --boot-only-s6=yes --selinux=enforcing`;
the old choices are not read), and copy your edits from `~/sdcompat-etc`
into `/etc/uinitconverter` (the file names are the same except
`sdcompat.conf` → `uinitconverter.conf`).

To remove: `sudo initconvert init --uninstall`, then
`dnf remove uinitconverter uinitconverter-gen uinitconverter-runner uinitconverter-timers uinitconverterd`.
