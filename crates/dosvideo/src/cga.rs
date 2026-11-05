//! IBM CGA and the Tandy 1000 / PCjr video subsystem, both built on the Motorola 6845 CRTC.
//!
//! CGA: 16 KiB at `B800:0000`, even scanlines in the first 8 KiB and odd ones in the second,
//! the 4-color modes with their two palettes and the "mode 5" cyan/red/white palette, the
//! 640x200 monochrome mode, 40/80-column text with blinking and the cursor, *snow* when the CPU
//! touches memory during display in 80-column text, composite artifact colors and the tweaked
//! 160x100 16-color text mode (all of it comes from the 6845 registers, nothing is special).
//!
//! Tandy: 32 KiB, four-way interleaved, plus the video gate array with 16 palette registers and
//! the 160x200 and 320x200 16-color and 640x200 4-color modes.

use crate::crt::{Beam, Scanout, Timing};
use crate::font::Font;
use crate::palette::{CGA, rgba};

/// The 6845's 18 registers.
#[derive(Clone, Debug, Default)]
pub(crate) struct Mc6845 {
    pub index: u8,
    pub r: [u8; 18],
}

impl Mc6845 {
    /// Derive raster timing. `cd` is dots per character clock. The 6845 has no blanking
    /// registers, so the visible border is taken as up to `border_h` dots / `border_v` lines
    /// next to the active area, stopping at the sync pulses.
    pub fn timing(
        &self,
        dot_clock_hz: f64,
        cd: u32,
        vsync_lines: u32,
        border_h: u32,
        border_v: u32,
    ) -> Timing {
        let r = &self.r;
        let h_total = (r[0] as u32 + 1) * cd;
        let h_active = (r[1] as u32).max(1) * cd;
        let hrs = r[2] as u32 * cd;
        let hre = hrs + (r[3] & 0x0F).max(1) as u32 * cd;
        let rows = (r[9] & 0x1F) as u32 + 1;
        let v_total = (r[4] as u32 & 0x7F) * rows + rows + (r[5] & 0x1F) as u32;
        let v_active = (r[6] as u32 & 0x7F) * rows;
        let vrs = (r[7] as u32 & 0x7F) * rows;
        let vre = vrs + vsync_lines;
        let h_blank_start = (h_active + border_h).min(hrs.max(h_active));
        let h_blank_end = h_total.saturating_sub(border_h).max(hre.min(h_total));
        let v_blank_start = (v_active + border_v).min(vrs.max(v_active));
        let v_blank_end = v_total.saturating_sub(border_v).max(vre.min(v_total));
        Timing {
            dot_clock_hz,
            h_total,
            h_active,
            h_blank_start,
            h_blank_end,
            h_retrace_start: hrs,
            h_retrace_end: hre,
            v_total,
            v_active,
            v_blank_start,
            v_blank_end,
            v_retrace_start: vrs,
            v_retrace_end: vre,
        }
        .normalized()
    }

    pub fn start(&self) -> u32 {
        (((self.r[12] & 0x3F) as u32) << 8) | self.r[13] as u32
    }

    pub fn cursor(&self) -> u32 {
        (((self.r[14] & 0x3F) as u32) << 8) | self.r[15] as u32
    }

    pub fn write(&mut self, v: u8) -> bool {
        let i = self.index as usize;
        if i < 16 {
            self.r[i] = v;
            matches!(i, 0..=7 | 9)
        } else {
            false
        }
    }

    pub fn read(&self) -> u8 {
        let i = self.index as usize;
        if (12..=17).contains(&i) { self.r[i] } else { 0 }
    }
}

/// 14.31818 MHz, the PC master clock; CGA dots are counted at this rate.
pub const CGA_DOT_CLOCK: f64 = 14_318_180.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CgaKind {
    Cga,
    Tandy,
}

/// What the adapter is plugged into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CgaMonitor {
    /// An RGBI monitor (IBM 5153): 16 crisp colors.
    #[default]
    Rgb,
    /// An NTSC composite monitor or TV: colors bleed, and fine patterns in the 640-pixel mode
    /// turn into artifact colors.
    Composite,
}

pub struct Cga {
    pub(crate) kind: CgaKind,
    pub(crate) vram: Vec<u8>,
    pub(crate) crtc: Mc6845,
    pub(crate) mode: u8,
    pub(crate) color: u8,
    pub(crate) ga_index: u8,
    pub(crate) ga: [u8; 0x20],
    pub(crate) monitor: CgaMonitor,
    /// Hue rotation of the composite decoder in degrees (the TV's tint knob).
    pub composite_hue: f32,
    /// Emit snow when the CPU accesses memory during display in 80-column text mode.
    pub snow: bool,
    snow_events: Vec<(u32, u32, u8)>,
    start_latched: u32,
    row_ma: u32,
    row_scan: u32,
    frames: u32,
    cached: Timing,
    pub(crate) timing_dirty: bool,
    pub(crate) bios_mode: u8,
    scratch: Vec<u8>,
}

impl Cga {
    pub fn new(kind: CgaKind) -> Self {
        let mem = match kind {
            CgaKind::Cga => 0x4000,
            CgaKind::Tandy => 0x8000,
        };
        let mut c = Self {
            kind,
            vram: vec![0; mem],
            crtc: Mc6845::default(),
            mode: 0,
            color: 0,
            ga_index: 0,
            ga: [0; 0x20],
            monitor: CgaMonitor::Rgb,
            composite_hue: 300.0,
            snow: kind == CgaKind::Cga,
            snow_events: Vec::new(),
            start_latched: 0,
            row_ma: 0,
            row_scan: 0,
            frames: 0,
            cached: Timing {
                dot_clock_hz: CGA_DOT_CLOCK,
                h_total: 912,
                h_active: 640,
                h_blank_start: 688,
                h_blank_end: 864,
                h_retrace_start: 720,
                h_retrace_end: 800,
                v_total: 262,
                v_active: 200,
                v_blank_start: 224,
                v_blank_end: 238,
                v_retrace_start: 224,
                v_retrace_end: 240,
            },
            timing_dirty: true,
            bios_mode: 3,
            scratch: Vec::new(),
        };
        c.set_mode(3);
        c
    }

    fn text(&self) -> bool {
        self.mode & 0x02 == 0
    }

    fn char_dots(&self) -> u32 {
        if self.mode & 0x01 != 0 { 8 } else { 16 }
    }

    pub(crate) fn timing(&mut self) -> Timing {
        let t = self
            .crtc
            .timing(CGA_DOT_CLOCK, self.char_dots(), 16, 48, 24);
        self.cached = t;
        t
    }

    fn mem_mask(&self) -> usize {
        self.vram.len() - 1
    }

    pub(crate) fn maps(&self, phys: u32) -> bool {
        (0xB8000..0xC0000).contains(&phys)
    }

    pub(crate) fn mem_write(&mut self, phys: u32, v: u8, beam: &Beam) -> bool {
        if !self.maps(phys) {
            return false;
        }
        let m = self.mem_mask();
        self.vram[(phys as usize - 0xB8000) & m] = v;
        self.maybe_snow(beam, v);
        true
    }

    pub(crate) fn mem_read(&mut self, phys: u32, beam: &Beam) -> Option<u8> {
        if !self.maps(phys) {
            return None;
        }
        let v = self.vram[(phys as usize - 0xB8000) & self.mem_mask()];
        self.maybe_snow(beam, v);
        Some(v)
    }

    fn maybe_snow(&mut self, beam: &Beam, byte: u8) {
        // The CGA gives the CPU the memory bus whenever it asks; in 80-column text the CRTC
        // then latches whatever byte is on the bus instead of the character it wanted.
        if self.snow
            && self.kind == CgaKind::Cga
            && self.text()
            && self.mode & 1 != 0
            && beam.display_active
        {
            let cell = beam.dot - beam.dot % 8;
            self.snow_events.push((beam.line, cell, byte));
        }
    }

    pub(crate) fn port_out(&mut self, port: u16, v: u8) {
        match port {
            0x3D0 | 0x3D2 | 0x3D4 | 0x3D6 => self.crtc.index = v & 0x1F,
            0x3D1 | 0x3D3 | 0x3D5 | 0x3D7 => {
                if self.crtc.write(v) {
                    self.timing_dirty = true;
                }
            }
            0x3D8 => {
                if (self.mode ^ v) & 0x01 != 0 {
                    self.timing_dirty = true;
                }
                self.mode = v;
            }
            0x3D9 => self.color = v,
            0x3DA if self.kind == CgaKind::Tandy => self.ga_index = v & 0x1F,
            0x3DE if self.kind == CgaKind::Tandy => {
                self.ga[self.ga_index as usize] = v;
            }
            _ => {}
        }
    }

    pub(crate) fn port_in(&mut self, port: u16, beam: &Beam) -> u8 {
        match port {
            0x3D1 | 0x3D3 | 0x3D5 | 0x3D7 => self.crtc.read(),
            0x3DA => {
                let mut s = 0xF0;
                if !beam.display_active {
                    s |= 0x01;
                }
                if beam.v_retrace {
                    s |= 0x08;
                }
                s
            }
            _ => 0xFF,
        }
    }

    pub(crate) fn logical_resolution(&self) -> (u32, u32) {
        let t = &self.cached;
        let rows = (self.crtc.r[9] & 0x1F) as u32 + 1;
        if self.text() {
            return (t.h_active / self.char_dots() * 8, t.v_active);
        }
        // Every scanline is its own pixel row: the row-scan counter selects the memory bank.
        let _ = rows;
        ((self.crtc.r[1] as u32) * self.pixels_per_char(), t.v_active)
    }

    fn pixels_per_char(&self) -> u32 {
        if self.kind == CgaKind::Tandy && self.ga[3] & 0x10 != 0 {
            4
        } else if self.kind == CgaKind::Tandy && self.ga[3] & 0x08 != 0 {
            8
        } else if self.mode & 0x10 != 0 {
            16
        } else {
            8
        }
    }

    pub(crate) fn text_geometry(&self) -> Option<(u32, u32, u32)> {
        let rows = (self.crtc.r[9] & 0x1F) as u32 + 1;
        self.text()
            .then(|| (self.crtc.r[1] as u32, self.crtc.r[6] as u32 & 0x7F, rows))
    }

    /// RGBI color of a 2-bit pixel in the CGA 4-color mode.
    fn cga4(&self, v: u8) -> u8 {
        if v == 0 {
            return self.color & 0x0F;
        }
        let pal: [u8; 3] = if self.mode & 0x04 != 0 {
            [3, 4, 7]
        } else if self.color & 0x20 != 0 {
            [3, 5, 7]
        } else {
            [2, 4, 6]
        };
        pal[v as usize - 1] | if self.color & 0x10 != 0 { 8 } else { 0 }
    }

    fn gfx_addr(&self, ma: u32, b: u32) -> usize {
        let bank_bits = if self.kind == CgaKind::Tandy { 3 } else { 1 };
        ((((self.row_scan & bank_bits) << 13) | ((ma * 2 + b) & 0x1FFF)) as usize) & self.mem_mask()
    }

    /// RGBI index of active dot `x` on the current line.
    fn dot_index(&self, x: u32) -> u8 {
        let cd = self.char_dots();
        let col = x / cd;
        let cx = x % cd;
        let ma = self.row_ma + col;
        if self.text() {
            let a = ((ma * 2) as usize) & self.mem_mask();
            let (ch, at) = (self.vram[a], self.vram[(a + 1) & self.mem_mask()]);
            return self.text_dot(ma, ch, at, cx * 8 / cd);
        }
        let tandy16 = self.kind == CgaKind::Tandy && self.ga[3] & 0x10 != 0;
        let tandy4 = self.kind == CgaKind::Tandy && self.ga[3] & 0x08 != 0;
        if tandy16 {
            // 2 bytes, 4 pixels of 4 bits.
            let p = cx * 4 / cd;
            let byte = self.vram[self.gfx_addr(ma, p / 2)];
            let v = if p & 1 == 0 { byte >> 4 } else { byte & 0x0F };
            return self.ga[0x10 + (v & self.ga[1] & 0x0F) as usize] & 0x0F;
        }
        if tandy4 {
            let p = cx * 8 / cd;
            let b0 = self.vram[self.gfx_addr(ma, 0)];
            let b1 = self.vram[self.gfx_addr(ma, 1)];
            let v = ((b0 >> (7 - p)) & 1) | (((b1 >> (7 - p)) & 1) << 1);
            return self.ga[0x10 + v as usize] & 0x0F;
        }
        if self.mode & 0x10 != 0 {
            // 640x200: 16 pixels per character, one dot each.
            let p = cx * 16 / cd;
            let byte = self.vram[self.gfx_addr(ma, p / 8)];
            return if (byte >> (7 - p % 8)) & 1 != 0 {
                self.color & 0x0F
            } else {
                0
            };
        }
        let p = cx * 8 / cd;
        let byte = self.vram[self.gfx_addr(ma, p / 4)];
        self.cga4((byte >> (6 - 2 * (p % 4))) & 3)
    }

    fn text_dot(&self, ma: u32, ch: u8, at: u8, px: u32) -> u8 {
        let font = Font::cga();
        let rs = self.row_scan;
        let g = if rs < 8 {
            font.glyph(ch)[rs as usize]
        } else {
            0
        };
        let mut on = (g >> (7 - px)) & 1 != 0;
        let fg = at & 0x0F;
        let mut bg = at >> 4;
        if self.mode & 0x20 != 0 {
            bg &= 7;
            if at & 0x80 != 0 && (self.frames / 16) & 1 != 0 {
                on = false;
            }
        }
        let r10 = self.crtc.r[10];
        let cursor_visible = match (r10 >> 5) & 3 {
            1 => false,
            0 => (self.frames / 8) & 1 == 0,
            2 => (self.frames / 4) & 1 == 0,
            _ => (self.frames / 16) & 1 == 0,
        };
        if cursor_visible
            && ma == self.crtc.cursor()
            && rs >= (r10 & 0x1F) as u32
            && rs <= (self.crtc.r[11] & 0x1F) as u32
        {
            on = true;
        }
        if on { fg } else { bg }
    }

    pub(crate) fn set_monitor(&mut self, m: CgaMonitor) {
        self.monitor = m;
    }

    /// Program a BIOS mode like `INT 10h, AH=00h`.
    pub(crate) fn set_mode(&mut self, mode: u8) -> bool {
        let keep = mode & 0x80 != 0;
        let mode = mode & 0x7F;
        let tandy = self.kind == CgaKind::Tandy;
        // R0..R15
        let text40: [u8; 16] = [
            0x38, 0x28, 0x2D, 0x0A, 0x1F, 0x06, 0x19, 0x1C, 0x02, 0x07, 0x06, 0x07, 0, 0, 0, 0,
        ];
        let text80: [u8; 16] = [
            0x71, 0x50, 0x5A, 0x0A, 0x1F, 0x06, 0x19, 0x1C, 0x02, 0x07, 0x06, 0x07, 0, 0, 0, 0,
        ];
        let gfx: [u8; 16] = [
            0x38, 0x28, 0x2D, 0x0A, 0x7F, 0x06, 0x64, 0x70, 0x02, 0x01, 0x06, 0x07, 0, 0, 0, 0,
        ];
        let gfx80x4: [u8; 16] = [
            0x71, 0x50, 0x5A, 0x0E, 0x3F, 0x06, 0x32, 0x38, 0x02, 0x03, 0x06, 0x07, 0, 0, 0, 0,
        ];
        let gfx40x4: [u8; 16] = [
            0x38, 0x28, 0x2B, 0x0A, 0x3F, 0x06, 0x32, 0x38, 0x02, 0x03, 0x06, 0x07, 0, 0, 0, 0,
        ];
        let (regs, m, col, ga3) = match mode {
            0 => (text40, 0x2C, 0x00, 0x00),
            1 => (text40, 0x28, 0x00, 0x00),
            2 => (text80, 0x2D, 0x00, 0x00),
            3 => (text80, 0x29, 0x00, 0x00),
            4 => (gfx, 0x2A, 0x30, 0x00),
            5 => (gfx, 0x2E, 0x30, 0x00),
            6 => (gfx, 0x1E, 0x0F, 0x00),
            8 if tandy => (gfx40x4, 0x1A, 0x00, 0x10),
            9 if tandy => (gfx80x4, 0x1B, 0x00, 0x10),
            0x0A if tandy => (gfx80x4, 0x1B, 0x00, 0x08),
            _ => return false,
        };
        let mut r = [0u8; 18];
        r[..16].copy_from_slice(&regs);
        self.crtc.r = r;
        self.mode = m;
        self.color = col;
        self.ga = [0; 0x20];
        self.ga[1] = 0x0F;
        self.ga[3] = ga3;
        for i in 0..16 {
            self.ga[0x10 + i] = i as u8;
        }
        if mode == 0x0A {
            self.ga[0x10..0x14].copy_from_slice(&[0, 3, 5, 15]);
        }
        if !keep {
            if mode <= 3 {
                for w in self.vram.as_chunks_mut::<2>().0 {
                    w[0] = b' ';
                    w[1] = 0x07;
                }
            } else {
                self.vram.fill(0);
            }
        }
        self.start_latched = 0;
        self.bios_mode = mode;
        self.timing_dirty = true;
        true
    }

    fn render_rgb(&mut self, line: u32, x0: u32, out: &mut [[u8; 4]]) {
        for (i, px) in out.iter_mut().enumerate() {
            let x = x0 + i as u32;
            let idx = self.snow_at(line, x).unwrap_or_else(|| self.dot_index(x));
            *px = rgba(CGA[idx as usize]);
        }
    }

    fn snow_at(&self, line: u32, x: u32) -> Option<u8> {
        if self.snow_events.is_empty() {
            return None;
        }
        let cell = x - x % 8;
        self.snow_events
            .iter()
            .rev()
            .find(|e| e.0 == line && e.1 == cell)
            .map(|&(_, _, b)| {
                let glyph = Font::cga().glyph(b);
                let rs = self.row_scan.min(7) as usize;
                if (glyph[rs] >> (7 - x % 8)) & 1 != 0 {
                    b & 0x0F
                } else {
                    b >> 4
                }
            })
    }

    fn render_composite(&mut self, line: u32, x0: u32, out: &mut [[u8; 4]]) {
        let n = out.len() as u32;
        let ha = self.cached.h_active;
        let lo = x0.saturating_sub(2);
        let hi = (x0 + n + 2).min(ha);
        let mut buf = std::mem::take(&mut self.scratch);
        buf.clear();
        for x in lo..hi {
            buf.push(self.snow_at(line, x).unwrap_or_else(|| self.dot_index(x)));
        }
        let color_burst = self.mode & 0x04 == 0;
        let hue = self.composite_hue.to_radians();
        let sig = |x: u32, idx: u8| -> f32 {
            let (y, i, q) = rgbi_yiq(idx);
            let th = std::f32::consts::FRAC_PI_2 * (x % 4) as f32;
            y + i * th.cos() + q * th.sin()
        };
        for (k, px) in out.iter_mut().enumerate() {
            let x = x0 + k as u32;
            let (mut y, mut ci, mut cq) = (0.0f32, 0.0f32, 0.0f32);
            for d in 0..4u32 {
                let xs = (x + d).saturating_sub(1);
                let xs = xs.clamp(lo, hi.saturating_sub(1).max(lo));
                let idx = buf.get((xs - lo) as usize).copied().unwrap_or(0);
                let s = sig(xs, idx);
                let th = std::f32::consts::FRAC_PI_2 * (xs % 4) as f32 + hue;
                y += s;
                ci += s * th.cos();
                cq += s * th.sin();
            }
            y /= 4.0;
            ci /= 2.0;
            cq /= 2.0;
            if !color_burst {
                ci = 0.0;
                cq = 0.0;
            }
            let r = y + 0.956 * ci + 0.621 * cq;
            let g = y - 0.272 * ci - 0.647 * cq;
            let b = y - 1.106 * ci + 1.703 * cq;
            let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            *px = [to8(r), to8(g), to8(b), 255];
        }
        self.scratch = buf;
    }
}

fn rgbi_yiq(idx: u8) -> (f32, f32, f32) {
    let c = CGA[(idx & 0x0F) as usize];
    let (r, g, b) = (
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
    );
    (
        0.299 * r + 0.587 * g + 0.114 * b,
        0.596 * r - 0.274 * g - 0.322 * b,
        0.211 * r - 0.523 * g + 0.312 * b,
    )
}

impl Scanout for Cga {
    fn frame_start(&mut self) {
        self.row_ma = self.start_latched;
        self.row_scan = 0;
        self.frames = self.frames.wrapping_add(1);
    }

    fn line_start(&mut self, line: u32) {
        self.snow_events.retain(|e| e.0 >= line);
        if line == 0 || line >= self.cached.v_active {
            return;
        }
        let max = (self.crtc.r[9] & 0x1F) as u32;
        if self.row_scan >= max {
            self.row_scan = 0;
            self.row_ma = (self.row_ma + self.crtc.r[1] as u32) & 0x3FFF;
        } else {
            self.row_scan += 1;
        }
    }

    fn vretrace_start(&mut self) {
        self.start_latched = self.crtc.start();
    }

    fn render_active(&mut self, line: u32, x0: u32, out: &mut [[u8; 4]]) {
        if self.mode & 0x08 == 0 {
            out.fill([0, 0, 0, 255]);
            return;
        }
        match self.monitor {
            CgaMonitor::Rgb => self.render_rgb(line, x0, out),
            CgaMonitor::Composite => self.render_composite(line, x0, out),
        }
    }

    fn border(&self) -> [u8; 4] {
        if self.mode & 0x08 == 0 {
            return [0, 0, 0, 255];
        }
        let idx = if self.kind == CgaKind::Tandy && self.ga[3] & 0x18 != 0 {
            self.ga[2] & 0x0F
        } else if self.mode & 0x10 != 0 {
            0
        } else {
            self.color & 0x0F
        };
        if self.monitor == CgaMonitor::Composite && self.mode & 0x04 != 0 {
            let y = (rgbi_yiq(idx).0 * 255.0) as u8;
            return [y, y, y, 255];
        }
        rgba(CGA[idx as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cga_text_and_graphics_timing() {
        let mut c = Cga::new(CgaKind::Cga);
        let t = c.timing();
        assert_eq!(
            (t.h_total, t.h_active, t.v_total, t.v_active),
            (912, 640, 262, 200)
        );
        assert!((t.refresh_hz() - 59.92).abs() < 0.05);
        c.set_mode(4);
        let t = c.timing();
        assert_eq!(
            (t.h_total, t.h_active, t.v_total, t.v_active),
            (912, 640, 262, 200)
        );
        assert_eq!(c.logical_resolution(), (320, 200));
    }

    #[test]
    fn mode4_interleave_and_palette() {
        let mut c = Cga::new(CgaKind::Cga);
        c.set_mode(4);
        c.timing();
        c.vram[0] = 0b00_01_10_11; // leftmost pixel in the top bits
        c.frame_start();
        assert_eq!(c.dot_index(0), 0);
        assert_eq!(c.dot_index(2), 3 | 8); // light cyan (palette 1, intensity)
        assert_eq!(c.dot_index(4), 5 | 8); // light magenta
        assert_eq!(c.dot_index(6), 7 | 8);
        // Line 1 comes from the odd bank at 0x2000.
        c.vram[0x2000] = 0xFF;
        c.line_start(1);
        assert_eq!(c.dot_index(0), 15);
    }

    #[test]
    fn tandy_16_color_mode() {
        let mut c = Cga::new(CgaKind::Tandy);
        assert!(c.set_mode(9));
        c.timing();
        assert_eq!(c.logical_resolution(), (320, 200));
        c.vram[0] = 0x4E;
        c.frame_start();
        assert_eq!(c.dot_index(0), 4);
        assert_eq!(c.dot_index(2), 14);
        assert!(!Cga::new(CgaKind::Cga).set_mode(9));
    }
}
