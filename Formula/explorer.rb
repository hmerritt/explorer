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
        chmod 0755, bin/"explorer"
      else
        odie "Expected explorer.app, bin/lib/share bundle root, or explorer in archive; found: #{Dir.children(".").sort.join(", ")}"
      end

      if File.directory?(libexec/"explorer.app")
        bin.write_exec_script libexec/"explorer.app/bin/explorer"

        (share/"applications").install libexec/"explorer.app/share/applications/com.hmerritt.explorer.desktop"
        (share/"icons/hicolor/512x512/apps").install libexec/"explorer.app/share/icons/hicolor/512x512/apps/explorer.png"

        inreplace share/"applications/com.hmerritt.explorer.desktop" do |s|
          s.gsub!(/^Exec=.*/, "Exec=#{bin}/explorer %F")
          s.gsub!(/^Icon=.*/, "Icon=#{share}/icons/hicolor/512x512/apps/explorer.png")
        end

        (bin/"explorer-register-desktop").write <<~SH
          #!/usr/bin/env sh
          set -eu

          desktop_source="#{libexec}/explorer.app/share/applications/com.hmerritt.explorer.desktop"
          icon_path="#{libexec}/explorer.app/share/icons/hicolor/512x512/apps/explorer.png"

          if [ -z "${HOME:-}" ]; then
            echo "Explorer desktop registration skipped because HOME is not set."
            echo "Run explorer-register-desktop from your user session after install."
            exit 0
          fi

          if [ ! -f "$desktop_source" ]; then
            echo "Explorer desktop registration skipped because $desktop_source is missing."
            exit 0
          fi

          data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
          applications_dir="$data_home/applications"
          desktop_file="$applications_dir/com.hmerritt.explorer.desktop"

          mkdir -p "$applications_dir"
          cp "$desktop_source" "$desktop_file"

          sed -i \\
            -e "s|^Exec=.*|Exec=#{bin}/explorer %F|" \\
            -e "s|^Icon=.*|Icon=$icon_path|" \\
            "$desktop_file"

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
      bin.install "explorer"
      chmod 0755, bin/"explorer"

      if system "xattr", "-p", "com.apple.quarantine", bin/"explorer", out: File::NULL, err: File::NULL
        system "xattr", "-d", "com.apple.quarantine", bin/"explorer"
      end
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

    if OS.linux?
      if File.directory?(libexec/"explorer.app")
        assert_predicate libexec/"explorer.app/bin/explorer", :exist?
        assert_predicate libexec/"explorer.app/bin/explorer.bin", :exist?
        assert_predicate bin/"explorer-register-desktop", :exist?
        assert_predicate share/"applications/com.hmerritt.explorer.desktop", :exist?
        assert_predicate share/"icons/hicolor/512x512/apps/explorer.png", :exist?
      end
    end
  end
end
