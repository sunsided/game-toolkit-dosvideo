//! Text mode: character/attribute cells at `B800:0000` (color) or `B000:0000` (monochrome),
//! the hardware cursor, blinking, box drawing, and BIOS-style teletype output.
//!
//! An attribute byte is `background << 4 | foreground` (see [`crate::palette::color`]); bit 7
//! blinks the character unless blinking is turned off, in which case it selects bright
//! backgrounds instead.

use crate::font::cp437;
use crate::machine::{Dos, Video};

/// Build an attribute byte.
pub const fn attr(fg: u8, bg: u8) -> u8 {
    (bg << 4) | (fg & 0x0F)
}

/// Single-line box drawing characters: `┌ ┐ └ ┘ ─ │`.
pub const BOX_SINGLE: [u8; 6] = [0xDA, 0xBF, 0xC0, 0xD9, 0xC4, 0xB3];
/// Double-line box drawing characters: `╔ ╗ ╚ ╝ ═ ║`.
pub const BOX_DOUBLE: [u8; 6] = [0xC9, 0xBB, 0xC8, 0xBC, 0xCD, 0xBA];

impl Dos {
    /// Segment of text memory: `B000` for monochrome adapters, `B800` otherwise.
    pub fn text_base(&self) -> u32 {
        match self.video() {
            Video::Hercules(_) => 0xB0000,
            Video::Vga(v) if v.gc[6] & 0x0C == 0x08 => 0xB0000,
            _ => 0xB8000,
        }
    }

    /// `(columns, rows)` of the current text mode (80x25 when not in text mode).
    pub fn text_size(&self) -> (u32, u32) {
        self.video()
            .text_geometry()
            .map_or((80, 25), |(c, r, _)| (c, r))
    }

    fn cell_addr(&self, x: u32, y: u32) -> u32 {
        let (cols, _) = self.text_size();
        let page = self.start_address() as u32 * 2;
        self.text_base() + (page + (y * cols + x) * 2) % 0x8000
    }

    /// Put one character (CP437 code) with an attribute.
    pub fn text_put(&mut self, x: u32, y: u32, ch: u8, attr: u8) {
        let (cols, rows) = self.text_size();
        if x < cols && y < rows {
            let a = self.cell_addr(x, y);
            self.pokew(a, ch as u16 | (attr as u16) << 8);
        }
    }

    /// Read a cell back: `(character, attribute)`.
    pub fn text_get(&mut self, x: u32, y: u32) -> (u8, u8) {
        let a = self.cell_addr(x, y);
        let w = self.peekw(a);
        (w as u8, (w >> 8) as u8)
    }

    /// Print a string at a position (Unicode is mapped to CP437, so `"╔═╗"` works).
    pub fn text_print(&mut self, x: u32, y: u32, s: &str, attr: u8) {
        for (i, ch) in cp437(s).into_iter().enumerate() {
            self.text_put(x + i as u32, y, ch, attr);
        }
    }

    /// Print a string centered on row `y`.
    pub fn text_center(&mut self, y: u32, s: &str, attr: u8) {
        let (cols, _) = self.text_size();
        let n = s.chars().count() as u32;
        self.text_print(cols.saturating_sub(n) / 2, y, s, attr);
    }

    /// Fill a rectangle of cells.
    pub fn text_fill(&mut self, x: u32, y: u32, w: u32, h: u32, ch: u8, attr: u8) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.text_put(xx, yy, ch, attr);
            }
        }
    }

    /// Clear the screen to blanks in `attr`.
    pub fn text_clear(&mut self, attr: u8) {
        let (c, r) = self.text_size();
        self.text_fill(0, 0, c, r, b' ', attr);
    }

    /// Draw a box with [`BOX_SINGLE`] or [`BOX_DOUBLE`] characters, filling the inside.
    pub fn text_box(&mut self, x: u32, y: u32, w: u32, h: u32, attr: u8, chars: [u8; 6]) {
        if w < 2 || h < 2 {
            return;
        }
        let [tl, tr, bl, br, hz, vt] = chars;
        self.text_put(x, y, tl, attr);
        self.text_put(x + w - 1, y, tr, attr);
        self.text_put(x, y + h - 1, bl, attr);
        self.text_put(x + w - 1, y + h - 1, br, attr);
        for xx in x + 1..x + w - 1 {
            self.text_put(xx, y, hz, attr);
            self.text_put(xx, y + h - 1, hz, attr);
        }
        for yy in y + 1..y + h - 1 {
            self.text_put(x, yy, vt, attr);
            self.text_put(x + w - 1, yy, vt, attr);
            for xx in x + 1..x + w - 1 {
                self.text_put(xx, yy, b' ', attr);
            }
        }
    }

    /// Move the hardware cursor (CRTC 0x0E/0x0F).
    pub fn set_cursor(&mut self, x: u32, y: u32) {
        let (cols, _) = self.text_size();
        let pos = self.start_address() as u32 + y * cols + x;
        self.crtc(0x0E, (pos >> 8) as u8);
        self.crtc(0x0F, pos as u8);
        self.tty = (x, y);
    }

    /// Set the cursor shape: first and last scanline of the character cell it covers.
    pub fn cursor_shape(&mut self, start: u8, end: u8) {
        self.crtc(0x0A, start & 0x1F);
        self.crtc(0x0B, end & 0x1F);
    }

    /// Hide the hardware cursor.
    pub fn hide_cursor(&mut self) {
        let cga_like = matches!(self.video(), Video::Cga(_) | Video::Hercules(_));
        // 6845: cursor mode bits 5-6 = 01 means off. VGA: bit 5 disables.
        self.crtc(0x0A, if cga_like { 0x20 } else { 0x20 | 0x1F });
    }

    /// Blinking on (attribute bit 7 blinks) or off (bit 7 is background intensity, 16
    /// background colors).
    pub fn set_blink(&mut self, on: bool) {
        match self.video() {
            Video::Cga(c) => {
                let m = c.mode;
                self.outb(0x3D8, if on { m | 0x20 } else { m & !0x20 });
            }
            Video::Hercules(h) => {
                let m = h.mode;
                self.outb(0x3B8, if on { m | 0x20 } else { m & !0x20 });
            }
            Video::Vga(_) | Video::Svga(_) => {
                let cur = self.video().as_vga().map_or(0, |v| v.attr[0x10]);
                self.attr(0x10 | 0x20, if on { cur | 0x08 } else { cur & !0x08 });
            }
        }
    }

    /// BIOS teletype output (`INT 10h, AH=0Eh`): print at the cursor, handle `\r`, `\n` and
    /// backspace, wrap, and scroll the screen up at the bottom.
    pub fn bios_print(&mut self, s: &str, attr: u8) {
        let (cols, rows) = self.text_size();
        let (mut x, mut y) = self.tty;
        for ch in cp437(s) {
            match ch {
                b'\r' => x = 0,
                b'\n' => {
                    x = 0;
                    y += 1;
                }
                0x08 => x = x.saturating_sub(1),
                _ => {
                    self.text_put(x, y, ch, attr);
                    x += 1;
                    if x >= cols {
                        x = 0;
                        y += 1;
                    }
                }
            }
            if y >= rows {
                self.text_scroll_up(attr);
                y = rows - 1;
            }
        }
        self.set_cursor(x, y);
    }

    /// Scroll the whole text screen up one line (`INT 10h, AH=06h`), blanking the last row.
    /// On a CGA in 80-column mode the BIOS turns the video off while it copies, to avoid snow,
    /// and the screen flickers. So does this.
    pub fn text_scroll_up(&mut self, attr: u8) {
        let (cols, rows) = self.text_size();
        self.text_scroll_window(0, 0, cols, rows, 1, attr);
    }

    /// Scroll a window of the text screen up by `lines` (`INT 10h, AH=06h`), blanking the rows
    /// that come in at the bottom. `lines == 0` clears the window, like the BIOS.
    pub fn text_scroll_window(&mut self, x: u32, y: u32, w: u32, h: u32, lines: u32, attr: u8) {
        let (cols, rows) = self.text_size();
        let w = w.min(cols.saturating_sub(x));
        let h = h.min(rows.saturating_sub(y));
        if w == 0 || h == 0 {
            return;
        }
        let lines = if lines == 0 { h } else { lines.min(h) };
        let cga80 = matches!(self.video(), Video::Cga(c) if c.mode & 0x03 == 0x01 && c.snow);
        let mode = self.video().as_cga().map_or(0, |c| c.mode);
        if cga80 {
            self.wait_vretrace();
            self.outb(0x3D8, mode & !0x08);
        }
        let mut buf = vec![0u8; (w * 2) as usize];
        for row in y..y + h - lines {
            let src = self.cell_addr(x, row + lines);
            self.peek_slice(src, &mut buf);
            let dst = self.cell_addr(x, row);
            self.poke_slice(dst, &buf);
        }
        self.text_fill(x, y + h - lines, w, lines, b' ', attr);
        if cga80 {
            self.outb(0x3D8, mode);
        }
    }
}
