#!/usr/bin/env bash
# Install the tracked pre-commit hook at .git/hooks/pre-commit.
#
# Run once per fresh clone:
#   bash scripts/install_hooks.sh
#
# The installed hook is a symlink to scripts/hooks/pre-commit, so
# updates to the tracked file take effect on every contributor's machine
# without re-running this script.

set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

src=scripts/hooks/pre-commit
target=.git/hooks/pre-commit

if [ ! -f "$src" ]; then
    echo "Source hook not found: $src" >&2
    exit 1
fi
chmod +x "$src"

if [ -e "$target" ] && [ ! -L "$target" ]; then
    echo "Refusing to overwrite a non-symlink hook: $target" >&2
    echo "Move it aside or delete it, then re-run." >&2
    exit 1
fi

# Use a relative target so the symlink survives moving the repo.
ln -sf "../../$src" "$target"
echo "Installed: $target -> $src"
