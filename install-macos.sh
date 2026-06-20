#!/usr/bin/env sh
set -eu

APP_NAME="Explorer"
BUNDLE_ID="com.hmerritt.explorer"
BINARY_NAME="explorer"

main() {
    if [ "$(uname -s)" != "Darwin" ]; then
        echo "install-macos.sh only supports macOS."
        exit 1
    fi

    require_command awk
    require_command codesign
    require_command iconutil
    require_command install
    require_command plutil
    require_command sips

    script_dir="$(CDPATH= cd "$(dirname "$0")" && pwd -P)"
    cd "$script_dir"

    source_binary="$(resolve_repo_path "${EXPLORER_BINARY_PATH:-target/release/$BINARY_NAME}")"
    if [ ! -f "$source_binary" ]; then
        echo "Could not find Explorer release binary at:"
        echo "  $source_binary"
        echo
        echo "Build it first with:"
        echo "  cargo build --release"
        exit 1
    fi

    icon_source="$script_dir/assets/explorer.png"
    if [ ! -f "$icon_source" ]; then
        echo "Could not find app icon source at:"
        echo "  $icon_source"
        exit 1
    fi

    version="${EXPLORER_VERSION:-$(cargo_package_version)}"
    if [ -z "$version" ]; then
        echo "Could not read package version from Cargo.toml."
        echo "Set EXPLORER_VERSION to install anyway."
        exit 1
    fi

    install_root="${EXPLORER_INSTALL_DIR:-}"
    if [ -z "$install_root" ]; then
        if [ -z "${HOME:-}" ]; then
            echo "HOME is not set; set EXPLORER_INSTALL_DIR to choose an install directory."
            exit 1
        fi
        install_root="$HOME/Applications"
    fi

    install_root="$(expand_path "$install_root")"
    while [ "$install_root" != "/" ] && [ "${install_root%/}" != "$install_root" ]; do
        install_root="${install_root%/}"
    done

    case "$install_root" in
        *.app)
            app_path="$install_root"
            ;;
        *)
            app_path="$install_root/$APP_NAME.app"
            ;;
    esac

    app_parent="$(dirname "$app_path")"
    mkdir -p "$app_parent"

    temp="$(mktemp -d "$app_parent/.explorer-install.XXXXXX")"
    backup_app=""
    trap 'cleanup "$temp" "$backup_app"' EXIT INT HUP TERM

    staged_app="$temp/$APP_NAME.app"
    contents_dir="$staged_app/Contents"
    macos_dir="$contents_dir/MacOS"
    resources_dir="$contents_dir/Resources"
    iconset_dir="$temp/explorer.iconset"

    mkdir -p "$macos_dir" "$resources_dir" "$iconset_dir"
    install -m 755 "$source_binary" "$macos_dir/$BINARY_NAME"
    create_icns "$icon_source" "$iconset_dir" "$resources_dir/explorer.icns"
    write_info_plist "$contents_dir/Info.plist" "$version"

    plutil -lint "$contents_dir/Info.plist" >/dev/null
    codesign --force --deep --sign - "$staged_app" >/dev/null
    codesign --verify --deep --strict --verbose=2 "$staged_app"

    if [ -e "$app_path" ] || [ -L "$app_path" ]; then
        backup_app="$temp/$APP_NAME.app.previous"
        mv "$app_path" "$backup_app"
    fi

    if ! mv "$staged_app" "$app_path"; then
        if [ -n "$backup_app" ] && { [ -e "$backup_app" ] || [ -L "$backup_app" ]; }; then
            mv "$backup_app" "$app_path"
        fi
        echo "Failed to install $APP_NAME.app to $app_path"
        exit 1
    fi

    backup_app=""
    remove_quarantine "$app_path"
    register_launch_services "$app_path"

    echo "$APP_NAME $version installed at:"
    echo "  $app_path"
    echo
    echo "Launch it with:"
    echo "  open \"$app_path\""
}

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Required command '$1' was not found in PATH."
        exit 1
    fi
}

resolve_repo_path() {
    case "$1" in
        "~")
            printf '%s\n' "${HOME:?HOME is not set}"
            ;;
        "~/"*)
            printf '%s/%s\n' "${HOME:?HOME is not set}" "${1#~/}"
            ;;
        /*)
            printf '%s\n' "$1"
            ;;
        *)
            printf '%s/%s\n' "$script_dir" "$1"
            ;;
    esac
}

expand_path() {
    case "$1" in
        "~")
            printf '%s\n' "${HOME:?HOME is not set}"
            ;;
        "~/"*)
            printf '%s/%s\n' "${HOME:?HOME is not set}" "${1#~/}"
            ;;
        /*)
            printf '%s\n' "$1"
            ;;
        *)
            printf '%s/%s\n' "$(pwd -P)" "$1"
            ;;
    esac
}

cargo_package_version() {
    awk '
        $0 ~ /^[[:space:]]*\[package\][[:space:]]*$/ {
            in_package = 1
            next
        }
        in_package && $0 ~ /^[[:space:]]*\[/ {
            exit
        }
        in_package && $0 ~ /^[[:space:]]*version[[:space:]]*=/ {
            sub(/^[[:space:]]*version[[:space:]]*=[[:space:]]*"/, "")
            sub(/".*$/, "")
            print
            exit
        }
    ' Cargo.toml
}

create_icns() {
    icon_source="$1"
    iconset_dir="$2"
    icon_path="$3"

    sips -z 16 16 "$icon_source" --out "$iconset_dir/icon_16x16.png" >/dev/null
    sips -z 32 32 "$icon_source" --out "$iconset_dir/icon_16x16@2x.png" >/dev/null
    sips -z 32 32 "$icon_source" --out "$iconset_dir/icon_32x32.png" >/dev/null
    sips -z 64 64 "$icon_source" --out "$iconset_dir/icon_32x32@2x.png" >/dev/null
    sips -z 128 128 "$icon_source" --out "$iconset_dir/icon_128x128.png" >/dev/null
    sips -z 256 256 "$icon_source" --out "$iconset_dir/icon_128x128@2x.png" >/dev/null
    sips -z 256 256 "$icon_source" --out "$iconset_dir/icon_256x256.png" >/dev/null
    sips -z 512 512 "$icon_source" --out "$iconset_dir/icon_256x256@2x.png" >/dev/null
    sips -z 512 512 "$icon_source" --out "$iconset_dir/icon_512x512.png" >/dev/null
    sips -z 1024 1024 "$icon_source" --out "$iconset_dir/icon_512x512@2x.png" >/dev/null
    iconutil -c icns "$iconset_dir" -o "$icon_path"
}

write_info_plist() {
    info_plist="$1"
    version="$2"

    cat > "$info_plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>$APP_NAME</string>
  <key>CFBundleExecutable</key>
  <string>$BINARY_NAME</string>
  <key>CFBundleIconFile</key>
  <string>explorer</string>
  <key>CFBundleIdentifier</key>
  <string>$BUNDLE_ID</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>$APP_NAME</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$version</string>
  <key>CFBundleVersion</key>
  <string>$version</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.utilities</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
EOF
}

remove_quarantine() {
    app_path="$1"

    if command -v xattr >/dev/null 2>&1; then
        xattr -dr com.apple.quarantine "$app_path" 2>/dev/null || true
    fi
}

register_launch_services() {
    app_path="$1"
    lsregister="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

    if [ -x "$lsregister" ]; then
        "$lsregister" -f "$app_path" >/dev/null 2>&1 || true
    fi
}

cleanup() {
    temp="$1"
    backup_app="$2"

    if [ -n "$backup_app" ] && { [ -e "$backup_app" ] || [ -L "$backup_app" ]; }; then
        echo "Install interrupted; restoring previous $APP_NAME.app."
        rm -rf "$app_path"
        mv "$backup_app" "$app_path"
    fi

    rm -rf "$temp"
}

main "$@"
