import AppKit
import UserNotifications

/// Posts a notification when a long-running command finishes in a pane the user is not looking at.
final class CommandNotifier: NSObject, UNUserNotificationCenterDelegate {
    static let shared = CommandNotifier()
    private var asked = false
    private let launchID = UUID().uuidString
    /// Called when the user clicks a notification: (session, pane).
    var onActivate: ((UInt32, UInt32) -> Void)?

    func install() { UNUserNotificationCenter.current().delegate = self }

    func post(title: String, body: String, subtitle: String, session: UInt32, pane: UInt32) {
        let center = UNUserNotificationCenter.current()
        let send = {
            let c = UNMutableNotificationContent()
            c.title = title; c.subtitle = subtitle; c.body = body
            c.sound = .default
            c.userInfo = ["launch": self.launchID, "session": Int(session), "pane": Int(pane)]
            c.threadIdentifier = "s\(session)"
            center.add(UNNotificationRequest(identifier: UUID().uuidString, content: c, trigger: nil))
        }
        center.getNotificationSettings { st in
            switch st.authorizationStatus {
            case .authorized, .provisional: send()
            case .notDetermined:
                guard !self.asked else { return }
                self.asked = true
                center.requestAuthorization(options: [.alert, .sound]) { ok, _ in if ok { send() } }
            default: break
            }
        }
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent n: UNNotification,
                                withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) { done([.banner, .sound]) }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive r: UNNotificationResponse,
                                withCompletionHandler done: @escaping () -> Void) {
        let u = r.notification.request.content.userInfo
        if u["launch"] as? String == launchID, let s = u["session"] as? Int, let p = u["pane"] as? Int {
            DispatchQueue.main.async { self.onActivate?(UInt32(s), UInt32(p)) }
        }
        done()
    }
}

extension TerminalView {
    /// Reads the commands that finished and notifies for slow ones when this pane is not in front.
    func handleFinishedCommands() {
        guard let s = stream else { return }
        var evs = [AlenoFinished](repeating: AlenoFinished(), count: 16)
        let n = aleno_pane_take_finished(s, &evs, evs.count)
        let limit = settings.terminal.notifyAfterSeconds
        guard limit > 0, n > 0 else { return }
        let inFront = NSApp.isActive && window?.isKeyWindow == true && !isHiddenOrHasHiddenAncestor
            && window?.occlusionState.contains(.visible) == true
        if inFront { return }
        for e in evs.prefix(n) where Int(e.dur_ms) >= limit * 1000 {
            let failed = e.exit_code != Int32.min && e.exit_code != 0
            let took = TerminalView.formatDuration(Int(e.dur_ms))
            let title = failed ? "Command failed (exit \(e.exit_code)) after \(took)" : "Command finished in \(took)"
            var cmd = ""
            if e.cmd_row >= 0, let c = aleno_pane_text(s, UInt32(e.cmd_row), 0, UInt32(e.cmd_row), UInt32.max) {
                cmd = String(cString: c); aleno_string_free(c)
            }
            // Drop the prompt glyphs in front of the command (anything before the first word-ish character).
            let word = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "./~$_-"))
            if let i = cmd.unicodeScalars.firstIndex(where: { word.contains($0) }) { cmd = String(cmd.unicodeScalars[i...]) }
            cmd = String(cmd.trimmingCharacters(in: .whitespaces).prefix(120))
            let dir = (cwd.isEmpty ? "" : (URL(string: cwd)?.path ?? cwd) as String)
            CommandNotifier.shared.post(title: title, body: cmd.isEmpty ? "A command in \(title)" : cmd,
                                        subtitle: (dir as NSString).lastPathComponent, session: sessionId, pane: paneId)
        }
    }
}
