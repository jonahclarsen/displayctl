#!/bin/bash
set -euo pipefail

# Pin the upstream version used by the successful ASUS XG32UCWMG test.
revision=04d949794102eb8df01ad3681afff6464a3eede2
build_dir=$(mktemp -d /tmp/displayctl-m1ddc.XXXXXX)
git clone --no-checkout https://github.com/waydabber/m1ddc.git "$build_dir/source"
git -C "$build_dir/source" checkout --detach "$revision"
make -C "$build_dir/source"
mkdir -p "$HOME/.local/libexec/displayctl"
install -m 755 "$build_dir/source/m1ddc" "$HOME/.local/libexec/displayctl/m1ddc"
echo "Installed brightness helper from $revision. Build sources retained in $build_dir."
