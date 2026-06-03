cask "explorer" do
  version "<REPLACE_ME>"

  on_arm do
    url "https://github.com/hmerritt/explorer/releases/download/#{version}/explorer-#{version}-macos-arm64-apple-silicon.zip"
    sha256 "<REPLACE_ME>"
  end

  on_intel do
    url "https://github.com/hmerritt/explorer/releases/download/#{version}/explorer-#{version}-macos-amd64-intel.zip"
    sha256 "<REPLACE_ME>"
  end

  name "Explorer"
  desc "File Explorer"
  homepage "https://github.com/hmerritt/explorer"

  app "Explorer.app"
  binary "#{appdir}/Explorer.app/Contents/MacOS/explorer", target: "explorer"

  zap trash: "~/Library/Application Support/com.hmerritt.explorer"
end
