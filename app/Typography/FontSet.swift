import AppKit
import CoreText

/// Font family resolved at a specific backing scale, with terminal cell metrics in *pixels*.
final class FontSet {
    let scale: CGFloat
    let family: String
    let pointSize: Double
    let regular: CTFont
    let bold: CTFont
    let italic: CTFont
    let boldItalic: CTFont
    let cellWidth: Int
    let cellHeight: Int
    /// Distance from the bottom of the cell to the baseline.
    let baseline: CGFloat
    let underlinePosition: CGFloat   // from top of cell, pixels
    let underlineThickness: CGFloat

    init(family: String, size: Double, lineHeight: Double, spacing: Double, scale: CGFloat) {
        self.scale = scale
        self.family = family
        self.pointSize = size
        let px = CGFloat(size) * scale
        let base = FontSet.makeBase(family: family, size: px)
        let cascade = FontSet.symbolFallbacks
        regular = FontSet.withCascade(base, cascade)
        bold = FontSet.withCascade(FontSet.variant(base, size: px, family: family, bold: true, italic: false), cascade)
        italic = FontSet.withCascade(FontSet.variant(base, size: px, family: family, bold: false, italic: true), cascade)
        boldItalic = FontSet.withCascade(FontSet.variant(base, size: px, family: family, bold: true, italic: true), cascade)

        var glyph = CTFontGetGlyphWithName(base, "M" as CFString)
        var adv = CGSize.zero
        CTFontGetAdvancesForGlyphs(base, .horizontal, &glyph, &adv, 1)
        cellWidth = max(1, Int((adv.width + CGFloat(spacing) * scale).rounded()))
        let ascent = CTFontGetAscent(base), descent = CTFontGetDescent(base), leading = CTFontGetLeading(base)
        let natural = (ascent + descent + leading)
        let h = max(CGFloat(1), (natural * CGFloat(lineHeight)).rounded())
        cellHeight = Int(h)
        let extra = h - (ascent + descent)
        baseline = (descent + extra / 2).rounded()
        let ulPos = CTFontGetUnderlinePosition(base)   // negative: below baseline
        underlineThickness = max(1, CTFontGetUnderlineThickness(base).rounded())
        underlinePosition = max(1, (h - baseline - ulPos).rounded())
    }

    /// Symbol fonts instantiated at this set's pixel size.
    lazy var symbolFonts: [CTFont] = FontSet.symbolFallbacks.map { CTFontCreateWithFontDescriptor($0, CGFloat(pointSize) * scale, nil) }

    /// Primary font unless it lacks the (first) character and a symbol font has it.
    func resolve(_ text: String, base: CTFont) -> CTFont {
        guard let u = text.unicodeScalars.first, u.value < 0x10000 else { return base }
        var ch = UniChar(u.value); var g = CGGlyph(0)
        if CTFontGetGlyphsForCharacters(base, &ch, &g, 1) { return base }
        for f in symbolFonts where CTFontGetGlyphsForCharacters(f, &ch, &g, 1) { return f }
        return base
    }

    private var ligatureCache: [String: Bool] = [:]

    /// True when shaping `text` as one run gives different glyphs than drawing each character alone, i.e. the font
    /// has a ligature or contextual alternate for it (Fira Code, JetBrains Mono, ...). Cached per run.
    func ligates(_ text: String, bold b: Bool, italic i: Bool) -> Bool {
        let key = "\(b ? 1 : 0)\(i ? 1 : 0)|\(text)"
        if let v = ligatureCache[key] { return v }
        let f = font(bold: b, italic: i)
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: [kCTFontAttributeName as NSAttributedString.Key: f]))
        var shaped: [CGGlyph] = []
        for run in CTLineGetGlyphRuns(line) as! [CTRun] {
            let n = CTRunGetGlyphCount(run)
            var g = [CGGlyph](repeating: 0, count: n)
            CTRunGetGlyphs(run, CFRange(location: 0, length: 0), &g)
            shaped += g
        }
        var chars = Array(text.utf16)
        var naive = [CGGlyph](repeating: 0, count: chars.count)
        CTFontGetGlyphsForCharacters(f, &chars, &naive, chars.count)
        let differs = shaped != naive
        ligatureCache[key] = differs
        return differs
    }

    func font(bold b: Bool, italic i: Bool) -> CTFont {
        switch (b, i) {
        case (false, false): return regular
        case (true, false): return bold
        case (false, true): return italic
        case (true, true): return boldItalic
        }
    }

    /// Symbol fonts consulted (in order) before the system's automatic fallback: fonts bundled in
    /// Resources/Fonts, then installed Nerd Font / Powerline symbol families.
    static let symbolFallbacks: [CTFontDescriptor] = {
        var out: [CTFontDescriptor] = []
        if let dir = Bundle.main.resourceURL?.appendingPathComponent("Fonts"),
           let files = try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil) {
            for f in files where ["ttf", "otf"].contains(f.pathExtension.lowercased()) {
                CTFontManagerRegisterFontsForURL(f as CFURL, .process, nil)
                if let ds = CTFontManagerCreateFontDescriptorsFromURL(f as CFURL) as? [CTFontDescriptor] { out += ds }
            }
        }
        // Only families that are really installed: a descriptor for a missing name would match an unrelated font.
        // (Checked per name; enumerating every installed family is far slower.)
        for name in ["Symbols Nerd Font Mono", "Symbols Nerd Font", "JetBrainsMono Nerd Font Mono", "MesloLGS NF"] {
            let f = CTFontCreateWithName(name as CFString, 12, nil)
            if (CTFontCopyFamilyName(f) as String) == name { out.append(CTFontDescriptorCreateWithNameAndSize(name as CFString, 0)) }
        }
        return out
    }()

    private static func withCascade(_ font: CTFont, _ list: [CTFontDescriptor]) -> CTFont {
        let attrs: [CFString: Any] = [kCTFontCascadeListAttribute: list]
        let d = CTFontDescriptorCreateWithAttributes(attrs as CFDictionary)
        return CTFontCreateCopyWithAttributes(font, CTFontGetSize(font), nil, d)
    }

    private static func makeBase(family: String, size: CGFloat) -> CTFont {
        if family.lowercased() == "sf mono" || family.lowercased() == "system" {
            return NSFont.monospacedSystemFont(ofSize: size, weight: .regular) as CTFont
        }
        let f = CTFontCreateWithName(family as CFString, size, nil)
        let fam = CTFontCopyFamilyName(f) as String
        let isFixed = CTFontGetSymbolicTraits(f).contains(.traitMonoSpace)
        if fam.lowercased() == family.lowercased() || isFixed { return f }
        NSLog("Aleno: font family \"%@\" not found; using the system monospaced font", family)
        return NSFont.monospacedSystemFont(ofSize: size, weight: .regular) as CTFont
    }

    private static func variant(_ base: CTFont, size: CGFloat, family: String, bold: Bool, italic: Bool) -> CTFont {
        if (family.lowercased() == "sf mono" || family.lowercased() == "system") && bold && !italic {
            return NSFont.monospacedSystemFont(ofSize: size, weight: .bold) as CTFont
        }
        var traits: CTFontSymbolicTraits = []
        if bold { traits.insert(.traitBold) }
        if italic { traits.insert(.traitItalic) }
        if let v = CTFontCreateCopyWithSymbolicTraits(base, size, nil, traits, traits) { return v }
        if bold, let v = CTFontCreateCopyWithSymbolicTraits(base, size, nil, .traitBold, .traitBold) { return v }
        return base
    }
}
