#!/usr/bin/env sh
set -eu

REPOSITORY="hmerritt/explorer"
APP_DIR_NAME="explorer.app"
APP_ID="com.hmerritt.explorer"
INSTALLER_MARKER="# Installed by Explorer install.sh"

stage=""
app_path=""
command_path=""
desktop_file_path=""
applications_dir=""
backup_app=""
backup_desktop=""
desktop_temp=""
command_temp=""
app_backup_made=0
new_app_installed=0
desktop_backup_made=0
desktop_changed=0
command_created=0
committed=0

usage() {
    cat <<'EOF'
Install or remove Explorer for the current Linux user.

Usage:
  install.sh
  install.sh --uninstall
  install.sh --help

Environment:
  EXPLORER_VERSION      Release to install (default: latest), for example 0.17.0.
  EXPLORER_BUNDLE_PATH  Install an existing Explorer Linux tarball instead of
                        resolving and downloading a GitHub release.
  XDG_DATA_HOME         Base directory for the desktop entry.
  XDG_CONFIG_HOME       Base directory removed by --uninstall.

Examples:
  curl -fsSL https://raw.githubusercontent.com/hmerritt/explorer/master/install.sh | sh
  curl -fsSL https://raw.githubusercontent.com/hmerritt/explorer/master/install.sh | EXPLORER_VERSION=0.17.0 sh
  curl -fsSL https://raw.githubusercontent.com/hmerritt/explorer/master/install.sh | sh -s -- --uninstall

The installer places Explorer in ~/.local/explorer.app and links the command at
~/.local/bin/explorer. Uninstall also removes Explorer's settings and caches.
EOF
}

die() {
    echo "Error: $*" >&2
    exit 1
}

path_exists() {
    [ -e "$1" ] || [ -L "$1" ]
}

strip_trailing_slashes() {
    path="$1"
    while [ "$path" != "/" ] && [ "${path%/}" != "$path" ]; do
        path="${path%/}"
    done
    printf '%s\n' "$path"
}

configure_paths() {
    [ -n "${HOME:-}" ] || die "HOME is not set."

    home="$(strip_trailing_slashes "$HOME")"
    [ "$home" != "/" ] || die "Refusing to install or uninstall with HOME set to '/'."

    data_home="$(strip_trailing_slashes "${XDG_DATA_HOME:-$home/.local/share}")"
    config_home="$(strip_trailing_slashes "${XDG_CONFIG_HOME:-$home/.config}")"

    case "$home$data_home$config_home" in
        *'"'* | *'
'*)
            die "HOME and XDG paths must not contain quotes or newlines."
            ;;
    esac

    install_root="$home/.local"
    app_path="$install_root/$APP_DIR_NAME"
    bin_dir="$install_root/bin"
    command_path="$bin_dir/explorer"
    applications_dir="$data_home/applications"
    desktop_file_path="$applications_dir/$APP_ID.desktop"
    config_path="$config_home/explorer"
}

require_linux() {
    platform="$(uname -s)"
    [ "$platform" = "Linux" ] || die "Unsupported platform $platform."
}

resolve_architecture() {
    arch="$(uname -m)"
    case "$arch" in
        x86_64 | amd64)
            asset_arch="amd64"
            ;;
        aarch64 | arm64)
            asset_arch="arm64"
            ;;
        *)
            die "Unsupported architecture $arch."
            ;;
    esac
}

select_downloader() {
    if command -v curl >/dev/null 2>&1; then
        downloader="curl"
    elif command -v wget >/dev/null 2>&1; then
        downloader="wget"
    else
        die "Could not find 'curl' or 'wget' in PATH."
    fi
}

download_file() {
    url="$1"
    output="$2"

    case "$downloader" in
        curl)
            command curl -fsSL --retry 3 --connect-timeout 15 -o "$output" "$url"
            ;;
        wget)
            command wget -q --tries=3 -O "$output" "$url"
            ;;
    esac
}

validate_version() {
    version="$1"
    if ! printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
        die "Invalid Explorer version '$version'. Expected a numeric version such as 0.17.0."
    fi
}

resolve_release_version() {
    requested_version="${EXPLORER_VERSION:-latest}"

    if [ "$requested_version" != "latest" ]; then
        validate_version "$requested_version"
        printf '%s\n' "$requested_version"
        return
    fi

    metadata="$stage/latest-release.json"
    api_url="https://api.github.com/repos/$REPOSITORY/releases/latest"

    if ! download_file "$api_url" "$metadata"; then
        die "Could not resolve the latest Explorer release from GitHub. Retry later or set EXPLORER_VERSION to an exact version such as 0.17.0."
    fi

    version="$(sed -n 's/^[[:space:]]*"tag_name":[[:space:]]*"\([^"]*\)".*$/\1/p' "$metadata" | sed -n '1p')"
    if [ -z "$version" ]; then
        die "GitHub did not return a latest Explorer release. Set EXPLORER_VERSION to an exact version such as 0.17.0."
    fi

    validate_version "$version"
    printf '%s\n' "$version"
}

release_url() {
    version="$1"
    printf 'https://github.com/%s/releases/download/%s/explorer-%s-linux-%s.tar.gz\n' \
        "$REPOSITORY" "$version" "$version" "$asset_arch"
}

command_is_owned() {
    [ -L "$command_path" ] && [ "$(readlink "$command_path")" = "$app_path/bin/explorer" ]
}

desktop_is_owned() {
    [ -f "$desktop_file_path" ] &&
        [ ! -L "$desktop_file_path" ] &&
        {
            grep -Fqx "$INSTALLER_MARKER" "$desktop_file_path" 2>/dev/null ||
                grep -Fq "$app_path/bin/explorer" "$desktop_file_path" 2>/dev/null
        }
}

refresh_desktop_database() {
    if command -v update-desktop-database >/dev/null 2>&1 && [ -d "$applications_dir" ]; then
        update-desktop-database "$applications_dir" >/dev/null 2>&1 || true
    fi
}

validate_removal_path() {
    path="$1"
    expected_name="$2"

    [ -n "$path" ] || die "Refusing to remove an empty path."
    [ "$path" != "/" ] || die "Refusing to remove '/'."
    [ "${path##*/}" = "$expected_name" ] || die "Refusing to remove unexpected path '$path'."
}

uninstall() {
    validate_removal_path "$app_path" "$APP_DIR_NAME"
    validate_removal_path "$config_path" "explorer"

    removed=0

    if path_exists "$command_path"; then
        if command_is_owned; then
            rm -f "$command_path"
            removed=1
        else
            echo "Keeping unrelated command at $command_path" >&2
        fi
    fi

    if path_exists "$desktop_file_path"; then
        if desktop_is_owned; then
            rm -f "$desktop_file_path"
            removed=1
        else
            echo "Keeping unrelated desktop entry at $desktop_file_path" >&2
        fi
    fi

    if path_exists "$app_path"; then
        rm -rf "$app_path"
        removed=1
    fi

    if path_exists "$config_path"; then
        rm -rf "$config_path"
        removed=1
    fi

    refresh_desktop_database

    if [ "$removed" -eq 1 ]; then
        echo "Explorer has been uninstalled, including its settings and caches."
    else
        echo "Explorer is not installed for this user."
    fi
}

escape_sed_replacement() {
    printf '%s' "$1" | sed -e 's/[\\&]/\\&/g' -e 's/|/\\|/g'
}

prepare_desktop_entry() {
    source_desktop="$1"
    output_desktop="$2"
    launcher="$app_path/bin/explorer"
    icon="$app_path/share/icons/hicolor/512x512/apps/explorer.png"

    grep -q '^Exec=' "$source_desktop" || die "The Explorer bundle has an invalid desktop entry (missing Exec)."
    grep -q '^Icon=' "$source_desktop" || die "The Explorer bundle has an invalid desktop entry (missing Icon)."

    escaped_exec="$(escape_sed_replacement "\"$launcher\" %F")"
    escaped_icon="$(escape_sed_replacement "$icon")"

    {
        printf '%s\n' "$INSTALLER_MARKER"
        sed \
            -e "s|^Exec=.*|Exec=$escaped_exec|" \
            -e "s|^Icon=.*|Icon=$escaped_icon|" \
            "$source_desktop"
    } > "$output_desktop"
}

validate_bundle() {
    bundle="$1"
    listing="$stage/archive-contents.txt"
    unpack_dir="$stage/unpack"

    if ! tar -tzf "$bundle" > "$listing"; then
        die "The Explorer bundle is not a readable gzip-compressed tar archive."
    fi

    if grep -Eq '(^/|(^|/)\.\.(/|$))' "$listing"; then
        die "The Explorer bundle contains an unsafe path."
    fi

    for required_path in \
        explorer.app/bin/explorer \
        explorer.app/bin/explorer.bin \
        explorer.app/share/applications/com.hmerritt.explorer.desktop \
        explorer.app/share/icons/hicolor/512x512/apps/explorer.png
    do
        grep -Fqx "$required_path" "$listing" || die "The Explorer bundle is missing $required_path."
    done

    mkdir -p "$unpack_dir"
    tar -xzf "$bundle" -C "$unpack_dir"

    unpacked_app="$unpack_dir/$APP_DIR_NAME"
    [ -x "$unpacked_app/bin/explorer" ] || die "The Explorer launcher is missing or is not executable."
    [ -x "$unpacked_app/bin/explorer.bin" ] || die "The Explorer binary is missing or is not executable."
    [ -f "$unpacked_app/share/applications/$APP_ID.desktop" ] || die "The Explorer desktop entry is missing."
    [ -f "$unpacked_app/share/icons/hicolor/512x512/apps/explorer.png" ] || die "The Explorer icon is missing."

    prepared_desktop="$stage/$APP_ID.desktop"
    prepare_desktop_entry "$unpacked_app/share/applications/$APP_ID.desktop" "$prepared_desktop"
}

rollback_install() {
    if [ "$desktop_changed" -eq 1 ]; then
        if [ "$desktop_backup_made" -eq 1 ] && [ -f "$backup_desktop" ]; then
            rollback_desktop="$applications_dir/.$APP_ID.desktop.rollback.$$"
            cp -p "$backup_desktop" "$rollback_desktop" && mv -f "$rollback_desktop" "$desktop_file_path"
        else
            rm -f "$desktop_file_path"
        fi
    fi

    if [ "$command_created" -eq 1 ] && command_is_owned; then
        rm -f "$command_path"
    fi

    if [ "$new_app_installed" -eq 1 ] && path_exists "$app_path"; then
        rm -rf "$app_path"
    fi

    if [ "$app_backup_made" -eq 1 ] && path_exists "$backup_app"; then
        mv "$backup_app" "$app_path" || echo "Warning: could not restore the previous Explorer installation at $app_path" >&2
    fi
}

cleanup_install() {
    status=$?
    trap - 0 HUP INT TERM
    set +e

    rm -f "${desktop_temp:-}" "${command_temp:-}" 2>/dev/null || true

    if [ "$committed" -ne 1 ]; then
        rollback_install
    fi

    if [ -n "$stage" ] && [ -d "$stage" ]; then
        rm -rf "$stage"
    fi

    exit "$status"
}

handle_signal() {
    exit 1
}

preflight_install_paths() {
    if path_exists "$command_path" && ! command_is_owned; then
        die "Refusing to overwrite unrelated command at $command_path. Move it or choose a different PATH entry first."
    fi

    if path_exists "$desktop_file_path" && ! desktop_is_owned; then
        die "Refusing to overwrite unrelated desktop entry at $desktop_file_path."
    fi
}

commit_install() {
    unpacked_app="$stage/unpack/$APP_DIR_NAME"
    prepared_desktop="$stage/$APP_ID.desktop"

    mkdir -p "$bin_dir" "$applications_dir"

    if path_exists "$desktop_file_path"; then
        backup_desktop="$stage/previous-$APP_ID.desktop"
        cp -p "$desktop_file_path" "$backup_desktop"
        desktop_backup_made=1
    fi

    if path_exists "$app_path"; then
        backup_app="$stage/previous-$APP_DIR_NAME"
        app_backup_made=1
        mv "$app_path" "$backup_app"
    fi

    new_app_installed=1
    mv "$unpacked_app" "$app_path"

    desktop_temp="$applications_dir/.$APP_ID.desktop.new.$$"
    cp "$prepared_desktop" "$desktop_temp"
    chmod 644 "$desktop_temp"
    desktop_changed=1
    mv -f "$desktop_temp" "$desktop_file_path"
    desktop_temp=""

    if ! path_exists "$command_path"; then
        command_temp="$bin_dir/.explorer.new.$$"
        ln -s "$app_path/bin/explorer" "$command_temp"
        command_created=1
        mv "$command_temp" "$command_path"
        command_temp=""
    fi

    committed=1
}

print_path_guidance() {
    if [ "$(command -v explorer 2>/dev/null || true)" = "$command_path" ]; then
        echo "Explorer has been installed. Run with 'explorer'."
        return
    fi

    echo "Explorer has been installed. To run it from your terminal, add ~/.local/bin to PATH:"

    case "${SHELL:-}" in
        *zsh)
            echo "   echo 'export PATH=\$HOME/.local/bin:\$PATH' >> ~/.zshrc"
            echo "   source ~/.zshrc"
            ;;
        *fish)
            echo "   fish_add_path -U $home/.local/bin"
            ;;
        *)
            echo "   echo 'export PATH=\$HOME/.local/bin:\$PATH' >> ~/.bashrc"
            echo "   source ~/.bashrc"
            ;;
    esac

    echo "To run Explorer now: '$command_path'"
}

install_explorer() {
    resolve_architecture
    preflight_install_paths

    mkdir -p "$install_root"
    stage="$(mktemp -d "$install_root/.explorer-install.XXXXXX")"
    trap 'cleanup_install' 0
    trap 'handle_signal' HUP INT TERM

    bundle="$stage/explorer-linux-$asset_arch.tar.gz"

    if [ -n "${EXPLORER_BUNDLE_PATH:-}" ]; then
        [ -f "$EXPLORER_BUNDLE_PATH" ] || die "EXPLORER_BUNDLE_PATH does not name a file: $EXPLORER_BUNDLE_PATH"
        echo "Installing Explorer from local bundle $EXPLORER_BUNDLE_PATH"
        cp "$EXPLORER_BUNDLE_PATH" "$bundle"
    else
        select_downloader
        resolved_version="$(resolve_release_version)"
        url="$(release_url "$resolved_version")"
        echo "Downloading Explorer $resolved_version from GitHub"
        if ! download_file "$url" "$bundle"; then
            die "Could not download Explorer $resolved_version for Linux $asset_arch from $url"
        fi
    fi

    validate_bundle "$bundle"
    commit_install
    refresh_desktop_database
    print_path_guidance
}

main() {
    mode="install"

    case "$#" in
        0)
            ;;
        1)
            case "$1" in
                -h | --help)
                    usage
                    exit 0
                    ;;
                --uninstall)
                    mode="uninstall"
                    ;;
                *)
                    usage >&2
                    die "Unknown argument '$1'."
                    ;;
            esac
            ;;
        *)
            usage >&2
            die "Expected no arguments or --uninstall."
            ;;
    esac

    require_linux
    configure_paths

    if [ "$mode" = "uninstall" ]; then
        uninstall
    else
        install_explorer
    fi
}

main "$@"
