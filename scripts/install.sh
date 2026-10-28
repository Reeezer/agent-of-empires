#!/bin/bash
set -euo pipefail

if [ "$(uname -s)" != Darwin ]; then
    echo 'This fork supports macOS only.' >&2
    exit 1
fi

case "$(uname -m)" in
    arm64) arch=arm64 ;;
    x86_64) arch=amd64 ;;
    *) echo 'Unsupported Mac architecture.' >&2; exit 1 ;;
esac

for tool in tmux node npx; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo 'Install the prerequisites first: brew install tmux node' >&2
        exit 1
    fi
done

install_dir="${INSTALL_DIR:-$HOME/.local/bin}"
asset="aoe-darwin-$arch.tar.gz"
release='https://github.com/Reeezer/agent-of-empires/releases/latest/download'
tmp_dir=$(mktemp -d)
trap 'rm -rf "$tmp_dir"' EXIT

curl -fsSL "$release/$asset" -o "$tmp_dir/$asset"
curl -fsSL "$release/$asset.sha256" -o "$tmp_dir/$asset.sha256"
(cd "$tmp_dir" && shasum -a 256 -c "$asset.sha256")
tar xzf "$tmp_dir/$asset" -C "$tmp_dir"
npx --yes ccusage@20.0.40 --version >/dev/null

mkdir -p "$install_dir"
install -m 755 "$tmp_dir/aoe-darwin-$arch" "$install_dir/aoe"
"$install_dir/aoe" --version
echo "Installed in $install_dir. Run aoe to get started."
