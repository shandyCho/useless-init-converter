# One of the uinitconverter RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under uinitconverter-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           uinitconverter-devices
# Version comes from Cargo.toml and release from the build time and git
# commit (build-rpms.sh defines them); build with that script, not with
# rpmbuild directly.
Version:        %{uinitconverter_version}
Release:        %{uinitconverter_release}%{?dist}
Summary:        Starts the units udev's devices want under s6
# Choose a license before distributing.
License:        TBD
Source0:        uinitconverter-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
# The units a device wants are started through initconvertd.
Requires:       uinitconverterd = %{version}-%{release}

%description
A long-running s6 service that reads udev's database and udevd's events and
starts the units devices want (SYSTEMD_WANTS: bluetooth.target for a
Bluetooth controller, printer.target, sound.target, ...) through
initconvertd.

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
cargo build --release --offline -p uinitconverter-devices

%check
export UINITCONVERTER_BUILD=%{version}-%{release}
# The library crates it is built from too: they are not packaged.
cargo test --release --offline -p uinitconverter-devices -p uinitconverter-base -p uinitconverter-client -p uinitconverter-version

%install
install -Dm0755 target/release/initconvert-devices %{buildroot}%{_libexecdir}/uinitconverter/initconvert-devices

%files
%dir %{_libexecdir}/uinitconverter
%{_libexecdir}/uinitconverter/initconvert-devices

%changelog
* Fri Nov 13 2026 uinitconverter developers
- 0.5.0, see CHANGELOG-us.md

* Thu Nov 12 2026 uinitconverter developers
- Split out of uinitconverterd (the daemon read udev's events itself)
