import AppKit

/// Translates key events and mouse events into the byte sequences terminal applications expect.
enum KeyEncoder {
    // Terminal mode bits (aleno_terminal::mode)
    static let appCursor: UInt32 = 1 << 0, appKeypad: UInt32 = 1 << 1, bracketedPaste: UInt32 = 1 << 2
    static let mousePress: UInt32 = 1 << 3, mouseDrag: UInt32 = 1 << 4, mouseMotion: UInt32 = 1 << 5, mouseSGR: UInt32 = 1 << 6
    static let focusEvents: UInt32 = 1 << 7, altScreen: UInt32 = 1 << 8, altScroll: UInt32 = 1 << 11

    /// xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4)
    static func modParam(_ f: NSEvent.ModifierFlags) -> Int {
        1 + (f.contains(.shift) ? 1 : 0) + (f.contains(.option) ? 2 : 0) + (f.contains(.control) ? 4 : 0)
    }

    /// Returns the bytes for keys that are *not* plain text input; nil means "treat as text".
    static func special(event: NSEvent, modes: UInt32, optionAsMeta: Bool) -> [UInt8]? {
        let f = event.modifierFlags.intersection([.shift, .control, .option, .command])
        let mp = modParam(f)
        let hasMod = mp > 1
        func csi(_ final: String) -> [UInt8] {
            let s = hasMod ? "\u{1b}[1;\(mp)\(final)" : (modes & appCursor != 0 ? "\u{1b}O\(final)" : "\u{1b}[\(final)")
            return Array(s.utf8)
        }
        func tilde(_ n: Int) -> [UInt8] { Array((hasMod ? "\u{1b}[\(n);\(mp)~" : "\u{1b}[\(n)~").utf8) }
        func fn(_ final: String) -> [UInt8] { Array((hasMod ? "\u{1b}[1;\(mp)\(final)" : "\u{1b}O\(final)").utf8) }
        switch event.keyCode {
        case 126: return csi("A")
        case 125: return csi("B")
        case 124: return csi("C")
        case 123: return csi("D")
        case 115: return csi("H")
        case 119: return csi("F")
        case 116: return tilde(5)
        case 121: return tilde(6)
        case 117: return tilde(3)
        case 114: return tilde(2)
        case 122: return fn("P")
        case 120: return fn("Q")
        case 99: return fn("R")
        case 118: return fn("S")
        case 96: return tilde(15)
        case 97: return tilde(17)
        case 98: return tilde(18)
        case 100: return tilde(19)
        case 101: return tilde(20)
        case 109: return tilde(21)
        case 103: return tilde(23)
        case 111: return tilde(24)
        case 36, 76: return f.contains(.option) && optionAsMeta ? [0x1b, 0x0d] : [0x0d]
        case 48:
            if f.contains(.shift) { return Array("\u{1b}[Z".utf8) }
            return f.contains(.option) && optionAsMeta ? [0x1b, 0x09] : [0x09]
        case 51:
            if f.contains(.control) { return [0x08] }
            return f.contains(.option) && optionAsMeta ? [0x1b, 0x7f] : [0x7f]
        case 53: return [0x1b]
        default: break
        }
        // Control combinations
        if f.contains(.control) && !f.contains(.command), let s = event.charactersIgnoringModifiers, let u = s.unicodeScalars.first, s.unicodeScalars.count == 1 {
            var b: UInt8? = nil
            switch u.value {
            case 0x61...0x7a: b = UInt8(u.value - 0x60)
            case 0x41...0x5a: b = UInt8(u.value - 0x40)
            case 0x20, 0x32, 0x40: b = 0
            case 0x5b, 0x33: b = 0x1b
            case 0x5c, 0x34: b = 0x1c
            case 0x5d, 0x35: b = 0x1d
            case 0x5e, 0x36: b = 0x1e
            case 0x5f, 0x2f, 0x37: b = 0x1f
            case 0x38: b = 0x7f
            default: break
            }
            if let b { return f.contains(.option) && optionAsMeta ? [0x1b, b] : [b] }
        }
        // Option as Meta: ESC-prefix the unmodified character.
        if f.contains(.option) && optionAsMeta && !f.contains(.command) && !f.contains(.control),
           let s = event.charactersIgnoringModifiers, !s.isEmpty {
            return [0x1b] + Array(s.utf8)
        }
        return nil
    }

    /// X10 / SGR mouse report. `button`: 0 left, 1 middle, 2 right, 64 wheel up, 65 wheel down; `motion` adds 32.
    static func mouse(button: Int, col: Int, row: Int, press: Bool, motion: Bool, flags: NSEvent.ModifierFlags, modes: UInt32) -> [UInt8]? {
        var b = button
        if motion { b += 32 }
        if flags.contains(.shift) { b += 4 }
        if flags.contains(.option) { b += 8 }
        if flags.contains(.control) { b += 16 }
        if modes & mouseSGR != 0 {
            return Array("\u{1b}[<\(b);\(col);\(row)\(press ? "M" : "m")".utf8)
        }
        if col > 223 || row > 223 { return nil }
        let code = press ? b : (button >= 64 ? b : (3 + (b & ~3 & 0xFC) + (b & 0x1C)))
        return [0x1b, 0x5b, 0x4d, UInt8(32 + code), UInt8(32 + col), UInt8(32 + row)]
    }

    static func escapePath(_ p: String) -> String {
        var out = ""
        for ch in p {
            if " \\'\"()[]{}<>&;|*?$`!#~^".contains(ch) { out.append("\\") }
            out.append(ch)
        }
        return out
    }
}
