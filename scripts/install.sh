#!/usr/bin/env bash
# Install screen_record binary + .desktop + icon into ~/.local.
# Works for both pre-built release tarballs and source builds.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"

BIN_SRC=""
if [[ -x "$ROOT/screen_record" ]]; then
    BIN_SRC="$ROOT/screen_record"            # release tarball layout
elif [[ -x "$ROOT/target/release/screen_record" ]]; then
    BIN_SRC="$ROOT/target/release/screen_record"
elif [[ -x "$ROOT/target/debug/screen_record" ]]; then
    BIN_SRC="$ROOT/target/debug/screen_record"
else
    echo "No built binary found. Run: cargo build --release" >&2
    exit 1
fi

BIN_DST="$HOME/.local/bin/screen_record"
DESKTOP_DST="$HOME/.local/share/applications/dev.local.ScreenRecord.desktop"
ICON_ROOT="$HOME/.local/share/icons/hicolor"

mkdir -p "$(dirname "$BIN_DST")" "$(dirname "$DESKTOP_DST")" "$ICON_ROOT"

echo "→ install binary:  $BIN_DST"
install -m 755 "$BIN_SRC" "$BIN_DST"

echo "→ install desktop: $DESKTOP_DST"
sed "s|^Exec=.*|Exec=$BIN_DST|" "$ROOT/data/dev.local.ScreenRecord.desktop" > "$DESKTOP_DST"
chmod 644 "$DESKTOP_DST"

echo "→ install icons (hicolor theme)"
for size_dir in "$ROOT/data/icons/hicolor"/*; do
    [[ -d "$size_dir" ]] || continue
    sz=$(basename "$size_dir")
    src_icon="$size_dir/apps/dev.local.ScreenRecord.png"
    if [[ -f "$src_icon" ]]; then
        dst_dir="$ICON_ROOT/$sz/apps"
        mkdir -p "$dst_dir"
        install -m 644 "$src_icon" "$dst_dir/dev.local.ScreenRecord.png"
    fi
done

command -v update-desktop-database >/dev/null && update-desktop-database "$HOME/.local/share/applications" || true
command -v gtk-update-icon-cache   >/dev/null && gtk-update-icon-cache -q "$HOME/.local/share/icons/hicolor" || true

cat <<EOF

✓ installed to ~/.local/bin/screen_record
  Add to PATH if missing:  export PATH="\$HOME/.local/bin:\$PATH"
  Launch:                  screen_record
  Or via system app menu:  "Screen Record"
EOF
