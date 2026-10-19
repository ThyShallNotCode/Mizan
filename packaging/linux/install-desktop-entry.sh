#!/bin/sh
# Adds Mizan to this user's applications menu: the desktop entry, pointing at omr-desktop in this
# folder, and the icon. Run it from the unpacked folder, and again after moving the folder.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
data="${XDG_DATA_HOME:-$HOME/.local/share}"
mkdir -p "$data/applications" "$data/icons/hicolor/256x256/apps" "$data/icons/hicolor/512x512/apps"
cp "$here/mizan-256.png" "$data/icons/hicolor/256x256/apps/mizan.png"
cp "$here/mizan-512.png" "$data/icons/hicolor/512x512/apps/mizan.png"
sed "s|^Exec=.*|Exec=\"$here/omr-desktop\"|" "$here/mizan.desktop" > "$data/applications/mizan.desktop"
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$data/applications" || true
fi
echo "Mizan is in the applications menu ($data/applications/mizan.desktop)."
