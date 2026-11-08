# One of the uinitconverter RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under uinitconverter-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           uinitconverter-gen
# Version comes from Cargo.toml and release from the build time and git
# commit (build-rpms.sh defines them); build with that script, not with
# rpmbuild directly.
Version:        %{uinitconverter_version}
Release:        %{uinitconverter_release}%{?dist}
Summary:        Translates systemd unit files into an s6-rc database
# Choose a license before distributing.
License:        TBD
Source0:        uinitconverter-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
Requires:       s6-rc
# The generated services run initconvert-runner; .timer and .path units run
# initconvert-timers.
Requires:       uinitconverter-runner = %{version}-%{release}
Requires:       uinitconverter-timers = %{version}-%{release}

%description
Reads systemd unit files, compiles them together with hand-written s6-rc
services into an s6-rc database (s6-rc-compile) and switches the live state
to it (s6-rc-update). Also enable, disable, mask and preset units.

%prep
%autosetup -n uinitconverter-%{version}

%build
# --version prints the package's version-release.
export UINITCONVERTER_BUILD=%{version}-%{release}
%if %{defined uinitconverter_target}
# build-rpms.sh --shared-target (test builds only): every package builds
# from one copy of the source at a fixed path into one target directory, so
# cargo reuses what an earlier package or build compiled (it rebuilds a
# crate whose source path changed). %%install reads target/ as usual.
rm -rf %{uinitconverter_target}/src
cp -a . %{uinitconverter_target}/src
ln -sfn %{uinitconverter_target}/target target
export CARGO_TARGET_DIR=%{uinitconverter_target}/target
cd %{uinitconverter_target}/src
%endif
cargo build --release --offline -p uinitconverter-gen

%check
export UINITCONVERTER_BUILD=%{version}-%{release}
# The library crates it is built from too: they are not packaged.
cargo test --release --offline -p uinitconverter-gen -p uinitconverter-base -p uinitconverter-install -p uinitconverter-client -p uinitconverter-version

%install
install -Dm0755 target/release/initconvert-gen %{buildroot}%{_libexecdir}/uinitconverter/initconvert-gen
install -d %{buildroot}%{_sharedstatedir}/uinitconverter

%files
%dir %{_libexecdir}/uinitconverter
%{_libexecdir}/uinitconverter/initconvert-gen
%dir %{_sharedstatedir}/uinitconverter

%changelog
* Mon Nov 09 2026 uinitconverter developers
- 0.3.0, see CHANGELOG-us.md

* Mon Nov 02 2026 uinitconverter developers
- 0.2.2, see CHANGELOG-us.md

* Mon Nov 02 2026 uinitconverter developers
- 0.2.1, see CHANGELOG-us.md

* Sun Nov 01 2026 uinitconverter developers
- 0.2.0, see CHANGELOG-us.md

* Wed Oct 21 2026 uinitconverter developers
- Split out of the uinitconverter package
