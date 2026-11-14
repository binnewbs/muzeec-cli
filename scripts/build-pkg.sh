#!/usr/bin/env bash
# Build the Arch package. Never installs anything: the tooling stops at the
# artifact and prints the `sudo pacman -U ...` command for you to run.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

FAST=0
NATIVE=0
VENDOR=0
for arg in "$@"; do
  case "$arg" in
    --fast) FAST=1 ;;
    --native) NATIVE=1 ;;
    --vendor) VENDOR=1 ;;
    -h|--help)
      echo "usage: $0 [--fast] [--native] [--vendor]"
      exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

if [ "$(id -u)" -eq 0 ]; then
  echo "refusing to run as root (makepkg must not run as root)" >&2
  exit 1
fi

VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
PKGVER="$(grep -m1 '^pkgver=' packaging/arch/PKGBUILD | cut -d= -f2)"
if [ "$VERSION" != "$PKGVER" ]; then
  echo "version drift: Cargo.toml=$VERSION PKGBUILD=$PKGVER" >&2
  exit 1
fi
echo "muzeec-cli $VERSION"

if [ "$FAST" -eq 0 ]; then
  echo "==> cargo fmt --check"
  cargo fmt --all -- --check
  echo "==> cargo clippy -D warnings"
  cargo clippy --workspace --all-targets -- -D warnings
  echo "==> cargo test"
  cargo test --workspace
fi

if [ "$NATIVE" -eq 1 ]; then
  export RUSTFLAGS="-C target-cpu=native ${RUSTFLAGS:-}"
fi

if [ "$VENDOR" -eq 1 ]; then
  echo "==> vendoring crates"
  cargo vendor vendor > /dev/null
  mkdir -p .cargo
  cat > .cargo/config.toml <<EOF
[source.crates-io]
replace-with = "vendored-sources"
[source.vendored-sources]
directory = "vendor"
EOF
fi

SRC="packaging/arch/muzeec-cli-$VERSION.tar.gz"
echo "==> creating source tarball"
tar --exclude='./target' --exclude='./dist' --exclude='./vendor' --exclude='./.git' \
    --exclude='./packaging/arch/src' --exclude='./packaging/arch/pkg' \
    --exclude='./packaging/arch/muzeec-cli-*.tar.gz' \
    --exclude='*.pkg.tar.zst' \
    -czf "$SRC" \
    --transform "s,^\.,muzeec-cli-$VERSION," .

echo "==> makepkg"
# -f: overwrite existing package. No -s (syncdeps) because the toolchain may be
# provided by rustup rather than pacman; --nodeps skips the dependency checks.
( cd packaging/arch && makepkg -f --noconfirm --cleanbuild --nodeps )

ARTIFACT="$(ls -t packaging/arch/muzeec-cli-"$VERSION"-*.pkg.tar.zst 2>/dev/null | head -1 || true)"
if [ -z "$ARTIFACT" ]; then
  echo "package artifact not found" >&2
  exit 1
fi

mkdir -p dist
cp -f "$ARTIFACT" dist/
OUT="$ROOT/dist/$(basename "$ARTIFACT")"

echo
echo "==> artifact"
ls -lh "$OUT"
echo "sha256: $(sha256sum "$OUT" | cut -d' ' -f1)"

if command -v namcap >/dev/null 2>&1; then
  echo "==> namcap"
  namcap "$OUT" || echo "namcap reported warnings (review above)"
fi
if command -v desktop-file-validate >/dev/null 2>&1; then
  echo "==> desktop-file-validate"
  desktop-file-validate assets/muzeec-cli.desktop || true
fi

echo
echo "Install with:"
echo "  sudo pacman -U $OUT"
echo
echo "Nothing was installed by this script."
