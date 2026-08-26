#!/usr/bin/env bash
# Build and install Mezzanine from this checkout: both binaries (mezz, elevator)
# plus the VS Code extension.
#
#   ./scripts/install.sh                      # default VS Code profile
#   ./scripts/install.sh Playground Work      # one or more named profiles
#
# Env overrides:
#   MEZZ_CODE_CLI=cursor   # or code-insiders, codium — defaults to the first found
#   MEZZ_SKIP_EXTENSION=1  # skip the VS Code extension
#   MEZZ_SKIP_UI=1         # skip the browser UI (no node toolchain needed)
set -euo pipefail

# Repo root = parent of this script's directory, so the script works from
# anywhere and for any checkout location.
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXT_DIR="$REPO_ROOT/vscode-extension"

# Where `cargo install` puts things, by cargo's own resolution order. Assuming
# ~/.cargo breaks anyone who has moved CARGO_HOME, and the UI has to land
# beside the binary that looks for it.
CARGO_BIN="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}/bin"

echo "==> Installing mezz + elevator binaries to $CARGO_BIN"
cargo install --path "$REPO_ROOT" --force

# The browser UI `mezz watch` serves.
#
# `ui_dir.rs` looks for a `ui/dist` beside the running executable — the
# candidate meant for an installed layout, "what an installed layout can
# satisfy without the repo being present". Nothing populated it, so an
# installed mezz could only ever serve a UI when launched from a repo root,
# which is the coupling SRV-008 set out to remove. People worked around it by
# writing `ui_dir` into their settings, pointing at a build that then never
# updated (SRV-018).
#
# Copied, not symlinked: a link into this checkout would break when the
# checkout moves, and would make an installed mezz depend on the repo it came
# from.
#
# A UI stays optional throughout: with none installed, `mezz watch` still runs,
# still serves its API and SSE stream, and its root page explains how to point
# a UI hosted elsewhere at it. The VS Code extension is unaffected either way —
# it carries `webview-dist` inside the .vsix and never consults `ui_dir`.
if [[ -n "${MEZZ_SKIP_UI:-}" ]]; then
  echo "==> MEZZ_SKIP_UI set — not installing the browser UI."
  # Skipping means "leave it alone", so a copy from an earlier run keeps being
  # served. Say how to get rid of one rather than deleting it unasked.
  if [[ -e "$CARGO_BIN/ui/dist" ]]; then
    echo "    (an earlier install left one at $CARGO_BIN/ui/dist — rm -rf it to serve none)"
  fi
elif ! command -v npm >/dev/null 2>&1; then
  echo "!! npm not on PATH — skipping the browser UI." >&2
  echo "   The binaries are installed; \`mezz watch\` will explain how to connect one." >&2
else
  echo "==> Building browser UI -> $CARGO_BIN/ui/dist"
  # Deps only when they're absent. `npm ci` is deliberately not used here:
  # every other path into `ui/` assumes an existing node_modules — the
  # extension's own `build:webview` is a bare `cd ../ui && npm run build` —
  # and `ui/package-lock.json` is currently out of sync with its package.json,
  # so a `ci` would fail the whole install over a drifted lockfile.
  if [[ ! -d "$REPO_ROOT/ui/node_modules" ]]; then
    (cd "$REPO_ROOT/ui" && npm install)
  fi
  (cd "$REPO_ROOT/ui" && npm run build)
  # Asset filenames are content-hashed, so overlaying leaves every previous
  # bundle behind and the directory grows without bound. Replace it.
  rm -rf "${CARGO_BIN:?}/ui/dist"
  mkdir -p "$CARGO_BIN/ui"
  cp -R "$REPO_ROOT/ui/dist" "$CARGO_BIN/ui/dist"
fi

if [[ -n "${MEZZ_SKIP_EXTENSION:-}" ]]; then
  echo "==> MEZZ_SKIP_EXTENSION set — done."
  exit 0
fi

# Find an editor CLI. Other developers may be on Cursor/VSCodium, or may not
# have installed the shell command at all.
CODE_CLI="${MEZZ_CODE_CLI:-}"
if [[ -z "$CODE_CLI" ]]; then
  for candidate in code code-insiders cursor codium; do
    if command -v "$candidate" >/dev/null 2>&1; then CODE_CLI="$candidate"; break; fi
  done
fi
if [[ -z "$CODE_CLI" ]]; then
  echo "!! No VS Code CLI on PATH. In VS Code run:" >&2
  echo "   Command Palette -> Shell Command: Install 'code' command in PATH" >&2
  echo "   (or set MEZZ_CODE_CLI=<your-cli>, or MEZZ_SKIP_EXTENSION=1)" >&2
  exit 1
fi

echo "==> Building the extension (editor CLI: $CODE_CLI)"
# `npm ci` when the lockfile is present so contributors get the pinned tree;
# fall back to `npm install` for checkouts without one.
if [[ -f "$EXT_DIR/package-lock.json" ]]; then
  (cd "$EXT_DIR" && npm ci)
else
  (cd "$EXT_DIR" && npm install)
fi

# Remove stale VSIXs so the "newest file" pick can't select a previous build.
rm -f "$EXT_DIR"/*.vsix
(cd "$EXT_DIR" && npm run package)

VSIX="$(ls -t "$EXT_DIR"/*.vsix | head -n1)"
[[ -f "$VSIX" ]] || { echo "!! Packaging produced no .vsix" >&2; exit 1; }
echo "==> Packaged $(basename "$VSIX")"

# No profile arguments -> install into the default profile.
PROFILES=("$@")
if [[ ${#PROFILES[@]} -eq 0 ]]; then
  "$CODE_CLI" --install-extension "$VSIX" --force
else
  for profile in "${PROFILES[@]}"; do
    echo "==> Installing into profile: $profile"
    "$CODE_CLI" --profile "$profile" --install-extension "$VSIX" --force
  done
fi

echo
echo "Done. Reload each window: Cmd+Shift+P -> Developer: Reload Window"
