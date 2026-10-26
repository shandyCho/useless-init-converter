# Build from a tarball that contains vendored crates:
#   cargo vendor && tar czf sdcompat-linkwatch-0.1.0.tar.gz --transform 's,^,sdcompat-linkwatch-0.1.0/,' \
#       Cargo.toml Cargo.lock src vendor .cargo
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           sdcompat-linkwatch
Version:        0.1.0
Release:        1%{?dist}
Summary:        Unblocks s6-rc-update waits that missed a supervisor's start
# Choose a license before distributing.
License:        TBD
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust

%description
s6-rc-update (and s6-svlink) wait for each new supervisor's start event.
When s6-svscan starts a supervisor between their check and their
subscription, the event goes to nobody and the wait never ends, holding
s6-rc's lock. This watches /proc and sends the missed event to such a
wait once it has lasted 10 seconds, for directories whose supervisor runs.

%prep
%autosetup

%build
cargo build --release --offline

%check
cargo test --release --offline

%install
install -Dm0755 target/release/sdcompat-linkwatch %{buildroot}%{_libexecdir}/sdcompat/sdcompat-linkwatch

%files
%dir %{_libexecdir}/sdcompat
%{_libexecdir}/sdcompat/sdcompat-linkwatch

%changelog
* Mon Oct 26 2026 sdcompat developers - 0.1.0-1
- First version
