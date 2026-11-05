//! The standard colors of the PC display adapters.

/// An RGB triple, 8 bits per channel.
pub type Rgb = [u8; 3];

/// The 16 RGBI colors of CGA (and of EGA in its 200-line modes), with the brown hack on 6.
pub const CGA: [Rgb; 16] = [
    [0x00, 0x00, 0x00],
    [0x00, 0x00, 0xAA],
    [0x00, 0xAA, 0x00],
    [0x00, 0xAA, 0xAA],
    [0xAA, 0x00, 0x00],
    [0xAA, 0x00, 0xAA],
    [0xAA, 0x55, 0x00],
    [0xAA, 0xAA, 0xAA],
    [0x55, 0x55, 0x55],
    [0x55, 0x55, 0xFF],
    [0x55, 0xFF, 0x55],
    [0x55, 0xFF, 0xFF],
    [0xFF, 0x55, 0x55],
    [0xFF, 0x55, 0xFF],
    [0xFF, 0xFF, 0x55],
    [0xFF, 0xFF, 0xFF],
];

/// Names of the 16 text-mode colors, for attributes.
pub mod color {
    pub const BLACK: u8 = 0;
    pub const BLUE: u8 = 1;
    pub const GREEN: u8 = 2;
    pub const CYAN: u8 = 3;
    pub const RED: u8 = 4;
    pub const MAGENTA: u8 = 5;
    pub const BROWN: u8 = 6;
    pub const LIGHT_GRAY: u8 = 7;
    pub const DARK_GRAY: u8 = 8;
    pub const LIGHT_BLUE: u8 = 9;
    pub const LIGHT_GREEN: u8 = 10;
    pub const LIGHT_CYAN: u8 = 11;
    pub const LIGHT_RED: u8 = 12;
    pub const LIGHT_MAGENTA: u8 = 13;
    pub const YELLOW: u8 = 14;
    pub const WHITE: u8 = 15;
}

/// One of the 64 EGA colors (`rgbRGB` bit layout: bits 0-2 are the 2/3 primaries, bits 3-5 the
/// 1/3 secondaries).
pub const fn ega64(i: u8) -> Rgb {
    const fn c(i: u8, hi: u8, lo: u8) -> u8 {
        (((i >> hi) & 1) * 0xAA) + (((i >> lo) & 1) * 0x55)
    }
    [c(i, 2, 5), c(i, 1, 4), c(i, 0, 3)]
}

/// How an EGA monitor in a 200-line mode reads a 6-bit palette value: as CGA-style RGBI, with
/// bit 4 as intensity. The secondary red and blue bits are ignored.
pub const fn ega200(i: u8) -> Rgb {
    let rgbi = (i & 0x07) | ((i >> 1) & 0x08);
    CGA[rgbi as usize]
}

/// Scale a 6-bit DAC component to 8 bits.
pub const fn dac6_to_8(v: u8) -> u8 {
    let v = v & 0x3F;
    (v << 2) | (v >> 4)
}

/// The VGA BIOS default 256-color palette, as 6-bit DAC values: the 16 EGA colors, 16 grays and
/// nine 24-hue wheels (three saturations at three brightnesses), then eight blacks.
pub fn vga_default_dac() -> [[u8; 3]; 256] {
    let mut pal = [[0u8; 3]; 256];
    for (i, p) in pal.iter_mut().take(16).enumerate() {
        let c = CGA[i];
        *p = [c[0] >> 2, c[1] >> 2, c[2] >> 2];
    }
    const GRAYS: [u8; 16] = [0, 5, 8, 11, 14, 17, 20, 24, 28, 32, 36, 40, 45, 50, 56, 63];
    for (i, g) in GRAYS.iter().enumerate() {
        pal[16 + i] = [*g; 3];
    }
    const LEVELS: [[u8; 5]; 9] = [
        [0, 16, 31, 47, 63],
        [31, 39, 47, 55, 63],
        [45, 49, 54, 58, 63],
        [0, 7, 14, 21, 28],
        [14, 17, 21, 24, 28],
        [20, 22, 24, 26, 28],
        [0, 4, 8, 12, 16],
        [8, 10, 12, 14, 16],
        [11, 12, 13, 15, 16],
    ];
    let mut idx = 32;
    for l in LEVELS {
        // Walk the hue circle blue -> magenta -> red -> yellow -> green -> cyan -> blue in 24
        // steps, each step raising or lowering one component by one level.
        let (lo, hi) = (0usize, 4usize);
        let mut c = [lo, lo, hi]; // r, g, b level indices
        // (component, direction) for the six legs
        let legs: [(usize, i32); 6] = [(0, 1), (2, -1), (1, 1), (0, -1), (2, 1), (1, -1)];
        for (comp, dir) in legs {
            for _ in 0..4 {
                pal[idx] = [l[c[0]], l[c[1]], l[c[2]]];
                idx += 1;
                c[comp] = (c[comp] as i32 + dir) as usize;
            }
        }
    }
    pal
}

/// Build the DAC contents the VGA BIOS loads for the 16-color modes: the first 64 entries are
/// the EGA colors (or the 200-line RGBI interpretation), the rest left as in `base`.
pub fn vga_ega_dac(two_hundred_lines: bool) -> [[u8; 3]; 256] {
    let mut pal = vga_default_dac();
    for (i, p) in pal.iter_mut().take(64).enumerate() {
        let c = if two_hundred_lines {
            ega200(i as u8)
        } else {
            ega64(i as u8)
        };
        *p = [c[0] >> 2, c[1] >> 2, c[2] >> 2];
    }
    pal
}

/// Monochrome monitor phosphors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Phosphor {
    /// P39-ish green, the classic IBM 5151 look.
    #[default]
    Green,
    /// P3 amber.
    Amber,
    /// "Paper white" P4.
    White,
}

impl Phosphor {
    /// The color at full intensity.
    pub fn bright(self) -> Rgb {
        match self {
            Phosphor::Green => [0x66, 0xFF, 0x66],
            Phosphor::Amber => [0xFF, 0xC0, 0x30],
            Phosphor::White => [0xF0, 0xF0, 0xFF],
        }
    }

    /// The color at normal intensity.
    pub fn normal(self) -> Rgb {
        let b = self.bright();
        [
            (b[0] as u16 * 2 / 3) as u8,
            (b[1] as u16 * 2 / 3) as u8,
            (b[2] as u16 * 2 / 3) as u8,
        ]
    }
}

pub(crate) const fn rgba(c: Rgb) -> [u8; 4] {
    [c[0], c[1], c[2], 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_palette_matches_known_entries() {
        let p = vga_default_dac();
        assert_eq!(p[32], [0, 0, 63]);
        assert_eq!(p[40], [63, 0, 0]);
        assert_eq!(p[44], [63, 63, 0]);
        assert_eq!(p[48], [0, 63, 0]);
        assert_eq!(p[52], [0, 63, 63]);
        assert_eq!(p[55], [0, 16, 63]);
        assert_eq!(p[56], [31, 31, 63]);
        assert_eq!(p[248], [0, 0, 0]);
        assert_eq!(p[31], [63, 63, 63]);
    }

    #[test]
    fn ega_colors() {
        assert_eq!(ega64(0x14), [0xAA, 0x55, 0x00]); // brown
        assert_eq!(ega64(0x3F), [0xFF, 0xFF, 0xFF]);
        assert_eq!(ega200(0x14), [0xFF, 0x55, 0x55]); // bit 4 is intensity here
        assert_eq!(ega200(0x06), [0xAA, 0x55, 0x00]);
        assert_eq!(ega200(0x17), [0xFF, 0xFF, 0xFF]);
    }
}
