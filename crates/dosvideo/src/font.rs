//! Code page 437 bitmap fonts, the character sets of the PC adapters.
//!
//! The 8x16 set is the VGA font; the 8x14 (EGA, MDA/Hercules) and 8x8 (CGA, and VGA's 50-line
//! text) sets are derived from it. Load your own with [`Font::from_raw`].

use std::sync::OnceLock;

static VGA16: &[u8; 4096] = include_bytes!("../fonts/cp437_8x16.bin");

/// A 256-glyph, 8-pixel-wide bitmap font. One byte per glyph row, most significant bit leftmost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Font {
    height: u8,
    data: Vec<u8>,
}

impl Font {
    /// A font from raw glyph rows: `256 * height` bytes.
    pub fn from_raw(height: u8, data: &[u8]) -> Option<Self> {
        (height > 0 && height <= 32 && data.len() == 256 * height as usize).then(|| Self {
            height,
            data: data.to_vec(),
        })
    }

    /// The VGA 8x16 font.
    pub fn vga() -> &'static Font {
        static F: OnceLock<Font> = OnceLock::new();
        F.get_or_init(|| Font {
            height: 16,
            data: VGA16.to_vec(),
        })
    }

    /// The EGA / MDA 8x14 font.
    pub fn ega() -> &'static Font {
        static F: OnceLock<Font> = OnceLock::new();
        F.get_or_init(|| derive14(VGA16))
    }

    /// The CGA 8x8 font.
    pub fn cga() -> &'static Font {
        static F: OnceLock<Font> = OnceLock::new();
        F.get_or_init(|| derive8(VGA16))
    }

    /// The VGA font with the letters replaced by the Standard Galactic Alphabet, the alien
    /// script of the Commander Keen games: `A`-`Z` and `a`-`z` both map to the 26 SGA glyphs,
    /// everything else (digits, punctuation, box drawing) stays as it is. Load it with
    /// [`Dos::load_font`](crate::Dos::load_font) for galactic text mode, or draw with it via
    /// [`Bitmap::text`](crate::draw::Bitmap::text).
    pub fn sga() -> &'static Font {
        static F: OnceLock<Font> = OnceLock::new();
        F.get_or_init(|| Font {
            height: 16,
            data: sga16().to_vec(),
        })
    }

    /// The SGA font at 8x14, for EGA and Hercules text.
    pub fn sga_ega() -> &'static Font {
        static F: OnceLock<Font> = OnceLock::new();
        F.get_or_init(|| derive14(&sga16()))
    }

    /// The SGA font at 8x8, for CGA text and VGA 50-line text.
    pub fn sga_cga() -> &'static Font {
        static F: OnceLock<Font> = OnceLock::new();
        F.get_or_init(|| derive8(&sga16()))
    }

    pub fn height(&self) -> u8 {
        self.height
    }

    /// The rows of glyph `ch`.
    pub fn glyph(&self, ch: u8) -> &[u8] {
        let h = self.height as usize;
        &self.data[ch as usize * h..(ch as usize + 1) * h]
    }

    /// All glyph rows, `256 * height` bytes.
    pub fn raw(&self) -> &[u8] {
        &self.data
    }

    /// Width of a string in pixels.
    pub fn text_width(&self, s: &str) -> u32 {
        s.chars().count() as u32 * 8
    }
}

/// An 8x14 font from an 8x16 one.
fn derive14(src: &[u8; 4096]) -> Font {
    let mut data = Vec::with_capacity(256 * 14);
    for g in src.as_chunks::<16>().0 {
        // VGA glyphs carry one or two blank rows above and below; drop row 0 and 15 unless the
        // glyph reaches them (box drawing, blocks), then drop the bottom two.
        let (a, b) = if g[0] == 0 && g[15] == 0 {
            (1, 15)
        } else {
            (0, 14)
        };
        data.extend_from_slice(&g[a..b]);
    }
    Font { height: 14, data }
}

/// An 8x8 font from an 8x16 one.
fn derive8(src: &[u8; 4096]) -> Font {
    let mut data = Vec::with_capacity(256 * 8);
    for (code, g) in src.as_chunks::<16>().0.iter().enumerate() {
        data.extend_from_slice(&squash_glyph(code as u8, g));
    }
    Font { height: 8, data }
}

/// The 26 Standard Galactic Alphabet letters, rows 2..=11 of an 8x16 cell (in the VGA font's
/// style: 2-pixel verticals, 1-pixel horizontals). Drawn for this crate after the letter
/// forms commonly given as ᔑ ʖ ᓵ ↸ ᒷ ⎓ ⊣ ⍑ ╎ ⋮ ꖌ ꖎ ᒲ リ 𝙹 !¡ ᑑ ∷ ᓭ ℸ̣ ⚍ ⍊ ∴ ·/ || ⨅.
const SGA: [[&str; 10]; 26] = [
    [
        "....###.", "...##.##", "...##...", "...##...", "...##...", "...##...", "...##...",
        "...##...", "##.##...", ".###....",
    ],
    [
        "..##....", "..##....", "..##....", "..##....", "..###...", "...###..", "....##..",
        "....##..", "..####..", "..##....",
    ],
    [
        ".##.....", ".##.....", ".##.....", ".###....", "..####..", "....###.", ".....##.",
        ".....##.", ".....##.", ".....##.",
    ],
    [
        "########", "........", "#####...", "##......", "#.#.....", "#..#....", "....#...",
        ".....#..", "......#.", ".......#",
    ],
    [
        "##......", "##......", "##......", "##......", "##...##.", "##...##.", "##......",
        "##......", "##......", "#######.",
    ],
    [
        "........", "........", "........", "########", "........", "........", "##.##.##",
        "........", "........", "........",
    ],
    [
        "......##", "......##", "......##", "......##", "########", "......##", "......##",
        "......##", "......##", "......##",
    ],
    [
        "........", "########", "........", "########", "...##...", "...##...", "...##...",
        "...##...", "...##...", "...##...",
    ],
    [
        "...##...", "...##...", "...##...", "...##...", "........", "........", "...##...",
        "...##...", "...##...", "...##...",
    ],
    [
        "........", "...##...", "...##...", "........", "...##...", "...##...", "........",
        "...##...", "...##...", "........",
    ],
    [
        "...##...", "...##...", "...##...", "...##...", "##.##.##", "##.##.##", "...##...",
        "...##...", "...##...", "...##...",
    ],
    [
        "..##....", "..##....", "..##.##.", "..##.##.", "..##....", "..##....", "..##.##.",
        "..##.##.", "..##....", "..##....",
    ],
    [
        "......##", "......##", "......##", "......##", ".##...##", ".##...##", "......##",
        "......##", "......##", ".#######",
    ],
    [
        ".##..##.", ".##..##.", ".##..##.", ".##..##.", ".##..##.", ".....##.", ".....##.",
        "....##..", "...##...", "..##....",
    ],
    [
        "########", "....##..", "....##..", "....##..", "....##..", "....##..", "....##..",
        "##..##..", "##..##..", ".####...",
    ],
    [
        ".##..##.", ".##..##.", ".##.....", ".##..##.", ".##..##.", ".##..##.", ".##..##.",
        ".....##.", ".##..##.", ".##..##.",
    ],
    [
        "#####...", "######..", "....##..", ".....##.", ".....##.", ".....##.", ".....##.",
        "....##..", "######..", "#####...",
    ],
    [
        "........", ".##..##.", ".##..##.", "........", "........", ".##..##.", ".##..##.",
        "........", "........", "........",
    ],
    [
        ".##.....", ".##.....", ".###....", "..#####.", "....###.", ".....##.", ".....##.",
        ".....##.", ".....##.", ".....##.",
    ],
    [
        "########", "......##", "......##", "......##", "......##", "......##", "......##",
        "......##", "##....##", "##......",
    ],
    [
        "........", "........", "###..###", "###..###", "........", "........", "########",
        "########", "........", "........",
    ],
    [
        "...##...", "...##...", "...##...", "...##...", "...##...", "...##...", "########",
        "........", "########", "........",
    ],
    [
        "........", "...##...", "...##...", "........", "........", "##....##", "##....##",
        "........", "........", "........",
    ],
    [
        ".......#", "......##", "......#.", ".....##.", "##...#..", "##..##..", "....#...",
        "...##...", "...#....", "..##....",
    ],
    [
        ".##..##.", ".##..##.", ".##..##.", ".##..##.", ".##..##.", ".##..##.", ".##..##.",
        ".##..##.", ".##..##.", ".##..##.",
    ],
    [
        "########", "##....##", "##....##", "##....##", "##....##", "##....##", "##....##",
        "##....##", "##....##", "##....##",
    ],
];

/// The VGA font with SGA letters.
fn sga16() -> [u8; 4096] {
    let mut data = *VGA16;
    for (i, rows) in SGA.iter().enumerate() {
        let mut g = [0u8; 16];
        for (r, row) in rows.iter().enumerate() {
            g[r + 2] =
                row.bytes().enumerate().fold(
                    0,
                    |acc, (x, c)| if c == b'#' { acc | (0x80 >> x) } else { acc },
                );
        }
        for base in *b"Aa" {
            let at = (base as usize + i) * 16;
            data[at..at + 16].copy_from_slice(&g);
        }
    }
    data
}

/// Fit a 16-row glyph into 8 rows.
fn squash_glyph(code: u8, g: &[u8]) -> [u8; 8] {
    let mut out = [0u8; 8];
    let full_height = g[0] != 0 || g[15] != 0;
    if full_height {
        // Box drawing and blocks must still touch the cell edges: sample rows evenly.
        for (r, o) in out.iter_mut().enumerate() {
            *o = g[r * 2] | g[r * 2 + 1] & g[(r * 2 + 2).min(15)];
            if r == 7 {
                *o = g[14] | g[15];
            }
        }
        // Keep the horizontal stroke row of box-drawing characters.
        if (0xB3..=0xDA).contains(&code) {
            for i in 0..16 {
                if g[i] == 0xFF || g[i] == 0xF8 || g[i] == 0x1F {
                    out[(i / 2).min(7)] |= g[i];
                }
            }
        }
        return out;
    }
    // Text glyphs: content sits in rows 2..14 (12 rows incl. descenders). Map 12 -> 7 rows plus
    // a blank bottom row by OR-ing neighbours, which keeps thin strokes visible.
    const MAP: [(usize, usize); 7] = [(2, 3), (4, 4), (5, 6), (7, 7), (8, 9), (10, 10), (11, 12)];
    for (r, (a, b)) in MAP.iter().enumerate() {
        out[r] = g[*a] | g[*b];
    }
    out[7] = g[13] | g[14];
    out
}

/// Unicode for a CP437 code (for printing Rust strings into text mode).
pub fn cp437_from_char(c: char) -> u8 {
    if (c as u32) < 0x80 && c as u32 >= 0x20 {
        return c as u8;
    }
    CP437_HIGH
        .iter()
        .position(|&h| h == c)
        .map(|i| (i + 0x80) as u8)
        .or_else(|| CP437_LOW.iter().position(|&h| h == c).map(|i| i as u8))
        .unwrap_or(b'?')
}

/// Encode a string as CP437 bytes; unmappable characters become `?`.
pub fn cp437(s: &str) -> Vec<u8> {
    s.chars().map(cp437_from_char).collect()
}

const CP437_LOW: [char; 32] = [
    '\0', '☺', '☻', '♥', '♦', '♣', '♠', '•', '◘', '○', '◙', '♂', '♀', '♪', '♫', '☼', '►', '◄', '↕',
    '‼', '¶', '§', '▬', '↨', '↑', '↓', '→', '←', '∟', '↔', '▲', '▼',
];

const CP437_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', 'É', 'æ', 'Æ',
    'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', 'á', 'í', 'ó', 'ú', 'ñ', 'Ñ',
    'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕',
    '╣', '║', '╗', '╝', '╜', '╛', '┐', '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦',
    '╠', '═', '╬', '╧', '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐',
    '▀', 'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', '≡', '±',
    '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_have_right_sizes() {
        assert_eq!(Font::vga().raw().len(), 4096);
        assert_eq!(Font::ega().raw().len(), 256 * 14);
        assert_eq!(Font::cga().raw().len(), 256 * 8);
        assert!(Font::vga().glyph(b'A').iter().any(|&r| r != 0));
        assert!(Font::cga().glyph(0xDB).iter().all(|&r| r == 0xFF));
        assert_eq!(Font::cga().glyph(0xDE)[0], 0x0F);
    }

    #[test]
    fn sga_replaces_letters_only() {
        let s = Font::sga();
        assert_ne!(s.glyph(b'A'), Font::vga().glyph(b'A'));
        assert_eq!(s.glyph(b'A'), s.glyph(b'a'));
        assert_eq!(s.glyph(b'7'), Font::vga().glyph(b'7'));
        assert_eq!(s.glyph(0xC9), Font::vga().glyph(0xC9));
        assert_eq!(Font::sga_cga().height(), 8);
        assert_eq!(Font::sga_ega().height(), 14);
    }

    #[test]
    fn cp437_round_trip() {
        assert_eq!(cp437("A╔═╗☺"), vec![b'A', 0xC9, 0xCD, 0xBB, 0x01]);
    }
}
