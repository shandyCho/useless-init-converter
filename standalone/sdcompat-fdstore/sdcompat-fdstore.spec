# Build from a tarball that contains vendored crates:
#   cargo vendor && tar czf sdcompat-fdstore-0.1.0.tar.gz --transform 's,^,sdcompat-fdstore-0.1.0/,' \
#       Cargo.toml Cargo.lock src vendor .cargo
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           sdcompat-fdstore
Version:        0.1.0
Release:        1%{?dist}
Summary:        Holds file descriptors for services across restarts
# Choose a license before distributing.
License:        TBD
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust

%description
Keeps file descriptors that services hand over with FDSTORE=1, and the
listening sockets of socket-activated services, in a process of its own, so
they survive restarts of the service and of the service manager. Reads no
configuration or unit files; the client says how many fds a key may hold.

%prep
%autosetup

%build
cargo build --release --offline

%check
cargo test --release --offline

%install
install -Dm0755 target/release/sdcompat-fdstore %{buildroot}%{_libexecdir}/sdcompat/sdcompat-fdstore

%files
%dir %{_libexecdir}/sdcompat
%{_libexecdir}/sdcompat/sdcompat-fdstore

%changelog
* Wed Oct 21 2026 sdcompat developers - 0.1.0-1
- Split out of sdcompat
