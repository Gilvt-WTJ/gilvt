# Homebrew cask. The published copy lives in the tap repo Gilvt-WTJ/homebrew-gilvt (Casks/gilvt.rb);
# keep the two identical. After each release, set `version` and `sha256` there (HACKING.md, 发一个新版本).
cask "gilvt" do
  version "0.4.1"
  sha256 "f1df3e5f7a514a42acee9fffdfa972a0182bc7edc789a5e376eaa7fe501b9f4d"

  url "https://github.com/Gilvt-WTJ/gilvt/releases/download/v#{version}/Gilvt-#{version}.dmg"
  name "gilvt"
  desc "Native terminal for running and reviewing parallel Claude Code and Codex sessions"
  homepage "https://gilvt.com/"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true
  depends_on macos: :big_sur

  app "Gilvt.app"
  # The `gilvt` CLI also runs inside gilvt panes (it is on their PATH); this exposes it to other shells.
  binary "#{appdir}/Gilvt.app/Contents/MacOS/gilvt"

  zap trash: [
    "~/.config/gilvt",
    "~/Library/Application Support/gilvt",
    "~/Library/Caches/gilvt",
    "~/Library/Caches/com.gilvt.app",
    "~/Library/HTTPStorages/com.gilvt.app",
    "~/Library/Preferences/com.gilvt.app.plist",
    "~/Library/Saved Application State/com.gilvt.app.savedState",
  ]
end
