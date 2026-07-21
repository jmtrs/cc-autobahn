#!/usr/bin/env bash
set -euo pipefail

BUNDLE_ROOT=${1:-src-tauri/target/release/bundle}
mapfile -t DEBS < <(find "$BUNDLE_ROOT/deb" -type f -name '*.deb')
mapfile -t RPMS < <(find "$BUNDLE_ROOT/rpm" -type f -name '*.rpm')
mapfile -t APPIMAGES < <(find "$BUNDLE_ROOT/appimage" -type f -name '*.AppImage')

test "${#DEBS[@]}" -eq 1
test "${#RPMS[@]}" -eq 1
test "${#APPIMAGES[@]}" -eq 1

DEB=${DEBS[0]}
RPM=${RPMS[0]}
APPIMAGE=$(realpath "${APPIMAGES[0]}")
dpkg-deb --info "$DEB"
test "$(dpkg-deb --field "$DEB" Architecture)" = 'amd64'
dpkg-deb --contents "$DEB" | grep -F 'usr/share/metainfo/com.jmtrs.cc-autobahn.metainfo.xml'
DEB_DEPENDS=$(dpkg-deb --field "$DEB" Depends)
grep -qw 'bash' <<<"$DEB_DEPENDS"
grep -qw 'curl' <<<"$DEB_DEPENDS"
grep -qw 'unzip' <<<"$DEB_DEPENDS"

RPM_REQUIRES=$(rpm -qpR "$RPM")
grep -qx 'bash' <<<"$RPM_REQUIRES"
grep -qx 'curl' <<<"$RPM_REQUIRES"
grep -qx 'unzip' <<<"$RPM_REQUIRES"
rpm -qlp "$RPM" | grep -F '/usr/share/metainfo/com.jmtrs.cc-autobahn.metainfo.xml'
rpm -qip "$RPM" | grep -E '^License[[:space:]]*:[[:space:]]*MIT$'
rpm -qip "$RPM" | grep -E '^Architecture[[:space:]]*:[[:space:]]*x86_64$'
file "$APPIMAGE" | grep -E 'ELF 64-bit.*x86-64'

EXTRACTED=$(mktemp -d)
trap 'rm -rf "$EXTRACTED"' EXIT
dpkg-deb --extract "$DEB" "$EXTRACTED"
desktop-file-validate "$EXTRACTED/usr/share/applications/cc-autobahn.desktop"
appstreamcli validate --no-net "$EXTRACTED/usr/share/metainfo/com.jmtrs.cc-autobahn.metainfo.xml"

mkdir "$EXTRACTED/appimage"
(
  cd "$EXTRACTED/appimage"
  "$APPIMAGE" --appimage-extract >/dev/null
)
APPIMAGE_META="$EXTRACTED/appimage/squashfs-root/usr/share/metainfo/com.jmtrs.cc-autobahn.metainfo.xml"
test -f "$APPIMAGE_META"
appstreamcli validate --no-net "$APPIMAGE_META"

printf 'Validated: %s\nValidated: %s\nValidated: %s\n' "$DEB" "$RPM" "$APPIMAGE"
