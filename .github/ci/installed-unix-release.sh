#!/usr/bin/env bash
set -euo pipefail

if [[ ${GITHUB_ACTIONS:-} != true || ${RUNNER_ENVIRONMENT:-} != github-hosted ]]; then
  echo 'Run the installed GUI smoke on a disposable GitHub-hosted runner.' >&2
  exit 2
fi

repo="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
target="${CARGO_TARGET_DIR:?Cargo release target required}"
scratch="$(mktemp -d "${RUNNER_TEMP:?Runner temp directory required}/mcp-builder-installed.XXXXXX")"
scratch="$(cd -- "$scratch" && pwd -P)"
app_pid=
installed_package=
mount=
cleanup() {
  local result=$?
  trap - EXIT
  if [[ -n "$app_pid" ]]; then
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" || true
  fi
  if [[ -n "$mount" ]]; then hdiutil detach "$mount" >/dev/null || result=1; fi
  if [[ -n "$installed_package" ]]; then sudo apt-get purge -y "$installed_package" || result=1; fi
  if (( result != 0 )) && [[ -f "$scratch/desktop.log" ]]; then cat "$scratch/desktop.log" >&2; fi
  rm -rf -- "$scratch"
  exit "$result"
}
trap cleanup EXIT

mkdir -- "$scratch/tmp" "$scratch/profile" "$scratch/catalog"
export TMPDIR="$scratch/tmp"
manifest="$scratch/catalog/mcp-project.json"
cp -- "$repo/crates/builder-project/tests/fixtures/catalog-v1.json" "$manifest"
shopt -s nullglob
case "$RUNNER_OS" in
  macOS)
    release="$target/aarch64-apple-darwin/release"
    packages=("$release"/bundle/dmg/*.dmg)
    ;;
  Linux)
    release="$target/x86_64-unknown-linux-gnu/release"
    packages=("$release"/bundle/deb/*.deb)
    ;;
  *)
    echo "Unexpected runner OS $RUNNER_OS" >&2
    exit 2
    ;;
esac
if (( ${#packages[@]} != 1 )); then
  echo "Expected exactly one installer in $release/bundle" >&2
  exit 1
fi
installer="${packages[0]}"
checksum="$(shasum -a 256 "$installer" | cut -d ' ' -f1)"
echo "Installer SHA-256: $checksum"

if [[ "$RUNNER_OS" == macOS ]]; then
  hdiutil verify "$installer"
  mkdir -- "$scratch/mount" "$scratch/Applications"
  hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$scratch/mount" "$installer"
  mount="$scratch/mount"
  bundle='MCP Server Builder.app'
  app="$scratch/Applications/$bundle"
  ditto "$mount/$bundle" "$app"
  hdiutil detach "$mount"
  mount=
  /usr/bin/codesign --verify --deep --strict "$app"
  /usr/bin/codesign -dv --verbose=4 "$app" 2>&1 | grep -Fx 'Signature=adhoc'
  cli="$app/Contents/MacOS/mcp-builder"
  desktop="$app/Contents/MacOS/mcp-builder-desktop"
  cmp -- "$release/bundle/macos/$bundle/Contents/MacOS/mcp-builder" "$cli"
  xcrun swiftc -O "$repo/.github/ci/installed-macos-window.swift" -o "$scratch/check-window"
else
  package="$(dpkg-deb -f "$installer" Package)"
  [[ "$package" =~ ^[a-z0-9][a-z0-9.+-]+$ && "$(dpkg-deb -f "$installer" Architecture)" == amd64 ]]
  if dpkg-query -W -f='${db:Status-Status}' "$package" 2>/dev/null | grep -qx installed; then
    echo "The smoke requires a runner without $package installed." >&2
    exit 1
  fi
  installed_package="$package"
  sudo apt-get install -y --no-install-recommends "$installer"
  cli=/usr/bin/mcp-builder
  desktop=/usr/bin/mcp-builder-desktop
  cmp -- "$release/mcp-builder" "$cli"
fi

[[ -x "$cli" && -x "$desktop" ]]
"$cli" --version
desktop_env=("TMPDIR=$TMPDIR")
if [[ "$RUNNER_OS" == Linux ]]; then
  desktop_env+=("XDG_DATA_HOME=$scratch/profile/data" "XDG_CONFIG_HOME=$scratch/profile/config"
                "XDG_CACHE_HOME=$scratch/profile/cache")
fi
env "${desktop_env[@]}" "$cli" --json project open "$scratch/catalog" |
  node -e 'const reply = JSON.parse(require("node:fs").readFileSync(0, "utf8")); if (!reply.ok || reply.data.document.name !== "Catalog sample") process.exit(1);'
env "${desktop_env[@]}" "$desktop" >"$scratch/desktop.log" 2>&1 &
app_pid=$!
if [[ "$RUNNER_OS" == macOS ]]; then
  "$scratch/check-window" "$app_pid"
else
  visible=0
  for (( attempt=0; attempt < 120; attempt++ )); do
    kill -0 "$app_pid"
    if xdotool search --all --onlyvisible --pid "$app_pid" --name '^MCP Server Builder$' >/dev/null 2>&1; then
      visible=1
      break
    fi
    sleep 0.25
  done
  if (( visible == 0 )); then echo 'The installed application did not open a native window.' >&2; exit 1; fi
  echo 'The installed application opened a native window.'
fi
kill "$app_pid"
wait "$app_pid" || true
app_pid=

node "$repo/.github/ci/standalone-smoke.mjs" "$cli"

if [[ "$RUNNER_OS" == macOS ]]; then
  rm -rf -- "$app"
else
  sudo apt-get purge -y "$installed_package"
  installed_package=
fi
[[ ! -e "$desktop" && -f "$manifest" ]]
[[ "$(shasum -a 256 "$installer" | cut -d ' ' -f1)" == "$checksum" ]]

printf 'Installed %s package, opened its desktop window, ran bundled CLI replay and standalone tests, and removed it with the project intact. Installer SHA-256 %s.\n' "$RUNNER_OS" "$checksum" >> "$GITHUB_STEP_SUMMARY"
printf 'installer-sha256=%s\n' "$checksum" >> "$GITHUB_OUTPUT"
