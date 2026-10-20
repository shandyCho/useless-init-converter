# One of the sdcompat RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under sdcompat-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           sdcompat-gen
# Version comes from Cargo.toml and release from the build time and git
# commit (build-rpms.sh defines them); build with that script, not with
# rpmbuild directly.
Version:        %{sdcompat_version}
Release:        %{sdcompat_release}%{?dist}
Summary:        Translates systemd unit files into an s6-rc database
# Choose a license before distributing.
License:        TBD
Source0:        sdcompat-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
Requires:       s6-rc
# The generated services run sdcompat-runner; .timer and .path units run
# sdcompat-timers.
Requires:       sdcompat-runner = %{version}-%{release}
Requires:       sdcompat-timers = %{version}-%{release}

%description
Reads systemd unit files, compiles them together with hand-written s6-rc
services into an s6-rc database (s6-rc-compile) and switches the live state
to it (s6-rc-update). Also enable, disable, mask and preset units.

%prep
%autosetup -n sdcompat-%{version}

%build
cargo build --release --offline -p sdcompat-gen

%check
cargo test --release --offline -p sdcompat-gen

%install
install -Dm0755 target/release/sdcompat-gen %{buildroot}%{_libexecdir}/sdcompat/sdcompat-gen
install -d %{buildroot}%{_sharedstatedir}/sdcompat

%files
%dir %{_libexecdir}/sdcompat
%{_libexecdir}/sdcompat/sdcompat-gen
%dir %{_sharedstatedir}/sdcompat

%changelog
* Wed Oct 21 2026 sdcompat developers - 0.1.0-1
- Split out of the sdcompat package
