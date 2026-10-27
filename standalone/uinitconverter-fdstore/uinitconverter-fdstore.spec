# Build with ./build-rpm.sh, which makes the source tarball (Cargo.toml
# Cargo.lock src vendor .cargo under uinitconverter-fdstore-VERSION/, with .cargo/config.toml
# pointing [source.crates-io] at vendor/) and defines build_release from the
# build time and git commit, so each rebuild is an upgrade for dnf. Plain
# rpmbuild on such a tarball gives release 1.
Name:           uinitconverter-fdstore
Version:        0.1.0
Release:        %{?build_release}%{!?build_release:1}%{?dist}
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
install -Dm0755 target/release/uinitconverter-fdstore %{buildroot}%{_libexecdir}/uinitconverter/uinitconverter-fdstore

%files
%dir %{_libexecdir}/uinitconverter
%{_libexecdir}/uinitconverter/uinitconverter-fdstore

%changelog
* Wed Oct 21 2026 uinitconverter developers - 0.1.0-1
- Split out of uinitconverter
