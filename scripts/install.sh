#!/bin/sh
# Installs the latest Agro release binary for this machine. No root needed.
#
#   curl -fsSL https://raw.githubusercontent.com/AgroUPlus/Agro/main/scripts/install.sh | sh
#
# AGRO_INSTALL_DIR  where the binary goes (default ~/.local/bin)
# AGRO_VERSION      a tag such as v0.2.0 (default: the latest release)
set -eu

repo="AgroUPlus/Agro"
dir="${AGRO_INSTALL_DIR:-$HOME/.local/bin}"

[ "$(uname -s)" = "Linux" ] || { echo "Prebuilt binaries are Linux only. Build from source: https://github.com/$repo#build" >&2; exit 1; }
case "$(uname -m)" in
  x86_64 | amd64) arch=amd64 ;;
  aarch64 | arm64) arch=arm64 ;;
  *) echo "No prebuilt binary for $(uname -m). Build from source: https://github.com/$repo#build" >&2; exit 1 ;;
esac

if [ -n "${AGRO_VERSION:-}" ]; then
  base="https://github.com/$repo/releases/download/$AGRO_VERSION"
else
  base="https://github.com/$repo/releases/latest/download"
fi
name="agro-linux-$arch"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $name..."
curl -fsSL "$base/$name" -o "$tmp/$name"
curl -fsSL "$base/$name.sha256" -o "$tmp/$name.sha256"
(cd "$tmp" && sha256sum -c "$name.sha256" >/dev/null) || { echo "Checksum mismatch, nothing installed." >&2; exit 1; }

mkdir -p "$dir"
install -m 755 "$tmp/$name" "$dir/agro"
echo "Installed $dir/agro"
case ":$PATH:" in *":$dir:"*) ;; *) echo "Add $dir to your PATH to run it as 'agro'." ;; esac
cat <<MSG

Start it (data lives in the folder you start it from):
  mkdir -p ~/agro && cd ~/agro
  AGRO_LIBRARY_ROOT=~/Music agro

First run prints a setup link in the log. Open it to create the admin account.

Two-factor sign-in is recommended; the dashboard will offer it. The server makes its own key in agro_secret.key, so back that file up with agro_data.db.
MSG
