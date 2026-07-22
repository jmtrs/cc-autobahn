#!/usr/bin/env bash
# Rewrite the AppImage's AppRun so it prefers a *host* WebKitGTK over the frozen
# copy linuxdeploy bundled at build time, then repack it in place.
#
# Why (docs/DECISIONS.md D66): the bundled WebKitGTK is fixed at build time and
# aborts at startup with
#   Could not create default EGL display: EGL_BAD_PARAMETER. Aborting...
# on older Intel/Mesa combinations (e.g. Intel HD 4000 / Ivy Bridge + Mesa >=26
# using the crocus driver) — upstream WebKit bug #280239, fixed in 2.52. Because
# the panel is frameless/transparent (D14/D57), that abort looks identical to a
# working-but-hidden window: nothing is ever drawn. A host WebKitGTK tracks the
# host Mesa and does not hit this, so we prefer it when present and fall back to
# the bundled stack only where the host has none.
#
# How: linuxdeploy's default AppRun execs AppRun.wrapped, a C shim that forces
# the bundled "$APPDIR/usr/lib" FIRST in LD_LIBRARY_PATH — which we cannot
# reorder from outside. The inner binary uses DT_RUNPATH ($ORIGIN/../lib), which
# the loader searches AFTER LD_LIBRARY_PATH, so putting a host lib dir first in
# LD_LIBRARY_PATH shadows the bundled WebKit. We therefore bypass AppRun.wrapped
# and build the environment ourselves (still sourcing linuxdeploy's GTK hook for
# GDK_BACKEND/theme/schema setup).
#
# Usage: appimage-prefer-system-webkit.sh [APPIMAGE|BUNDLE_ROOT]
#   default BUNDLE_ROOT: src-tauri/target/release/bundle
set -euo pipefail

ARG=${1:-src-tauri/target/release/bundle}
if [[ "$ARG" == *.AppImage ]]; then
  APPIMAGE=$(realpath "$ARG")
else
  mapfile -t APPIMAGES < <(find "$ARG/appimage" -type f -name '*.AppImage')
  test "${#APPIMAGES[@]}" -eq 1
  APPIMAGE=$(realpath "${APPIMAGES[0]}")
fi
echo "Patching AppRun in: $APPIMAGE"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

(
  cd "$WORK"
  "$APPIMAGE" --appimage-extract >/dev/null
)
ROOT="$WORK/squashfs-root"
test -f "$ROOT/apprun-hooks/linuxdeploy-plugin-gtk.sh" # sanity: expected layout

# The single executable Tauri ships (productName). Discover it so a rename does
# not silently break this script.
mapfile -t BINS < <(find "$ROOT/usr/bin" -maxdepth 1 -type f -executable -printf '%f\n')
test "${#BINS[@]}" -eq 1
APP_BIN=${BINS[0]}

cat > "$ROOT/AppRun" <<EOF
#! /usr/bin/env bash
# Patched by scripts/appimage-prefer-system-webkit.sh — prefers a host
# WebKitGTK over the frozen bundled copy (docs/DECISIONS.md D66).
set -e
this_dir="\$(readlink -f "\$(dirname "\$0")")"
export APPDIR="\${APPDIR:-\$this_dir}"

# GTK/GDK/theme/schema env (GDK_BACKEND=x11, XDG_DATA_DIRS, GSETTINGS_SCHEMA_DIR, ...).
source "\$this_dir/apprun-hooks/linuxdeploy-plugin-gtk.sh"

bundled_libs="\$APPDIR/usr/lib:\$APPDIR/usr/lib/x86_64-linux-gnu"
# Host WebKitGTK, if the loader knows one. DT_RUNPATH (\$ORIGIN/../lib) is
# searched after LD_LIBRARY_PATH, so a host lib dir placed first shadows the
# bundled WebKit; the bundled dirs stay on the path as a fallback for any lib
# the host lacks.
sys_webkit="\$(ldconfig -p 2>/dev/null | awk '/libwebkit2gtk-4\\.1\\.so\\.0/{print \$NF; exit}')"
if [ -n "\$sys_webkit" ]; then
  export LD_LIBRARY_PATH="\$(dirname "\$sys_webkit"):\$bundled_libs\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
else
  export LD_LIBRARY_PATH="\$bundled_libs\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
fi
export PATH="\$APPDIR/usr/bin:\$PATH"

exec "\$APPDIR/usr/bin/$APP_BIN" "\$@"
EOF
chmod +x "$ROOT/AppRun"
echo "New AppRun (exec target: usr/bin/$APP_BIN):"
sed 's/^/  | /' "$ROOT/AppRun"

# Repack. appimagetool is an AppImage; run it FUSE-less in CI.
APPIMAGETOOL=$(command -v appimagetool || true)
if [ -z "$APPIMAGETOOL" ]; then
  echo "Fetching appimagetool..."
  wget -q -O "$WORK/appimagetool" \
    https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
  chmod +x "$WORK/appimagetool"
  APPIMAGETOOL="$WORK/appimagetool"
fi

OUT="$WORK/repacked.AppImage"
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" --no-appstream "$ROOT" "$OUT"
chmod +x "$OUT"
mv -f "$OUT" "$APPIMAGE"
echo "Repacked: $APPIMAGE"
