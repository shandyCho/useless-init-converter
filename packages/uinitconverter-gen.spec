# One of the uinitconverter RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under uinitconverter-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           uinitconverter-gen
# Version comes from Cargo.toml and release from the build time and git
# commit (build-rpms.sh defines them); build with that script, not with
# rpmbuild directly.
Version:        %{uinitconverter_version}
Release:        %{uinitconverter_release}%{?dist}
Summary:        Translates systemd unit files into an s6-rc database
# Choose a license before distributing.
License:        TBD
Source0:        uinitconverter-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
Requires:       s6-rc
# The generated services run uinitconverter-runner; .timer and .path units run
# uinitconverter-timers.
Requires:       uinitconverter-runner = %{version}-%{release}
Requires:       uinitconverter-timers = %{version}-%{release}

%description
Reads systemd unit files, compiles them together with hand-written s6-rc
services into an s6-rc database (s6-rc-compile) and switches the live state
to it (s6-rc-update). Also enable, disable, mask and preset units.

%prep
%autosetup -n uinitconverter-%{version}

%build
# --version prints the package's version-release.
export UINITCONVERTER_BUILD=%{version}-%{release}
cargo build --release --offline -p uinitconverter-gen

%check
export UINITCONVERTER_BUILD=%{version}-%{release}
# The library crates it is built from too: they are not packaged.
cargo test --release --offline -p uinitconverter-gen -p uinitconverter-base -p uinitconverter-install -p uinitconverter-client -p uinitconverter-version

%install
install -Dm0755 target/release/uinitconverter-gen %{buildroot}%{_libexecdir}/uinitconverter/uinitconverter-gen
install -d %{buildroot}%{_sharedstatedir}/uinitconverter

%files
%dir %{_libexecdir}/uinitconverter
%{_libexecdir}/uinitconverter/uinitconverter-gen
%dir %{_sharedstatedir}/uinitconverter

%changelog
* Wed Oct 21 2026 uinitconverter developers
- Split out of the uinitconverter package
