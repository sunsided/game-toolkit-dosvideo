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
        F.get_or_init(|| {
            let mut data = Vec::with_capacity(256 * 14);
            for g in VGA16.as_chunks::<16>().0 {
                // VGA glyphs carry one or two blank rows above and below; drop row 0 and 15
                // unless the glyph reaches them (box drawing, blocks), then drop the bottom two.
                let (a, b) = if g[0] == 0 && g[15] == 0 {
                    (1, 15)
                } else {
                    (0, 14)
                };
                data.extend_from_slice(&g[a..b]);
            }
            Font { height: 14, data }
        })
    }

    /// The CGA 8x8 font.
    pub fn cga() -> &'static Font {
        static F: OnceLock<Font> = OnceLock::new();
        F.get_or_init(|| {
            let mut data = Vec::with_capacity(256 * 8);
            for (code, g) in VGA16.as_chunks::<16>().0.iter().enumerate() {
                data.extend_from_slice(&squash_glyph(code as u8, g));
            }
            Font { height: 8, data }
        })
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
    fn cp437_round_trip() {
        assert_eq!(cp437("A╔═╗☺"), vec![b'A', 0xC9, 0xCD, 0xBB, 0x01]);
    }
}
