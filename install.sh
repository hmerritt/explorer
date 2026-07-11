#!/usr/bin/env sh
set -eu

EXPLORER_VERSION="0.16.0"

main() {
    platform="$(uname -s)"
    arch="$(uname -m)"

    if [ -n "${TMPDIR:-}" ] && [ -d "${TMPDIR}" ]; then
        temp="$(mktemp -d "$TMPDIR/explorer-XXXXXX")"
    else
        temp="$(mktemp -d "/tmp/explorer-XXXXXX")"
    fi
    trap 'rm -rf "$temp"' EXIT INT HUP TERM

    if [ "$platform" != "Linux" ]; then
        echo "Unsupported platform $platform"
        exit 1
    fi

    case "$arch" in
        x86_64 | amd64)
            asset_arch="amd64"
            ;;
        aarch64 | arm64)
            asset_arch="arm64"
            ;;
        *)
            echo "Unsupported architecture $arch"
            exit 1
            ;;
    esac

    if command -v curl >/dev/null 2>&1; then
        download() {
            command curl -fL "$@"
        }
    elif command -v wget >/dev/null 2>&1; then
        download() {
            wget -O- "$@"
        }
    else
        echo "Could not find 'curl' or 'wget' in your path"
        exit 1
    fi

    linux

    if [ "$(command -v explorer 2>/dev/null || true)" = "$HOME/.local/bin/explorer" ]; then
        echo "Explorer has been installed. Run with 'explorer'"
    else
        echo "To run Explorer from your terminal, you must add ~/.local/bin to your PATH"
        echo "Run:"

        case "${SHELL:-}" in
            *zsh)
                echo "   echo 'export PATH=\$HOME/.local/bin:\$PATH' >> ~/.zshrc"
                echo "   source ~/.zshrc"
                ;;
            *fish)
                echo "   fish_add_path -U $HOME/.local/bin"
                ;;
            *)
                echo "   echo 'export PATH=\$HOME/.local/bin:\$PATH' >> ~/.bashrc"
                echo "   source ~/.bashrc"
                ;;
        esac

        echo "To run Explorer now, '$HOME/.local/bin/explorer'"
    fi
}

release_url() {
    printf "https://github.com/hmerritt/explorer/releases/download/%s/explorer-%s-linux-%s.tar.gz" \
        "$EXPLORER_VERSION" "$EXPLORER_VERSION" "$asset_arch"
}

linux() {
    bundle="$temp/explorer-linux-$asset_arch.tar.gz"

    if [ -n "${EXPLORER_BUNDLE_PATH:-}" ]; then
        cp "$EXPLORER_BUNDLE_PATH" "$bundle"
    else
        url="$(release_url)"
        echo "Downloading Explorer $EXPLORER_VERSION from GitHub"
        download "$url" > "$bundle"
    fi

    rm -rf "$HOME/.local/explorer.app"
    mkdir -p "$HOME/.local" "$HOME/.local/bin" "$HOME/.local/share/applications"
    tar -xzf "$bundle" -C "$HOME/.local"

    ln -sf "$HOME/.local/explorer.app/bin/explorer" "$HOME/.local/bin/explorer"

    desktop_file_path="$HOME/.local/share/applications/com.hmerritt.explorer.desktop"
    cp "$HOME/.local/explorer.app/share/applications/com.hmerritt.explorer.desktop" "$desktop_file_path"
    sed -i "s|^Icon=.*|Icon=$HOME/.local/explorer.app/share/icons/hicolor/512x512/apps/explorer.png|g" "$desktop_file_path"
    sed -i "s|^Exec=.*|Exec=$HOME/.local/explorer.app/bin/explorer %F|g" "$desktop_file_path"

    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$HOME/.local/share/applications" >/dev/null 2>&1 || true
    fi
}

main "$@"
