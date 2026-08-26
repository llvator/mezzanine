#!/usr/bin/env bash
# Recompile Mezzanine in place, without installing anything.
#
#   ./scripts/build.sh                # binaries only (release)
#   ./scripts/build.sh --debug        # binaries only (debug — much faster)
#   ./scripts/build.sh --ui           # + browser UI      -> ui/dist
#   ./scripts/build.sh --webview      # + VS Code webview -> vscode-extension/webview-dist
#   ./scripts/build.sh --all          # binaries + both frontends
#
# Artifacts land in target/{release,debug}/ and the dist dirs above; nothing
# is copied to ~/.cargo/bin or into VS Code. Use scripts/install.sh for that.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

PROFILE="release"
BUILD_UI=0
BUILD_WEBVIEW=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --debug)   PROFILE="debug" ;;
    --release) PROFILE="release" ;;
    --ui)      BUILD_UI=1 ;;
    --webview) BUILD_WEBVIEW=1 ;;
    --all)     BUILD_UI=1; BUILD_WEBVIEW=1 ;;
    # Usage text is the header comment block, minus the shebang — printed by
    # awk so it can't drift out of sync when the header is edited.
    -h|--help) awk 'NR>1 && /^#/ {sub(/^# ?/, ""); print; next} NR>1 {exit}' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "!! Unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

echo "==> cargo build ($PROFILE): mezz, elevator"
if [[ "$PROFILE" == "release" ]]; then
  cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml"
else
  cargo build --manifest-path "$REPO_ROOT/Cargo.toml"
fi
echo "    binaries at $REPO_ROOT/target/$PROFILE/{mezz,elevator}"

# Both frontend builds share ui/'s node_modules, so install once if needed.
if [[ $BUILD_UI -eq 1 || $BUILD_WEBVIEW -eq 1 ]]; then
  if [[ ! -d "$REPO_ROOT/ui/node_modules" ]]; then
    echo "==> ui/: installing dependencies"
    (cd "$REPO_ROOT/ui" && npm install)
  fi
fi

if [[ $BUILD_UI -eq 1 ]]; then
  # Served by `mezz watch` as the relative path ui/dist (src/server/mod.rs),
  # so it only resolves when the server is started from the repo root.
  echo "==> Building browser UI -> ui/dist"
  (cd "$REPO_ROOT/ui" && npm run build)
fi

if [[ $BUILD_WEBVIEW -eq 1 ]]; then
  # VSCODE_BUILD=1 redirects Vite's outDir to the extension folder. This is
  # the copy that gets bundled into the .vsix.
  echo "==> Building VS Code webview -> vscode-extension/webview-dist"
  (cd "$REPO_ROOT/ui" && npm run build:vscode)
fi

echo
echo "Done. Run the local binary with: $REPO_ROOT/target/$PROFILE/mezz"
