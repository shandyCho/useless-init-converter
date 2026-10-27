# One of the uinitconverter RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under uinitconverter-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           uinitconverter-timers
# Version comes from Cargo.toml and release from the build time and git
# commit (build-rpms.sh defines them); build with that script, not with
# rpmbuild directly.
Version:        %{uinitconverter_version}
Release:        %{uinitconverter_release}%{?dist}
Summary:        Runs systemd .timer and .path units under s6
# Choose a license before distributing.
License:        TBD
Source0:        uinitconverter-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
# The units a timer or path triggers are started through uinitconverterd.
Requires:       uinitconverterd = %{version}-%{release}

%description
Long-running s6 services for .timer and .path units: waits for the calendar
time, the elapsed time or the file system change, then starts the unit
through uinitconverterd.

%prep
%autosetup -n uinitconverter-%{version}

%build
# --version prints the package's version-release.
export UINITCONVERTER_BUILD=%{version}-%{release}
cargo build --release --offline -p uinitconverter-timers

%check
export UINITCONVERTER_BUILD=%{version}-%{release}
# The library crates it is built from too: they are not packaged.
cargo test --release --offline -p uinitconverter-timers -p uinitconverter-base -p uinitconverter-client -p uinitconverter-version

%install
install -Dm0755 target/release/uinitconverter-timers %{buildroot}%{_libexecdir}/uinitconverter/uinitconverter-timers

%files
%dir %{_libexecdir}/uinitconverter
%{_libexecdir}/uinitconverter/uinitconverter-timers

%changelog
* Wed Oct 21 2026 uinitconverter developers
- Split out of the uinitconverter package
