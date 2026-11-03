# Building the packages: test builds and release builds

**English** | [한국어](PACKAGEBUILD.md)

How to build the RPMs, for testing and for a release, step by step with the
commands. What each package contains is in
[packages/README-us.md](../packages/README-us.md).

## Overview

There are three sets of packages, each with its own build script:

| Set | Packages | Script | Result (default) |
|---|---|---|---|
| skarnet.org | skalibs, execline, s6, s6-rc, s6-linux-init, s6-frontend, mdevd, s6-dns, s6-networking | `dist/rpm/build-skarnet-rpms.sh` | `~/rpmbuild-skarnet/rpms` |
| Core | uinitconverter, uinitconverterd, uinitconverter-gen, -runner, -timers, -user | `packages/build-rpms.sh` | `~/rpmbuild-uinitconverter/rpms` |
| Standalone tools | uinitconverter-fdstore, -notify, -linkwatch, -journal-relay | `standalone/*/build-rpm.sh` | `~/rpmbuild-uinitconverter-standalone/rpms` |

None of the scripts installs anything or needs sudo. rpmbuild builds without
network: the Rust crates are vendored into the source tarball.

**Test build or release build?**

| | Test build | Release build |
|---|---|---|
| For | trying a change: in the VM, on your own machine | publishing on GitHub |
| Core command | `packages/build-rpms.sh --test DIR` | `packages/github-release.sh` |
| Tree | may have uncommitted changes (release gets `.dirty`) | must be committed |
| Time (six core packages) | about 3-4 min, less when the cache is kept | about 11 min |
| `cargo test` in rpmbuild (`%check`) | skipped | runs |
| debuginfo/debugsource packages | none | built (not published) |
| Version bump, changelogs, notes | no | yes |

The packages contain the same files either way. A test build only skips
the tests inside rpmbuild and shares one compile between the specs.

## 0. Once: the build tools and the skarnet.org packages

```sh
sudo dnf install -y git curl rpm-build gcc make cpio cargo rust
```

The skarnet.org packages are needed to install and run uinitconverter,
not to build it. Fedora doesn't package them. Download the versions pinned
in `dist/skarnet.versions` (the script checks each file's SHA-256 and
downloads nothing itself), then build:

```sh
mkdir -p ~/skarnet-sources
grep -v '^#' dist/skarnet.versions | while read -r name ver _; do
    [ -n "$name" ] || continue
    curl -fL -o ~/skarnet-sources/"$name-$ver.tar.gz" \
        "https://skarnet.org/software/$name/$name-$ver.tar.gz"
done
dist/rpm/build-skarnet-rpms.sh          # result: ~/rpmbuild-skarnet/rpms
```

Rebuild them only when `dist/skarnet.versions` changes. Options:
`--sources DIR` (tarballs elsewhere), `--srpm` (source RPMs only), and an
output directory as the last argument. With mock installed (and you in the
`mock` group), they build in clean chroots.

## 1. Test build

### Build

```sh
packages/build-rpms.sh --test ~/uinitconverter-test          # core: ~/uinitconverter-test/rpms
for s in standalone/*/build-rpm.sh; do "$s"; done             # standalone (only when they changed)
```

`--test` is two options, which can also be used one at a time:

| Option | What it does |
|---|---|
| `--shared-target` | Every spec builds from one copy of the source at a fixed path into one cargo target directory, both under `DIR/cargo-target`. Cargo then compiles each crate once instead of once per spec. The directory is kept, so the next build in the same `DIR` recompiles only what changed. No debuginfo or debugsource packages |
| `--nocheck` | rpmbuild skips `%check`, so no `cargo test` runs inside it. Run the tests in the tree instead (below) |

Keep using the same `DIR` for test builds: the kept cache is what makes the
next one fast. Times on the development machine, six core packages: about
11 min without options, 253 s with `--test` and an empty cache, 198 s with
the cache kept. Most of the rest is the optimized (LTO) link of each binary.

Every build gets a newer release number, `1.YYYYMMDDHHMMSS.gCOMMIT`, with
`.dirty` when the tree has uncommitted changes. So dnf always sees a rebuild
as an upgrade, even when the version is the same.

### Run the tests

The tests that `--nocheck` skipped, and the sandbox suite (no root, nothing
on the host changes):

```sh
cargo test
for p in standalone/*/; do cargo test --manifest-path "$p/Cargo.toml"; done
tests/sandbox.sh /tmp/sb && tests/check.sh /tmp/sb; tests/sandbox.sh /tmp/sb stop
tests/user-sandbox.sh /tmp/usb
```

See [TESTING-us.md](../TESTING-us.md) for the variants
(`SANDBOX_JOURNAL=no`, `SANDBOX_FDSTORE=no`, `SANDBOX_S6NET_BIN=`).

### Try the RPMs in the VM

The GNOME test VM (set up once with `tests/gnome-vm.sh fetch` and
`provision`, see TESTING-us.md) installs the RPMs on a fresh copy each run:

```sh
# The s6 boot: the RPMs, initconvert init, a reboot, checks.
CORE_RPMS=~/uinitconverter-test/rpms tests/vm-s6-boot.sh ~/uinitconverter-vm s6
# The s6 user manager on a normal systemd boot (uinitconverter-user only).
BOOT=systemd-rpm CORE_RPMS=~/uinitconverter-test/rpms tests/vm-user-manager.sh ~/uinitconverter-vm session
```

`vm-s6-boot.sh` takes the standalone RPMs from
`~/uinitconverter-vm/rpmbuild/standalone/rpms` (`STANDALONE_RPMS=` to change
it) and the skarnet.org ones from `~/rpmbuild-skarnet/rpms` (`SKARNET_RPMS=`).

### Install on your own machine

Take a VM snapshot or have the normal systemd boot to fall back on. First
time:

```sh
sudo dnf install ~/rpmbuild-skarnet/rpms/*.rpm
sudo dnf install ~/uinitconverter-test/rpms/*.rpm ~/rpmbuild-uinitconverter-standalone/rpms/*.rpm
sudo initconvert init
```

After a rebuild, from the systemd boot:

```sh
sudo dnf upgrade ~/uinitconverter-test/rpms/*.rpm
sudo initconvert init --yes
```

`rpms/` holds only the last build's packages, so `*.rpm` is always one
matching set (the six core packages require each other's exact
version-release).

## 2. Release build

The release publishes the core and standalone RPMs, `SHA256SUMS` and the
release text on the project's GitHub releases. The skarnet.org packages are
not in it: users build them as in step 0.

### 2.1 Prepare the release commit

1. **Version.** Set `[workspace.package] version` in `Cargo.toml`, then run
   `cargo build` so `Cargo.lock` follows. A tag can only be used once, so
   every release needs a new version. The standalone tools have versions of
   their own (their own `Cargo.toml` and spec); bump them only when they
   changed.
2. **README pair.** `VERSION=` in the install steps of `README.md` and
   `README-us.md`.
3. **Changelog pair.** In `CHANGELOG.md` and `CHANGELOG-us.md`, rename the
   `Unreleased` section to the version and the date.
4. **Release notes pair.** Write `packages/release-notes/VERSION.md` and
   `VERSION-us.md` (copy the previous ones for the layout). `github-release.sh`
   puts them in the release text.
5. **Spec changelogs.** Add an entry to the `%changelog` of each of the six
   specs in `packages/`. The specs carry no version or release;
   `build-rpms.sh` sets them.

### 2.2 Test the release tree

The whole suite, on exactly the tree you'll tag:

```sh
cargo fmt --check && cargo test
for p in standalone/*/; do cargo test --manifest-path "$p/Cargo.toml"; done
for v in "" SANDBOX_JOURNAL=no SANDBOX_FDSTORE=no; do
    env $v tests/sandbox.sh /tmp/sb && env $v tests/check.sh /tmp/sb; env $v tests/sandbox.sh /tmp/sb stop
done
tests/user-sandbox.sh /tmp/usb
```

Then the VM runs from section 1 on RPMs of this tree, and at least one real
boot when the change touches the boot.

### 2.3 Commit and build

```sh
git add -A && git commit -m "Release VERSION"
packages/github-release.sh                 # result: ~/uinitconverter-release
```

`github-release.sh` refuses to run with uncommitted changes. It builds the
core and the standalone tools in a fresh `~/uinitconverter-release/work`
(normal builds: separate specs, `%check` runs) and collects:

- `~/uinitconverter-release/assets/`: the binary RPMs (no -devel or
  debuginfo packages) and `SHA256SUMS`;
- `~/uinitconverter-release/NOTES.md`: the release text, with the commit,
  the Fedora release and architecture, the install commands and the package
  list.

Check the assets:

```sh
cd ~/uinitconverter-release/assets && sha256sum -c SHA256SUMS && ls
```

The RPMs are built for this machine's Fedora release and architecture
(for example `fc45`, `x86_64`). Build on each Fedora release you want to
support.

`UINITCONVERTER_RELEASE=N` in front of the command sets the release number
yourself, instead of the build time and commit.

### 2.4 Publish

This needs GitHub credentials, so it runs where you're logged in (this
development machine has none):

```sh
git push origin main
gh release create vVERSION -R shandyCho/useless-init-converter --target COMMIT \
    --title "uinitconverter VERSION" --notes-file ~/uinitconverter-release/NOTES.md \
    ~/uinitconverter-release/assets/*
```

`github-release.sh` prints this line with the version and commit filled in.
Without the GitHub CLI: Releases > Draft a new release on GitHub, the tag
`vVERSION` on that commit, NOTES.md as the text, and every file in `assets/`
attached.

## Source RPMs

For building elsewhere (mock, a build service), each script can make
source RPMs instead, with the version, release and vendored tarball inside,
so they rebuild offline as they are:

```sh
dist/rpm/build-skarnet-rpms.sh --srpm          # ~/rpmbuild-skarnet/srpms; build in skarnet.versions order
packages/build-rpms.sh --srpm                  # ~/rpmbuild-uinitconverter/srpms; the six go together
standalone/uinitconverter-fdstore/build-rpm.sh --srpm
```

`--test`, `--shared-target` and `--nocheck` build binary RPMs, so they don't
go with `--srpm`.

## When something goes wrong

| Message | Cause, and what to do |
|---|---|
| `error: P failed, see DIR/P.log` | rpmbuild failed for package P; the log has rpmbuild's output |
| `uncommitted changes in ...: commit them first` | `github-release.sh` builds only a committed tree |
| a skarnet tarball listed as missing, or a SHA-256 mismatch | download it again into `~/skarnet-sources`; the sums are in `dist/skarnet.versions` |
| `rpmbuild missing` / `cargo missing` | step 0's `dnf install` |
| `dnf upgrade` installs nothing | the release didn't change: rebuild, or use `dnf reinstall` |
