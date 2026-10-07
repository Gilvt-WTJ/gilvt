# Homebrew cask template. Lives in the tap repo (homebrew-gilvt/Casks/gilvt.rb); the release workflow
# rewrites `version` and `sha256` after each tagged release. The public repo is Gilvt-WTJ/gilvt.
cask "gilvt" do
  version "0.1.0"
  sha256 "REPLACE_WITH_DMG_SHA256"

  url "https://github.com/Gilvt-WTJ/gilvt/releases/download/v#{version}/Gilvt-#{version}.dmg"
  name "Gilvt"
  desc "Terminal for the agent era: tracks Claude Code and Codex sessions"
  homepage "https://github.com/Gilvt-WTJ/gilvt"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: ">= :big_sur"

  app "Gilvt.app"
  # The `gilvt` CLI also runs inside gilvt panes (it is on their PATH); this exposes it to other shells.
  binary "#{appdir}/Gilvt.app/Contents/MacOS/gilvt"

  zap trash: [
    "~/Library/Application Support/gilvt",
    "~/Library/Preferences/com.gilvt.app.plist",
    "~/.config/gilvt",
  ]
end
