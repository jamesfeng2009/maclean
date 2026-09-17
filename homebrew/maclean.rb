cask "maclean" do
  # version / sha256 由 scripts/update-homebrew.sh 在发布 GitHub Release 后自动回填。
  # 手动修改容易漏，别手写。
  version "0.2.0"

  on_arm do
    url "https://github.com/jamesfeng2009/maclean/releases/download/v#{version}/maclean-#{version}-arm64.dmg"
    sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  end

  on_intel do
    url "https://github.com/jamesfeng2009/maclean/releases/download/v#{version}/maclean-#{version}-x86_64.dmg"
    sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  end

  name "Maclean"
  desc "macOS disk cleaning tool built with Rust — covers dev caches, APFS snapshots, app uninstall"
  homepage "https://github.com/jamesfeng2009/maclean"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: ">= :big_sur"

  app "maclean.app"

  zap trash: [
    "~/Library/Preferences/com.maclean.app.plist",
    "~/Library/Caches/com.maclean.app",
    "~/Library/Application Support/com.maclean.app",
    "~/.maclean",
  ]
end
