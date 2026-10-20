# One of the sdcompat RPMs built from the core workspace (see packages/README-us.md).
# All of them build from the same tarball, which packages/build-rpms.sh makes:
#   Cargo.toml Cargo.lock crates dist vendor .cargo  under sdcompat-VERSION/
# with .cargo/config.toml pointing [source.crates-io] at vendor/.
Name:           sdcompat-runner
Version:        0.1.0
Release:        1%{?dist}
Summary:        Runs services with systemd's execution semantics under s6
# Choose a license before distributing.
License:        TBD
Source0:        sdcompat-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
Requires:       s6
Requires:       shadow-utils
# OnFailure= and OnSuccess= units are started through sdcompatd.
Recommends:     sdcompatd = %{version}-%{release}

%description
Started by the s6 run and finish scripts sdcompat-gen writes, and by
sdcompatd: sets up the sandbox, credentials, cgroup, seccomp filter, sockets
and environment a unit asks for, then executes its commands.

%prep
%autosetup -n sdcompat-%{version}

%build
cargo build --release --offline -p sdcompat-runner

%check
cargo test --release --offline -p sdcompat-runner

%install
install -Dm0755 target/release/sdcompat-runner %{buildroot}%{_libexecdir}/sdcompat/sdcompat-runner

%files
%dir %{_libexecdir}/sdcompat
%{_libexecdir}/sdcompat/sdcompat-runner

%changelog
* Wed Oct 21 2026 sdcompat developers - 0.1.0-1
- Split out of the sdcompat package
