# Build from a tarball that contains vendored crates:
#   cargo vendor && tar czf sdcompat-journal-relay-0.1.0.tar.gz --transform 's,^,sdcompat-journal-relay-0.1.0/,' \
#       Cargo.toml Cargo.lock src vendor .cargo
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           sdcompat-journal-relay
Version:        0.1.0
Release:        1%{?dist}
Summary:        journald's client sockets, forwarded to syslog
# Choose a license before distributing.
License:        TBD
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
Recommends:     rsyslog

%description
Listens on journald's native and stdout stream sockets in
/run/systemd/journal and forwards what arrives to syslog (/dev/log), for
systems that run without systemd-journald. Some programs, such as
dbus-broker-launch, refuse to start when those sockets are missing.

%prep
%autosetup

%build
cargo build --release --offline

%check
cargo test --release --offline

%install
install -Dm0755 target/release/sdcompat-journal-relay %{buildroot}%{_libexecdir}/sdcompat/sdcompat-journal-relay

%files
%dir %{_libexecdir}/sdcompat
%{_libexecdir}/sdcompat/sdcompat-journal-relay

%changelog
* Wed Oct 21 2026 sdcompat developers - 0.1.0-1
- Split out of sdcompat
