#!/usr/bin/env bash
# Build a release binary + pack it with data/, scripts/, README, LICENSE into
# screen-recorder-linux-x86_64.tar.gz suitable for GitHub Releases.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
export PATH="$HOME/.cargo/bin:$PATH"

VERSION="${VERSION:-$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)}"
PKG_NAME="screen-recorder-${VERSION}-linux-x86_64"
STAGE="$ROOT/dist/$PKG_NAME"
OUT_TAR="$ROOT/dist/$PKG_NAME.tar.gz"

echo "→ cargo build --release"
cargo build --release

echo "→ stage in $STAGE"
rm -rf "$STAGE"
mkdir -p "$STAGE/scripts" "$STAGE/data"

cp target/release/screen_record "$STAGE/"
cp data/dev.local.ScreenRecord.desktop "$STAGE/data/"
cp -r data/icons "$STAGE/data/"
cp data/style.css "$STAGE/data/"
cp scripts/install.sh "$STAGE/scripts/"
cp README.md LICENSE "$STAGE/"

echo "→ strip binary"
strip "$STAGE/screen_record" || true

echo "→ create $OUT_TAR"
cd "$ROOT/dist"
tar czf "$PKG_NAME.tar.gz" "$PKG_NAME"
cd "$ROOT"

SIZE=$(du -h "$OUT_TAR" | cut -f1)
SHA=$(sha256sum "$OUT_TAR" | awk '{print $1}')

cat <<EOF

✓ packaged: $OUT_TAR ($SIZE)
  sha256:   $SHA

Next steps:
  git tag v$VERSION
  git push --tags
  gh release create v$VERSION "$OUT_TAR" \\
    --title "v$VERSION" \\
    --notes-file docs/release-notes-$VERSION.md   # или --generate-notes
EOF
