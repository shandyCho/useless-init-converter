# Build with ./build-rpm.sh, which makes the source tarball (Cargo.toml
# Cargo.lock src vendor .cargo under uinitconverter-journal-relay-VERSION/, with .cargo/config.toml
# pointing [source.crates-io] at vendor/) and defines build_release from the
# build time and git commit, so each rebuild is an upgrade for dnf. Plain
# rpmbuild on such a tarball gives release 1.
Name:           uinitconverter-journal-relay
Version:        0.2.0
Release:        %{?build_release}%{!?build_release:1}%{?dist}
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
install -Dm0755 target/release/initconvert-journal-relay %{buildroot}%{_libexecdir}/uinitconverter/initconvert-journal-relay

%files
%dir %{_libexecdir}/uinitconverter
%{_libexecdir}/uinitconverter/initconvert-journal-relay

%changelog
* Mon Nov 09 2026 uinitconverter developers - 0.2.0-1
- The command is now initconvert-journal-relay (the package keeps its name)

* Wed Oct 21 2026 uinitconverter developers - 0.1.0-1
- Split out of uinitconverter
