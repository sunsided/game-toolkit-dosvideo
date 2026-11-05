//! Drawing the way DOS programs drew: palette programming, register helpers, pixel plotting for
//! every adapter's memory layout, Mode X with page flipping, and an off-screen [`Bitmap`] for
//! double buffering in system memory.
//!
//! Everything here goes through [`Dos::outb`] / [`Dos::poke`], so it costs bus time and shows
//! up on screen where the beam is when it happens.

use crate::font::Font;
use crate::machine::{Dos, Video};

/// An 8-bit-per-pixel bitmap in system memory: a back buffer, a sprite, a tile sheet.
///
/// Drawing into it is free (it is not video memory). Copy it to the screen with
/// [`Dos::blit13`], [`ModeX::blit`] or the per-adapter helpers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Bitmap {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; (width * height) as usize],
        }
    }

    pub fn from_data(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        (data.len() == (width * height) as usize).then_some(Self {
            width,
            height,
            data,
        })
    }

    /// Build a bitmap from rows of characters, mapping each character through `palette`
    /// (characters not in it become 0, the transparent color). Handy for small sprites.
    pub fn from_art(rows: &[&str], palette: &[(char, u8)]) -> Self {
        let h = rows.len() as u32;
        let w = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0) as u32;
        let mut b = Self::new(w, h);
        for (y, r) in rows.iter().enumerate() {
            for (x, ch) in r.chars().enumerate() {
                let c = palette.iter().find(|p| p.0 == ch).map_or(0, |p| p.1);
                b.data[y * w as usize + x] = c;
            }
        }
        b
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return 0;
        }
        self.data[(y as u32 * self.width + x as u32) as usize]
    }

    #[inline]
    pub fn pset(&mut self, x: i32, y: i32, c: u8) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        self.data[(y as u32 * self.width + x as u32) as usize] = c;
    }

    pub fn clear(&mut self, c: u8) {
        self.data.fill(c);
    }

    pub fn row(&self, y: u32) -> &[u8] {
        let w = self.width as usize;
        &self.data[y as usize * w..(y as usize + 1) * w]
    }

    pub fn row_mut(&mut self, y: u32) -> &mut [u8] {
        let w = self.width as usize;
        &mut self.data[y as usize * w..(y as usize + 1) * w]
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: u8) {
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(self.width as i32);
        let y1 = (y + h).min(self.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for yy in y0..y1 {
            let o = (yy as u32 * self.width) as usize;
            self.data[o + x0 as usize..o + x1 as usize].fill(c);
        }
    }

    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: u8) {
        self.hline(x, x + w - 1, y, c);
        self.hline(x, x + w - 1, y + h - 1, c);
        self.vline(x, y, y + h - 1, c);
        self.vline(x + w - 1, y, y + h - 1, c);
    }

    pub fn hline(&mut self, x0: i32, x1: i32, y: i32, c: u8) {
        let (a, b) = (x0.min(x1), x0.max(x1));
        self.fill_rect(a, y, b - a + 1, 1, c);
    }

    pub fn vline(&mut self, x: i32, y0: i32, y1: i32, c: u8) {
        let (a, b) = (y0.min(y1), y0.max(y1));
        self.fill_rect(x, a, 1, b - a + 1, c);
    }

    /// Bresenham line.
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: u8) {
        let (mut x, mut y) = (x0, y0);
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.pset(x, y, c);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Midpoint circle; `fill` draws a disc.
    pub fn circle(&mut self, cx: i32, cy: i32, r: i32, c: u8, fill: bool) {
        let (mut x, mut y, mut d) = (r, 0, 1 - r);
        while x >= y {
            if fill {
                self.hline(cx - x, cx + x, cy + y, c);
                self.hline(cx - x, cx + x, cy - y, c);
                self.hline(cx - y, cx + y, cy + x, c);
                self.hline(cx - y, cx + y, cy - x, c);
            } else {
                for (px, py) in [
                    (x, y),
                    (y, x),
                    (-y, x),
                    (-x, y),
                    (-x, -y),
                    (-y, -x),
                    (y, -x),
                    (x, -y),
                ] {
                    self.pset(cx + px, cy + py, c);
                }
            }
            y += 1;
            if d < 0 {
                d += 2 * y + 1;
            } else {
                x -= 1;
                d += 2 * (y - x) + 1;
            }
        }
    }

    /// Copy `src` with its top-left at `(x, y)`; `transparent` pixels are skipped.
    pub fn blit(&mut self, src: &Bitmap, x: i32, y: i32, transparent: Option<u8>) {
        for sy in 0..src.height as i32 {
            let dy = y + sy;
            if dy < 0 || dy >= self.height as i32 {
                continue;
            }
            for sx in 0..src.width as i32 {
                let dx = x + sx;
                if dx < 0 || dx >= self.width as i32 {
                    continue;
                }
                let c = src.data[(sy as u32 * src.width + sx as u32) as usize];
                if Some(c) != transparent {
                    self.data[(dy as u32 * self.width + dx as u32) as usize] = c;
                }
            }
        }
    }

    /// Copy a `w` x `h` region of `src` starting at `(sx, sy)`.
    #[allow(clippy::too_many_arguments)]
    pub fn blit_region(
        &mut self,
        src: &Bitmap,
        sx: i32,
        sy: i32,
        w: i32,
        h: i32,
        x: i32,
        y: i32,
        transparent: Option<u8>,
    ) {
        for yy in 0..h {
            for xx in 0..w {
                let c = src.get(sx + xx, sy + yy);
                if Some(c) != transparent {
                    self.pset(x + xx, y + yy, c);
                }
            }
        }
    }

    /// Draw text with an 8-pixel-wide bitmap font; `bg` fills the cell background.
    pub fn text(&mut self, x: i32, y: i32, s: &str, fg: u8, bg: Option<u8>, font: &Font) {
        let h = font.height() as i32;
        for (i, ch) in crate::font::cp437(s).into_iter().enumerate() {
            let g = font.glyph(ch);
            let cx = x + i as i32 * 8;
            for r in 0..h {
                for b in 0..8 {
                    if (g[r as usize] >> (7 - b)) & 1 != 0 {
                        self.pset(cx + b, y + r, fg);
                    } else if let Some(bg) = bg {
                        self.pset(cx + b, y + r, bg);
                    }
                }
            }
        }
    }
}

/// Which port the CRTC answers on in the current configuration.
fn crtc_port(dos: &Dos) -> u16 {
    match dos.video() {
        Video::Hercules(_) => 0x3B4,
        Video::Vga(v) if v.misc & 1 == 0 => 0x3B4,
        Video::Svga(s) if s.vga.misc & 1 == 0 => 0x3B4,
        _ => 0x3D4,
    }
}

fn status_port(dos: &Dos) -> u16 {
    crtc_port(dos) + 6
}

fn vga_ref(dos: &Dos) -> Option<&crate::vga::Vga> {
    dos.video().as_vga()
}

impl Dos {
    // ---------------------------------------------------------------------------------------
    // Palette

    /// Set DAC entry `i` to a 6-bit `[r, g, b]` (each `0..=63`), via ports 0x3C8/0x3C9.
    pub fn set_palette(&mut self, i: u8, rgb: [u8; 3]) {
        self.outb(0x3C8, i);
        self.outb(0x3C9, rgb[0]);
        self.outb(0x3C9, rgb[1]);
        self.outb(0x3C9, rgb[2]);
    }

    /// Set DAC entries from `start` on (the auto-increment makes this one index write).
    pub fn set_palette_range(&mut self, start: u8, colors: &[[u8; 3]]) {
        self.outb(0x3C8, start);
        for c in colors {
            self.outb(0x3C9, c[0]);
            self.outb(0x3C9, c[1]);
            self.outb(0x3C9, c[2]);
        }
    }

    /// Set a DAC entry from 8-bit RGB (converted to 6 bits, unless the DAC is in 8-bit mode).
    pub fn set_palette8(&mut self, i: u8, rgb: [u8; 3]) {
        let eight = vga_ref(self).is_some_and(|v| v.dac_8bit);
        let c = if eight {
            rgb
        } else {
            [rgb[0] >> 2, rgb[1] >> 2, rgb[2] >> 2]
        };
        self.set_palette(i, c);
    }

    /// Read DAC entry `i` via 0x3C7/0x3C9.
    pub fn palette(&mut self, i: u8) -> [u8; 3] {
        self.outb(0x3C7, i);
        [self.inb(0x3C9), self.inb(0x3C9), self.inb(0x3C9)]
    }

    /// Read the whole DAC without bus traffic (what a program keeps as its own copy).
    pub fn dac(&self) -> Option<[[u8; 3]; 256]> {
        vga_ref(self).map(|v| v.dac)
    }

    /// Rotate DAC entries `first..=last` by one: classic palette cycling (water, fire, plasma)
    /// for the price of a few port writes.
    pub fn cycle_palette(&mut self, first: u8, last: u8) {
        let Some(dac) = self.dac() else { return };
        let (a, b) = (first as usize, last as usize);
        if b <= a {
            return;
        }
        let mut seg: Vec<[u8; 3]> = dac[a..=b].to_vec();
        seg.rotate_right(1);
        self.set_palette_range(first, &seg);
    }

    /// Fade the whole palette towards black: `level` 0 (black) ..= 64 (original `base`).
    pub fn fade_palette(&mut self, base: &[[u8; 3]; 256], level: u8) {
        let l = level.min(64) as u16;
        let faded: Vec<[u8; 3]> = base
            .iter()
            .map(|c| {
                [
                    (c[0] as u16 * l / 64) as u8,
                    (c[1] as u16 * l / 64) as u8,
                    (c[2] as u16 * l / 64) as u8,
                ]
            })
            .collect();
        self.set_palette_range(0, &faded);
    }

    // ---------------------------------------------------------------------------------------
    // VGA / EGA registers

    /// Write sequencer register `index`.
    pub fn seq(&mut self, index: u8, v: u8) {
        self.outb(0x3C4, index);
        self.outb(0x3C5, v);
    }

    /// Write graphics controller register `index`.
    pub fn gc(&mut self, index: u8, v: u8) {
        self.outb(0x3CE, index);
        self.outb(0x3CF, v);
    }

    /// Write CRTC register `index` (on the color or mono port, whichever is active).
    pub fn crtc(&mut self, index: u8, v: u8) {
        let p = crtc_port(self);
        self.outb(p, index);
        self.outb(p + 1, v);
    }

    /// Current value of CRTC register `index` (from the card; EGA registers cannot be read
    /// back through the ports, so DOS programs kept a copy - this is that copy).
    pub fn crtc_value(&self, index: u8) -> u8 {
        match self.video() {
            Video::Vga(v) => v.crtc.get(index as usize).copied().unwrap_or(0),
            Video::Svga(s) => s.vga.crtc.get(index as usize).copied().unwrap_or(0),
            Video::Cga(c) => c.crtc.r.get(index as usize).copied().unwrap_or(0),
            Video::Hercules(h) => h.crtc.r.get(index as usize).copied().unwrap_or(0),
        }
    }

    /// Write attribute controller register `index`: reset the flip-flop by reading the status
    /// port, write index and value, then set the palette address source bit again (forgetting
    /// that blanks the screen).
    pub fn attr(&mut self, index: u8, v: u8) {
        let s = status_port(self);
        self.inb(s);
        self.outb(0x3C0, index & 0x1F);
        self.outb(0x3C0, v);
        self.outb(0x3C0, 0x20);
    }

    /// Sequencer map mask: which planes CPU writes go to.
    pub fn map_mask(&mut self, planes: u8) {
        self.seq(2, planes & 0x0F);
    }

    /// Graphics controller read map select.
    pub fn read_map(&mut self, plane: u8) {
        self.gc(4, plane & 3);
    }

    /// Graphics controller write mode (0-3), keeping the read mode.
    pub fn write_mode(&mut self, mode: u8) {
        let cur = vga_ref(self).map_or(0, |v| v.gc[5]);
        self.gc(5, (cur & !3) | (mode & 3));
    }

    /// Graphics controller bit mask.
    pub fn bit_mask(&mut self, mask: u8) {
        self.gc(8, mask);
    }

    /// Set the CRTC start address (in CRTC address units: words in text mode, 4-pixel groups in
    /// mode 13h and Mode X, 8-pixel groups in 16-color modes). Latched at the next vertical
    /// retrace, so set it during the display and *then* wait for retrace.
    pub fn set_start_address(&mut self, addr: u16) {
        self.crtc(0x0C, (addr >> 8) as u8);
        self.crtc(0x0D, addr as u8);
    }

    /// Current CRTC start address.
    pub fn start_address(&self) -> u16 {
        ((self.crtc_value(0x0C) as u16) << 8) | self.crtc_value(0x0D) as u16
    }

    /// Set the CRTC offset register: the distance between rows, in words (pairs of CRTC
    /// address units). Make it bigger than the screen for a virtual screen to scroll around.
    pub fn set_offset(&mut self, offset: u8) {
        self.crtc(0x13, offset);
    }

    /// Split screen: from scanline `line + 1` on, the display restarts at address 0 (and,
    /// with attribute mode bit 5, ignores pel panning). Spreads the 10-bit value over CRTC
    /// 0x18, bit 4 of the overflow register and bit 6 of max scan line. `0x3FF` turns it off.
    pub fn set_line_compare(&mut self, line: u16) {
        let ov = self.crtc_value(7);
        let msl = self.crtc_value(9);
        self.crtc(0x18, line as u8);
        self.crtc(7, (ov & !0x10) | (((line >> 8) & 1) as u8) << 4);
        self.crtc(9, (msl & !0x40) | (((line >> 9) & 1) as u8) << 6);
    }

    /// Horizontal pel panning (0-7 pixels in 16-color modes; 0, 2, 4, 6 for 0-3 pixels in
    /// 256-color modes; 8 means 0 in 9-dot text).
    pub fn set_pel_pan(&mut self, pan: u8) {
        self.attr(0x13 | 0x20, pan & 0x0F);
    }

    /// Overscan (border) color: a DAC index on VGA, a 6-bit color on EGA. On CGA it is the
    /// low four bits of the color select register.
    pub fn set_border(&mut self, c: u8) {
        match self.video() {
            Video::Cga(cga) => {
                let col = cga.color;
                self.outb(0x3D9, (col & 0xF0) | (c & 0x0F));
            }
            Video::Hercules(_) => {}
            _ => self.attr(0x11 | 0x20, c),
        }
    }

    /// Set one of the 16 attribute palette registers (EGA: the 6-bit color shown for index `i`).
    pub fn set_attr_palette(&mut self, i: u8, v: u8) {
        self.attr((i & 0x0F) | 0x20, v & 0x3F);
    }

    /// Turn the display off (sequencer clocking mode bit 5) or on.
    pub fn screen_on(&mut self, on: bool) {
        let cur = vga_ref(self).map_or(0, |v| v.seq[1]);
        self.seq(1, if on { cur & !0x20 } else { cur | 0x20 });
    }

    // ---------------------------------------------------------------------------------------
    // Mode 13h

    /// Plot a pixel in mode 13h.
    pub fn pset13(&mut self, x: u32, y: u32, c: u8) {
        if x < 320 && y < 200 {
            self.poke(0xA0000 + y * 320 + x, c);
        }
    }

    /// Copy a 320-pixel-wide bitmap to `A000:0000` (the double-buffer flip of every mode 13h
    /// game). How long it takes depends on the bus; if the beam overtakes it, it tears.
    pub fn blit13(&mut self, bmp: &Bitmap) {
        let n = (bmp.width * bmp.height).min(64000) as usize;
        self.poke_slice(0xA0000, &bmp.data[..n]);
    }

    // ---------------------------------------------------------------------------------------
    // Any graphics mode

    /// Plot a pixel in whatever graphics mode is active, using that mode's memory layout:
    /// mode 13h (and Mode X page 0), the 16-color planar modes, CGA, Tandy, Hercules graphics
    /// page 0, VBE 8-bit modes. `c` is the color index (or `0`/`1` on Hercules).
    pub fn pset(&mut self, x: u32, y: u32, c: u8) {
        let (w, h) = self.resolution();
        if x >= w || y >= h {
            return;
        }
        let mode = self.mode();
        let chained = self.video().as_vga().is_some_and(|v| v.seq[4] & 0x08 != 0);
        if self.vbe_mode().is_some() {
            let addr = y * self.vbe_pitch() + x;
            self.poke(crate::svga::LFB_BASE + addr, c);
            return;
        }
        match self.video() {
            Video::Vga(_) | Video::Svga(_) if mode == 0x13 => {
                if chained {
                    self.pset13(x, y, c);
                } else {
                    self.map_mask(1 << (x & 3));
                    self.poke(0xA0000 + y * (w / 4) + x / 4, c);
                }
            }
            Video::Vga(_) | Video::Svga(_) => self.planar_pset(x, y, c),
            Video::Cga(cga) => {
                let tandy = cga.kind == crate::cga::CgaKind::Tandy;
                let (addr, shift, bits) = match mode {
                    4 | 5 => (0x2000 * (y & 1) + (y >> 1) * 80 + x / 4, 6 - 2 * (x & 3), 2),
                    6 => (0x2000 * (y & 1) + (y >> 1) * 80 + x / 8, 7 - (x & 7), 1),
                    8 if tandy => (0x2000 * (y & 3) + (y >> 2) * 80 + x / 2, 4 - 4 * (x & 1), 4),
                    9 if tandy => (
                        0x2000 * (y & 3) + (y >> 2) * 160 + x / 2,
                        4 - 4 * (x & 1),
                        4,
                    ),
                    0x0A if tandy => {
                        let a = 0xB8000 + 0x2000 * (y & 3) + (y >> 2) * 160 + (x / 8) * 2;
                        let bit = 7 - (x & 7);
                        for plane in 0..2 {
                            let old = self.peek(a + plane);
                            let v = (old & !(1 << bit)) | (((c >> plane) & 1) << bit);
                            self.poke(a + plane, v);
                        }
                        return;
                    }
                    _ => return,
                };
                let a = 0xB8000 + addr;
                let mask = (((1u16 << bits) - 1) as u8) << shift;
                let old = self.peek(a);
                self.poke(a, (old & !mask) | ((c << shift) & mask));
            }
            Video::Hercules(_) => self.herc_pset(x, y, c != 0, 0),
        }
    }

    /// Plot a pixel in a 16-color planar mode (0Dh, 0Eh, 10h, 12h), the textbook way:
    /// set/reset supplies the color, the bit mask selects the pixel, and a dummy read loads
    /// the latches so the other seven pixels of the byte survive.
    pub fn planar_pset(&mut self, x: u32, y: u32, c: u8) {
        let (w, _) = self.resolution();
        let addr = 0xA0000 + y * (w / 8) + x / 8;
        self.gc(0, c & 0x0F);
        self.gc(1, 0x0F);
        self.gc(8, 0x80 >> (x & 7));
        self.peek(addr);
        self.poke(addr, 0xFF);
        self.gc(1, 0x00);
        self.gc(8, 0xFF);
    }

    /// Fill a rectangle in a 16-color planar mode with set/reset, eight pixels per byte write.
    pub fn planar_fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, c: u8) {
        let (sw, sh) = self.resolution();
        let x1 = (x + w).min(sw);
        let y1 = (y + h).min(sh);
        if x >= x1 || y >= y1 {
            return;
        }
        let pitch = sw / 8;
        self.gc(0, c & 0x0F);
        self.gc(1, 0x0F);
        let (bx0, bx1) = (x / 8, (x1 - 1) / 8);
        for yy in y..y1 {
            for bx in bx0..=bx1 {
                let mut m = 0xFFu8;
                if bx == bx0 {
                    m &= 0xFF >> (x & 7);
                }
                if bx == bx1 {
                    m &= 0xFF << (7 - ((x1 - 1) & 7));
                }
                let addr = 0xA0000 + yy * pitch + bx;
                if m != 0xFF {
                    self.gc(8, m);
                    self.peek(addr);
                } else {
                    self.gc(8, 0xFF);
                }
                self.poke(addr, 0xFF);
            }
        }
        self.gc(1, 0x00);
        self.gc(8, 0xFF);
    }

    /// Copy an 8-bit bitmap (colors 0-15) into a 16-color planar mode at a byte-aligned `x`,
    /// one plane at a time through the map mask. Assumes the BIOS row pitch; see
    /// [`Dos::planar_blit_at`] for virtual screens and off-screen memory.
    pub fn planar_blit(&mut self, bmp: &Bitmap, x: u32, y: u32) {
        let pitch = self.resolution().0 / 8;
        self.planar_blit_at(bmp, 0xA0000 + y * pitch + x / 8, pitch);
    }

    /// Copy an 8-bit bitmap (colors 0-15) to planar video memory at physical address `addr`,
    /// rows `pitch` bytes apart: for a widened offset register, or for tiles stored off-screen
    /// to be copied with latches later.
    pub fn planar_blit_at(&mut self, bmp: &Bitmap, addr: u32, pitch: u32) {
        for plane in 0..4u8 {
            self.map_mask(1 << plane);
            for row in 0..bmp.height {
                let bytes: Vec<u8> = bmp
                    .row(row)
                    .chunks(8)
                    .map(|c| {
                        c.iter()
                            .enumerate()
                            .fold(0u8, |acc, (i, &p)| acc | (((p >> plane) & 1) << (7 - i)))
                    })
                    .collect();
                self.poke_slice(addr + row * pitch, &bytes);
            }
        }
        self.map_mask(0x0F);
    }

    /// Plot a Hercules graphics pixel on `page`.
    pub fn herc_pset(&mut self, x: u32, y: u32, on: bool, page: u32) {
        if x >= 720 || y >= 348 {
            return;
        }
        let a = 0xB0000 + (page & 1) * 0x8000 + (y & 3) * 0x2000 + (y >> 2) * 90 + x / 8;
        let bit = 0x80 >> (x & 7);
        let old = self.peek(a);
        self.poke(a, if on { old | bit } else { old & !bit });
    }

    /// Copy a 1-bit-per-pixel image (bytes, MSB left, 90 bytes per row, 348 rows) to a
    /// Hercules page, de-interleaving the rows.
    pub fn herc_blit(&mut self, packed: &[u8], page: u32) {
        for y in 0..348u32 {
            let row = &packed[(y * 90) as usize..((y + 1) * 90) as usize];
            let a = 0xB0000 + (page & 1) * 0x8000 + (y & 3) * 0x2000 + (y >> 2) * 90;
            self.poke_slice(a, row);
        }
    }

    /// Copy a 320x200 bitmap of colors 0-3 to the CGA 4-color mode, interleaving even and odd
    /// lines into the two banks.
    pub fn cga_blit(&mut self, bmp: &Bitmap) {
        for y in 0..200u32.min(bmp.height) {
            let row = bmp.row(y);
            let bytes: Vec<u8> = row
                .chunks(4)
                .take(80)
                .map(|c| {
                    c.iter()
                        .enumerate()
                        .fold(0, |a, (i, &p)| a | ((p & 3) << (6 - 2 * i)))
                })
                .collect();
            self.poke_slice(0xB8000 + 0x2000 * (y & 1) + (y >> 1) * 80, &bytes);
        }
    }

    /// Copy a 320x200 bitmap of colors 0-15 to Tandy mode 9.
    pub fn tandy_blit(&mut self, bmp: &Bitmap) {
        for y in 0..200u32.min(bmp.height) {
            let row = bmp.row(y);
            let bytes: Vec<u8> = row
                .chunks(2)
                .take(160)
                .map(|c| (c[0] << 4) | (c.get(1).copied().unwrap_or(0) & 0x0F))
                .collect();
            self.poke_slice(0xB8000 + 0x2000 * (y & 3) + (y >> 2) * 160, &bytes);
        }
    }

    // ---------------------------------------------------------------------------------------
    // VBE

    /// Write one pixel in a VBE mode through the bank window: switch banks if the pixel lies
    /// in another one (the expensive part), then poke into `A000`. `color` is a palette index
    /// or a packed 15/16/32-bit value.
    pub fn vbe_pset_banked(&mut self, x: u32, y: u32, color: u32) {
        let Some(m) = self.vbe_mode() else { return };
        if x >= m.width || y >= m.height {
            return;
        }
        let bpp = m.bytes_per_pixel();
        let off = y * self.vbe_pitch() + x * bpp;
        let bank = off / crate::svga::BANK_SIZE;
        if bank != self.vbe_bank() {
            self.vbe_set_bank(bank);
        }
        let a = 0xA0000 + off % crate::svga::BANK_SIZE;
        match bpp {
            1 => self.poke(a, color as u8),
            2 => self.pokew(a, color as u16),
            _ => self.poke32(a, color),
        }
    }

    /// Write one pixel in a VBE mode through the linear framebuffer.
    pub fn vbe_pset_lfb(&mut self, x: u32, y: u32, color: u32) {
        let Some(m) = self.vbe_mode() else { return };
        if x >= m.width || y >= m.height {
            return;
        }
        let bpp = m.bytes_per_pixel();
        let a = crate::svga::LFB_BASE + y * self.vbe_pitch() + x * bpp;
        match bpp {
            1 => self.poke(a, color as u8),
            2 => self.pokew(a, color as u16),
            _ => self.poke32(a, color),
        }
    }

    /// Copy raw bytes to VBE memory at byte offset `off`, through the bank window: split into
    /// chunks at bank boundaries, switching banks in between.
    pub fn vbe_write_banked(&mut self, off: u32, data: &[u8]) {
        let mut done = 0usize;
        while done < data.len() {
            let o = off + done as u32;
            let bank = o / crate::svga::BANK_SIZE;
            if bank != self.vbe_bank() {
                self.vbe_set_bank(bank);
            }
            let in_bank = o % crate::svga::BANK_SIZE;
            let n = ((crate::svga::BANK_SIZE - in_bank) as usize).min(data.len() - done);
            self.poke_slice(0xA0000 + in_bank, &data[done..done + n]);
            done += n;
        }
    }

    /// Copy raw bytes to VBE memory at byte offset `off`, through the linear framebuffer.
    pub fn vbe_write_lfb(&mut self, off: u32, data: &[u8]) {
        self.poke_slice(crate::svga::LFB_BASE + off, data);
    }
}

/// Mode X: VGA 256-color graphics with chain-4 off, so all 256 KiB are usable as four pages of
/// planar memory, with page flipping, hardware scrolling, split screens and four-pixels-at-once
/// fills.
///
/// Pixel `(x, y)` lives in plane `x & 3` at offset `y * width/4 + x/4` of a page. To write it,
/// select the plane with the sequencer map mask, then poke the byte.
#[derive(Clone, Copy, Debug)]
pub struct ModeX {
    pub width: u32,
    pub height: u32,
    /// Bytes (per plane) of one page.
    pub page_size: u32,
    /// Page shown.
    pub visible: u32,
    /// Page drawn to.
    pub active: u32,
}

impl ModeX {
    /// Set 320x240 ("Mode X", square pixels at 60 Hz) when `height` is 240, else 320x200
    /// unchained ("Mode Y", 70 Hz). Uses the register sequence from Michael Abrash's articles;
    /// note the CRTC protect bit that has to be cleared before the timing registers take.
    pub fn set(dos: &mut Dos, height: u32) -> Option<Self> {
        if !dos.set_mode(0x13) {
            return None;
        }
        dos.seq(4, 0x06); // chain-4 off, odd/even off
        if height == 240 {
            dos.seq(0, 0x01); // synchronous reset
            dos.outb(0x3C2, 0xE3); // 25 MHz, 480-line sync polarity
            dos.seq(0, 0x03);
            dos.outb(0x3D4, 0x11);
            let v = dos.inb(0x3D5);
            dos.outb(0x3D5, v & 0x7F); // unprotect CRTC 0-7
            for (i, v) in [
                (0x06u8, 0x0Du8),
                (0x07, 0x3E),
                (0x09, 0x41),
                (0x10, 0xEA),
                (0x11, 0xAC),
                (0x12, 0xDF),
                (0x15, 0xE7),
                (0x16, 0x06),
            ] {
                dos.crtc(i, v);
            }
        }
        dos.crtc(0x14, 0x00); // dword mode off
        dos.crtc(0x17, 0xE3); // byte mode
        dos.map_mask(0x0F);
        dos.fill(0xA0000, 0x10000, 0);
        if height == 240 {
            dos.mouse_mut().set_range(0, 639, 0, 239);
        }
        let h = if height == 240 { 240 } else { 200 };
        let page_size = 320 / 4 * h;
        Some(Self {
            width: 320,
            height: h,
            page_size,
            visible: 0,
            active: 1,
        })
    }

    /// Number of pages that fit into 64 KiB per plane.
    pub fn pages(&self) -> u32 {
        0x10000 / self.page_size
    }

    /// Byte offset of `page`.
    pub fn page_offset(&self, page: u32) -> u32 {
        page * self.page_size
    }

    fn pitch(&self) -> u32 {
        self.width / 4
    }

    /// Plot a pixel on the active page.
    pub fn pset(&self, dos: &mut Dos, x: u32, y: u32, c: u8) {
        if x >= self.width || y >= self.height {
            return;
        }
        dos.map_mask(1 << (x & 3));
        dos.poke(
            0xA0000 + self.page_offset(self.active) + y * self.pitch() + x / 4,
            c,
        );
    }

    /// Fill a rectangle on the active page; the middle part writes four pixels per byte.
    pub fn fill_rect(&self, dos: &mut Dos, x: i32, y: i32, w: i32, h: i32, c: u8) {
        let x0 = x.max(0) as u32;
        let y0 = y.max(0) as u32;
        let x1 = ((x + w).max(0) as u32).min(self.width);
        let y1 = ((y + h).max(0) as u32).min(self.height);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let base = 0xA0000 + self.page_offset(self.active);
        let (b0, b1) = (x0 / 4, (x1 - 1) / 4);
        let lm = 0x0Fu8 << (x0 & 3) & 0x0F;
        let rm = 0x0Fu8 >> (3 - ((x1 - 1) & 3));
        for yy in y0..y1 {
            let row = base + yy * self.pitch();
            if b0 == b1 {
                dos.map_mask(lm & rm);
                dos.poke(row + b0, c);
                continue;
            }
            dos.map_mask(lm);
            dos.poke(row + b0, c);
            if b1 > b0 + 1 {
                dos.map_mask(0x0F);
                dos.fill(row + b0 + 1, (b1 - b0 - 1) as usize, c);
            }
            dos.map_mask(rm);
            dos.poke(row + b1, c);
        }
    }

    /// Clear the active page.
    pub fn clear(&self, dos: &mut Dos, c: u8) {
        dos.map_mask(0x0F);
        dos.fill(
            0xA0000 + self.page_offset(self.active),
            self.page_size as usize,
            c,
        );
    }

    /// Copy a bitmap to the active page, plane by plane (four passes, one map mask write each).
    /// `transparent` pixels are skipped (slower: pixel by pixel).
    pub fn blit(&self, dos: &mut Dos, bmp: &Bitmap, x: i32, y: i32, transparent: Option<u8>) {
        let base = 0xA0000 + self.page_offset(self.active);
        for plane in 0..4i32 {
            dos.map_mask(1 << plane);
            for row in 0..bmp.height as i32 {
                let dy = y + row;
                if dy < 0 || dy >= self.height as i32 {
                    continue;
                }
                // Source columns that land on this plane.
                let first = (plane - x).rem_euclid(4);
                let mut run: Vec<u8> = Vec::with_capacity(bmp.width as usize / 4 + 1);
                let mut run_start = None;
                let mut sx = first;
                while sx < bmp.width as i32 {
                    let dx = x + sx;
                    let c = bmp.get(sx, row);
                    if dx >= 0 && dx < self.width as i32 && Some(c) != transparent {
                        if run_start.is_none() {
                            run_start = Some(dx as u32 / 4);
                        }
                        run.push(c);
                    } else if let Some(s) = run_start.take() {
                        dos.poke_slice(base + dy as u32 * self.pitch() + s, &run);
                        run.clear();
                    }
                    sx += 4;
                }
                if let Some(s) = run_start {
                    dos.poke_slice(base + dy as u32 * self.pitch() + s, &run);
                }
            }
        }
        dos.map_mask(0x0F);
    }

    /// Copy `len` bytes (x4 planes) from one place in video memory to another with write mode 1:
    /// each read fills the latches with four pixels, each write stores them. Four times the
    /// bandwidth of a CPU copy; how Mode X games moved backgrounds around.
    pub fn copy_vram(&self, dos: &mut Dos, src: u32, dst: u32, len: u32) {
        dos.map_mask(0x0F);
        dos.write_mode(1);
        dos.copy_vram(0xA0000 + src, 0xA0000 + dst, len as usize);
        dos.write_mode(0);
    }

    /// Show the active page and draw on the next one. The start address is written now and
    /// latched by the CRTC at the next vertical retrace; waiting for that retrace makes sure
    /// we never draw on the page that is still being displayed.
    pub fn flip(&mut self, dos: &mut Dos) {
        self.visible = self.active;
        dos.set_start_address(self.page_offset(self.visible) as u16);
        dos.wait_vretrace();
        self.active = (self.active + 1) % 2;
        if self.active == self.visible {
            self.active = (self.active + 1) % 2;
        }
    }

    /// Like [`ModeX::flip`] but cycling through `n` pages (triple buffering).
    pub fn flip_n(&mut self, dos: &mut Dos, n: u32) {
        self.visible = self.active;
        dos.set_start_address(self.page_offset(self.visible) as u16);
        dos.wait_vretrace();
        self.active = (self.active + 1) % n.clamp(2, self.pages());
    }
}
