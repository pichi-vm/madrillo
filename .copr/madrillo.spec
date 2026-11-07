Name:           madrillo
Version:        0.1.0~dev
Release:        1%{?buildtag}%{?dist}
Summary:        Native process monitor for persistent workloads

License:        Apache-2.0 AND MIT AND 0BSD AND Unicode-3.0
URL:            https://github.com/pichi-vm/madrillo
Source0:        %{name}-%{version}.tar.gz
ExclusiveArch:  x86_64 aarch64

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc
BuildRequires:  git

%description
Native process monitor for persistent workloads.

%prep
%autosetup

%build
export CARGO_HOME="$PWD/.cargo-home"
cargo build --locked --release --bin madrillo --features cli

%install
install -Dpm0755 target/release/madrillo %{buildroot}%{_bindir}/madrillo

# Preserve license and notice files from the fetched dependencies.
find .cargo-home -type f \( -iname 'license*' -o -iname 'licence*' -o \
    -iname 'copying*' -o -iname 'notice*' -o -iname 'copyright*' \) \
    -exec install -Dm0644 '{}' '%{buildroot}%{_licensedir}/%{name}/dependencies/{}' \;

install -Dm0644 /usr/share/doc/rust/COPYRIGHT-library.html \
    %{buildroot}%{_licensedir}/%{name}/rust/COPYRIGHT-library.html
cp -a /usr/share/doc/rust/licenses %{buildroot}%{_licensedir}/%{name}/rust/

%check
%{buildroot}%{_bindir}/madrillo --version

%files
%license %{_licensedir}/%{name}/dependencies/
%license %{_licensedir}/%{name}/rust/
%license LICENSE
%doc README.md
%{_bindir}/madrillo
