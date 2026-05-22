#!/bin/sh

set -eu

RELEASE="latest"
INSTALL_VARIANT="${CODEX_INSTALL_VARIANT:-${CODEX_INSTALL_PRODUCT:-}}"

LOCK_STALE_AFTER_SECS=600

product_name=""
command_name=""
package_asset_prefix=""
npm_package_name=""
legacy_platform_npm_prefix=""
release_repo=""
bin_dir=""
bin_path=""
product_home_dir=""
standalone_root=""
releases_dir=""
current_link=""
lock_file=""
lock_dir=""
path_action="already"
path_profile=""
conflict_manager=""
conflict_path=""
lock_kind=""
tmp_dir=""

case "${0##*/}" in
  install-claudex.sh | claudex-install.sh)
    if [ -z "$INSTALL_VARIANT" ]; then
      INSTALL_VARIANT="claudex"
    fi
    ;;
esac

configure_product() {
  if [ -z "$INSTALL_VARIANT" ]; then
    INSTALL_VARIANT="codex"
  fi

  case "$INSTALL_VARIANT" in
    codex)
      product_name="Codex"
      command_name="codex"
      package_asset_prefix="codex"
      npm_package_name="@openai/codex"
      legacy_platform_npm_prefix="codex"
      release_repo="${CODEX_RELEASE_REPO:-openai/codex}"
      bin_dir="${CODEX_INSTALL_DIR:-$HOME/.local/bin}"
      product_home_dir="${CODEX_HOME:-$HOME/.codex}"
      ;;
    claudex)
      product_name="Claudex"
      command_name="claudex"
      package_asset_prefix="claudex"
      npm_package_name="claudex"
      legacy_platform_npm_prefix=""
      release_repo="${CLAUDEX_RELEASE_REPO:-${CODEX_RELEASE_REPO:-KingDonRush/codex}}"
      bin_dir="${CLAUDEX_INSTALL_DIR:-${CODEX_INSTALL_DIR:-$HOME/.local/bin}}"
      product_home_dir="${CLAUDEX_HOME:-$HOME/.claudex}"
      ;;
    *)
      echo "Unsupported install variant: $INSTALL_VARIANT" >&2
      exit 1
      ;;
  esac

  bin_path="$bin_dir/$command_name"
  standalone_root="$product_home_dir/packages/standalone"
  releases_dir="$standalone_root/releases"
  current_link="$standalone_root/current"
  lock_file="$standalone_root/install.lock"
  lock_dir="$standalone_root/install.lock.d"

  BIN_DIR="$bin_dir"
  BIN_PATH="$bin_path"
  STANDALONE_ROOT="$standalone_root"
  RELEASES_DIR="$releases_dir"
  CURRENT_LINK="$current_link"
  LOCK_FILE="$lock_file"
  LOCK_DIR="$lock_dir"
}

step() {
  printf '==> %s\n' "$1"
}

warn() {
  printf 'WARNING: %s\n' "$1" >&2
}

normalize_version() {
  case "$1" in
    "" | latest)
      printf 'latest\n'
      ;;
    rust-v*)
      printf '%s\n' "${1#rust-v}"
      ;;
    v*)
      printf '%s\n' "${1#v}"
      ;;
    *)
      printf '%s\n' "$1"
      ;;
  esac
}

parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --release)
        if [ "$#" -lt 2 ]; then
          echo "--release requires a value." >&2
          exit 1
        fi
        RELEASE="$2"
        shift
        ;;
      --variant | --product)
        if [ "$#" -lt 2 ]; then
          echo "$1 requires a value." >&2
          exit 1
        fi
        INSTALL_VARIANT="$2"
        shift
        ;;
      --help | -h)
        cat <<EOF
Usage: install.sh [--release VERSION] [--variant codex|claudex]
EOF
        exit 0
        ;;
      *)
        echo "Unknown argument: $1" >&2
        exit 1
        ;;
    esac
    shift
  done
}

download_file() {
  url="$1"
  output="$2"

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$output"
    return
  fi

  if command -v wget >/dev/null 2>&1; then
    wget -q -O "$output" "$url"
    return
  fi

  echo "curl or wget is required to install $product_name." >&2
  exit 1
}

download_text() {
  url="$1"

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url"
    return
  fi

  if command -v wget >/dev/null 2>&1; then
    wget -q -O - "$url"
    return
  fi

  echo "curl or wget is required to install $product_name." >&2
  exit 1
}

release_url_for_asset() {
  asset="$1"
  resolved_version="$2"

  printf 'https://github.com/%s/releases/download/rust-v%s/%s\n' "$release_repo" "$resolved_version" "$asset"
}

release_metadata_url() {
  resolved_version="$1"

  printf 'https://api.github.com/repos/%s/releases/tags/rust-v%s\n' "$release_repo" "$resolved_version"
}

release_asset_digest_or_empty() {
  asset="$1"
  resolved_version="$2"
  release_json="$(download_text "$(release_metadata_url "$resolved_version")")"

  digest="$(printf '%s\n' "$release_json" | awk -v asset="$asset" '
    /"name":[[:space:]]*"[^"]+"/ {
      name = $0
      sub(/^.*"name":[[:space:]]*"/, "", name)
      sub(/".*$/, "", name)
      if (name == asset) {
        in_asset = 1
        asset_depth = depth
      }
    }

    in_asset && /"digest":[[:space:]]*"[^"]+"/ {
      digest = $0
      sub(/^.*"digest":[[:space:]]*"/, "", digest)
      sub(/".*$/, "", digest)
    }

    {
      line = $0
      opens = gsub(/\{/, "{", line)
      closes = gsub(/\}/, "}", line)
      depth += opens - closes

      if (in_asset && depth < asset_depth) {
        in_asset = 0
      }
    }

    END {
      if (digest != "") {
        print digest
      }
    }
  ')"

  case "$digest" in
    sha256:????????????????????????????????????????????????????????????????)
      printf '%s\n' "${digest#sha256:}"
      ;;
    *)
      return 1
      ;;
  esac
}

release_asset_exists() {
  asset="$1"
  resolved_version="$2"

  release_asset_digest_or_empty "$asset" "$resolved_version" >/dev/null 2>&1
}

release_asset_digest() {
  asset="$1"
  resolved_version="$2"

  digest="$(release_asset_digest_or_empty "$asset" "$resolved_version" || true)"
  if [ -z "$digest" ]; then
    echo "Could not find SHA-256 digest for release asset $asset." >&2
    exit 1
  fi

  printf '%s\n' "$digest"
}

package_archive_digest() {
  asset="$1"
  manifest_path="$2"

  digest="$(awk -v asset="$asset" '
    $2 == asset && $1 ~ /^[0-9a-fA-F]{64}$/ {
      print tolower($1)
      found = 1
      exit
    }
    END {
      if (!found) {
        exit 1
      }
    }
  ' "$manifest_path" 2>/dev/null || true)"

  if [ -z "$digest" ]; then
    echo "Could not find SHA-256 digest for $asset in $(basename "$manifest_path")." >&2
    exit 1
  fi

  printf '%s\n' "$digest"
}

file_sha256() {
  path="$1"

  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$path" | awk '{print $1}'
    return
  fi

  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$path" | awk '{print $1}'
    return
  fi

  if command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "$path" | sed 's/^.*= //'
    return
  fi

  echo "sha256sum, shasum, or openssl is required to verify the $product_name download." >&2
  exit 1
}

verify_archive_digest() {
  archive_path="$1"
  expected_digest="$2"
  actual_digest="$(file_sha256 "$archive_path")"

  if [ "$actual_digest" != "$expected_digest" ]; then
    echo "Downloaded $product_name archive checksum did not match expected digest." >&2
    echo "expected: $expected_digest" >&2
    echo "actual:   $actual_digest" >&2
    exit 1
  fi
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "$1 is required to install $product_name." >&2
    exit 1
  fi
}

resolve_version() {
  normalized_version="$(normalize_version "$RELEASE")"

  if [ "$normalized_version" != "latest" ]; then
    printf '%s\n' "$normalized_version"
    return
  fi

  release_json="$(download_text "https://api.github.com/repos/$release_repo/releases/latest")"
  resolved="$(printf '%s\n' "$release_json" | sed -n 's/.*"tag_name":[[:space:]]*"rust-v\([^"]*\)".*/\1/p' | head -n 1)"

  if [ -z "$resolved" ]; then
    echo "Failed to resolve the latest $product_name release version." >&2
    exit 1
  fi

  printf '%s\n' "$resolved"
}

pick_profile() {
  # Use the same shell-specific split Homebrew documents because there is no
  # universal startup file across macOS/Linux login and interactive shells.
  case "$os:${SHELL:-}" in
    darwin:*/zsh)
      printf '%s\n' "$HOME/.zprofile"
      ;;
    darwin:*/bash)
      printf '%s\n' "$HOME/.bash_profile"
      ;;
    linux:*/zsh)
      printf '%s\n' "$HOME/.zshrc"
      ;;
    linux:*/bash)
      printf '%s\n' "$HOME/.bashrc"
      ;;
    *)
      printf '%s\n' "$HOME/.profile"
      ;;
  esac
}

add_to_path() {
  path_action="already"
  path_profile=""

  case ":$PATH:" in
    *":$BIN_DIR:"*)
      return
      ;;
  esac

  profile="$(pick_profile)"
  path_profile="$profile"
  begin_marker="# >>> $product_name installer >>>"
  end_marker="# <<< $product_name installer <<<"
  path_line="export PATH=\"$BIN_DIR:\$PATH\""

  if [ -f "$profile" ] && grep -F "$begin_marker" "$profile" >/dev/null 2>&1; then
    if grep -F "$path_line" "$profile" >/dev/null 2>&1; then
      path_action="configured"
      return
    fi

    if grep -F "$end_marker" "$profile" >/dev/null 2>&1; then
      rewrite_path_block "$profile" "$begin_marker" "$end_marker" "$path_line"
      path_action="updated"
      return
    fi
  fi

  append_path_block "$profile" "$begin_marker" "$end_marker" "$path_line"
  path_action="added"
}

append_path_block() {
  profile="$1"
  begin_marker="$2"
  end_marker="$3"
  path_line="$4"

  {
    printf '\n%s\n' "$begin_marker"
    printf '%s\n' "$path_line"
    printf '%s\n' "$end_marker"
  } >>"$profile"
}

rewrite_path_block() {
  profile="$1"
  begin_marker="$2"
  end_marker="$3"
  path_line="$4"
  tmp_profile="$tmp_dir/profile.$$.tmp"

  awk -v begin="$begin_marker" -v end="$end_marker" -v line="$path_line" '
    BEGIN {
      in_block = 0
      replaced = 0
    }
    $0 == begin {
      if (!replaced) {
        print begin
        print line
        print end
        replaced = 1
      }
      in_block = 1
      next
    }
    in_block {
      if ($0 == end) {
        in_block = 0
      }
      next
    }
    {
      print
    }
    END {
      if (in_block != 0) {
        exit 1
      }
    }
  ' "$profile" >"$tmp_profile"
  mv "$tmp_profile" "$profile"
}

mkdir_lock_is_stale() {
  [ -d "$LOCK_DIR" ] || return 1

  pid="$(cat "$LOCK_DIR/pid" 2>/dev/null || true)"
  started_at="$(cat "$LOCK_DIR/started_at" 2>/dev/null || true)"
  now="$(date +%s 2>/dev/null || printf '0')"

  case "$started_at" in
    ''|*[!0-9]*)
      started_at=0
      ;;
  esac

  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    return 1
  fi

  if [ "$started_at" -eq 0 ] || [ "$now" -eq 0 ]; then
    return 0
  fi

  [ $((now - started_at)) -ge "$LOCK_STALE_AFTER_SECS" ]
}

acquire_install_lock() {
  mkdir -p "$STANDALONE_ROOT"

  if [ "$os" = "darwin" ] && command -v lockf >/dev/null 2>&1; then
    : >>"$LOCK_FILE"
    exec 9<>"$LOCK_FILE"
    lockf 9
    lock_kind="lockf"
    return
  fi

  if command -v flock >/dev/null 2>&1; then
    exec 9>"$LOCK_FILE"
    flock 9
    lock_kind="flock"
    return
  fi

  while ! mkdir "$LOCK_DIR" 2>/dev/null; do
    if mkdir_lock_is_stale; then
      warn "Removing stale installer lock at $LOCK_DIR"
      rm -rf "$LOCK_DIR"
      continue
    fi
    sleep 1
  done

  printf '%s\n' "$$" >"$LOCK_DIR/pid"
  date +%s >"$LOCK_DIR/started_at" 2>/dev/null || true
  lock_kind="mkdir"
}

release_install_lock() {
  if [ "$lock_kind" = "mkdir" ]; then
    rm -rf "$LOCK_DIR" 2>/dev/null || true
  elif [ "$lock_kind" = "flock" ] || [ "$lock_kind" = "lockf" ]; then
    exec 9>&- 2>/dev/null || true
  fi
  lock_kind=""
}

cleanup_stale_install_artifacts() {
  mkdir -p "$RELEASES_DIR" "$STANDALONE_ROOT"

  find "$RELEASES_DIR" -mindepth 1 -maxdepth 1 -name '.staging.*' -exec rm -rf {} +
  find "$STANDALONE_ROOT" -mindepth 1 -maxdepth 1 -name '.current.*' -exec rm -f {} +

  if [ -d "$BIN_DIR" ]; then
    find "$BIN_DIR" -mindepth 1 -maxdepth 1 -name ".$command_name.*" -exec rm -f {} +
  fi
}

replace_path_with_symlink() {
  link_path="$1"
  link_target="$2"
  tmp_link="$3"

  rm -f "$tmp_link"
  ln -s "$link_target" "$tmp_link"

  if mv -Tf "$tmp_link" "$link_path" 2>/dev/null; then
    return
  fi

  if mv -hf "$tmp_link" "$link_path" 2>/dev/null; then
    return
  fi

  rm -f "$link_path"
  mv -f "$tmp_link" "$link_path"
}

version_from_binary() {
  codex_path="$1"

  if [ ! -x "$codex_path" ]; then
    return 1
  fi

  "$codex_path" --version 2>/dev/null | sed -n 's/.* \([0-9][0-9A-Za-z.+-]*\)$/\1/p' | head -n 1
}

current_installed_version() {
  version="$(version_from_binary "$CURRENT_LINK/bin/$command_name" || true)"
  if [ -n "$version" ]; then
    printf '%s\n' "$version"
    return 0
  fi

  version="$(version_from_binary "$CURRENT_LINK/$command_name" || true)"
  if [ -n "$version" ]; then
    printf '%s\n' "$version"
    return 0
  fi

  return 0
}

resolve_existing_codex() {
  command -v "$command_name" 2>/dev/null || true
}

classify_existing_codex() {
  existing_path="$1"

  if [ -z "$existing_path" ] || [ "$existing_path" = "$BIN_PATH" ]; then
    return 1
  fi

  case "$existing_path" in
    /opt/homebrew/* | /usr/local/*)
      if [ "$os" = "darwin" ]; then
        printf 'brew\n'
        return 0
      fi
      ;;
  esac

  if [ -f "$existing_path" ] && grep -F "#!/usr/bin/env node" "$existing_path" >/dev/null 2>&1; then
    case "$existing_path" in
      *".bun"*)
        printf 'bun\n'
        ;;
      *)
        printf 'npm\n'
        ;;
    esac
    return 0
  fi

  return 1
}

prompt_yes_no() {
  prompt="$1"

  if ( : </dev/tty ) 2>/dev/null; then
    printf '%s [y/N] ' "$prompt" >/dev/tty
    if ! IFS= read -r answer </dev/tty; then
      return 1
    fi
  elif [ -t 0 ]; then
    printf '%s [y/N] ' "$prompt"
    if ! IFS= read -r answer; then
      return 1
    fi
  else
    return 1
  fi

  case "$answer" in
    y | Y | yes | YES)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

print_launch_instructions() {
  case "$path_action" in
    added)
      step "Current terminal: export PATH=\"$BIN_DIR:\$PATH\" && $command_name"
      step "Future terminals: open a new terminal and run: $command_name"
      step "PATH was added to $path_profile"
      ;;
    updated)
      step "Current terminal: export PATH=\"$BIN_DIR:\$PATH\" && $command_name"
      step "Future terminals: open a new terminal and run: $command_name"
      step "PATH was updated in $path_profile"
      ;;
    configured)
      step "Current terminal: export PATH=\"$BIN_DIR:\$PATH\" && $command_name"
      step "Future terminals: open a new terminal and run: $command_name"
      step "PATH is already configured in $path_profile"
      ;;
    *)
      step "Current terminal: $command_name"
      step "Future terminals: open a new terminal and run: $command_name"
      ;;
  esac
}

maybe_launch_codex_now() {
  if prompt_yes_no "Start $product_name now?"; then
    step "Launching $product_name"
    "$BIN_PATH"
  fi
}

detect_conflicting_install() {
  existing_path="$(resolve_existing_codex)"
  manager="$(classify_existing_codex "$existing_path" || true)"

  if [ -z "$manager" ]; then
    return
  fi

  conflict_manager="$manager"
  conflict_path="$existing_path"
  step "Detected existing $manager-managed $product_name at $existing_path"
  warn "Multiple managed $product_name installs can be ambiguous because PATH order decides which one runs."
}

handle_conflicting_install() {
  if [ -z "$conflict_manager" ]; then
    return
  fi

  case "$conflict_manager" in
    brew)
      uninstall_cmd="brew uninstall --cask $command_name"
      ;;
    bun)
      uninstall_cmd="bun remove -g $npm_package_name"
      ;;
    *)
      uninstall_cmd="npm uninstall -g $npm_package_name"
      ;;
  esac

  if prompt_yes_no "Uninstall the existing $conflict_manager-managed $product_name now?"; then
    step "Running: $uninstall_cmd"
    if ! sh -c "$uninstall_cmd"; then
      warn "Failed to uninstall the existing $conflict_manager-managed $product_name. Continuing with the standalone install."
    fi
  else
    warn "Leaving the existing $conflict_manager-managed $product_name installed. PATH order will determine which $command_name runs."
  fi
}

install_package_release() {
  release_dir="$1"
  archive_path="$2"
  stage_release="$RELEASES_DIR/.staging.$(basename "$release_dir").$$"

  mkdir -p "$RELEASES_DIR"
  rm -rf "$stage_release"
  mkdir -p "$stage_release"
  tar -xzf "$archive_path" -C "$stage_release"
  chmod 0755 "$stage_release/bin/$command_name" "$stage_release/codex-path/rg"
  if [ "$command_name" = "claudex" ] && [ -f "$stage_release/bin/codex" ]; then
    chmod 0755 "$stage_release/bin/codex"
  fi
  if [ -f "$stage_release/codex-resources/bwrap" ]; then
    chmod 0755 "$stage_release/codex-resources/bwrap"
  fi
  ln -sf "bin/$command_name" "$stage_release/$command_name"

  if [ -e "$release_dir" ] || [ -L "$release_dir" ]; then
    rm -rf "$release_dir"
  fi
  mv "$stage_release" "$release_dir"
}

install_legacy_platform_npm_release() {
  release_dir="$1"
  archive_path="$2"
  target="$3"
  stage_release="$RELEASES_DIR/.staging.$(basename "$release_dir").$$"
  extract_dir="$tmp_dir/extract"
  vendor_root="$extract_dir/package/vendor/$target"

  mkdir -p "$RELEASES_DIR"
  rm -rf "$stage_release" "$extract_dir"
  mkdir -p "$stage_release/codex-resources" "$extract_dir"
  tar -xzf "$archive_path" -C "$extract_dir"

  cp "$vendor_root/codex/codex" "$stage_release/codex"
  cp "$vendor_root/path/rg" "$stage_release/codex-resources/rg"
  chmod 0755 "$stage_release/codex" "$stage_release/codex-resources/rg"
  if [ -f "$vendor_root/codex-resources/bwrap" ]; then
    cp "$vendor_root/codex-resources/bwrap" "$stage_release/codex-resources/bwrap"
    chmod 0755 "$stage_release/codex-resources/bwrap"
  fi

  if [ -e "$release_dir" ] || [ -L "$release_dir" ]; then
    rm -rf "$release_dir"
  fi
  mv "$stage_release" "$release_dir"
}

release_dir_is_complete() {
  release_dir="$1"
  expected_version="$2"
  expected_target="$3"
  layout="$4"

  [ -d "$release_dir" ] &&
    [ "$(basename "$release_dir")" = "$expected_version-$expected_target" ] ||
    return 1

  case "$layout" in
    package)
      [ -f "$release_dir/codex-package.json" ] &&
        [ -x "$release_dir/bin/$command_name" ] &&
        [ -x "$release_dir/$command_name" ] &&
        [ -x "$release_dir/codex-path/rg" ] ||
        return 1
      if [ "$command_name" = "claudex" ] && [ ! -x "$release_dir/bin/codex" ]; then
        return 1
      fi
      ;;
    legacy-platform-npm)
      [ -x "$release_dir/codex" ] &&
        [ -x "$release_dir/codex-resources/rg" ] ||
        return 1
      ;;
    *)
      return 1
      ;;
  esac

  case "$layout:$expected_target" in
    package:*linux* | legacy-platform-npm:*linux*) [ -x "$release_dir/codex-resources/bwrap" ] ;;
    *) true ;;
  esac
}

update_current_link() {
  release_dir="$1"
  tmp_link="$STANDALONE_ROOT/.current.$$"

  replace_path_with_symlink "$CURRENT_LINK" "$release_dir" "$tmp_link"
}

release_codex_relative_path() {
  release_dir="$1"

  if [ -x "$release_dir/bin/$command_name" ]; then
    printf 'bin/%s\n' "$command_name"
  else
    printf '%s\n' "$command_name"
  fi
}

update_visible_command() {
  release_dir="$1"
  mkdir -p "$BIN_DIR"
  tmp_link="$BIN_DIR/.$command_name.$$"
  codex_relative_path="$(release_codex_relative_path "$release_dir")"

  replace_path_with_symlink "$BIN_PATH" "$CURRENT_LINK/$codex_relative_path" "$tmp_link"
}

verify_visible_command() {
  "$BIN_PATH" --version >/dev/null
}

parse_args "$@"
configure_product

require_command mktemp
require_command tar

case "$(uname -s)" in
  Darwin)
    os="darwin"
    ;;
  Linux)
    os="linux"
    ;;
  *)
    echo "install.sh supports macOS and Linux. Use install.ps1 on Windows." >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  x86_64 | amd64)
    arch="x86_64"
    ;;
  arm64 | aarch64)
    arch="aarch64"
    ;;
  *)
    echo "Unsupported architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

if [ "$os" = "darwin" ] && [ "$arch" = "x86_64" ]; then
  if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = "1" ]; then
    arch="aarch64"
  fi
fi

if [ "$os" = "darwin" ]; then
  if [ "$arch" = "aarch64" ]; then
    npm_tag="darwin-arm64"
    vendor_target="aarch64-apple-darwin"
    platform_label="macOS (Apple Silicon)"
  else
    npm_tag="darwin-x64"
    vendor_target="x86_64-apple-darwin"
    platform_label="macOS (Intel)"
  fi
else
  if [ "$arch" = "aarch64" ]; then
    npm_tag="linux-arm64"
    vendor_target="aarch64-unknown-linux-musl"
    platform_label="Linux (ARM64)"
  else
    npm_tag="linux-x64"
    vendor_target="x86_64-unknown-linux-musl"
    platform_label="Linux (x64)"
  fi
fi

resolved_version="$(resolve_version)"
package_asset="$package_asset_prefix-package-$vendor_target.tar.gz"
checksum_asset="codex-package_SHA256SUMS"
if release_asset_exists "$package_asset" "$resolved_version" &&
  release_asset_exists "$checksum_asset" "$resolved_version"; then
  install_layout="package"
  asset="$package_asset"
elif [ -n "$legacy_platform_npm_prefix" ] &&
  release_asset_exists "$legacy_platform_npm_prefix-npm-$npm_tag-$resolved_version.tgz" "$resolved_version"; then
  install_layout="legacy-platform-npm"
  asset="$legacy_platform_npm_prefix-npm-$npm_tag-$resolved_version.tgz"
else
  echo "Could not find $product_name package release assets for $product_name $resolved_version." >&2
  exit 1
fi
download_url="$(release_url_for_asset "$asset" "$resolved_version")"
checksum_url="$(release_url_for_asset "$checksum_asset" "$resolved_version")"
release_name="$resolved_version-$vendor_target"
release_dir="$RELEASES_DIR/$release_name"
current_version="$(current_installed_version)"

if [ -n "$current_version" ] && [ "$current_version" != "$resolved_version" ]; then
  step "Updating $product_name CLI from $current_version to $resolved_version"
elif [ -n "$current_version" ]; then
  step "Updating $product_name CLI"
else
  step "Installing $product_name CLI"
fi
step "Detected platform: $platform_label"
step "Resolved version: $resolved_version"

detect_conflicting_install

tmp_dir="$(mktemp -d)"
cleanup() {
  release_install_lock
  if [ -n "$tmp_dir" ]; then
    rm -rf "$tmp_dir"
  fi
}
trap cleanup EXIT INT TERM

acquire_install_lock
cleanup_stale_install_artifacts

if ! release_dir_is_complete "$release_dir" "$resolved_version" "$vendor_target" "$install_layout"; then
  if [ -e "$release_dir" ] || [ -L "$release_dir" ]; then
    warn "Found incomplete existing release at $release_dir; reinstalling."
  fi

  archive_path="$tmp_dir/$asset"
  checksum_path="$tmp_dir/$checksum_asset"

  step "Downloading $product_name CLI"
  if [ "$install_layout" = "package" ]; then
    checksum_digest="$(release_asset_digest "$checksum_asset" "$resolved_version")"
    download_file "$checksum_url" "$checksum_path"
    verify_archive_digest "$checksum_path" "$checksum_digest"
    expected_digest="$(package_archive_digest "$asset" "$checksum_path")"
  else
    expected_digest="$(release_asset_digest "$asset" "$resolved_version")"
  fi
  download_file "$download_url" "$archive_path"
  verify_archive_digest "$archive_path" "$expected_digest"

  step "Installing standalone package to $release_dir"
  if [ "$install_layout" = "package" ]; then
    install_package_release "$release_dir" "$archive_path"
  else
    install_legacy_platform_npm_release "$release_dir" "$archive_path" "$vendor_target"
  fi
fi
update_current_link "$release_dir"
update_visible_command "$release_dir"
add_to_path
verify_visible_command
release_install_lock
handle_conflicting_install

case "$path_action" in
  added)
    print_launch_instructions
    ;;
  updated)
    print_launch_instructions
    ;;
  configured)
    print_launch_instructions
    ;;
  *)
    step "$BIN_DIR is already on PATH"
    print_launch_instructions
    ;;
esac

printf '%s CLI %s installed successfully.\n' "$product_name" "$resolved_version"
maybe_launch_codex_now
