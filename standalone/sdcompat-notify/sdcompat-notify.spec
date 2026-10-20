# Build from a tarball that contains vendored crates:
#   cargo vendor && tar czf sdcompat-notify-0.1.0.tar.gz --transform 's,^,sdcompat-notify-0.1.0/,' \
#       Cargo.toml Cargo.lock src vendor .cargo
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           sdcompat-notify
Version:        0.1.0
Release:        1%{?dist}
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
