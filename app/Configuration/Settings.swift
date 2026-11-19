import AppKit

/// Fully validated configuration. Every field has a default, so a missing/empty file is valid.
struct Settings: Equatable {
    struct Font: Equatable {
        var family = "SF Mono"
        var size: Double = 13
        var lineHeight: Double = 1.2
        var characterSpacing: Double = 0
        var ligatures = true
    }
    struct Colors: Equatable {
        var foreground: UInt32?
        var background: UInt32?
        var cursor: UInt32?
        var selection: UInt32?
        var palette: [UInt32]?
    }
    struct Appearance: Equatable {
        var theme = "system"
        var opacity: Double = 1.0
        var blur = true
        var blurStyle = "blur"
        var blurRadius: Double? = nil
        var padding: Double = 12
        var colors = Colors()
    }
    struct Terminal: Equatable {
        var shell = "default"
        var scrollback = 10000
        var cursorStyle = "bar"
        var cursorBlink = true
        var optionAsMeta = true
        /// Notify when a command running at least this many seconds finishes in a pane that is not in front (0 = never).
        var notifyAfterSeconds = 10
    }
    struct Workspace: Equatable {
        var confirmOnClose = false
        /// Ask before quitting Aleno while windows are open.
        var confirmOnQuit = true
        /// When to play the welcome animation: "first-launch", "always" or "never".
        var welcome = "first-launch"
    }

    var font = Font()
    var appearance = Appearance()
    var terminal = Terminal()
    var workspace = Workspace()
    /// action name -> shortcut string such as "cmd+shift+d" (overrides on top of `defaultKeybindings`).
    var keybindings: [String: String] = [:]

    static let defaultKeybindings: [String: String] = [
        "newTab": "cmd+t", "newWindow": "cmd+n", "closePane": "cmd+w", "closeTab": "cmd+shift+w", "reopenClosedTab": "cmd+shift+t",
        "nextTab": "cmd+shift+]", "previousTab": "cmd+shift+[",
        "splitRight": "cmd+d", "splitDown": "cmd+shift+d",
        "focusLeft": "cmd+alt+left", "focusRight": "cmd+alt+right", "focusUp": "cmd+alt+up", "focusDown": "cmd+alt+down",
        "resizeLeft": "cmd+ctrl+left", "resizeRight": "cmd+ctrl+right", "resizeUp": "cmd+ctrl+up", "resizeDown": "cmd+ctrl+down",
        "find": "cmd+f", "findNext": "cmd+g", "findPrevious": "cmd+shift+g",
        "copy": "cmd+c", "paste": "cmd+v", "selectAll": "cmd+a", "clearScrollback": "cmd+k",
        "scrollPageUp": "shift+pageup", "scrollPageDown": "shift+pagedown", "scrollToTop": "cmd+home", "scrollToBottom": "cmd+end",
        "copyLastOutput": "cmd+alt+c", "selectLastOutput": "cmd+alt+a",
        "increaseFontSize": "cmd+=", "decreaseFontSize": "cmd+-", "resetFontSize": "cmd+0",
        "openSettings": "cmd+,", "toggleFullScreen": "cmd+ctrl+f",
    ]
    static let knownActions = Set(defaultKeybindings.keys)

    func shortcut(for action: String) -> String? {
        if let s = keybindings[action] { return s.isEmpty ? nil : s }
        return Settings.defaultKeybindings[action]
    }

    // MARK: Parsing & validation

    struct Diagnostics {
        var errors: [String] = []
        var warnings: [String] = []
    }

    static func parse(data: Data) -> (Settings?, Diagnostics) {
        var diag = Diagnostics()
        if data.allSatisfy({ $0 == 0x20 || $0 == 0x0A || $0 == 0x0D || $0 == 0x09 }) { return (Settings(), diag) }
        let obj: Any
        do { obj = try JSONSerialization.jsonObject(with: data, options: []) } catch {
            diag.errors.append("invalid JSON: \(Settings.describeJSONError(error, data: data))")
            return (nil, diag)
        }
        guard let root = obj as? [String: Any] else {
            diag.errors.append("top level must be a JSON object")
            return (nil, diag)
        }
        var s = Settings()
        let v = Validator(diag: diag)
        v.object(root, path: "", known: ["$schema", "font", "appearance", "terminal", "workspace", "keybindings"]) { key, val in
            switch key {
            case "font": v.object(val, path: "font", known: ["family", "size", "lineHeight", "characterSpacing", "ligatures"]) { k, x in
                switch k {
                case "family": if let t = v.string(x, "font.family", nonEmpty: true) { s.font.family = t }
                case "size": if let n = v.number(x, "font.size", 6...72) { s.font.size = n }
                case "lineHeight": if let n = v.number(x, "font.lineHeight", 0.8...3) { s.font.lineHeight = n }
                case "characterSpacing": if let n = v.number(x, "font.characterSpacing", -5...20) { s.font.characterSpacing = n }
                case "ligatures": if let b = v.bool(x, "font.ligatures") { s.font.ligatures = b }
                default: break
                }
            }
            case "appearance": v.object(val, path: "appearance", known: ["theme", "opacity", "padding", "blur", "blurStyle", "blurRadius", "colors"]) { k, x in
                switch k {
                case "theme": if let t = v.enumString(x, "appearance.theme", ["system", "light", "dark"]) { s.appearance.theme = t }
                case "opacity": if let n = v.number(x, "appearance.opacity", 0...1) { s.appearance.opacity = n }
                case "blur": if let b = v.bool(x, "appearance.blur") { s.appearance.blur = b }
                case "blurStyle": if let t = v.enumString(x, "appearance.blurStyle", ["blur", "vibrancy", "glass"]) { s.appearance.blurStyle = t }
                case "blurRadius": if let n = v.number(x, "appearance.blurRadius", 0...100) { s.appearance.blurRadius = n }
                case "padding": if let n = v.number(x, "appearance.padding", 0...100) { s.appearance.padding = n }
                case "colors": v.object(x, path: "appearance.colors", known: ["foreground", "background", "cursor", "selection", "palette"]) { ck, cv in
                    switch ck {
                    case "foreground": s.appearance.colors.foreground = v.color(cv, "appearance.colors.foreground")
                    case "background": s.appearance.colors.background = v.color(cv, "appearance.colors.background")
                    case "cursor": s.appearance.colors.cursor = v.color(cv, "appearance.colors.cursor")
                    case "selection": s.appearance.colors.selection = v.color(cv, "appearance.colors.selection")
                    case "palette":
                        if let arr = cv as? [Any], arr.count == 16 {
                            let cols = arr.enumerated().compactMap { v.color($0.element, "appearance.colors.palette[\($0.offset)]") }
                            if cols.count == 16 { s.appearance.colors.palette = cols }
                        } else { v.fail("appearance.colors.palette must be an array of exactly 16 colors") }
                    default: break
                    }
                }
                default: break
                }
            }
            case "terminal": v.object(val, path: "terminal", known: ["shell", "scrollback", "cursorStyle", "cursorBlink", "optionAsMeta", "notifyAfterSeconds"]) { k, x in
                switch k {
                case "shell":
                    if let t = v.string(x, "terminal.shell", nonEmpty: true) {
                        if t != "default" && !t.hasPrefix("/") && t.contains("/") { v.fail("terminal.shell must be \"default\", an absolute path, or a bare program name") }
                        else if t.contains(where: { $0.isNewline || $0 == "\0" }) { v.fail("terminal.shell contains invalid characters") }
                        else { s.terminal.shell = t }
                    }
                case "scrollback": if let n = v.number(x, "terminal.scrollback", 0...1_000_000, integer: true) { s.terminal.scrollback = Int(n) }
                case "cursorStyle": if let t = v.enumString(x, "terminal.cursorStyle", ["block", "bar", "underline"]) { s.terminal.cursorStyle = t }
                case "cursorBlink": if let b = v.bool(x, "terminal.cursorBlink") { s.terminal.cursorBlink = b }
                case "notifyAfterSeconds": if let n = v.number(x, "terminal.notifyAfterSeconds", 0...86400, integer: true) { s.terminal.notifyAfterSeconds = Int(n) }
                case "optionAsMeta": if let b = v.bool(x, "terminal.optionAsMeta") { s.terminal.optionAsMeta = b }
                default: break
                }
            }
            case "workspace": v.object(val, path: "workspace", known: ["restoreSessions", "confirmOnClose", "confirmOnQuit", "welcome"]) { k, x in
                switch k {
                case "restoreSessions": break // Accept legacy settings without restoring sessions.
                case "confirmOnClose": if let b = v.bool(x, "workspace.confirmOnClose") { s.workspace.confirmOnClose = b }
                case "confirmOnQuit": if let b = v.bool(x, "workspace.confirmOnQuit") { s.workspace.confirmOnQuit = b }
                case "welcome": if let t = v.enumString(x, "workspace.welcome", ["first-launch", "always", "never"]) { s.workspace.welcome = t }
                default: break
                }
            }
            case "keybindings":
                v.object(val, path: "keybindings", known: Settings.knownActions.union(["sessionPicker", "renameSession", "detachSession"])) { k, x in
                    guard Settings.knownActions.contains(k) else { return }
                    if let str = v.string(x, "keybindings.\(k)", nonEmpty: false) {
                        if str.isEmpty || KeyChord.parse(str) != nil { s.keybindings[k] = str }
                        else { v.fail("keybindings.\(k): cannot parse shortcut \"\(str)\"") }
                    }
                }
            default: break
            }
        }
        diag = v.diag
        if !diag.errors.isEmpty { return (nil, diag) }
        return (s, diag)
    }

    static func describeJSONError(_ e: Error, data: Data) -> String {
        let ns = e as NSError
        var msg = (ns.userInfo[NSDebugDescriptionErrorKey] as? String) ?? ns.localizedDescription
        if let idx = ns.userInfo["NSJSONSerializationErrorIndex"] as? Int, idx <= data.count {
            let prefix = data.prefix(idx)
            let line = prefix.filter { $0 == 0x0A }.count + 1
            let col = idx - (prefix.lastIndex(of: 0x0A).map { $0 + 1 } ?? 0) + 1
            msg += " (line \(line), column \(col))"
        }
        return msg
    }

    // MARK: Default file

    static let defaultFileText = """
    {
      "$schema": "./settings.schema.json",
      "font": {
        "family": "SF Mono",
        "size": 13,
        "lineHeight": 1.2,
        "ligatures": true
      },
      "appearance": {
        "theme": "system",
        "opacity": 1.0,
        "padding": 12
      },
      "terminal": {
        "shell": "default",
        "scrollback": 10000,
        "cursorStyle": "bar",
        "cursorBlink": true
      },
      "workspace": {
        "confirmOnClose": false
      },
      "keybindings": {
        "newTab": "cmd+t",
        "splitRight": "cmd+d",
        "splitDown": "cmd+shift+d"
      }
    }

    """
}

private final class Validator {
    var diag: Settings.Diagnostics
    init(diag: Settings.Diagnostics) { self.diag = diag }

    func fail(_ m: String) { diag.errors.append(m) }

    func object(_ val: Any, path: String, known: Set<String>, _ body: (String, Any) -> Void) {
        guard let d = val as? [String: Any] else {
            fail("\(path.isEmpty ? "settings" : path) must be an object")
            return
        }
        for (k, x) in d.sorted(by: { $0.key < $1.key }) {
            if known.contains(k) { body(k, x) }
            else { diag.warnings.append("unknown key \"\(path.isEmpty ? k : path + "." + k)\"" + Self.suggest(k, known)) }
        }
    }

    static func suggest(_ k: String, _ known: Set<String>) -> String {
        let best = known.map { ($0, editDistance(k.lowercased(), $0.lowercased())) }.min { $0.1 < $1.1 }
        if let b = best, b.1 <= 2 { return " (did you mean \"\(b.0)\"?)" }
        return ""
    }
    static func editDistance(_ a: String, _ b: String) -> Int {
        let a = Array(a), b = Array(b)
        var dp = Array(0...b.count)
        for i in 1...max(a.count, 1) where !a.isEmpty {
            var prev = dp[0]; dp[0] = i
            for j in 1...max(b.count, 1) where !b.isEmpty {
                let t = dp[j]
                dp[j] = min(dp[j] + 1, dp[j - 1] + 1, prev + (a[i - 1] == b[j - 1] ? 0 : 1))
                prev = t
            }
        }
        return dp[b.count]
    }

    func string(_ x: Any, _ path: String, nonEmpty: Bool) -> String? {
        guard let s = x as? String, !(x is NSNumber) else { fail("\(path) must be a string"); return nil }
        if nonEmpty && s.isEmpty { fail("\(path) must not be empty"); return nil }
        return s
    }
    func bool(_ x: Any, _ path: String) -> Bool? {
        if let n = x as? NSNumber, CFGetTypeID(n) == CFBooleanGetTypeID() { return n.boolValue }
        fail("\(path) must be true or false"); return nil
    }
    func number(_ x: Any, _ path: String, _ range: ClosedRange<Double>, integer: Bool = false) -> Double? {
        guard let n = x as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID() else { fail("\(path) must be a number"); return nil }
        let d = n.doubleValue
        if !range.contains(d) || d.isNaN { fail("\(path) must be between \(range.lowerBound) and \(range.upperBound) (got \(d))"); return nil }
        if integer && d != d.rounded() { fail("\(path) must be an integer"); return nil }
        return d
    }
    func enumString(_ x: Any, _ path: String, _ allowed: [String]) -> String? {
        guard let s = x as? String else { fail("\(path) must be one of \(allowed.joined(separator: ", "))"); return nil }
        if !allowed.contains(s) { fail("\(path) must be one of \(allowed.joined(separator: ", ")) (got \"\(s)\")"); return nil }
        return s
    }
    func color(_ x: Any, _ path: String) -> UInt32? {
        guard let s = x as? String, s.hasPrefix("#"), s.count == 7, let v = UInt32(s.dropFirst(), radix: 16) else {
            fail("\(path) must be a color like \"#rrggbb\""); return nil
        }
        return v
    }
}

// MARK: - Key chords

struct KeyChord: Equatable {
    var key: String               // lower-case character or a named key ("left", "pageup", ...)
    var modifiers: NSEvent.ModifierFlags

    static let named: [String: Int] = [
        "left": NSLeftArrowFunctionKey, "right": NSRightArrowFunctionKey, "up": NSUpArrowFunctionKey, "down": NSDownArrowFunctionKey,
        "pageup": NSPageUpFunctionKey, "pagedown": NSPageDownFunctionKey, "home": NSHomeFunctionKey, "end": NSEndFunctionKey,
        "delete": NSDeleteFunctionKey, "tab": 0x09, "enter": 0x0D, "return": 0x0D, "space": 0x20, "escape": 0x1B,
    ]

    static func parse(_ s: String) -> KeyChord? {
        let parts = s.lowercased().split(separator: "+", omittingEmptySubsequences: false).map(String.init)
        guard var key = parts.last, !parts.isEmpty else { return nil }
        var mods: NSEvent.ModifierFlags = []
        var body = Array(parts.dropLast())
        // "cmd++" means cmd and '+'
        if key.isEmpty, body.last == "" { body.removeLast(); key = "+" }
        for p in body {
            switch p {
            case "cmd", "command", "super": mods.insert(.command)
            case "shift": mods.insert(.shift)
            case "alt", "opt", "option": mods.insert(.option)
            case "ctrl", "control": mods.insert(.control)
            default: return nil
            }
        }
        if key.isEmpty { return nil }
        if key.count > 1 && named[key] == nil { return nil }
        return KeyChord(key: key, modifiers: mods)
    }

    /// Menu-item key-equivalent string.
    var keyEquivalent: String {
        if let code = KeyChord.named[key] { return String(UnicodeScalar(UInt32(code))!) }
        return key
    }
}
