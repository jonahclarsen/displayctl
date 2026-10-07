#!/bin/zsh
# Runs the menu bar app in dev mode (debug build, verbose logs). The LaunchAgent runs this.
set -eu
cd "${0:A:h:h}"
echo "--- $(date '+%Y-%m-%d %H:%M:%S') starting displayctl dev"
scripts/build-cli.sh
[[ -d node_modules ]] || pnpm install --frozen-lockfile
exec pnpm tauri dev
