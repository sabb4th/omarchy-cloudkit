#!/usr/bin/env bash
# Install omarchy-cloudkit into the running Omarchy shell.
#
#   ./install.sh             copy this checkout into ~/.config/omarchy/plugins
#   ./install.sh --in-place   build inside an Omarchy marketplace checkout
#   ./install.sh --link      symlink it instead (for development)
#   ./install.sh --uninstall stop the service, disable the widget, remove what this script installed
#   --no-restart             on an update, don't restart the shell and guacd to load the new code
#   --no-enable              install without adding the widget to the bar
#   --no-install-deps        check dependencies without installing missing packages
#   --no-build               use an already compiled bin/cloudkit-native
#
# Build from this checkout before enabling the widget. Only installations carrying
# this project's manifest and installer marker may be replaced.
set -euo pipefail

SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_ID="sabb4th.cloudkit"
PLUGIN_DIR="$HOME/.config/omarchy/plugins/$PLUGIN_ID"
CLI_LINK="$HOME/.local/bin/omarchy-cloudkit"
MARKER=".cloudkit-install"
COPIED=(manifest.json ui bin assets LICENSE README.md docs)
MODE="copy"
ENABLE=true
RESTART=true
UPDATE=false
INSTALL_DEPS=true
BUILD=true
MIN_RCLONE="1.75.2"

for arg in "$@"; do
  case "$arg" in
    --copy) MODE="copy" ;;
    --link) MODE="link" ;;
    --in-place) MODE="in-place" ;;
    --no-enable) ENABLE=false ;;
    --no-restart) RESTART=false ;;
    --no-install-deps) INSTALL_DEPS=false ;;
    --no-build) BUILD=false ;;
    --uninstall) MODE="uninstall" ;;
    -h|--help)
      sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) echo "Unknown option: $arg" >&2; exit 1 ;;
  esac
done

say() { printf '\033[1;32m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m==>\033[0m %s\n' "$*" >&2; }

is_ours() {
  # A symlink to a checkout with our manifest, or a copy this script made
  if [[ -L $PLUGIN_DIR ]]; then
    local target
    target="$(readlink -f "$PLUGIN_DIR")" || return 1
    grep -Fq "\"id\": \"$PLUGIN_ID\"" "$target/manifest.json" 2>/dev/null
  else
    [[ -f $PLUGIN_DIR/$MARKER ]] && grep -Fq "\"id\": \"$PLUGIN_ID\"" "$PLUGIN_DIR/manifest.json" 2>/dev/null
  fi
}

cli_link_is_ours() {
  # -m also resolves dangling links, without mistaking another project's bin/guac for ours.
  [[ -L $CLI_LINK && "$(readlink -m "$CLI_LINK")" == "$(readlink -m "$PLUGIN_DIR/bin/omarchy-cloudkit")" ]]
}

reload_shell() {
  command -v omarchy-shell >/dev/null 2>&1 && omarchy-shell shell rescanPlugins >/dev/null 2>&1 || true
}

stop_daemon() {
  if [[ -x $SRC/bin/guac ]]; then
    "$SRC/bin/omarchy-cloudkit" stop >/dev/null 2>&1 || true
  fi
}

# Files still uploading or folders syncing in the running guacd (0 when it isn't running)
daemon_busy() {
  "$SRC/bin/cloudkit-native" busy
}

# The shell loads a changed widget only when it restarts (its plugin reload keeps cached
# QML), and the daemon keeps running its original executable until restarted.
restart_after_update() {
  $UPDATE && $RESTART || return 0
  local busy
  if busy=$(daemon_busy) && [[ $busy == 0 ]]; then
    stop_daemon
  else
    warn "guacd is busy or its state could not be verified; it keeps running. Later: omarchy-cloudkit stop"
  fi
  if command -v omarchy-restart-shell >/dev/null 2>&1; then
    say "Restarting the shell to load the new widget (guacd starts again with it)"
    omarchy-restart-shell >/dev/null 2>&1 || warn "Could not restart the shell: omarchy restart shell"
  else
    warn "Restart the shell to load the new widget"
  fi
}

if [[ $MODE == "uninstall" ]]; then
  if [[ -x $SRC/bin/cloudkit-native ]]; then
    busy=$(daemon_busy) || { warn "Cannot verify daemon activity; nothing removed."; exit 1; }
    [[ $busy == 0 ]] || { warn "Wait for uploads and syncs to finish before uninstalling."; exit 1; }
  fi
  REMOVE_CLI=false
  cli_link_is_ours && REMOVE_CLI=true
  if command -v omarchy >/dev/null 2>&1; then
    omarchy plugin disable "$PLUGIN_ID" >/dev/null 2>&1 || true
  fi
  stop_daemon
  unit="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/omarchy-cloudkit.service"
  if [[ -f $unit ]] && grep -Fq "$PLUGIN_DIR/bin/cloudkit-native" "$unit"; then
    systemctl --user disable --now omarchy-cloudkit.service || { warn "Could not stop the login service; installation preserved."; exit 1; }
    "$SRC/bin/omarchy-cloudkit" service uninstall
  fi
  if [[ -e $PLUGIN_DIR || -L $PLUGIN_DIR ]]; then
    if is_ours; then
      if [[ -L $PLUGIN_DIR ]]; then rm "$PLUGIN_DIR"; else rm -rf "$PLUGIN_DIR"; fi
      say "Removed $PLUGIN_DIR"
    else
      warn "Left $PLUGIN_DIR alone: it wasn't installed by this script (use: omarchy plugin remove $PLUGIN_ID)"
    fi
  fi
  if $REMOVE_CLI; then
    rm "$CLI_LINK"
    say "Removed $CLI_LINK"
  fi
  reload_shell
  say "Uninstalled. Your settings (~/.config/guacamole), rclone accounts and local folders were kept."
  exit 0
fi

# Install only missing packages. Never refresh package databases without a full
# system upgrade, and never run the plugin/file operations as root.
if (( EUID == 0 )); then
  warn "Run this installer as your desktop user; only package installation uses elevation."
  exit 1
fi
missing=()
for entry in rclone:rclone fusermount3:fuse3 qs:quickshell wl-copy:wl-clipboard notify-send:libnotify gio:glib2 xdg-open:xdg-utils; do
  command -v "${entry%%:*}" >/dev/null 2>&1 || missing+=("${entry#*:}")
done
if $BUILD; then
  command -v cargo >/dev/null 2>&1 || missing+=(rust)
  command -v cc >/dev/null 2>&1 || missing+=(gcc)
fi
if (( ${#missing[@]} )); then
  if ! $INSTALL_DEPS; then
    warn "Missing packages: ${missing[*]}"
    exit 1
  fi
  if ! command -v pacman >/dev/null 2>&1; then
    warn "Automatic dependency installation currently supports Arch/Omarchy only: ${missing[*]}"
    exit 1
  fi
  say "Installing missing packages: ${missing[*]}"
  if [[ -t 0 ]] && command -v sudo >/dev/null 2>&1; then
    sudo pacman -S --needed -- "${missing[@]}"
  else
    pkexec pacman -S --needed --noconfirm -- "${missing[@]}"
  fi
fi
# Older distributions may ship rclone without the RC/bisync APIs we use.
# Refuse before replacing an installation instead of entering a restart loop.
rclone_version=$(rclone version)
if [[ $rclone_version =~ ^rclone[[:space:]]v([0-9]+\.[0-9]+\.[0-9]+) ]]; then
  detected_rclone="${BASH_REMATCH[1]}"
else
  warn "Cannot determine rclone version; installation preserved."
  exit 1
fi
if ! printf '%s\n' "$MIN_RCLONE" "$detected_rclone" | sort -V -C; then
  warn "rclone >= $MIN_RCLONE is required (found $detected_rclone). Update rclone before installing."
  exit 1
fi

if $BUILD; then
  say "Building omarchy-cloudkit"
  cargo build --manifest-path "$SRC/Cargo.toml" --release --locked
  # Rename rather than truncate: an existing bridge may still be executing.
  native_tmp=$(mktemp "$SRC/bin/.cloudkit-native.XXXXXX")
  trap 'rm -f -- "$native_tmp"' EXIT
  install -m 755 "$SRC/target/release/omarchy-cloudkit" "$native_tmp"
  mv -f -- "$native_tmp" "$SRC/bin/cloudkit-native"
  trap - EXIT
elif [[ ! -x $SRC/bin/cloudkit-native ]]; then
  warn "Missing bin/cloudkit-native; omit --no-build to compile it."
  exit 1
fi

mkdir -p "$(dirname "$PLUGIN_DIR")"
if [[ $MODE == "in-place" ]]; then
  if [[ $(readlink -f -- "$PLUGIN_DIR") != "$SRC" ]]; then
    warn "--in-place must run from $PLUGIN_DIR; nothing replaced."
    exit 1
  fi
  UPDATE=true
elif [[ -e $PLUGIN_DIR || -L $PLUGIN_DIR ]]; then
  UPDATE=true
  if ! is_ours; then
    warn "$PLUGIN_DIR exists and was not installed by this script."
    warn "Use the plugin manager for that installation."
    exit 1
  fi
fi

if [[ $MODE == "in-place" ]]; then
  say "Built in place; checkout and Git history preserved"
elif [[ $MODE == "link" && -L $PLUGIN_DIR && "$(readlink -f "$PLUGIN_DIR")" == "$SRC" ]]; then
  say "Already linked to this checkout"
else
  # Stage everything before moving the previous installation. A failed copy or
  # build leaves the working installation intact.
  stage=$(mktemp -d "$(dirname "$PLUGIN_DIR")/.cloudkit-stage.XXXXXX")
  previous=""
  cleanup_stage() {
    [[ ! -e $stage && ! -L $stage ]] || rm -rf -- "$stage"
    if [[ -n $previous && ( -e $previous || -L $previous ) && ! -e $PLUGIN_DIR && ! -L $PLUGIN_DIR ]]; then
      mv -- "$previous" "$PLUGIN_DIR"
    fi
  }
  trap cleanup_stage EXIT
  if [[ $MODE == "link" ]]; then
    rmdir -- "$stage"
    ln -s -- "$SRC" "$stage"
  else
    for entry in "${COPIED[@]}"; do
      [[ -e $SRC/$entry ]] && cp -a -- "$SRC/$entry" "$stage/"
    done
    # Only the Rust executable is distributed; the earlier bridge prototype is
    # not needed by this version.
    if [[ -f $stage/bin/guac-native ]]; then rm -- "$stage/bin/guac-native"; fi
    touch "$stage/$MARKER"
  fi
  if $UPDATE; then
    previous=$(mktemp -d "$(dirname "$PLUGIN_DIR")/.cloudkit-previous.XXXXXX")
    rmdir -- "$previous"
    mv -- "$PLUGIN_DIR" "$previous"
  fi
  mv -- "$stage" "$PLUGIN_DIR"
  if [[ -n $previous ]]; then rm -rf -- "$previous"; fi
  trap - EXIT
  say "Installed $PLUGIN_DIR"
fi

mkdir -p "$(dirname "$CLI_LINK")"
if [[ ! -e $CLI_LINK && ! -L $CLI_LINK ]] || cli_link_is_ours; then
  ln -sfn "$PLUGIN_DIR/bin/omarchy-cloudkit" "$CLI_LINK"
  say "Linked $CLI_LINK"
else
  warn "Left $CLI_LINK alone (it isn't ours)"
fi

reload_shell
if $ENABLE && command -v omarchy >/dev/null 2>&1; then
  # The shell discovers the plugin asynchronously after the rescan: give it a moment
  enabled=false
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    if omarchy plugin enable "$PLUGIN_ID" >/dev/null 2>&1; then enabled=true; break; fi
    sleep 0.5
  done
  if $enabled; then
    say "Enabled the Clouds widget in the bar"
  else
    warn "Could not enable the widget automatically: omarchy plugin enable $PLUGIN_ID"
  fi
fi
restart_after_update
say "Done. Click the cloud in the bar, or run: omarchy-cloudkit status"
