class Explorer < Formula
  release_version = "<REPLACE_ME>"
  base_url = "https://github.com/hmerritt/explorer/releases/download"

  desc "File Explorer"
  homepage "https://github.com/hmerritt/explorer"
  version release_version

  if OS.mac? && Hardware::CPU.arm?
    url "#{base_url}/#{release_version}/explorer-#{release_version}-macos-arm64-apple-silicon.zip"
    sha256 "<REPLACE_ME>"
  end

  if OS.mac? && Hardware::CPU.intel?
    url "#{base_url}/#{release_version}/explorer-#{release_version}-macos-amd64-intel.zip"
    sha256 "<REPLACE_ME>"
  end

  if OS.linux? && Hardware::CPU.arm?
    url "#{base_url}/#{release_version}/explorer-#{release_version}-linux-arm64.tar.gz"
    sha256 "<REPLACE_ME>"
  end

  if OS.linux? && Hardware::CPU.intel?
    url "#{base_url}/#{release_version}/explorer-#{release_version}-linux-amd64.tar.gz"
    sha256 "<REPLACE_ME>"
  end

  def chmod_executable(path)
    chmod 0755, path
  rescue StandardError => e
    opoo "Could not mark #{path} executable: #{e.message}"
  end

  def install
    if OS.linux?
      if File.directory?("explorer.app")
        libexec.install "explorer.app"
      elsif File.exist?("bin/explorer") && File.exist?("bin/explorer.bin")
        explorer_app = libexec/"explorer.app"
        explorer_app.install "bin"
        explorer_app.install "lib" if File.directory?("lib")
        explorer_app.install "share" if File.directory?("share")
      elsif File.exist?("explorer")
        bin.install "explorer"
        chmod_executable bin/"explorer"
      else
        odie "Expected explorer.app, bin/lib/share bundle root, or explorer in archive; found: #{Dir.children(".").sort.join(", ")}"
      end

      if File.directory?(libexec/"explorer.app")
        bin.write_exec_script libexec/"explorer.app/bin/explorer"

        (share/"icons/hicolor/512x512/apps").install libexec/"explorer.app/share/icons/hicolor/512x512/apps/explorer.png"

        (share/"applications").mkpath
        (share/"applications/com.hmerritt.explorer.desktop").write <<~DESKTOP
          [Desktop Entry]
          Type=Application
          Name=Explorer
          Comment=File Explorer for Windows, macOS, and Linux, built with GPUI.
          Exec=#{opt_bin}/explorer %F
          Icon=#{opt_share}/icons/hicolor/512x512/apps/explorer.png
          Terminal=false
          Categories=Utility;FileManager;
          MimeType=inode/directory;
          StartupNotify=true
        DESKTOP

        (bin/"explorer-register-desktop").write <<~SH
          #!/usr/bin/env sh
          set -eu

          bundle_icon="#{opt_libexec}/explorer.app/share/icons/hicolor/512x512/apps/explorer.png"
          share_icon="#{opt_share}/icons/hicolor/512x512/apps/explorer.png"

          if [ -z "${HOME:-}" ]; then
            echo "Explorer desktop registration skipped because HOME is not set."
            echo "Run explorer-register-desktop from your user session after install."
            exit 0
          fi

          if [ -f "$bundle_icon" ]; then
            icon_path="$bundle_icon"
          elif [ -f "$share_icon" ]; then
            icon_path="$share_icon"
          else
            icon_path="explorer"
          fi

          data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
          applications_dir="$data_home/applications"
          desktop_file="$applications_dir/com.hmerritt.explorer.desktop"

          mkdir -p "$applications_dir"
          cat > "$desktop_file" <<EOF
          [Desktop Entry]
          Type=Application
          Name=Explorer
          Comment=File Explorer for Windows, macOS, and Linux, built with GPUI.
          Exec=#{opt_bin}/explorer %F
          Icon=$icon_path
          Terminal=false
          Categories=Utility;FileManager;
          MimeType=inode/directory;
          StartupNotify=true
          EOF

          if command -v update-desktop-database >/dev/null 2>&1; then
            update-desktop-database "$applications_dir" >/dev/null 2>&1 || true
          fi

          if command -v xdg-desktop-menu >/dev/null 2>&1; then
            xdg-desktop-menu forceupdate --mode user >/dev/null 2>&1 || true
          fi

          echo "Explorer desktop launcher registered at $desktop_file"
        SH
        chmod 0755, bin/"explorer-register-desktop"
      end
    else
      explorer_app = libexec/"Explorer.app"
      if File.directory?("Explorer.app")
        libexec.install "Explorer.app"
      elsif File.exist?("Contents/Info.plist")
        explorer_app.mkpath
        explorer_app.install "Contents"
      else
        odie "Expected Explorer.app or Contents/Info.plist in archive; found: #{Dir.children(".").sort.join(", ")}"
      end

      (bin/"explorer").write <<~SH
        #!/usr/bin/env sh
        set -eu

        exec /usr/bin/open -na "#{opt_libexec}/Explorer.app" --args "$@"
      SH
      chmod_executable bin/"explorer"
    end
  end

  def post_install
    return unless OS.linux?

    register_desktop = bin/"explorer-register-desktop"
    system register_desktop if register_desktop.exist?
  end

  def caveats
    return unless OS.linux?

    <<~EOS
      Explorer registers a launcher entry at:
        ${XDG_DATA_HOME:-$HOME/.local/share}/applications/com.hmerritt.explorer.desktop

      The launcher entry is generated directly and can be refreshed after a
      reinstall, upgrade from an older launcher, or desktop-session change.

      If it does not appear immediately, refresh your desktop shell or run:
        explorer-register-desktop

      If you move Homebrew or change desktop sessions, refresh the launcher
      registration with:
        explorer-register-desktop

      To remove the launcher entry, delete:
        ~/.local/share/applications/com.hmerritt.explorer.desktop
    EOS
  end

  test do
    assert_predicate bin/"explorer", :exist?

    if OS.mac?
      assert_predicate libexec/"Explorer.app", :directory?
      assert_predicate libexec/"Explorer.app/Contents/Info.plist", :exist?
      assert_predicate libexec/"Explorer.app/Contents/MacOS/explorer", :exist?
      assert_predicate libexec/"Explorer.app/Contents/Resources/explorer.icns", :exist?

      launcher = (bin/"explorer").read
      expected_launcher = "/usr/bin/open -na \"#{opt_libexec}/Explorer.app\" --args " + '"$@"'
      assert_match expected_launcher, launcher
    elsif OS.linux?
      if File.directory?(libexec/"explorer.app")
        assert_predicate libexec/"explorer.app/bin/explorer", :exist?
        assert_predicate libexec/"explorer.app/bin/explorer.bin", :exist?
        assert_predicate bin/"explorer-register-desktop", :exist?
        assert_predicate share/"applications/com.hmerritt.explorer.desktop", :exist?
        assert_predicate share/"icons/hicolor/512x512/apps/explorer.png", :exist?

        icon_path = "icons/hicolor/512x512/apps/explorer.png"
        stable_exec = "Exec=#{opt_bin}/explorer %F"
        stable_icon = "Icon=#{opt_share}/#{icon_path}"
        versioned_exec = "Exec=#{bin}/explorer %F"
        versioned_icon = "Icon=#{share}/#{icon_path}"

        desktop_entry = (share/"applications/com.hmerritt.explorer.desktop").read
        assert_match stable_exec, desktop_entry
        assert_match stable_icon, desktop_entry
        refute_match versioned_exec, desktop_entry
        refute_match versioned_icon, desktop_entry

        register_desktop = (bin/"explorer-register-desktop").read
        assert_match stable_exec, register_desktop
        assert_match "bundle_icon=\"#{opt_libexec}/explorer.app/share/#{icon_path}\"", register_desktop
        assert_match "share_icon=\"#{opt_share}/#{icon_path}\"", register_desktop
        refute_match versioned_exec, register_desktop
      end
    end
  end
end
