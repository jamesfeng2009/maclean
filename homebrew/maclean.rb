cask "maclean" do
  version "0.2.0"
  sha256 arm:   "0000000000000000000000000000000000000000000000000000000000000000",
         intel: "0000000000000000000000000000000000000000000000000000000000000000"

  url "https://github.com/jamesfeng2009/maclean/releases/download/v#{version}/maclean-#{version}-universal.dmg"
  name "Maclean"
  desc "macOS disk cleaning tool built with Rust — covers 40+ dev caches, APFS snapshots, app uninstall"
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
