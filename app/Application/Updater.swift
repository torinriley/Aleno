import AppKit
import Sparkle

/// Automatic updates through Sparkle. Updates are downloaded in the background, verified against the EdDSA key in
/// Info.plist (SUPublicEDKey) and installed when Aleno quits, so a running shell is never interrupted.
///
/// The updater only starts when the build carries a public key: debug builds and forks without one stay quiet
/// instead of showing Sparkle's configuration error.
final class Updater {
    static let shared = Updater()

    private let controller: SPUStandardUpdaterController?

    private init() {
        let key = Bundle.main.object(forInfoDictionaryKey: "SUPublicEDKey") as? String ?? ""
        #if DEBUG
        let enabled = false
        #else
        let enabled = !key.isEmpty
        #endif
        controller = enabled ? SPUStandardUpdaterController(startingUpdater: true, updaterDelegate: nil, userDriverDelegate: nil) : nil
    }

    var isAvailable: Bool { controller != nil }

    /// Starts the updater (and with it the scheduled background checks). Call once at launch.
    func start() { _ = controller }

    /// A menu item for "Check for Updates…", or nil when updates are not configured in this build.
    func makeMenuItem() -> NSMenuItem? {
        guard let controller else { return nil }
        let item = NSMenuItem(title: "Check for Updates…", action: #selector(SPUStandardUpdaterController.checkForUpdates(_:)), keyEquivalent: "")
        item.target = controller
        return item
    }
}
