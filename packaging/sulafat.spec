#
# spec file for package sulafat
#
# Copyright (c) 2026 Rodrigo Brito
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#

Name:           sulafat
Version:        1.0.4
Release:        0
Summary:        SSH client for the Lyra OS ecosystem
License:        GPL-3.0-or-later
Group:          Productivity/Networking/SSH
URL:            https://github.com/britors/Sulafat
Source0:        %{name}-%{version}.tar.zst
Source1:        vendor.tar.zst

BuildRequires:  cargo
BuildRequires:  cargo-packaging
BuildRequires:  rust >= 1.85
BuildRequires:  gtk4-devel >= 4.12
BuildRequires:  libadwaita-devel >= 1.5
BuildRequires:  vte-devel >= 0.80
BuildRequires:  pkgconfig
BuildRequires:  python3
BuildRequires:  desktop-file-utils
BuildRequires:  appstream-glib
BuildRequires:  fdupes
BuildRequires:  gettext-tools
BuildRequires:  openssh-clients
BuildRequires:  zstd
Requires:       openssh-clients

%description
Sulafat é o cliente SSH do ecossistema Lyra OS, para conexão com máquinas
Linux/Unix (desktops e servidores). É um aplicativo independente, utilizável em qualquer
distribuição Linux moderna, com integração visual prioritária ao Lyra (GNOME/Wayland).

O protocolo SSH é delegado por completo ao binário `ssh` do OpenSSH, executado dentro de um
terminal embutido (VTE) — sem implementação própria de SSH. Isso herda de graça chaves,
ssh-agent, known_hosts, certificados, ProxyJump, multiplexação e todo o ~/.ssh/config existente
do usuário. O valor do Sulafat está no gerenciamento: hosts organizados com busca, grupos e cor
por ambiente, sessões em abas.

Implementado em Rust, usando GTK4 + libadwaita e VTE. Nenhuma senha ou passphrase é manuseada ou
armazenada pelo Sulafat — isso é papel do ssh-agent e do GNOME Keyring, já integrados ao sistema.

%prep
# -a1 extracts Source0, then unpacks Source1 (vendor.tar.zst) on top of it; the vendor
# tarball produced by the cargo_vendor OBS service already includes .cargo/config.toml, so
# no manual step is needed to point cargo at the vendored crates.
%autosetup -a1

%build
%{cargo_build}
for locale in $(cat po/LINGUAS); do
    msgfmt --check --check-format -o po/${locale}.mo po/${locale}.po
done

%install
install -Dm0755 target/release/sulafat %{buildroot}%{_bindir}/sulafat
install -Dm0644 data/org.lyraos.Sulafat.desktop \
    %{buildroot}%{_datadir}/applications/org.lyraos.Sulafat.desktop
install -Dm0644 data/org.lyraos.Sulafat.metainfo.xml \
    %{buildroot}%{_datadir}/metainfo/org.lyraos.Sulafat.metainfo.xml
install -Dm0644 data/icons/org.lyraos.Sulafat.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/org.lyraos.Sulafat.svg
install -Dm0644 data/icons/org.lyraos.Sulafat-symbolic.svg \
    %{buildroot}%{_datadir}/icons/hicolor/symbolic/apps/org.lyraos.Sulafat-symbolic.svg
for locale in $(cat po/LINGUAS); do
    install -Dm0644 po/${locale}.mo \
        %{buildroot}%{_datadir}/locale/${locale}/LC_MESSAGES/sulafat.mo
done

desktop-file-validate %{buildroot}%{_datadir}/applications/org.lyraos.Sulafat.desktop
appstream-util validate-relax --nonet \
    %{buildroot}%{_datadir}/metainfo/org.lyraos.Sulafat.metainfo.xml

%check
# Widget construction is covered separately under a virtual display; unit tests need no display
# or SSH server and include locale selection plus the core parser's round-trip fidelity tests.
cargo test --offline --workspace
python3 scripts/check-i18n.py

%post
%desktop_database_post
%icon_theme_cache_post

%postun
%desktop_database_postun
%icon_theme_cache_postun

%files
%license LICENSE
%doc README.md
%{_bindir}/sulafat
%{_datadir}/applications/org.lyraos.Sulafat.desktop
%{_datadir}/metainfo/org.lyraos.Sulafat.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/org.lyraos.Sulafat.svg
%{_datadir}/icons/hicolor/symbolic/apps/org.lyraos.Sulafat-symbolic.svg
%lang(es_ES) %{_datadir}/locale/es_ES/LC_MESSAGES/sulafat.mo
%lang(pt_BR) %{_datadir}/locale/pt_BR/LC_MESSAGES/sulafat.mo

%changelog
