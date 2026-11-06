# Copr SCM method "make srpm" uses .copr/Makefile. Only that step fetches
# dependencies; the binary build uses Cargo.lock and the vendored sources.
%global crate madrillo
%global cargo_install_lib 0

Name:           madrillo
Version:        0.1.0
Release:        1%{?dist}
Summary:        Native process monitor for persistent workloads
# Linux CLI dependency closure; choose MIT where offered as an alternative.
License:        Apache-2.0 AND MIT AND 0BSD AND Unicode-3.0
URL:            https://github.com/pichi-vm/madrillo
Source0:        %{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.xz

ExclusiveArch:  x86_64 aarch64
BuildRequires:  cargo-rpm-macros >= 26
BuildRequires:  rust >= 1.94
BuildRequires:  cargo >= 1.94
BuildRequires:  gcc
BuildRequires:  python3

%description
Madrillo supervises a native workload and its descendants, retains console
logs and completion state, and permits clients to detach and reconnect.
This package supplies the monitor executable used by Pichi. The shared Rust
types remain a separate source dependency with an empty default feature set.

%prep
%autosetup -n %{name}-%{version} -a1
# The RPM uses the native distribution compiler with the locked dependency graph.
rm -f rust-toolchain.toml
%cargo_prep -v vendor

%build
%cargo_build -f cli -- --locked --bin madrillo
%cargo_license_summary -f cli
%{cargo_license -f cli} > LICENSE.dependencies
%cargo_vendor_manifest

%install
%cargo_install -f cli -- --locked --bin madrillo
# Keep the vendored license/notice texts as well as the dependency inventory.
find vendor -type f \( -iname 'license*' -o -iname 'licence*' -o \
    -iname 'copying*' -o -iname 'notice*' -o -iname 'copyright*' \) -print0 | \
    while IFS= read -r -d '' file; do
        install -Dm0644 "$file" "%{buildroot}%{_licensedir}/%{name}/dependencies/${file#vendor/}"
    done
install -Dm0644 /usr/share/doc/rust/COPYRIGHT-library.html \
    %{buildroot}%{_licensedir}/%{name}/rust/COPYRIGHT-library.html
cp -a /usr/share/doc/rust/licenses %{buildroot}%{_licensedir}/%{name}/rust/

%check
%{buildroot}%{_bindir}/madrillo --version
state=$(mktemp -d)
trap 'rm -rf "$state"' EXIT
%{buildroot}%{_bindir}/madrillo --state-dir "$state/workload" -- /usr/bin/true
python3 - "$state/workload/result.json" <<'PY'
import json, sys
result = json.load(open(sys.argv[1]))
assert result['leader'] == {'type': 'exit', 'code': 0}, result
assert result['cleanup']['type'] == 'complete', result
assert not result['monitor_errors'] and not result['log_errors'], result
print('Packaged monitor launch and retained completion passed')
PY

%files
%license LICENSE LICENSE.dependencies cargo-vendor.txt
%license %{_licensedir}/%{name}/dependencies/
%license %{_licensedir}/%{name}/rust/
%doc README.md
%{_bindir}/madrillo

%changelog
* Fri Nov 06 2026 Madrillo contributors - 0.1.0-1
- Package the standalone monitor with locked, offline dependencies.
