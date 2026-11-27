%global debug_package %{nil}
%global pkg_ver %{?_pkg_ver}%{!?_pkg_ver:0.1.1}
%global alpha_tag %{?_alpha_tag}%{!?_alpha_tag:alpha.2}
%global alpha_rel %{?_alpha_rel}%{!?_alpha_rel:0.alpha.2}

Name:           fabrico11y
Version:        %{pkg_ver}
Release:        %{alpha_rel}%{?dist}
Summary:        Local telemetry custody and query tools
License:        Apache-2.0 AND MIT AND BSD-3-Clause AND Unicode-3.0 AND Zlib
URL:            https://github.com/kmosoti/FabricO11y
Source0:        fabrico11y-%{pkg_ver}-%{alpha_tag}.tar.gz
ExclusiveArch:  x86_64
BuildRequires:  rust >= 1.100.0
BuildRequires:  cargo >= 1.100.0
BuildRequires:  gcc
BuildRequires:  pkgconf-pkg-config
BuildRequires:  python3
BuildRequires:  systemd-rpm-macros

%description
FabricO11y local telemetry collection, custody, and query tools.
The user services are opt-in and require explicit configuration.

%prep
%autosetup -n fabrico11y-%{pkg_ver}-%{alpha_tag}

%build
# The builder must supply Rust 1.100.0 and an already populated Cargo cache.
: "${CARGO_BUILD_TARGET_DIR:=/data/cargo-target}"
export CARGO_BUILD_TARGET_DIR
python3 packaging/dependency-notices.py dependency-notices
test "$(rustc --version | cut -d' ' -f2)" = 1.100.0
cargo build --locked --offline --release -p fabric -p fabric-cli -p fabric-query -p fabric-server -p fabric-mcp --bins -j 2

%install
: "${CARGO_BUILD_TARGET_DIR:=/data/cargo-target}"
export CARGO_BUILD_TARGET_DIR
install -d %{buildroot}%{_bindir} %{buildroot}%{_userunitdir} %{buildroot}%{_datadir}/doc/%{name}/examples
for bin in fabric spindle fabric-server fabric-adapter fabric-query fabric-mcp; do
    install -m 0755 "${CARGO_BUILD_TARGET_DIR:?}/release/$bin" %{buildroot}%{_bindir}/
done
install -d %{buildroot}%{_licensedir}/%{name}
cp -a dependency-notices %{buildroot}%{_licensedir}/%{name}/dependencies
install -m 0644 packaging/rpm/units/* %{buildroot}%{_userunitdir}/
cp -a config examples %{buildroot}%{_datadir}/doc/%{name}/examples/
install -m 0644 docs/collection.md docs/deployment-verification.md %{buildroot}%{_datadir}/doc/%{name}/

%files
%license LICENSE NOTICE
%license %{_licensedir}/%{name}/dependencies
%doc packaging/rpm/README.md
%{_datadir}/doc/%{name}/collection.md
%{_datadir}/doc/%{name}/deployment-verification.md
%{_bindir}/fabric
%{_bindir}/spindle
%{_bindir}/fabric-server
%{_bindir}/fabric-adapter
%{_bindir}/fabric-query
%{_bindir}/fabric-mcp
%{_userunitdir}/fabrico11y*.service
%{_userunitdir}/fabrico11y.slice
%{_datadir}/doc/%{name}/examples
