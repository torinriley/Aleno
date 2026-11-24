# Homebrew cask for Aleno. This repository is its own tap:
#
#   brew tap torinriley/aleno https://github.com/torinriley/Aleno
#   brew install --cask aleno
#
# It always installs the newest GitHub release: `releases/latest/download/` redirects to the latest release's
# Aleno.dmg, whose name never changes. Aleno then keeps itself current with Sparkle, hence `auto_updates true`.
cask "aleno" do
  version :latest
  sha256 :no_check

  url "https://github.com/torinriley/Aleno/releases/latest/download/Aleno.dmg"
  name "Aleno"
  desc "Fast, minimal, native terminal emulator"
  homepage "https://github.com/torinriley/Aleno"

  auto_updates true
  depends_on arch: :arm64
  depends_on macos: :sequoia

  app "Aleno.app"

  zap trash: [
    "~/.config/aleno",
    "~/Library/Application Support/Aleno",
    "~/Library/Caches/com.aleno.Aleno",
    "~/Library/HTTPStorages/com.aleno.Aleno",
    "~/Library/Preferences/com.aleno.Aleno.plist",
    "~/Library/Saved Application State/com.aleno.Aleno.savedState",
  ]
end
