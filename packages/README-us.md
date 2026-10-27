# uinitconverter RPM packages

[한국어](README.md)

Each executable of the core (`crates/`) is its own RPM package. The source
stays in the one Cargo workspace, and all five packages build from the same
source tarball. The library crates (`uinitconverter-base`, `uinitconverter-install`,
`uinitconverter-exec`, `uinitconverter-client`) are linked statically into the
executables, so they are used only to build and are not packaged.

| Package | Contains | Needs at run time (`Requires`) |
|---|---|---|
| `uinitconverter-runner` | `uinitconverter-runner` | s6, shadow-utils. Recommends `uinitconverterd` (`OnFailure=`/`OnSuccess=` units are started through the daemon) |
| `uinitconverterd` | `uinitconverterd`, the D-Bus policy (`uinitconverter.conf`) | s6, s6-rc, dbus-daemon, `uinitconverter-runner`. Recommends `uinitconverter-timers` (`systemd-run --on-calendar`) |
| `uinitconverter-timers` | `uinitconverter-timers` | `uinitconverterd` (timers start units through the daemon) |
| `uinitconverter-gen` | `uinitconverter-gen`, `/var/lib/uinitconverter` | s6-rc, `uinitconverter-runner`, `uinitconverter-timers` (the generated services run them) |
| `uinitconverter` | the `uinitconverter` command (with `uinitconverter init`, the boot setup), `early`, configuration, s6 services, s6-linux-init skeleton, SELinux module, kernel-install plugin, `/var/log/uinitconverter` | s6, s6-linux-init, s6-rc, execline, dbus-daemon, util-linux, policycoreutils-python-utils, grubby, and the four packages above |

- `dnf install uinitconverter` installs everything. The boot configuration and the
  SELinux module are in the `uinitconverter` package, so booting with s6 needs it.
- The tool packages can be updated on their own. They are tied to the same
  version (`= version-release`).
- The standalone tools (`standalone/`) are not here. Each directory has its
  own spec and `build-rpm.sh` (result: `~/rpmbuild-uinitconverter-standalone/rpms`), and
  `uinitconverter` only points at them with `Recommends`/`Suggests`.

## Building

```sh
packages/build-rpms.sh            # result: ~/rpmbuild-uinitconverter/rpms
packages/build-rpms.sh DIR        # somewhere else
packages/build-rpms.sh --srpm     # source RPMs only: ~/rpmbuild-uinitconverter/srpms
```

The script packs the workspace (`Cargo.toml`, `Cargo.lock`, `crates`, `dist`)
and the crates fetched with `cargo vendor` into one `uinitconverter-VERSION.tar.gz`,
then runs `rpmbuild -bb` for each spec. rpmbuild builds without network
(`--offline`). Nothing is installed and no sudo is used. Needs `rpm-build`,
`cargo` and `rust`.

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
  `--version` (e.g. `uinitconverter-gen 0.1.0-1.20261021073517.gc8aabf8.fc45`);
  built from source they print the commit, like `0.1.0 (git c8aabf8)`.
- To set the release yourself, e.g. for a published release:
  `UINITCONVERTER_RELEASE=1 packages/build-rpms.sh`.

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
uinitconverter init --dry-run
sudo uinitconverter init
```

dnf takes rsyslog, earlyoom and the other dependencies from Fedora's
repositories. There is no repository on GitHub, so `dnf upgrade` alone
does not see new releases: download the next release's files and run
`sudo dnf upgrade ./*.rpm`, then `sudo uinitconverter init --yes`. The skarnet
packages need rebuilding only when `dist/skarnet.versions` changes.

The specs still say `License: TBD`; choose a license before publishing.

## Installing local builds

```sh
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm      # skarnet tools (dist/rpm/build-skarnet-rpms.sh)
sudo dnf install ~/rpmbuild-uinitconverter/rpms/*.rpm
sudo dnf install ~/rpmbuild-uinitconverter-standalone/rpms/*.rpm   # optional (standalone/*/build-rpm.sh)
sudo uinitconverter init
```

After the Rust code changed, rebuild and upgrade:

```sh
git pull                                            # or make your change and commit it
packages/build-rpms.sh
sudo dnf upgrade ~/rpmbuild-uinitconverter/rpms/*.rpm
uinitconverter --version                                  # which build is installed
```

On a running s6 boot, the new uinitconverterd is used once the daemon restarts
(`s6-svc -r /run/service/uinitconverterd`). The runner and timers are used the next
time a service starts, and uinitconverter-gen at the next `uinitconverter reload`
(`systemctl daemon-reload` runs uinitconverterd's copy, so it changes once the
daemon restarts). Nothing is regenerated at boot.

Moving from a version without the s6-rc repository (before the `s6 set`
change): rerun `sudo uinitconverter init`. It removes `/etc/uinitconverter/s6/default`
(the set's commit makes the `default` bundle now; a reload refuses to run
while it exists), writes the new `/etc/s6.conf`, creates `/etc/s6/sources`,
loads the new SELinux module and creates the repository with the set
`current` from systemd's enable state.

The s6-linux-init skeleton (`rc.init`, `rc.shutdown`, `rc.shutdown.final`)
is different: the package installs the copy init works from, but the boot
uses `/etc/s6-linux-init/current`, which `s6-linux-init-maker` makes from it.
When an upgrade changes the skeleton, rerun `sudo uinitconverter init` (it keeps the
choices of the last run) to rebuild it; `early` and the rest take effect at
the next boot without that.

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
`uinitconverter init` creates a new one from systemd's enable state (`s6
set` changes made by hand have to be made again). Then install the
uinitconverter RPMs and run `sudo uinitconverter init` with the options you
used before (e.g. `--journal=no --boot-only-s6=yes --selinux=enforcing`;
the old choices are not read), and copy your edits from `~/sdcompat-etc`
into `/etc/uinitconverter` (the file names are the same except
`sdcompat.conf` → `uinitconverter.conf`).

To remove: `sudo uinitconverter init --uninstall`, then
`dnf remove uinitconverter uinitconverter-gen uinitconverter-runner uinitconverter-timers uinitconverterd`.
