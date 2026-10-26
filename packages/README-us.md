# sdcompat RPM packages

[한국어](README.md)

Each executable of the core (`crates/`) is its own RPM package. The source
stays in the one Cargo workspace, and all five packages build from the same
source tarball. The library crates (`sdcompat-base`, `sdcompat-install`,
`sdcompat-exec`, `sdcompat-client`) are linked statically into the
executables, so they are used only to build and are not packaged.

| Package | Contains | Needs at run time (`Requires`) |
|---|---|---|
| `sdcompat-runner` | `sdcompat-runner` | s6, shadow-utils. Recommends `sdcompatd` (`OnFailure=`/`OnSuccess=` units are started through the daemon) |
| `sdcompatd` | `sdcompatd`, the D-Bus policy (`sdcompat.conf`) | s6, s6-rc, dbus-daemon, `sdcompat-runner`. Recommends `sdcompat-timers` (`systemd-run --on-calendar`) |
| `sdcompat-timers` | `sdcompat-timers` | `sdcompatd` (timers start units through the daemon) |
| `sdcompat-gen` | `sdcompat-gen`, `/var/lib/sdcompat` | s6-rc, `sdcompat-runner`, `sdcompat-timers` (the generated services run them) |
| `sdcompat` | the `sdcompat` command (with `sdcompat init`, the boot setup), `early`, configuration, s6 services, s6-linux-init skeleton, SELinux module, kernel-install plugin, `/var/log/sdcompat` | s6, s6-linux-init, s6-rc, execline, dbus-daemon, util-linux, policycoreutils-python-utils, grubby, and the four packages above |

- `dnf install sdcompat` installs everything. The boot configuration and the
  SELinux module are in the `sdcompat` package, so booting with s6 needs it.
- The tool packages can be updated on their own. They are tied to the same
  version (`= version-release`).
- The standalone tools (`standalone/`) are not here. Each directory has its
  own spec and `build-rpm.sh` (result: `~/rpmbuild-sdcompat-standalone/rpms`), and
  `sdcompat` only points at them with `Recommends`/`Suggests`.

## Building

```sh
packages/build-rpms.sh            # result: ~/rpmbuild-sdcompat/rpms
packages/build-rpms.sh DIR        # somewhere else
packages/build-rpms.sh --srpm     # source RPMs only: ~/rpmbuild-sdcompat/srpms (see COPR)
```

The script packs the workspace (`Cargo.toml`, `Cargo.lock`, `crates`, `dist`)
and the crates fetched with `cargo vendor` into one `sdcompat-VERSION.tar.gz`,
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
  `sdcompat-gen-0.1.0-1.20261021071849.g0023111.fc45`. The build time comes
  first, so when only the Rust code changed and the version did not, dnf
  still sees the new build as an upgrade.
- The five packages are tied to one version-release, and the script always
  builds all five together. `rpms/` holds only the last build's five.
- The tools built as RPMs print the package's version-release for
  `--version` (e.g. `sdcompat-gen 0.1.0-1.20261021073517.gc8aabf8.fc45`);
  built from source they print the commit, like `0.1.0 (git c8aabf8)`.
- To set the release yourself, e.g. for a published release:
  `SDCOMPAT_RELEASE=1 packages/build-rpms.sh`.

## COPR

The packages can be built on [Fedora COPR](https://copr.fedorainfracloud.org)
from source RPMs, so users install them with dnf:

```sh
packages/copr-srpms.sh                 # all 16 source RPMs: ~/sdcompat-srpms
packages/copr-srpms.sh core            # only the five core ones (after a code change)
packages/copr-srpms.sh DIR core standalone
```

It runs each build script with `--srpm` (`packages/build-rpms.sh --srpm`,
`standalone/*/build-rpm.sh --srpm`, `dist/rpm/build-skarnet-rpms.sh --srpm`),
which writes the version and release into the spec and puts the vendored
crates (or the verified skarnet.org tarball) into the source RPM, so COPR
builds them offline as they are. Results: `DIR/srpms/` and
`DIR/BUILD-ORDER.txt`:

```
stage 1: skalibs-...src.rpm
stage 2: execline-...src.rpm mdevd-...src.rpm
stage 3: s6-...src.rpm
stage 4: s6-rc-...src.rpm s6-linux-init-...src.rpm
stage 5: s6-frontend-...src.rpm
stage 6: sdcompat-...src.rpm sdcompatd-...src.rpm ... sdcompat-notify-...src.rpm
```

A skarnet package builds against the `-devel` packages of the ones before
it, so start a stage once the earlier stages have succeeded in the COPR
project (the stages come from the specs' `BuildRequires`). The Rust
packages need only cargo and rust and can build in any order. The five core
packages require each other's exact version-release: upload the five of one
run together.

One-time setup on copr.fedorainfracloud.org: New Project, pick the Fedora
chroots (e.g. `fedora-45-x86_64`); no external repositories are needed.
Then per build:

- **Web:** the project's Builds > New Build > Upload, one source RPM at a
  time, stage by stage.
- **copr-cli** (`dnf install copr-cli`, API token in `~/.config/copr`):
  `copr-cli build OWNER/sdcompat FILE...` per stage; it waits for the builds.

The skarnet packages only need rebuilding when `dist/skarnet.versions`
changes; after a code change upload `packages/copr-srpms.sh core` (and
`standalone` when a standalone tool changed). Users then run:

```sh
sudo dnf copr enable OWNER/sdcompat
sudo dnf install sdcompat
sudo sdcompat init
```

The specs still say `License: TBD`; COPR builds them, but choose a license
before publishing.

## Installing local builds

```sh
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm      # skarnet tools (dist/rpm/build-skarnet-rpms.sh)
sudo dnf install ~/rpmbuild-sdcompat/rpms/*.rpm
sudo dnf install ~/rpmbuild-sdcompat-standalone/rpms/*.rpm   # optional (standalone/*/build-rpm.sh)
sudo sdcompat init
```

After the Rust code changed, rebuild and upgrade:

```sh
git pull                                            # or make your change and commit it
packages/build-rpms.sh
sudo dnf upgrade ~/rpmbuild-sdcompat/rpms/*.rpm
sdcompat --version                                  # which build is installed
```

On a running s6 boot, the new sdcompatd is used once the daemon restarts
(`s6-svc -r /run/service/sdcompatd`). The runner and timers are used the next
time a service starts, and sdcompat-gen at the next `sdcompat reload`
(`systemctl daemon-reload` runs sdcompatd's copy, so it changes once the
daemon restarts). Nothing is regenerated at boot.

Moving from a version without the s6-rc repository (before the `s6 set`
change): rerun `sudo sdcompat init`. It removes `/etc/sdcompat/s6/default`
(the set's commit makes the `default` bundle now; a reload refuses to run
while it exists), writes the new `/etc/s6.conf`, creates `/etc/s6/sources`,
loads the new SELinux module and creates the repository with the set
`current` from systemd's enable state.

The s6-linux-init skeleton (`rc.init`, `rc.shutdown`, `rc.shutdown.final`)
is different: the package installs the copy init works from, but the boot
uses `/etc/s6-linux-init/current`, which `s6-linux-init-maker` makes from it.
When an upgrade changes the skeleton, rerun `sudo sdcompat init` (it keeps the
choices of the last run) to rebuild it; `early` and the rest take effect at
the next boot without that.

Moving from an install made by the old `dist/setup-fedora.sh`: install the
packages and run `sudo sdcompat init`. It removes the old script's copy of
the kernel-install plugin in `/etc/kernel/install.d` (which would hide the
packaged one) and reads the settings the script left. Source-built skarnet
tools (`--build-s6`) are replaced by the RPMs' files; the old manifest
`/var/lib/sdcompat/skarnet-files.txt` can be deleted.

To remove: `sudo sdcompat init --uninstall`, then
`dnf remove sdcompat sdcompat-gen sdcompat-runner sdcompat-timers sdcompatd`.
