#!/usr/bin/env bash
# Builds FPS Viewer for Windows x64 from macOS/Linux and packages it into dist/:
#   FpsViewer-x64.exe          the app on its own (portable)
#   FpsViewer-Setup-x64.exe    the installer
#   SHA256SUMS.txt
#
#   ./release.sh              sign both with the local test certificate (for your own PCs)
#   ./release.sh --unsigned   no signature — for public releases
#
# Needs: cargo-xwin, makensis (brew install makensis), osslsigncode for signing.
# The version comes from Cargo.toml.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
DIST="$ROOT/dist"
TARGET=x86_64-pc-windows-msvc
SIGN_DIR="${FPSVIEWER_SIGNING_DIR:-$HOME/.fpsviewer-signing}"
SIGN=1
[[ "${1:-}" == "--unsigned" ]] && SIGN=0

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
export LC_ALL=en_US.UTF-8 # makensis rejects the UTF-8 script under the "C" locale
VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
echo "FPS Viewer $VERSION"

sign() { # sign <file>
  if [[ $SIGN == 0 ]]; then return; fi
  if [[ ! -f "$SIGN_DIR/codesign.pfx" ]]; then
    echo "  no certificate in $SIGN_DIR — leaving $(basename "$1") unsigned"; return
  fi
  osslsigncode sign -pkcs12 "$SIGN_DIR/codesign.pfx" -readpass "$SIGN_DIR/pfx-password.txt" \
    -n "FPS Viewer" -h sha256 -ts http://timestamp.digicert.com -in "$1" -out "$1.signed" >/dev/null
  mv -f "$1.signed" "$1"
  echo "  signed $(basename "$1")"
}

echo "• building"
(cd "$ROOT" && cargo xwin build --release --target "$TARGET" 2>&1 | grep -E "^(warning|error)|Finished" || true)
BUILT="$ROOT/target/$TARGET/release/FpsViewer.exe"
[[ -f "$BUILT" ]] || { echo "build failed"; exit 1; }

mkdir -p "$DIST"
rm -f "$DIST"/FpsViewer-x64.exe "$DIST"/FpsViewer-Setup-x64.exe "$DIST"/SHA256SUMS.txt

echo "• portable exe"
cp "$BUILT" "$DIST/FpsViewer-x64.exe"
sign "$DIST/FpsViewer-x64.exe"

echo "• installer"
makensis -V2 -DVERSION="$VERSION" -DICON="$ROOT/assets/app.ico" -DEXE="$DIST/FpsViewer-x64.exe" \
  -DOUTFILE="$DIST/FpsViewer-Setup-x64.exe" "$ROOT/installer/FpsViewer.nsi"
sign "$DIST/FpsViewer-Setup-x64.exe"

if [[ $SIGN == 1 && -f "$SIGN_DIR/FpsViewer-Test.cer" ]]; then
  cp "$SIGN_DIR/FpsViewer-Test.cer" "$DIST/"
fi

(cd "$DIST" && shasum -a 256 FpsViewer-x64.exe FpsViewer-Setup-x64.exe > SHA256SUMS.txt)
echo "• done"
ls -lh "$DIST"
cat "$DIST/SHA256SUMS.txt"
