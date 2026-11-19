import AppKit

struct RGBA { var r: Float, g: Float, b: Float, a: Float
    init(_ hex: UInt32, a: Float = 1) { r = Float((hex >> 16) & 0xFF) / 255; g = Float((hex >> 8) & 0xFF) / 255; b = Float(hex & 0xFF) / 255; self.a = a }
    var packed: UInt32 {  // RGBA8, premultiplication handled in the shader
        let q = { (f: Float) -> UInt32 in UInt32(max(0, min(255, (f * 255).rounded()))) }
        return q(r) | (q(g) << 8) | (q(b) << 16) | (q(a) << 24)
    }
}

/// Resolved colors for the current appearance.
struct Theme {
    var foreground: UInt32
    var background: UInt32
    var cursor: UInt32
    var selection: UInt32
    var palette: [UInt32]   // 256 entries

    static let darkAnsi: [UInt32] = [0x1d1f21, 0xcc6666, 0xb5bd68, 0xf0c674, 0x81a2be, 0xb294bb, 0x8abeb7, 0xc5c8c6,
                                     0x666666, 0xd54e53, 0xb9ca4a, 0xe7c547, 0x7aa6da, 0xc397d8, 0x70c0b1, 0xeaeaea]
    static let lightAnsi: [UInt32] = [0x000000, 0xc91b00, 0x00a600, 0x999900, 0x0225c7, 0xc930c7, 0x00a6b2, 0xbfbfbf,
                                      0x666666, 0xe50000, 0x00d900, 0xe5e500, 0x0000ff, 0xe500e5, 0x00e5e5, 0x7f7f7f]

    static func make(settings: Settings, dark: Bool) -> Theme {
        let base: [UInt32] = dark ? darkAnsi : lightAnsi
        var pal = (settings.appearance.colors.palette ?? base)
        let levels: [UInt32] = [0, 95, 135, 175, 215, 255]
        for i in 16..<232 {
            let n = i - 16
            pal.append((levels[n / 36] << 16) | (levels[(n / 6) % 6] << 8) | levels[n % 6])
        }
        for i in 0..<24 { let v = UInt32(8 + i * 10); pal.append((v << 16) | (v << 8) | v) }
        let c = settings.appearance.colors
        return Theme(
            foreground: c.foreground ?? (dark ? 0xd8dadc : 0x1d1f21),
            background: c.background ?? (dark ? 0x16181a : 0xfbfbfb),
            cursor: c.cursor ?? (dark ? 0xe8e8e8 : 0x303030),
            selection: c.selection ?? (dark ? 0x3d5a80 : 0xb3d4fc),
            palette: pal)
    }

    /// Resolve an `AlenoCell` color value (see aleno.h) against this theme.
    @inline(__always) func resolve(_ v: UInt32, isFg: Bool) -> UInt32 {
        switch v & 0xFF00_0000 {
        case 0x0100_0000: return palette[Int(v & 0xFF)]
        case 0x0200_0000: return v & 0x00FF_FFFF
        default: return isFg ? foreground : background
        }
    }
}
