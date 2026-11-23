import AppKit
import Sparkle

/// Updates through Sparkle. Aleno checks for a new version every time it launches (and hourly while it stays open).
/// When there is one, Sparkle asks whether to install it, downloads it, verifies it against the EdDSA key in
/// Info.plist (SUPublicEDKey) and relaunches into the new version.
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

    /// Starts the updater and checks for an update right away. Sparkle's own schedule would wait up to the check
    /// interval after the last check, so a user who opens Aleno once a day could miss a release. Call once at launch.
    /// The check runs a moment later so the first window is up before any prompt appears over it.
    func start() {
        guard let updater = controller?.updater else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) { updater.checkForUpdatesInBackground() }
    }

    /// A menu item for "Check for Updates…", or nil when updates are not configured in this build.
    func makeMenuItem() -> NSMenuItem? {
        guard let controller else { return nil }
        let item = NSMenuItem(title: "Check for Updates…", action: #selector(SPUStandardUpdaterController.checkForUpdates(_:)), keyEquivalent: "")
        item.target = controller
        return item
    }
}
