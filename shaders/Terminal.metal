#include <metal_stdlib>
using namespace metal;

// One instance per terminal cell. Layout must match `CellInstance` in Renderer.swift (24 bytes).
struct CellInstance {
    ushort2 pos;     // column, row
    ushort  flags;
    ushort  pad;
    uint    fg;      // RGBA8 (r in low byte)
    uint    bg;
    ushort4 rect;    // atlas tile: x, y, w, h (pixels)
};

struct Uniforms {
    float2 viewport;     // drawable size in pixels
    float2 origin;       // grid origin (padding) in pixels
    float2 cell;         // cell size in pixels
    float2 atlasSize;
    float4 cursorColor;
    float4 cursorTextColor;
    float4 selectionColor;
    uint2  cursorPos;
    uint   cursorMode;   // 0 hidden, 1 block, 2 hollow block, 3 bar, 4 underline
    uint   gridCols;
    float  scale;
    float  underlinePos;
    float  underlineThickness;
    uint   gridRows;
};

constant ushort F_GLYPH = 1, F_COLOR = 2, F_WIDE = 4, F_UNDERLINE = 8, F_STRIKE = 16, F_DOUBLE_UL = 32,
                F_SELECTED = 64, F_MATCH = 128, F_CURRENT = 256, F_DIM = 512, F_SKIP = 1024, F_NOEXTV = 2048,
                F_BLOCK = 4096;   // block element / braille drawn procedurally; `rect.x`: code point - 0x2580, or 0x100 + (code point - 0x2800)

struct VOut {
    float4 position [[position]];
    float4 fg [[flat]];
    float4 bg [[flat]];
    float2 uv;
    float2 local;
    float2 size [[flat]];
    float span [[flat]];
    ushort flags [[flat]];
    uint2  cellPos [[flat]];
    ushort code [[flat]];
};

vertex VOut cell_vertex(uint vid [[vertex_id]], uint iid [[instance_id]],
                        constant CellInstance *cells [[buffer(0)]],
                        constant Uniforms &u [[buffer(1)]]) {
    const float2 corners[6] = { float2(0, 0), float2(1, 0), float2(0, 1), float2(1, 0), float2(1, 1), float2(0, 1) };
    CellInstance c = cells[iid];
    VOut o;
    float2 corner = corners[vid];
    // `pad` carries the width in cells of a ligature run (0 = a normal cell, or a wide character via F_WIDE).
    float wcells = c.pad > 0 ? float(c.pad) : ((c.flags & F_WIDE) ? 2.0 : 1.0);
    float2 size = float2(u.cell.x * wcells, u.cell.y);
    // Edge cells extend their background into the padding and any leftover pixels (like Ghostty's
    // window-padding-color = extend), so full-screen apps with their own background reach the window edges.
    float2 gridEnd = u.origin + float2(float(u.gridCols), float(u.gridRows)) * u.cell;
    float left = (c.pos.x == 0) ? u.origin.x : 0.0;
    bool extV = !(c.flags & F_NOEXTV);
    float top = (c.pos.y == 0 && extV) ? u.origin.y : 0.0;
    float right = (uint(c.pos.x) + uint(wcells) >= u.gridCols) ? max(0.0, u.viewport.x - gridEnd.x) : 0.0;
    float bottom = (uint(c.pos.y) + 1 >= u.gridRows && extV) ? max(0.0, u.viewport.y - gridEnd.y) : 0.0;
    float2 local = float2(mix(-left, size.x + right, corner.x), mix(-top, size.y + bottom, corner.y));
    float2 px = u.origin + float2(c.pos) * u.cell + local;
    o.position = float4(px.x / u.viewport.x * 2.0 - 1.0, 1.0 - px.y / u.viewport.y * 2.0, 0, 1);
    if (c.flags & F_SKIP) o.position = float4(2, 2, 0, 1);   // second half of a wide glyph
    o.fg = unpack_unorm4x8_to_float(c.fg);
    o.bg = unpack_unorm4x8_to_float(c.bg);
    o.uv = float2(c.rect.xy) + local;
    o.local = local;
    o.size = size;
    o.span = wcells;
    o.flags = c.flags;
    o.cellPos = uint2(c.pos);
    o.code = c.rect.x;
    return o;
}

// Block elements (U+2580..U+259F) are drawn as exact fractions of the whole cell, so they meet with no gaps at any
// line height or font. `code` is the code point minus 0x2580, `p` the position inside the cell in 0..1.
float block_coverage(uint code, float2 p) {
    if (code == 0)  return p.y < 0.5 ? 1.0 : 0.0;                                   // upper half
    if (code <= 7)  return p.y >= 1.0 - float(code) / 8.0 ? 1.0 : 0.0;               // lower 1/8 .. 7/8
    if (code == 8)  return 1.0;                                                      // full block
    if (code <= 15) return p.x < float(16 - code) / 8.0 ? 1.0 : 0.0;                 // left 7/8 .. 1/8
    if (code == 16) return p.x >= 0.5 ? 1.0 : 0.0;                                   // right half
    if (code <= 19) return 0.25 * float(code - 16);                                  // light, medium, dark shade
    if (code == 20) return p.y < 0.125 ? 1.0 : 0.0;                                  // upper 1/8
    if (code == 21) return p.x >= 0.875 ? 1.0 : 0.0;                                 // right 1/8
    // Quadrants: bit 0 upper-left, 1 upper-right, 2 lower-left, 3 lower-right.
    const uint quad[10] = { 4, 8, 1, 13, 9, 7, 11, 2, 6, 14 };
    uint m = quad[min(code - 22, 9u)];
    uint q = (p.y < 0.5 ? 0u : 2u) + (p.x < 0.5 ? 0u : 1u);                         // 0 UL, 1 UR, 2 LL, 3 LR
    uint bit = q == 0 ? 1u : (q == 1 ? 2u : (q == 2 ? 4u : 8u));
    return (m & bit) != 0 ? 1.0 : 0.0;
}

// Braille patterns (U+2800..U+28FF) as round dots on a 2x4 grid filling the whole cell, for dot-matrix graphics.
float braille_coverage(uint bits, float2 local, float2 size) {
    float r = min(size.x / 4.0, size.y / 8.0) * 0.92;
    float cov = 0.0;
    for (uint i = 0; i < 8; i++) {
        if ((bits & (1u << i)) == 0) continue;
        // dots 1,2,3 then 4,5,6 run down the left and right columns; dots 7 and 8 are the bottom row.
        uint col = i < 3 ? 0 : (i < 6 ? 1 : (i == 6 ? 0 : 1));
        uint row = i < 3 ? i : (i < 6 ? i - 3 : 3);
        float2 c = float2((0.25 + 0.5 * float(col)) * size.x, (0.125 + 0.25 * float(row)) * size.y);
        float d = length(local - c);
        cov = max(cov, clamp(r - d + 0.5, 0.0, 1.0));      // one pixel of antialiasing
    }
    return cov;
}

fragment float4 cell_fragment(VOut in [[stage_in]],
                              texture2d<float> atlas [[texture(0)]],
                              constant Uniforms &u [[buffer(0)]]) {
    constexpr sampler s(coord::normalized, filter::nearest, address::clamp_to_edge);
    float4 bg = in.bg;
    float3 fg = in.fg.rgb;
    float alpha = bg.a;
    float3 rgb = bg.rgb;
    ushort f = in.flags;
    // Padding extension area: plain background only.
    if (in.local.x < 0.0 || in.local.y < 0.0 || in.local.x >= in.size.x || in.local.y >= in.size.y) {
        return float4(bg.rgb * bg.a, bg.a);
    }

    if (f & F_SELECTED) { rgb = u.selectionColor.rgb; alpha = 1.0; }
    if (f & F_MATCH)    { rgb = mix(rgb, float3(0.95, 0.80, 0.20), 0.55); alpha = 1.0; }
    if (f & F_CURRENT)  { rgb = float3(1.0, 0.60, 0.15); alpha = 1.0; fg = float3(0.08); }

    bool isCursor = u.cursorMode != 0 && in.cellPos.y == u.cursorPos.y && u.cursorPos.x >= in.cellPos.x && u.cursorPos.x < in.cellPos.x + uint(in.span);
    if (isCursor && u.cursorMode == 1) { rgb = u.cursorColor.rgb; alpha = 1.0; fg = u.cursorTextColor.rgb; }

    float cov = 0.0;
    float3 colorGlyph = float3(0);
    float colorA = 0.0;
    if (f & F_GLYPH) {
        float4 t = atlas.sample(s, in.uv / u.atlasSize);
        if (f & F_COLOR) { colorGlyph = t.rgb; colorA = t.a; }
        else { cov = t.a; }
    }
    if (f & F_BLOCK) cov = in.code >= 0x100 ? braille_coverage(uint(in.code - 0x100), in.local, in.size) : block_coverage(uint(in.code), in.local / in.size);
    if (f & F_DIM) cov *= 0.6;
    rgb = mix(rgb, fg, cov);
    rgb = colorGlyph + rgb * (1.0 - colorA);
    alpha = max(alpha, max(cov, colorA));

    float thick = max(1.0, u.underlineThickness);
    bool ul = (f & F_UNDERLINE) && in.local.y >= u.underlinePos && in.local.y < u.underlinePos + thick;
    bool dul = (f & F_DOUBLE_UL) && ((in.local.y >= u.underlinePos - thick * 1.5 && in.local.y < u.underlinePos - thick * 0.5) ||
                                    (in.local.y >= u.underlinePos + thick * 0.5 && in.local.y < u.underlinePos + thick * 1.5));
    float mid = u.cell.y * 0.52;
    bool st = (f & F_STRIKE) && in.local.y >= mid && in.local.y < mid + thick;
    if (ul || dul || st) { rgb = (isCursor && u.cursorMode == 1) ? u.cursorTextColor.rgb : fg; alpha = 1.0; }

    if (isCursor) {
        float t = max(1.0, round(u.scale));
        if (u.cursorMode == 2) {
            bool edge = in.local.x < t || in.local.x >= in.size.x - t || in.local.y < t || in.local.y >= in.size.y - t;
            if (edge) { rgb = u.cursorColor.rgb; alpha = 1.0; }
        } else if (u.cursorMode == 3) {
            if (in.local.x < max(2.0, t * 1.5)) { rgb = u.cursorColor.rgb; alpha = 1.0; }
        } else if (u.cursorMode == 4) {
            if (in.local.y >= in.size.y - t * 2.0) { rgb = u.cursorColor.rgb; alpha = 1.0; }
        }
    }
    return float4(rgb * alpha, alpha);   // premultiplied
}
