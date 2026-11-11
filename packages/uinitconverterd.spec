# One of the uinitconverter RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under uinitconverter-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           uinitconverterd
# Version comes from Cargo.toml and release from the build time and git
# commit (build-rpms.sh defines them); build with that script, not with
# rpmbuild directly.
Version:        %{uinitconverter_version}
Release:        %{uinitconverter_release}%{?dist}
Summary:        The org.freedesktop.systemd1 D-Bus API for s6-rc
# Choose a license before distributing.
License:        TBD
Source0:        uinitconverter-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
Requires:       s6
# s6-rc and dbus-daemon are the s6 boot's: the uinitconverter package
# requires them.
# Transient services (systemd-run) and the stop/reload commands run it.
Requires:       uinitconverter-runner = %{version}-%{release}
# Transient timers (systemd-run --on-calendar) run it.
Recommends:     uinitconverter-timers = %{version}-%{release}
# The s6 user manager is gone (user@UID runs systemd --user): updating
# removes its package, whose %%preun drops the user@ drop-in that
# `initconvert-user-setup on` wrote.
Obsoletes:      uinitconverter-user < %{version}-%{release}

%description
Serves the org.freedesktop.systemd1 D-Bus API on the system bus and on
/run/systemd/private, so systemctl, libsystemd and other clients keep working
with s6-rc: unit state, start/stop jobs, daemon-reload (it regenerates the
database itself), transient units and properties.

%prep
%autosetup -n uinitconverter-%{version}

%build
# --version prints the package's version-release.
export UINITCONVERTER_BUILD=%{version}-%{release}
%if %{defined uinitconverter_target}
# build-rpms.sh --shared-target (test builds only): every package builds
# from one copy of the source at a fixed path into one target directory, so
# cargo reuses what an earlier package or build compiled (it rebuilds a
# crate whose source path changed). %%install reads target/ as usual.
rm -rf %{uinitconverter_target}/src
cp -a . %{uinitconverter_target}/src
ln -sfn %{uinitconverter_target}/target target
export CARGO_TARGET_DIR=%{uinitconverter_target}/target
cd %{uinitconverter_target}/src
%endif
cargo build --release --offline -p uinitconverterd

%check
export UINITCONVERTER_BUILD=%{version}-%{release}
# The library crates it is built from too: they are not packaged.
cargo test --release --offline -p uinitconverterd -p uinitconverter-base -p uinitconverter-client -p uinitconverter-exec -p uinitconverter-gen -p uinitconverter-install -p uinitconverter-version

%install
install -Dm0755 target/release/initconvertd %{buildroot}%{_libexecdir}/uinitconverter/initconvertd
install -Dm0644 dist/dbus/uinitconverter.conf %{buildroot}%{_datadir}/dbus-1/system.d/uinitconverter.conf

%files
%dir %{_libexecdir}/uinitconverter
%{_libexecdir}/uinitconverter/initconvertd
%{_datadir}/dbus-1/system.d/uinitconverter.conf

%changelog
* Wed Nov 11 2026 uinitconverter developers
- 0.4.0, see CHANGELOG-us.md

* Tue Nov 10 2026 uinitconverter developers
- The s6 user manager is removed: no `initconvertd user-manager`; Obsoletes
  uinitconverter-user

* Mon Nov 09 2026 uinitconverter developers
- 0.3.0, see CHANGELOG-us.md

* Tue Nov 03 2026 uinitconverter developers
- No longer requires s6-rc and dbus-daemon (the uinitconverter package does):
  the user manager needs neither

* Mon Nov 02 2026 uinitconverter developers
- 0.2.2, see CHANGELOG-us.md

* Mon Nov 02 2026 uinitconverter developers
- 0.2.1, see CHANGELOG-us.md

* Sun Nov 01 2026 uinitconverter developers
- 0.2.0, see CHANGELOG-us.md

* Wed Oct 21 2026 uinitconverter developers
- Split out of the uinitconverter package
