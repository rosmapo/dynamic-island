#!/bin/sh
# Zostaví a nainštaluje dynamic-island pre používateľa (Debian 13).
set -e
cd "$(dirname "$0")/.."
cargo build --release
install -Dm755 target/release/dynamic-island "$HOME/.local/bin/dynamic-island"
[ -e "$HOME/.config/dynamic-island/config.toml" ] || install -Dm644 config/config.toml "$HOME/.config/dynamic-island/config.toml"
echo "Hotovo. Spusti: dynamic-island   (alebo ho daj do [autostart] vo wayfire.ini)"
