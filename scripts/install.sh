#!/bin/sh
# Install kicau. Usage:
#   curl -fsSL https://raw.githubusercontent.com/gitshrl/kicau/main/scripts/install.sh | sh
#
# Override the install directory with KICAU_BIN_DIR, and the version with
# KICAU_VERSION (defaults to the latest release).
set -eu

REPO="gitshrl/kicau"
BIN_DIR="${KICAU_BIN_DIR:-/usr/local/bin}"
VERSION="${KICAU_VERSION:-latest}"

die() {
	echo "kicau: $1" >&2
	exit 1
}

case "$(uname -s)-$(uname -m)" in
	Linux-x86_64 | Linux-amd64) asset="kicau-linux-x86_64.tar.gz" ;;
	Darwin-arm64 | Darwin-aarch64) asset="kicau-macos-arm64.tar.gz" ;;
	*) die "no prebuilt binary for $(uname -s) $(uname -m). Build from source: cargo install --git https://github.com/$REPO.git --locked" ;;
esac

command -v curl >/dev/null 2>&1 || die "curl is required"
command -v tar >/dev/null 2>&1 || die "tar is required"

if [ "$VERSION" = "latest" ]; then
	base="https://github.com/$REPO/releases/latest/download"
else
	base="https://github.com/$REPO/releases/download/$VERSION"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "kicau: downloading $asset"
curl -fsSL "$base/$asset" -o "$tmp/$asset" || die "download failed: $base/$asset"

# The checksum lives beside the asset, so this catches a truncated or corrupted
# download, not a compromised release.
if curl -fsSL "$base/checksums.txt" -o "$tmp/checksums.txt" 2>/dev/null; then
	if command -v sha256sum >/dev/null 2>&1; then
		want="$(grep " $asset\$" "$tmp/checksums.txt" | cut -d' ' -f1)"
		got="$(sha256sum "$tmp/$asset" | cut -d' ' -f1)"
	elif command -v shasum >/dev/null 2>&1; then
		want="$(grep " $asset\$" "$tmp/checksums.txt" | cut -d' ' -f1)"
		got="$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)"
	else
		want=""
	fi
	if [ -n "${want:-}" ] && [ "$want" != "$got" ]; then
		die "checksum mismatch for $asset"
	fi
fi

tar xzf "$tmp/$asset" -C "$tmp" || die "could not unpack $asset"
[ -f "$tmp/kicau" ] || die "archive did not contain kicau"
chmod +x "$tmp/kicau"

if [ -w "$BIN_DIR" ]; then
	mv "$tmp/kicau" "$BIN_DIR/kicau"
elif command -v sudo >/dev/null 2>&1; then
	echo "kicau: $BIN_DIR needs root"
	sudo mv "$tmp/kicau" "$BIN_DIR/kicau"
else
	die "$BIN_DIR is not writable. Retry with: KICAU_BIN_DIR=\$HOME/.local/bin sh"
fi

echo "kicau: installed to $BIN_DIR/kicau"
case ":$PATH:" in
	*":$BIN_DIR:"*) ;;
	*) echo "kicau: note that $BIN_DIR is not on your PATH" ;;
esac

# Teach any agent framework you already use how to drive kicau. The skill lives
# in the repo, not the release, so a doc update ships without a new binary build.
# Only frameworks present under $HOME get it; missing ones are skipped, not made.
skill_url="https://raw.githubusercontent.com/$REPO/main/skills/kicau/SKILL.md"
if curl -fsSL "$skill_url" -o "$tmp/SKILL.md" 2>/dev/null; then
	installed_skill=""
	for fw in .agents .claude .openclaw .hermes; do
		[ -d "$HOME/$fw" ] || continue
		dest="$HOME/$fw/skills/kicau"
		if mkdir -p "$dest" 2>/dev/null && cp "$tmp/SKILL.md" "$dest/SKILL.md" 2>/dev/null; then
			installed_skill="$installed_skill $fw"
		fi
	done
	[ -n "$installed_skill" ] && echo "kicau: installed the kicau skill to$installed_skill"
fi

echo
echo "Next: kicau login"
