# One of the sdcompat RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under sdcompat-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           sdcompat-timers
# Version comes from Cargo.toml and release from the build time and git
# commit (build-rpms.sh defines them); build with that script, not with
# rpmbuild directly.
Version:        %{sdcompat_version}
Release:        %{sdcompat_release}%{?dist}
Summary:        Runs systemd .timer and .path units under s6
# Choose a license before distributing.
License:        TBD
Source0:        sdcompat-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
# The units a timer or path triggers are started through sdcompatd.
Requires:       sdcompatd = %{version}-%{release}

%description
Long-running s6 services for .timer and .path units: waits for the calendar
time, the elapsed time or the file system change, then starts the unit
through sdcompatd.

%prep
%autosetup -n sdcompat-%{version}

%build
cargo build --release --offline -p sdcompat-timers

%check
cargo test --release --offline -p sdcompat-timers

%install
install -Dm0755 target/release/sdcompat-timers %{buildroot}%{_libexecdir}/sdcompat/sdcompat-timers

%files
%dir %{_libexecdir}/sdcompat
%{_libexecdir}/sdcompat/sdcompat-timers

%changelog
* Wed Oct 21 2026 sdcompat developers - 0.1.0-1
- Split out of the sdcompat package
