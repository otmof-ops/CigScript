#!/bin/sh
# SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
# SPDX-License-Identifier: Apache-2.0
# CigScript installer. POSIX sh, no sudo, asks before installing anything.
#
#   Review first (recommended):
#     curl -fsSL https://raw.githubusercontent.com/otmof-ops/CigScript/v1.0.0/install.sh -o install.sh
#     less install.sh && sh install.sh
#
#   Options:
#     --version vX.Y.Z   install this release (default: the newest stable release)
#     --prefix DIR       install directory (default: ~/.local/bin)
#     --from-source      build with cargo instead of downloading a binary
#     --no-modify-path   never touch shell rc files
#     --yes              answer yes to every question (still never sudo)
#
# Preflight: checks for curl (or wget), a sha256 tool, and, for --from-source,
# cargo. Missing tools are reported with the exact command that would install
# them; nothing is installed without a y.

set -u

REPO="otmof-ops/CigScript"
VERSION=""
PREFIX="${HOME}/.local/bin"
FROM_SOURCE=0
MODIFY_PATH=1
YES=0

say()  { printf '%s\n' "$*" >&2; }
fail() { say "install.sh: error: $*"; exit 1; }
ask() {
  if [ "$YES" = 1 ]; then return 0; fi
  printf '%s [y/N] ' "$*" >&2
  if [ -t 0 ]; then read -r ans </dev/tty; else read -r ans; fi
  case "$ans" in y|Y|yes|YES) return 0;; *) return 1;; esac
}
have() { command -v "$1" >/dev/null 2>&1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2;;
    --version=*) VERSION="${1#*=}"; shift;;
    --prefix) PREFIX="$2"; shift 2;;
    --prefix=*) PREFIX="${1#*=}"; shift;;
    --from-source) FROM_SOURCE=1; shift;;
    --no-modify-path) MODIFY_PATH=0; shift;;
    --yes|-y) YES=1; shift;;
    -h|--help) sed -n '2,20p' "$0"; exit 0;;
    *) fail "unknown option $1";;
  esac
done

# ----- preflight ------------------------------------------------------------------
os=$(uname -s | tr '[:upper:]' '[:lower:]')
arch=$(uname -m)
case "$os" in
  linux) os=linux;;
  darwin) os=macos;;
  msys*|mingw*|cygwin*) os=windows;;
  *) say "unsupported OS '$os'; use --from-source";;
esac
case "$arch" in
  x86_64|amd64) arch=x86_64;;
  aarch64|arm64) arch=aarch64;;
esac

pkg_cmd() {
  # The command that would install $1, for this machine's package manager.
  p="$1"
  if have apt-get; then echo "sudo apt-get install -y $p"
  elif have dnf; then echo "sudo dnf install -y $p"
  elif have pacman; then echo "sudo pacman -S --noconfirm $p"
  elif have zypper; then echo "sudo zypper install -y $p"
  elif have apk; then echo "sudo apk add $p"
  elif have brew; then echo "brew install $p"
  else echo ""; fi
}

need_tool() {
  # need_tool <binary> <package>: offer to install if missing, then re-check.
  if have "$1"; then return 0; fi
  cmd=$(pkg_cmd "$2")
  say "preflight: $1 is missing."
  if [ -n "$cmd" ] && ask "run \`$cmd\` now? (you will be asked for your password by sudo if it needs it)"; then
    # shellcheck disable=SC2086
    $cmd || fail "$cmd failed"
    have "$1" || fail "$1 is still missing after installing"
  else
    return 1
  fi
}

fetch() {
  # fetch <url> <dest>
  if have curl; then
    curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 --retry 3 --max-time 120 -o "$2" "$1"
  elif have wget; then
    wget -q --https-only -O "$2" "$1"
  else
    return 1
  fi
}

sha256_of() {
  if have sha256sum; then sha256sum "$1" | cut -d' ' -f1
  elif have shasum; then shasum -a 256 "$1" | cut -d' ' -f1
  elif have openssl; then openssl dgst -sha256 "$1" | awk '{print $NF}'
  else echo ""; fi
}

# ----- from source ----------------------------------------------------------------
build_from_source() {
  if ! have cargo; then
    say "preflight: cargo (the Rust toolchain) is missing; it is needed to build from source."
    if ask "install rustup into \$HOME/.cargo (official installer, no sudo)?"; then
      fetch https://sh.rustup.rs "${TMPDIR:-/tmp}/rustup-init.sh" || fail "could not download rustup"
      sh "${TMPDIR:-/tmp}/rustup-init.sh" -y --profile minimal --no-modify-path || fail "rustup failed"
      PATH="$HOME/.cargo/bin:$PATH"; export PATH
    else
      fail "cannot build without cargo"
    fi
  fi
  say "building CigScript ${VERSION:-main} from source (this takes a minute)..."
  if [ -n "$VERSION" ]; then
    cargo install --git "https://github.com/$REPO" --tag "$VERSION" --root "$PREFIX/.." --locked cigscript || fail "cargo install failed"
  else
    cargo install --git "https://github.com/$REPO" --root "$PREFIX/.." --locked cigscript || fail "cargo install failed"
  fi
}

# ----- download a release --------------------------------------------------------
download_release() {
  need_tool curl curl || need_tool wget wget || fail "curl or wget is required to download (or use --from-source)"
  if [ -n "$VERSION" ]; then
    tag="$VERSION"
  else
    say "resolving the newest release..."
    api="https://api.github.com/repos/$REPO/releases/latest"
    tmpjson="${TMPDIR:-/tmp}/cig-release.$$"
    if ! fetch "$api" "$tmpjson"; then
      rm -f "$tmpjson"
      fail "could not read releases for $REPO (a private repository needs \`gh\`; try --from-source or --version)"
    fi
    tag=$(sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' "$tmpjson" | head -1)
    rm -f "$tmpjson"
    [ -n "$tag" ] || fail "no release tag found"
  fi
  case "$tag" in v*) ;; *) tag="v$tag";; esac
  ext=""; [ "$os" = windows ] && ext=".exe"
  asset="cig-${tag}-${os}-${arch}${ext}"
  base="https://github.com/$REPO/releases/download/$tag"
  work="${TMPDIR:-/tmp}/cig-install.$$"
  mkdir -p "$work" || fail "cannot create $work"
  say "downloading $asset ($tag)..."
  fetch "$base/$asset" "$work/$asset" || { rm -rf "$work"; fail "no binary for ${os}-${arch} in $tag; try --from-source"; }
  fetch "$base/$asset.sha256" "$work/$asset.sha256" || { rm -rf "$work"; fail "release ships no checksum for $asset; refusing to install"; }
  expected=$(cut -d' ' -f1 "$work/$asset.sha256")
  actual=$(sha256_of "$work/$asset")
  [ -n "$actual" ] || { rm -rf "$work"; fail "no sha256 tool (sha256sum, shasum or openssl) to verify the download"; }
  [ "$expected" = "$actual" ] || { rm -rf "$work"; fail "checksum mismatch for $asset (expected $expected, got $actual)"; }
  say "verified: sha256 matches the published checksum (integrity, not authorship; see SECURITY.md)"
  mkdir -p "$PREFIX" || fail "cannot create $PREFIX"
  chmod 755 "$work/$asset"
  mv "$work/$asset" "$PREFIX/cig$ext" || fail "cannot move into $PREFIX"
  rm -rf "$work"
}

# ----- main -----------------------------------------------------------------------
say "CigScript installer: $os/$arch, prefix $PREFIX"
if [ "$FROM_SOURCE" = 1 ]; then build_from_source; else download_release; fi

CIG="$PREFIX/cig"
[ -x "$CIG" ] || CIG="$PREFIX/cig.exe"
[ -x "$CIG" ] || fail "cig did not land in $PREFIX"
say "installed: $("$CIG" --version)"

case ":$PATH:" in
  *":$PREFIX:"*) ;;
  *)
    say "$PREFIX is not on your PATH."
    if [ "$MODIFY_PATH" = 1 ]; then
      case "${SHELL:-}" in
        */fish) rc="$HOME/.config/fish/config.fish"; line="fish_add_path $PREFIX";;
        */zsh)  rc="$HOME/.zshrc";  line="export PATH=\"$PREFIX:\$PATH\"";;
        *)      rc="$HOME/.bashrc"; line="export PATH=\"$PREFIX:\$PATH\"";;
      esac
      if ask "append \`$line\` to $rc?"; then
        printf '\n# added by the CigScript installer\n%s\n' "$line" >> "$rc"
        say "done; open a new shell, or run: $line"
      else
        say "add it yourself: $line"
      fi
    else
      say "add it yourself: export PATH=\"$PREFIX:\$PATH\""
    fi
    ;;
esac

say ""
"$CIG" doctor || true
say ""
say "Try it:  echo 'exhale \"hello from cig\"' > hello.cig && $CIG run hello.cig"
