# Build with ./build-rpm.sh, which makes the source tarball (Cargo.toml
# Cargo.lock src vendor .cargo under uinitconverter-linkwatch-VERSION/, with .cargo/config.toml
# pointing [source.crates-io] at vendor/) and defines build_release from the
# build time and git commit, so each rebuild is an upgrade for dnf. Plain
# rpmbuild on such a tarball gives release 1.
Name:           uinitconverter-linkwatch
Version:        0.3.0
Release:        %{?build_release}%{!?build_release:1}%{?dist}
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
install -Dm0755 target/release/initconvert-linkwatch %{buildroot}%{_libexecdir}/uinitconverter/initconvert-linkwatch

%files
%dir %{_libexecdir}/uinitconverter
%{_libexecdir}/uinitconverter/initconvert-linkwatch

%changelog
* Thu Nov 12 2026 uinitconverter developers - 0.3.0-1
- Also unblocks a start whose s6-svc -u reached no supervisor (s6-svlisten
  waiting for a service nobody asked up): sends the s6-svc -u again

* Mon Nov 09 2026 uinitconverter developers - 0.2.0-1
- The command is now initconvert-linkwatch (the package keeps its name)

* Mon Oct 26 2026 uinitconverter developers - 0.1.0-1
- First version
