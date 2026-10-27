# One of the uinitconverter RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under uinitconverter-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           uinitconverter-runner
# Version comes from Cargo.toml and release from the build time and git
# commit (build-rpms.sh defines them); build with that script, not with
# rpmbuild directly.
Version:        %{uinitconverter_version}
Release:        %{uinitconverter_release}%{?dist}
Summary:        Runs services with systemd's execution semantics under s6
# Choose a license before distributing.
License:        TBD
Source0:        uinitconverter-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
Requires:       s6
Requires:       shadow-utils
# OnFailure= and OnSuccess= units are started through uinitconverterd.
Recommends:     uinitconverterd = %{version}-%{release}

%description
Started by the s6 run and finish scripts uinitconverter-gen writes, and by
uinitconverterd: sets up the sandbox, credentials, cgroup, seccomp filter, sockets
and environment a unit asks for, then executes its commands.

%prep
%autosetup -n uinitconverter-%{version}

%build
# --version prints the package's version-release.
export UINITCONVERTER_BUILD=%{version}-%{release}
cargo build --release --offline -p uinitconverter-runner

%check
export UINITCONVERTER_BUILD=%{version}-%{release}
# The library crates it is built from too: they are not packaged.
cargo test --release --offline -p uinitconverter-runner -p uinitconverter-base -p uinitconverter-client -p uinitconverter-exec -p uinitconverter-install -p uinitconverter-version

%install
install -Dm0755 target/release/uinitconverter-runner %{buildroot}%{_libexecdir}/uinitconverter/uinitconverter-runner

%files
%dir %{_libexecdir}/uinitconverter
%{_libexecdir}/uinitconverter/uinitconverter-runner

%changelog
* Wed Oct 21 2026 uinitconverter developers
- Split out of the uinitconverter package
