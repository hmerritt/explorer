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
      end
    else
      bin.install "explorer"
      chmod 0755, bin/"explorer"

      if system "xattr", "-p", "com.apple.quarantine", bin/"explorer", out: File::NULL, err: File::NULL
        system "xattr", "-d", "com.apple.quarantine", bin/"explorer"
      end
    end
  end

  test do
    assert_predicate bin/"explorer", :exist?

    if OS.linux?
      if File.directory?(libexec/"explorer.app")
        assert_predicate libexec/"explorer.app/bin/explorer", :exist?
        assert_predicate libexec/"explorer.app/bin/explorer.bin", :exist?
        assert_predicate share/"applications/com.hmerritt.explorer.desktop", :exist?
        assert_predicate share/"icons/hicolor/512x512/apps/explorer.png", :exist?
      end
    end
  end
end
