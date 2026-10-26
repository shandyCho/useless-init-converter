# Build with ./build-rpm.sh, which makes the source tarball (Cargo.toml
# Cargo.lock src vendor .cargo under sdcompat-notify-VERSION/, with .cargo/config.toml
# pointing [source.crates-io] at vendor/) and defines build_release from the
# build time and git commit, so each rebuild is an upgrade for dnf. Plain
# rpmbuild on such a tarball gives release 1.
Name:           sdcompat-notify
Version:        0.1.0
Release:        %{?build_release}%{!?build_release:1}%{?dist}
Summary:        systemd-notify replacement
# Choose a license before distributing.
License:        TBD
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust

%description
Sends one sd_notify message (READY=1, STATUS=, ...) to the socket in
$NOTIFY_SOCKET. Works under any service manager that sets it.

%prep
%autosetup

%build
cargo build --release --offline

%check
cargo test --release --offline

%install
install -Dm0755 target/release/sdcompat-notify %{buildroot}%{_libexecdir}/sdcompat/sdcompat-notify

%files
%dir %{_libexecdir}/sdcompat
%{_libexecdir}/sdcompat/sdcompat-notify

%changelog
* Wed Oct 21 2026 sdcompat developers - 0.1.0-1
- Split out of sdcompat
