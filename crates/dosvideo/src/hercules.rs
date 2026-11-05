//! Hercules Graphics Card: MDA-compatible 80x25 monochrome text plus a 720x348 graphics mode
//! with two pages.
//!
//! The quirks a Hercules program had to know: graphics must first be *allowed* through the
//! configuration switch (port 0x3BF), the second page only exists when it is enabled there too
//! (it overlaps a CGA's memory at B800), scanlines are interleaved four ways (`0x2000` apart),
//! the vertical-retrace bit in the status port is *inverted* compared to CGA, and there is no
//! BIOS support for the graphics mode: you program the 6845 yourself.

use crate::cga::Mc6845;
use crate::crt::{Beam, Scanout, Timing};
use crate::font::Font;
use crate::palette::{Phosphor, rgba};

/// 16.257 MHz.
pub const HGC_DOT_CLOCK: f64 = 16_257_000.0;

pub struct Hercules {
    pub(crate) vram: Vec<u8>,
    pub(crate) crtc: Mc6845,
    pub(crate) mode: u8,
    pub(crate) config: u8,
    pub phosphor: Phosphor,
    start_latched: u32,
    row_ma: u32,
    row_scan: u32,
    frames: u32,
    cached: Timing,
    pub(crate) timing_dirty: bool,
    pub(crate) bios_mode: u8,
}

/// 6845 values for the 720x348 graphics mode.
pub const HGC_GRAPHICS_CRTC: [u8; 12] = [
    0x35, 0x2D, 0x2E, 0x07, 0x5B, 0x02, 0x57, 0x57, 0x02, 0x03, 0x00, 0x00,
];
/// 6845 values for 80x25 text.
pub const HGC_TEXT_CRTC: [u8; 12] = [
    0x61, 0x50, 0x52, 0x0F, 0x19, 0x06, 0x19, 0x19, 0x02, 0x0D, 0x0B, 0x0C,
];

impl Hercules {
    pub fn new(phosphor: Phosphor) -> Self {
        let mut h = Self {
            vram: vec![0; 0x10000],
            crtc: Mc6845::default(),
            mode: 0x28,
            config: 0,
            phosphor,
            start_latched: 0,
            row_ma: 0,
            row_scan: 0,
            frames: 0,
            cached: Mc6845::default().timing(HGC_DOT_CLOCK, 9, 16, 9, 8),
            timing_dirty: true,
            bios_mode: 7,
        };
        h.set_mode(7);
        h
    }

    fn graphics(&self) -> bool {
        self.mode & 0x02 != 0 && self.config & 0x01 != 0
    }

    fn char_dots(&self) -> u32 {
        if self.graphics() { 16 } else { 9 }
    }

    pub(crate) fn timing(&mut self) -> Timing {
        let t = self.crtc.timing(HGC_DOT_CLOCK, self.char_dots(), 16, 18, 8);
        self.cached = t;
        t
    }

    pub(crate) fn maps(&self, phys: u32) -> bool {
        (0xB0000..0xB8000).contains(&phys)
            || (self.config & 0x02 != 0 && (0xB8000..0xC0000).contains(&phys))
    }

    pub(crate) fn mem_write(&mut self, phys: u32, v: u8) -> bool {
        if !self.maps(phys) {
            return false;
        }
        self.vram[(phys - 0xB0000) as usize & 0xFFFF] = v;
        true
    }

    pub(crate) fn mem_read(&mut self, phys: u32) -> Option<u8> {
        self.maps(phys)
            .then(|| self.vram[(phys - 0xB0000) as usize & 0xFFFF])
    }

    pub(crate) fn port_out(&mut self, port: u16, v: u8) {
        match port {
            0x3B0 | 0x3B2 | 0x3B4 | 0x3B6 => self.crtc.index = v & 0x1F,
            0x3B1 | 0x3B3 | 0x3B5 | 0x3B7 => {
                if self.crtc.write(v) {
                    self.timing_dirty = true;
                }
            }
            0x3B8 => {
                let was = self.graphics();
                self.mode = v;
                if was != self.graphics() {
                    self.timing_dirty = true;
                }
            }
            0x3BF => {
                let was = self.graphics();
                self.config = v & 0x03;
                if was != self.graphics() {
                    self.timing_dirty = true;
                }
            }
            _ => {}
        }
    }

    pub(crate) fn port_in(&mut self, port: u16, beam: &Beam) -> u8 {
        match port {
            0x3B1 | 0x3B3 | 0x3B5 | 0x3B7 => self.crtc.read(),
            0x3BA => {
                // Bit 0: horizontal retrace, bit 3: video dot, bit 7: vertical retrace *low*.
                let mut s = 0x70;
                if beam.h_retrace {
                    s |= 0x01;
                }
                if !beam.v_retrace {
                    s |= 0x80;
                }
                s
            }
            _ => 0xFF,
        }
    }

    pub(crate) fn logical_resolution(&self) -> (u32, u32) {
        let t = &self.cached;
        if self.graphics() {
            (self.crtc.r[1] as u32 * 16, t.v_active)
        } else {
            (t.h_active, t.v_active)
        }
    }

    pub(crate) fn text_geometry(&self) -> Option<(u32, u32, u32)> {
        (!self.graphics()).then(|| {
            (
                self.crtc.r[1] as u32,
                self.crtc.r[6] as u32 & 0x7F,
                (self.crtc.r[9] & 0x1F) as u32 + 1,
            )
        })
    }

    /// Text mode 7 (the only BIOS mode).
    pub(crate) fn set_mode(&mut self, mode: u8) -> bool {
        if mode & 0x7F != 7 {
            return false;
        }
        self.set_crtc(&HGC_TEXT_CRTC);
        self.mode = 0x28;
        if mode & 0x80 == 0 {
            for w in self.vram[..0x8000].as_chunks_mut::<2>().0 {
                w[0] = b' ';
                w[1] = 0x07;
            }
        }
        self.bios_mode = 7;
        self.timing_dirty = true;
        true
    }

    pub(crate) fn set_crtc(&mut self, regs: &[u8; 12]) {
        self.crtc.r[..12].copy_from_slice(regs);
        self.timing_dirty = true;
    }

    fn px(&self, intensity: u8) -> [u8; 4] {
        match intensity {
            0 => [0, 0, 0, 255],
            1 => rgba(self.phosphor.normal()),
            _ => rgba(self.phosphor.bright()),
        }
    }
}

impl Scanout for Hercules {
    fn frame_start(&mut self) {
        self.row_ma = self.start_latched;
        self.row_scan = 0;
        self.frames = self.frames.wrapping_add(1);
    }

    fn line_start(&mut self, line: u32) {
        if line == 0 || line >= self.cached.v_active {
            return;
        }
        if self.row_scan >= (self.crtc.r[9] & 0x1F) as u32 {
            self.row_scan = 0;
            self.row_ma = (self.row_ma + self.crtc.r[1] as u32) & 0x3FFF;
        } else {
            self.row_scan += 1;
        }
    }

    fn vretrace_start(&mut self) {
        self.start_latched = self.crtc.start();
    }

    fn render_active(&mut self, _line: u32, x0: u32, out: &mut [[u8; 4]]) {
        if self.mode & 0x08 == 0 {
            out.fill([0, 0, 0, 255]);
            return;
        }
        if self.graphics() {
            let page = if self.mode & 0x80 != 0 { 0x8000 } else { 0 };
            let bank = ((self.row_scan & 3) << 13) as usize;
            let on = self.px(2);
            let off = self.px(0);
            for (i, p) in out.iter_mut().enumerate() {
                let x = x0 + i as u32;
                let ma = self.row_ma + x / 16;
                let b = (x % 16) / 8;
                let a = page + bank + (((ma * 2 + b) & 0x1FFF) as usize);
                let byte = self.vram[a & 0xFFFF];
                *p = if (byte >> (7 - x % 8)) & 1 != 0 {
                    on
                } else {
                    off
                };
            }
            return;
        }
        let font = Font::ega();
        let rs = self.row_scan;
        let blink_en = self.mode & 0x20 != 0;
        let blink_on = (self.frames / 16) & 1 == 0;
        let r10 = self.crtc.r[10];
        let cur_on = (r10 >> 5) & 3 != 1 && (self.frames / 8) & 1 == 0;
        let cur = self.crtc.cursor();
        for (i, p) in out.iter_mut().enumerate() {
            let x = x0 + i as u32;
            let col = x / 9;
            let cx = x % 9;
            let ma = self.row_ma + col;
            let a = ((ma * 2) as usize) & 0x0FFF;
            let ch = self.vram[a];
            let at = self.vram[a + 1];
            let g = if rs < 14 {
                font.glyph(ch)[rs as usize]
            } else {
                0
            };
            let mut on = if cx < 8 {
                (g >> (7 - cx)) & 1 != 0
            } else {
                // Line-drawing characters extend into the 9th column.
                (0xC0..=0xDF).contains(&ch) && g & 1 != 0
            };
            if (at & 0x07) == 1 && rs == 12 {
                on = true; // underline
            }
            if blink_en && at & 0x80 != 0 && !blink_on {
                on = false;
            }
            let bright = at & 0x08 != 0;
            let (fg, bg) = match at & 0x77 {
                0x00 => (0, 0),
                0x70 => (0, if !blink_en && at & 0x80 != 0 { 2 } else { 1 }),
                _ => (if bright { 2 } else { 1 }, 0),
            };
            let mut level = if on { fg } else { bg };
            if cur_on
                && ma == cur
                && rs >= (r10 & 0x1F) as u32
                && rs <= (self.crtc.r[11] & 0x1F) as u32
            {
                level = 1;
            }
            *p = self.px(level);
        }
    }

    fn border(&self) -> [u8; 4] {
        [0, 0, 0, 255]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_graphics_timing() {
        let mut h = Hercules::new(Phosphor::Green);
        let t = h.timing();
        assert_eq!((t.h_active, t.v_active), (720, 350));
        assert!((t.refresh_hz() - 49.8).abs() < 0.3, "{}", t.refresh_hz());
        h.set_crtc(&HGC_GRAPHICS_CRTC);
        h.port_out(0x3B8, 0x0A);
        let t = h.timing();
        // Graphics not allowed by the config switch yet: the card still clocks 9-dot text
        // characters, so the graphics CRTC values give a 405-dot-wide garbage display.
        assert_eq!(t.h_active, 45 * 9);
        h.port_out(0x3BF, 0x01);
        h.port_out(0x3B8, 0x0A);
        let t = h.timing();
        assert_eq!((t.h_active, t.v_active), (720, 348));
        assert_eq!(h.logical_resolution(), (720, 348));
    }

    #[test]
    fn second_page_needs_config() {
        let mut h = Hercules::new(Phosphor::Amber);
        assert!(!h.mem_write(0xB8000, 1));
        h.port_out(0x3BF, 0x03);
        assert!(h.mem_write(0xB8000, 1));
        assert_eq!(h.vram[0x8000], 1);
    }
}
