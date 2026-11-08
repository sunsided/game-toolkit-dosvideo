//! IBM EGA and VGA, at the register level.
//!
//! 256 KiB of video memory in four 64 KiB planes, the Sequencer (map mask, chain-4, odd/even),
//! the Graphics Controller (latches, write modes 0-3, read modes 0-1, set/reset, bit mask, ALU
//! functions), the CRT Controller (all timing derived from the registers, start address latched
//! at vertical retrace, offset, line compare split screen, max scan line, byte/word/dword
//! addressing, CGA/Hercules row interleave), the Attribute Controller (palette, flip-flop,
//! pel panning, overscan, 8-bit mode) and, on VGA, the DAC with its 6-bit components.
//!
//! Tweaked modes (Mode X, 320x400, 360x240, ...) are not special cased anywhere: program the
//! registers like a DOS program would and the timing and memory layout follow.

use crate::crt::{Beam, Scanout, Timing};
use crate::font::Font;
use crate::palette::{self, dac6_to_8, rgba};

/// Which adapter the planar core emulates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VgaKind {
    /// IBM EGA with 256 KiB: write-only registers, no DAC (64 fixed colors, or 16 RGBI colors in
    /// the 200-line modes), no 256-color mode, no chain-4.
    Ega,
    /// IBM VGA.
    Vga,
}

const VRAM_ADDRS: usize = 0x10000;

pub struct Vga {
    pub(crate) kind: VgaKind,
    /// Four planes interleaved: byte `addr * 4 + plane`.
    pub(crate) vram: Vec<u8>,
    pub(crate) misc: u8,
    pub(crate) seq_index: u8,
    pub(crate) seq: [u8; 5],
    pub(crate) crtc_index: u8,
    pub(crate) crtc: [u8; 25],
    pub(crate) gc_index: u8,
    pub(crate) gc: [u8; 9],
    pub(crate) attr_index: u8,
    /// `false`: the next write to 0x3C0 is an index; `true`: it is data.
    pub(crate) attr_flip: bool,
    pub(crate) attr: [u8; 21],
    pub(crate) dac: [[u8; 3]; 256],
    dac_rgba: [[u8; 4]; 256],
    dac_w: u8,
    dac_r: u8,
    dac_sub: u8,
    dac_buf: [u8; 3],
    dac_reading: bool,
    pub(crate) pel_mask: u8,
    /// 8-bit DAC components (VESA function 08h); the classic VGA DAC is 6-bit.
    pub(crate) dac_8bit: bool,
    latch: [u8; 4],
    lut8: [u8; 256],
    lut8_dirty: bool,
    // CRTC scanout counters.
    start_latched: u32,
    row_ma: u32,
    row_scan: u32,
    doubled: bool,
    split_active: bool,
    frames: u32,
    cached: Timing,
    pub(crate) timing_dirty: bool,
    /// The BIOS mode number last set (used by the mouse driver and BIOS helpers).
    pub(crate) bios_mode: u8,
}

/// How the CRTC turns its memory address counter into a plane address.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AddrMode {
    Byte,
    Word,
    Dword,
}

impl Vga {
    pub fn new(kind: VgaKind) -> Self {
        let mut v = Self {
            kind,
            vram: vec![0; VRAM_ADDRS * 4],
            misc: 0x63,
            seq_index: 0,
            seq: [0; 5],
            crtc_index: 0,
            crtc: [0; 25],
            gc_index: 0,
            gc: [0; 9],
            attr_index: 0x20,
            attr_flip: false,
            attr: [0; 21],
            dac: palette::vga_default_dac(),
            dac_rgba: [[0, 0, 0, 255]; 256],
            dac_w: 0,
            dac_r: 0,
            dac_sub: 0,
            dac_buf: [0; 3],
            dac_reading: false,
            pel_mask: 0xFF,
            dac_8bit: false,
            latch: [0; 4],
            lut8: [0; 256],
            lut8_dirty: true,
            start_latched: 0,
            row_ma: 0,
            row_scan: 0,
            doubled: false,
            split_active: false,
            frames: 0,
            cached: Timing {
                dot_clock_hz: 25_175_000.0,
                h_total: 800,
                h_active: 640,
                h_blank_start: 640,
                h_blank_end: 800,
                h_retrace_start: 656,
                h_retrace_end: 752,
                v_total: 449,
                v_active: 400,
                v_blank_start: 400,
                v_blank_end: 449,
                v_retrace_start: 412,
                v_retrace_end: 414,
            },
            timing_dirty: true,
            bios_mode: 0x03,
        };
        v.refresh_dac_all();
        v.set_mode(0x03);
        v
    }

    pub fn kind(&self) -> VgaKind {
        self.kind
    }

    // ---------------------------------------------------------------------------------------
    // Timing

    pub(crate) fn timing(&mut self) -> Timing {
        let t = self.compute_timing();
        self.cached = t;
        t
    }

    fn compute_timing(&self) -> Timing {
        let c = &self.crtc;
        let ega = self.kind == VgaKind::Ega;
        let clocks: [f64; 4] = if ega {
            [14_318_180.0, 16_257_000.0, 14_318_180.0, 16_257_000.0]
        } else {
            [25_175_000.0, 28_322_000.0, 25_175_000.0, 28_322_000.0]
        };
        let dot_clock_hz = clocks[((self.misc >> 2) & 3) as usize];
        let cw = if self.seq[1] & 1 != 0 { 8 } else { 9 };
        let div = if self.seq[1] & 8 != 0 { 2 } else { 1 };
        let cd = cw * div;
        let (h_add, v_add) = if ega { (2, 1) } else { (5, 2) };
        let h_total_c = c[0] as u32 + h_add;
        let h_active_c = c[1] as u32 + 1;
        let hbs_c = c[2] as u32;
        let hbe_v = if ega {
            (c[3] & 0x1F) as u32
        } else {
            (c[3] & 0x1F) as u32 | (((c[5] >> 7) & 1) as u32) << 5
        };
        let hbe_mask = if ega { 0x1F } else { 0x3F };
        let hbe_c = (hbs_c + 1..=hbs_c + 64)
            .find(|x| x & hbe_mask == hbe_v)
            .unwrap_or(h_total_c);
        let hrs_c = c[4] as u32;
        let hre_v = (c[5] & 0x1F) as u32;
        let hre_c = (hrs_c + 1..=hrs_c + 32)
            .find(|x| x & 0x1F == hre_v)
            .unwrap_or(hrs_c + 1);
        let ov = c[7] as u32;
        let bit = |v: u32, b: u32| (v >> b) & 1;
        let (vt, vde, vrs, vbs) = if ega {
            (
                c[6] as u32 | bit(ov, 0) << 8,
                c[0x12] as u32 | bit(ov, 1) << 8,
                c[0x10] as u32 | bit(ov, 2) << 8,
                c[0x15] as u32 | bit(ov, 3) << 8,
            )
        } else {
            let msl = c[9] as u32;
            (
                c[6] as u32 | bit(ov, 0) << 8 | bit(ov, 5) << 9,
                c[0x12] as u32 | bit(ov, 1) << 8 | bit(ov, 6) << 9,
                c[0x10] as u32 | bit(ov, 2) << 8 | bit(ov, 7) << 9,
                c[0x15] as u32 | bit(ov, 3) << 8 | bit(msl, 5) << 9,
            )
        };
        let v_total = vt + v_add;
        let v_active = vde + 1;
        let vre_v = (c[0x11] & 0x0F) as u32;
        let vre = (vrs + 1..=vrs + 16)
            .find(|l| l & 0x0F == vre_v)
            .unwrap_or(vrs + 1);
        let (vbe_v, vbe_mask) = if ega {
            ((c[0x16] & 0x1F) as u32, 0x1F)
        } else {
            (c[0x16] as u32, 0xFF)
        };
        let vbe = (vbs + 1..=vbs + 256)
            .find(|l| l & vbe_mask == vbe_v)
            .unwrap_or(v_total);
        Timing {
            dot_clock_hz,
            h_total: h_total_c * cd,
            h_active: h_active_c * cd,
            // Blanking takes effect one character clock after the counter compare (it runs
            // through the same pipeline as the pixels), which centers the standard modes'
            // border: mode 10h gets 8 dots left and right instead of 16 and 0.
            h_blank_start: (hbs_c + 1) * cd,
            h_blank_end: (hbe_c + 1) * cd,
            h_retrace_start: hrs_c * cd,
            h_retrace_end: hre_c * cd,
            v_total,
            v_active,
            v_blank_start: vbs,
            v_blank_end: vbe,
            v_retrace_start: vrs,
            v_retrace_end: vre,
        }
        .normalized()
    }

    fn line_compare(&self) -> u32 {
        let c = &self.crtc;
        let mut lc = c[0x18] as u32 | (((c[7] >> 4) & 1) as u32) << 8;
        if self.kind == VgaKind::Vga {
            lc |= (((c[9] >> 6) & 1) as u32) << 9;
        }
        lc
    }

    /// The pixel resolution of the current mode (what a program would call "the screen size").
    pub(crate) fn logical_resolution(&self) -> (u32, u32) {
        let t = &self.cached;
        let text = self.gc[6] & 1 == 0;
        let div = if self.seq[1] & 8 != 0 { 2 } else { 1 };
        let mut w = t.h_active / div;
        if !text && self.attr[0x10] & 0x40 != 0 && self.kind == VgaKind::Vga {
            w /= 2;
        }
        let msl = (self.crtc[9] & 0x1F) as u32 + 1;
        let sd = if self.kind == VgaKind::Vga && self.crtc[9] & 0x80 != 0 {
            2
        } else {
            1
        };
        let h = if text {
            t.v_active
        } else {
            t.v_active / (msl * sd)
        };
        (w, h)
    }

    pub(crate) fn text_geometry(&self) -> Option<(u32, u32, u32)> {
        if self.gc[6] & 1 != 0 {
            return None;
        }
        let t = &self.cached;
        let cw = if self.seq[1] & 1 != 0 { 8 } else { 9 };
        let div = if self.seq[1] & 8 != 0 { 2 } else { 1 };
        let ch = (self.crtc[9] & 0x1F) as u32 + 1;
        Some((t.h_active / (cw * div), t.v_active / ch, ch))
    }

    // ---------------------------------------------------------------------------------------
    // Ports

    fn crtc_base(&self) -> u16 {
        if self.misc & 1 != 0 { 0x3D0 } else { 0x3B0 }
    }

    pub(crate) fn port_out(&mut self, port: u16, v: u8) {
        let base = self.crtc_base();
        match port {
            0x3C0 => {
                if !self.attr_flip {
                    self.attr_index = v & 0x3F;
                } else {
                    let i = (self.attr_index & 0x1F) as usize;
                    if i < self.attr.len() {
                        self.attr[i] = if i < 16 { v & 0x3F } else { v };
                        self.lut8_dirty = true;
                    }
                }
                self.attr_flip = !self.attr_flip;
            }
            0x3C2 => {
                self.misc = v;
                self.timing_dirty = true;
            }
            0x3C4 => self.seq_index = v & 0x07,
            0x3C5 => {
                let i = self.seq_index as usize;
                if i < self.seq.len() {
                    self.seq[i] = v;
                    if i == 1 {
                        self.timing_dirty = true;
                    }
                }
            }
            0x3C6 if self.kind == VgaKind::Vga => {
                self.pel_mask = v;
            }
            0x3C7 if self.kind == VgaKind::Vga => {
                self.dac_r = v;
                self.dac_sub = 0;
                self.dac_reading = true;
            }
            0x3C8 if self.kind == VgaKind::Vga => {
                self.dac_w = v;
                self.dac_sub = 0;
                self.dac_reading = false;
            }
            0x3C9 if self.kind == VgaKind::Vga => {
                let mask = if self.dac_8bit { 0xFF } else { 0x3F };
                self.dac_buf[self.dac_sub as usize] = v & mask;
                self.dac_sub += 1;
                if self.dac_sub == 3 {
                    let i = self.dac_w as usize;
                    self.dac[i] = self.dac_buf;
                    self.refresh_dac(i);
                    self.dac_w = self.dac_w.wrapping_add(1);
                    self.dac_sub = 0;
                }
            }
            0x3CE => self.gc_index = v & 0x0F,
            0x3CF => {
                let i = self.gc_index as usize;
                if i < self.gc.len() {
                    self.gc[i] = v;
                }
            }
            p if p == base + 4 => self.crtc_index = v & 0x1F,
            p if p == base + 5 => self.crtc_write(self.crtc_index as usize, v),
            _ => {}
        }
    }

    fn crtc_write(&mut self, i: usize, v: u8) {
        if i >= self.crtc.len() {
            return;
        }
        let protect = self.kind == VgaKind::Vga && self.crtc[0x11] & 0x80 != 0;
        if protect && i < 8 {
            // Registers 0-7 are write protected; only the line compare bit of the overflow
            // register gets through. A classic Mode X setup bug.
            if i == 7 {
                self.crtc[7] = (self.crtc[7] & !0x10) | (v & 0x10);
            }
            return;
        }
        self.crtc[i] = v;
        if matches!(i, 0..=7 | 9 | 0x10..=0x12 | 0x15 | 0x16) {
            self.timing_dirty = true;
        }
    }

    pub(crate) fn port_in(&mut self, port: u16, beam: &Beam) -> u8 {
        let vga = self.kind == VgaKind::Vga;
        let base = self.crtc_base();
        match port {
            0x3C0 if vga => self.attr_index,
            0x3C1 if vga => self
                .attr
                .get((self.attr_index & 0x1F) as usize)
                .copied()
                .unwrap_or(0),
            0x3C2 => 0x10,
            0x3C4 if vga => self.seq_index,
            0x3C5 if vga => self.seq.get(self.seq_index as usize).copied().unwrap_or(0),
            0x3C6 if vga => self.pel_mask,
            0x3C7 if vga => {
                if self.dac_reading {
                    0x03
                } else {
                    0x00
                }
            }
            0x3C8 if vga => self.dac_w,
            0x3C9 if vga => {
                let v = self.dac[self.dac_r as usize][self.dac_sub as usize];
                self.dac_sub += 1;
                if self.dac_sub == 3 {
                    self.dac_sub = 0;
                    self.dac_r = self.dac_r.wrapping_add(1);
                }
                v
            }
            0x3CC if vga => self.misc,
            0x3CE if vga => self.gc_index,
            0x3CF if vga => self.gc.get(self.gc_index as usize).copied().unwrap_or(0),
            p if p == base + 4 && vga => self.crtc_index,
            p if p == base + 5 => {
                let i = self.crtc_index as usize;
                if vga || (0x0C..=0x0F).contains(&i) {
                    self.crtc.get(i).copied().unwrap_or(0)
                } else {
                    0xFF
                }
            }
            p if p == base + 0xA => {
                // Input Status 1. Reading it also resets the attribute flip-flop.
                self.attr_flip = false;
                let mut s = 0;
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

    fn refresh_dac(&mut self, i: usize) {
        let c = self.dac[i];
        self.dac_rgba[i] = if self.dac_8bit {
            [c[0], c[1], c[2], 255]
        } else {
            [dac6_to_8(c[0]), dac6_to_8(c[1]), dac6_to_8(c[2]), 255]
        };
    }

    pub(crate) fn refresh_dac_all(&mut self) {
        for i in 0..256 {
            self.refresh_dac(i);
        }
    }

    pub(crate) fn dac_rgba(&self, i: u8) -> [u8; 4] {
        self.dac_rgba[(i & self.pel_mask) as usize]
    }

    // ---------------------------------------------------------------------------------------
    // CPU memory access

    /// Map a physical address to an offset in the graphics controller's memory window.
    fn window(&self, phys: u32) -> Option<u32> {
        let (base, size) = match (self.gc[6] >> 2) & 3 {
            0 => (0xA0000, 0x20000),
            1 => (0xA0000, 0x10000),
            2 => (0xB0000, 0x8000),
            _ => (0xB8000, 0x8000),
        };
        (phys >= base && phys < base + size).then(|| phys - base)
    }

    fn plane_target(&self, off: u32) -> (u8, usize) {
        let mut mask = self.seq[2] & 0x0F;
        let addr = if self.kind == VgaKind::Vga && self.seq[4] & 0x08 != 0 {
            // Chain-4: the low two address bits pick the plane; the plane address keeps them
            // (as zero), so only every fourth byte of each plane is used.
            mask &= 1 << (off & 3);
            off & !3
        } else if self.seq[4] & 0x04 == 0 {
            // Odd/even: even addresses go to planes 0/2, odd ones to 1/3.
            mask &= if off & 1 != 0 { 0b1010 } else { 0b0101 };
            off & !1
        } else {
            off
        };
        (mask, (addr as usize) & (VRAM_ADDRS - 1))
    }

    pub(crate) fn mem_write(&mut self, phys: u32, v: u8) -> bool {
        let Some(off) = self.window(phys) else {
            return false;
        };
        let (mask, addr) = self.plane_target(off);
        let mode = self.gc[5] & 3;
        let rot = self.gc[3] & 7;
        let func = (self.gc[3] >> 3) & 3;
        let sr = self.gc[0];
        let esr = self.gc[1];
        let bitmask = self.gc[8];
        let alu = |d: u8, l: u8| match func {
            0 => d,
            1 => d & l,
            2 => d | l,
            _ => d ^ l,
        };
        let rotated = v.rotate_right(rot as u32);
        for p in 0..4 {
            if mask & (1 << p) == 0 {
                continue;
            }
            let l = self.latch[p];
            let srp = if sr & (1 << p) != 0 { 0xFF } else { 0x00 };
            let out = match mode {
                0 => {
                    let d = if esr & (1 << p) != 0 { srp } else { rotated };
                    (alu(d, l) & bitmask) | (l & !bitmask)
                }
                1 => l,
                2 => {
                    let d = if v & (1 << p) != 0 { 0xFF } else { 0x00 };
                    (alu(d, l) & bitmask) | (l & !bitmask)
                }
                _ => {
                    let m = rotated & bitmask;
                    (alu(srp, l) & m) | (l & !m)
                }
            };
            self.vram[addr * 4 + p] = out;
        }
        true
    }

    pub(crate) fn mem_read(&mut self, phys: u32) -> Option<u8> {
        let off = self.window(phys)?;
        let (_, addr) = self.plane_target(off);
        self.latch
            .copy_from_slice(&self.vram[addr * 4..addr * 4 + 4]);
        if self.gc[5] & 0x08 == 0 {
            let plane = if self.kind == VgaKind::Vga && self.seq[4] & 0x08 != 0 {
                off & 3
            } else if self.seq[4] & 0x04 == 0 {
                (self.gc[4] as u32 & 2) | (off & 1)
            } else {
                self.gc[4] as u32 & 3
            };
            Some(self.latch[plane as usize])
        } else {
            let cc = self.gc[2];
            let dc = self.gc[7];
            let mut r = 0xFF;
            for p in 0..4 {
                if dc & (1 << p) != 0 {
                    let want = if cc & (1 << p) != 0 { 0xFF } else { 0x00 };
                    r &= !(self.latch[p] ^ want);
                }
            }
            Some(r)
        }
    }

    /// Direct plane access for tools and tests (bypasses the graphics controller).
    pub fn plane(&self, plane: usize) -> impl Iterator<Item = u8> + '_ {
        self.vram.iter().skip(plane & 3).step_by(4).copied()
    }

    #[cfg(test)]
    pub(crate) fn plane_byte(&self, plane: usize, addr: usize) -> u8 {
        self.vram[(addr & (VRAM_ADDRS - 1)) * 4 + (plane & 3)]
    }

    #[cfg(test)]
    pub(crate) fn set_plane_byte(&mut self, plane: usize, addr: usize, v: u8) {
        self.vram[(addr & (VRAM_ADDRS - 1)) * 4 + (plane & 3)] = v;
    }

    // ---------------------------------------------------------------------------------------
    // Scanout helpers

    fn addr_mode(&self) -> AddrMode {
        if self.kind == VgaKind::Vga && self.crtc[0x14] & 0x40 != 0 {
            AddrMode::Dword
        } else if self.crtc[0x17] & 0x40 == 0 {
            AddrMode::Word
        } else {
            AddrMode::Byte
        }
    }

    #[inline]
    fn ma_to_addr(&self, mode: AddrMode, ma: u32) -> usize {
        let mut a = match mode {
            AddrMode::Dword => ma << 2,
            AddrMode::Word => {
                let wrap_bit = if self.crtc[0x17] & 0x20 != 0 { 15 } else { 13 };
                (ma << 1) | ((ma >> wrap_bit) & 1)
            }
            AddrMode::Byte => ma,
        };
        let m17 = self.crtc[0x17];
        if m17 & 1 == 0 {
            a = (a & !(1 << 13)) | ((self.row_scan & 1) << 13);
        }
        if m17 & 2 == 0 {
            a = (a & !(1 << 14)) | (((self.row_scan >> 1) & 1) << 14);
        }
        (a as usize) & (VRAM_ADDRS - 1)
    }

    fn ega_color(&self, v: u8) -> [u8; 4] {
        if self.cached.v_active < 300 {
            rgba(palette::ega200(v))
        } else {
            rgba(palette::ega64(v))
        }
    }

    /// Color for a 4-bit attribute index through the palette registers (and the DAC on VGA).
    fn attr_color(&self, idx: u8) -> [u8; 4] {
        let pal = self.attr[(idx & 0x0F) as usize];
        if self.kind == VgaKind::Ega {
            return self.ega_color(pal);
        }
        let cs = self.attr[0x14];
        let mut di = if self.attr[0x10] & 0x80 != 0 {
            (pal & 0x0F) | ((cs & 3) << 4)
        } else {
            pal & 0x3F
        };
        di |= (cs & 0x0C) << 4;
        self.dac_rgba(di)
    }

    fn lut16(&self) -> [[u8; 4]; 16] {
        let mut l = [[0u8; 4]; 16];
        let enable = self.attr[0x12] & 0x0F;
        for (i, c) in l.iter_mut().enumerate() {
            *c = self.attr_color(i as u8 & enable);
        }
        l
    }

    fn refresh_lut8(&mut self) {
        if !self.lut8_dirty {
            return;
        }
        for v in 0..256usize {
            let lo = self.attr[v & 0x0F] & 0x0F;
            let hi = self.attr[v >> 4] & 0x0F;
            self.lut8[v] = (hi << 4) | lo;
        }
        self.lut8_dirty = false;
    }

    fn pel_pan(&self) -> u32 {
        if self.kind == VgaKind::Vga && self.split_active && self.attr[0x10] & 0x20 != 0 {
            0
        } else {
            (self.attr[0x13] & 0x0F) as u32
        }
    }

    fn render_256(&mut self, x0: u32, out: &mut [[u8; 4]]) {
        self.refresh_lut8();
        let div = if self.seq[1] & 8 != 0 { 4 } else { 2 };
        let pan = (self.pel_pan() >> 1) & 3;
        let mode = self.addr_mode();
        let mut last_ma = u32::MAX;
        let mut quad = [0u8; 4];
        for (i, px) in out.iter_mut().enumerate() {
            let k = (x0 + i as u32) / div + pan;
            let ma = self.row_ma + k / 4;
            if ma != last_ma {
                let a = self.ma_to_addr(mode, ma) * 4;
                quad.copy_from_slice(&self.vram[a..a + 4]);
                last_ma = ma;
            }
            let v = quad[(k & 3) as usize];
            *px = self.dac_rgba[(self.lut8[v as usize] & self.pel_mask) as usize];
        }
    }

    fn render_planar(&mut self, x0: u32, out: &mut [[u8; 4]]) {
        let lut = self.lut16();
        let div = if self.seq[1] & 8 != 0 { 2 } else { 1 };
        let pan = self.pel_pan() & 7;
        let mode = self.addr_mode();
        let mut last_ma = u32::MAX;
        let mut quad = [0u8; 4];
        for (i, px) in out.iter_mut().enumerate() {
            let k = (x0 + i as u32) / div + pan;
            let ma = self.row_ma + k / 8;
            if ma != last_ma {
                let a = self.ma_to_addr(mode, ma) * 4;
                quad.copy_from_slice(&self.vram[a..a + 4]);
                last_ma = ma;
            }
            let sh = 7 - (k & 7);
            let idx = ((quad[0] >> sh) & 1)
                | (((quad[1] >> sh) & 1) << 1)
                | (((quad[2] >> sh) & 1) << 2)
                | (((quad[3] >> sh) & 1) << 3);
            *px = lut[idx as usize];
        }
    }

    fn render_cga2(&mut self, x0: u32, out: &mut [[u8; 4]]) {
        let lut = self.lut16();
        let div = if self.seq[1] & 8 != 0 { 2 } else { 1 };
        let pan = self.pel_pan() & 7;
        let mode = self.addr_mode();
        for (i, px) in out.iter_mut().enumerate() {
            let k = (x0 + i as u32) / div + pan;
            let ma = self.row_ma + k / 8;
            let a = self.ma_to_addr(mode, ma) * 4;
            let j = k & 7;
            let plane = if j < 4 { 0 } else { 1 };
            let sh = 6 - 2 * (j & 3);
            let lo = (self.vram[a + plane] >> sh) & 3;
            let hi = (self.vram[a + plane + 2] >> sh) & 3;
            *px = lut[(lo | (hi << 2)) as usize];
        }
    }

    fn render_text(&mut self, x0: u32, out: &mut [[u8; 4]]) {
        let lut = self.lut16();
        let cw: u32 = if self.seq[1] & 1 != 0 { 8 } else { 9 };
        let div = if self.seq[1] & 8 != 0 { 2 } else { 1 };
        let raw_pan = self.pel_pan();
        let pan = if cw == 9 {
            if raw_pan >= 8 { 0 } else { raw_pan + 1 }
        } else {
            raw_pan & 7
        };
        let mode = self.addr_mode();
        let cs = self.seq[3];
        let map_a = (((cs >> 2) & 3) | ((cs >> 3) & 4)) as usize;
        let map_b = ((cs & 3) | ((cs >> 2) & 4)) as usize;
        const MAP_BASE: [usize; 8] = [
            0x0000, 0x4000, 0x8000, 0xC000, 0x2000, 0x6000, 0xA000, 0xE000,
        ];
        let line_gfx = self.attr[0x10] & 0x04 != 0;
        let blink_en = self.attr[0x10] & 0x08 != 0;
        let blink_on = (self.frames / 16) & 1 == 0;
        let cur_on = self.crtc[0x0A] & 0x20 == 0 && (self.frames / 8) & 1 == 0;
        let cur_loc = ((self.crtc[0x0E] as u32) << 8) | self.crtc[0x0F] as u32;
        let cur_s = (self.crtc[0x0A] & 0x1F) as u32;
        let cur_e = (self.crtc[0x0B] & 0x1F) as u32;
        let row_scan = self.row_scan;
        for (i, px) in out.iter_mut().enumerate() {
            let p = (x0 + i as u32) / div + pan;
            let col = p / cw;
            let cx = p % cw;
            let ma = self.row_ma + col;
            let a = self.ma_to_addr(mode, ma) * 4;
            let ch = self.vram[a];
            let at = self.vram[a + 1];
            let map = if map_a != map_b && at & 0x08 != 0 {
                map_a
            } else {
                map_b
            };
            let g = self.vram
                [((MAP_BASE[map] + ch as usize * 32 + row_scan as usize) & 0xFFFF) * 4 + 2];
            let mut on = if cx < 8 {
                (g >> (7 - cx)) & 1 != 0
            } else {
                line_gfx && (0xC0..=0xDF).contains(&ch) && g & 1 != 0
            };
            let fg = at & 0x0F;
            let mut bg = at >> 4;
            if blink_en {
                bg &= 7;
                if at & 0x80 != 0 && !blink_on {
                    on = false;
                }
            }
            if cur_on && ma == cur_loc && row_scan >= cur_s && row_scan <= cur_e {
                on = true;
            }
            *px = lut[if on { fg } else { bg } as usize];
        }
    }

    // ---------------------------------------------------------------------------------------
    // BIOS mode set

    /// Program the registers for a BIOS video mode, like `INT 10h, AH=00h`. Bit 7 of `mode`
    /// keeps video memory instead of clearing it. Returns `false` for modes the adapter lacks.
    pub(crate) fn set_mode(&mut self, mode: u8) -> bool {
        let keep = mode & 0x80 != 0;
        let mode = mode & 0x7F;
        let ega = self.kind == VgaKind::Ega;
        let tables = if ega { ega_mode(mode) } else { vga_mode(mode) };
        let Some(t) = tables else {
            return false;
        };
        self.misc = t.misc;
        self.seq = t.seq;
        self.crtc = t.crtc;
        self.gc = t.gc;
        self.attr[..16].copy_from_slice(&t.palette);
        self.attr[16..21].copy_from_slice(&t.attr);
        self.attr_flip = false;
        self.attr_index = 0x20;
        self.pel_mask = 0xFF;
        self.dac_8bit = false;
        self.lut8_dirty = true;
        self.latch = [0; 4];
        if !keep {
            self.vram.fill(0);
        }
        if !ega {
            self.dac = match mode {
                0x13 => palette::vga_default_dac(),
                0x0D | 0x0E => palette::vga_ega_dac(true),
                _ => palette::vga_ega_dac(false),
            };
            self.refresh_dac_all();
        }
        if self.gc[6] & 1 == 0 {
            // Text mode: fill the screen with blanks in light gray and load the font into
            // plane 2, where the character generator reads it.
            if !keep {
                for a in (0..0x8000).step_by(2) {
                    self.vram[a * 4] = b' ';
                    self.vram[a * 4 + 1] = 0x07;
                }
            }
            let font = match (self.crtc[9] & 0x1F) + 1 {
                16 => Font::vga(),
                14 => Font::ega(),
                _ => Font::cga(),
            };
            self.load_font(font, 0);
        }
        self.start_latched = 0;
        self.bios_mode = mode;
        self.timing_dirty = true;
        true
    }

    /// Load a font into character map `map` (0..8) in plane 2.
    pub(crate) fn load_font(&mut self, font: &Font, map: usize) {
        const MAP_BASE: [usize; 8] = [
            0x0000, 0x4000, 0x8000, 0xC000, 0x2000, 0x6000, 0xA000, 0xE000,
        ];
        let base = MAP_BASE[map & 7];
        let h = font.height() as usize;
        for ch in 0..256 {
            let g = font.glyph(ch as u8);
            for r in 0..32 {
                let v = g.get(r).copied().filter(|_| r < h).unwrap_or(0);
                self.vram[((base + ch * 32 + r) & 0xFFFF) * 4 + 2] = v;
            }
        }
    }
}

impl Scanout for Vga {
    fn frame_start(&mut self) {
        self.row_ma = self.start_latched;
        self.row_scan = 0;
        self.doubled = false;
        self.split_active = false;
        self.frames = self.frames.wrapping_add(1);
    }

    fn line_start(&mut self, line: u32) {
        if line == 0 || line >= self.cached.v_active {
            return;
        }
        if line - 1 == self.line_compare() {
            self.row_ma = 0;
            self.row_scan = 0;
            self.doubled = false;
            self.split_active = true;
            return;
        }
        if self.kind == VgaKind::Vga && self.crtc[9] & 0x80 != 0 && !self.doubled {
            self.doubled = true;
            return;
        }
        self.doubled = false;
        let msl = (self.crtc[9] & 0x1F) as u32;
        if self.row_scan >= msl {
            self.row_scan = 0;
            self.row_ma = self.row_ma.wrapping_add(self.crtc[0x13] as u32 * 2) & 0xFFFF;
        } else {
            self.row_scan += 1;
        }
    }

    fn vretrace_start(&mut self) {
        self.start_latched = ((self.crtc[0x0C] as u32) << 8) | self.crtc[0x0D] as u32;
    }

    fn render_active(&mut self, _line: u32, x0: u32, out: &mut [[u8; 4]]) {
        if self.seq[1] & 0x20 != 0 {
            out.fill([0, 0, 0, 255]);
            return;
        }
        if self.attr_index & 0x20 == 0 {
            // Palette address source cleared: the attribute controller owns the palette and
            // the screen shows only the overscan color.
            out.fill(self.border());
            return;
        }
        if self.gc[6] & 1 == 0 {
            self.render_text(x0, out);
        } else if self.kind == VgaKind::Vga && self.attr[0x10] & 0x40 != 0 {
            self.render_256(x0, out);
        } else if self.gc[5] & 0x20 != 0 {
            self.render_cga2(x0, out);
        } else {
            self.render_planar(x0, out);
        }
    }

    fn border(&self) -> [u8; 4] {
        if self.seq[1] & 0x20 != 0 {
            return [0, 0, 0, 255];
        }
        let o = self.attr[0x11];
        match self.kind {
            VgaKind::Ega => self.ega_color(o),
            VgaKind::Vga => self.dac_rgba(o),
        }
    }
}

pub(crate) struct ModeTables {
    misc: u8,
    seq: [u8; 5],
    crtc: [u8; 25],
    gc: [u8; 9],
    palette: [u8; 16],
    attr: [u8; 5],
}

const PAL_EGA: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x14, 0x07, 0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x3F,
];
const PAL_200: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
];
const PAL_ID: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F,
];

fn vga_mode(mode: u8) -> Option<ModeTables> {
    let gfx_gc = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x0F, 0xFF];
    Some(match mode {
        0x00 | 0x01 => ModeTables {
            misc: 0x67,
            seq: [0x03, 0x08, 0x03, 0x00, 0x02],
            crtc: [
                0x2D, 0x27, 0x28, 0x90, 0x2B, 0xA0, 0xBF, 0x1F, 0x00, 0x4F, 0x0D, 0x0E, 0x00, 0x00,
                0x00, 0x00, 0x9C, 0x8E, 0x8F, 0x14, 0x1F, 0x96, 0xB9, 0xA3, 0xFF,
            ],
            gc: [0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x0E, 0x00, 0xFF],
            palette: PAL_EGA,
            attr: [0x0C, 0x00, 0x0F, 0x08, 0x00],
        },
        0x02 | 0x03 => ModeTables {
            misc: 0x67,
            seq: [0x03, 0x00, 0x03, 0x00, 0x02],
            crtc: [
                0x5F, 0x4F, 0x50, 0x82, 0x55, 0x81, 0xBF, 0x1F, 0x00, 0x4F, 0x0D, 0x0E, 0x00, 0x00,
                0x00, 0x00, 0x9C, 0x8E, 0x8F, 0x28, 0x1F, 0x96, 0xB9, 0xA3, 0xFF,
            ],
            gc: [0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x0E, 0x00, 0xFF],
            palette: PAL_EGA,
            attr: [0x0C, 0x00, 0x0F, 0x08, 0x00],
        },
        0x0D => ModeTables {
            misc: 0x63,
            seq: [0x03, 0x09, 0x0F, 0x00, 0x06],
            crtc: [
                0x2D, 0x27, 0x28, 0x90, 0x2B, 0x80, 0xBF, 0x1F, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x9C, 0x8E, 0x8F, 0x14, 0x00, 0x96, 0xB9, 0xE3, 0xFF,
            ],
            gc: gfx_gc,
            palette: PAL_200,
            attr: [0x01, 0x00, 0x0F, 0x00, 0x00],
        },
        0x0E => ModeTables {
            misc: 0x63,
            seq: [0x03, 0x01, 0x0F, 0x00, 0x06],
            crtc: [
                0x5F, 0x4F, 0x50, 0x82, 0x54, 0x80, 0xBF, 0x1F, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x9C, 0x8E, 0x8F, 0x28, 0x00, 0x96, 0xB9, 0xE3, 0xFF,
            ],
            gc: gfx_gc,
            palette: PAL_200,
            attr: [0x01, 0x00, 0x0F, 0x00, 0x00],
        },
        0x10 => ModeTables {
            misc: 0xA3,
            seq: [0x03, 0x01, 0x0F, 0x00, 0x06],
            crtc: [
                0x5F, 0x4F, 0x50, 0x82, 0x54, 0x80, 0xBF, 0x1F, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x83, 0x85, 0x5D, 0x28, 0x0F, 0x63, 0xBA, 0xE3, 0xFF,
            ],
            gc: gfx_gc,
            palette: PAL_EGA,
            attr: [0x01, 0x00, 0x0F, 0x00, 0x00],
        },
        0x12 => ModeTables {
            misc: 0xE3,
            seq: [0x03, 0x01, 0x0F, 0x00, 0x06],
            crtc: [
                0x5F, 0x4F, 0x50, 0x82, 0x54, 0x80, 0x0B, 0x3E, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0xEA, 0x8C, 0xDF, 0x28, 0x00, 0xE7, 0x04, 0xE3, 0xFF,
            ],
            gc: gfx_gc,
            palette: PAL_EGA,
            attr: [0x01, 0x00, 0x0F, 0x00, 0x00],
        },
        0x13 => ModeTables {
            misc: 0x63,
            seq: [0x03, 0x01, 0x0F, 0x00, 0x0E],
            crtc: [
                0x5F, 0x4F, 0x50, 0x82, 0x54, 0x80, 0xBF, 0x1F, 0x00, 0x41, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x9C, 0x8E, 0x8F, 0x28, 0x40, 0x96, 0xB9, 0xA3, 0xFF,
            ],
            gc: [0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x05, 0x0F, 0xFF],
            palette: PAL_ID,
            attr: [0x41, 0x00, 0x0F, 0x00, 0x00],
        },
        _ => return None,
    })
}

fn ega_mode(mode: u8) -> Option<ModeTables> {
    let gfx_gc = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x0F, 0xFF];
    Some(match mode {
        0x02 | 0x03 => ModeTables {
            misc: 0xA7,
            seq: [0x03, 0x01, 0x03, 0x00, 0x03],
            crtc: [
                0x5B, 0x4F, 0x53, 0x37, 0x51, 0x5B, 0x6C, 0x1F, 0x00, 0x0D, 0x0B, 0x0C, 0x00, 0x00,
                0x00, 0x00, 0x5E, 0x2B, 0x5D, 0x28, 0x0F, 0x5E, 0x0A, 0xA3, 0xFF,
            ],
            gc: [0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x0E, 0x00, 0xFF],
            palette: PAL_EGA,
            attr: [0x08, 0x00, 0x0F, 0x00, 0x00],
        },
        0x0D => ModeTables {
            misc: 0x23,
            seq: [0x03, 0x0B, 0x0F, 0x00, 0x06],
            crtc: [
                0x37, 0x27, 0x2D, 0x37, 0x30, 0x14, 0x04, 0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0xE1, 0x24, 0xC7, 0x14, 0x00, 0xE0, 0xF0, 0xE3, 0xFF,
            ],
            gc: gfx_gc,
            palette: PAL_200,
            attr: [0x01, 0x00, 0x0F, 0x00, 0x00],
        },
        0x0E => ModeTables {
            misc: 0x23,
            seq: [0x03, 0x01, 0x0F, 0x00, 0x06],
            crtc: [
                0x70, 0x4F, 0x59, 0x2D, 0x5E, 0x06, 0x04, 0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0xE0, 0x23, 0xC7, 0x28, 0x00, 0xDF, 0xEF, 0xE3, 0xFF,
            ],
            gc: gfx_gc,
            palette: PAL_200,
            attr: [0x01, 0x00, 0x0F, 0x00, 0x00],
        },
        0x10 => ModeTables {
            misc: 0xA7,
            seq: [0x03, 0x01, 0x0F, 0x00, 0x06],
            crtc: [
                0x5B, 0x4F, 0x53, 0x37, 0x52, 0x00, 0x6C, 0x1F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x5E, 0x2B, 0x5D, 0x28, 0x0F, 0x5F, 0x0A, 0xE3, 0xFF,
            ],
            gc: gfx_gc,
            palette: PAL_EGA,
            attr: [0x01, 0x00, 0x0F, 0x00, 0x00],
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn mode13h_timing_is_70hz() {
        let mut v = Vga::new(VgaKind::Vga);
        assert!(v.set_mode(0x13));
        let t = v.timing();
        assert_eq!(
            (t.h_total, t.h_active, t.v_total, t.v_active),
            (800, 640, 449, 400)
        );
        assert!(approx(t.refresh_hz(), 70.086, 0.01), "{}", t.refresh_hz());
        assert_eq!(v.logical_resolution(), (320, 200));
    }

    #[test]
    fn mode12h_timing_is_60hz() {
        let mut v = Vga::new(VgaKind::Vga);
        v.set_mode(0x12);
        let t = v.timing();
        assert_eq!((t.h_total, t.v_total, t.v_active), (800, 525, 480));
        assert!(approx(t.refresh_hz(), 59.94, 0.01));
        assert_eq!(v.logical_resolution(), (640, 480));
    }

    #[test]
    fn text_mode_is_720x400() {
        let mut v = Vga::new(VgaKind::Vga);
        v.set_mode(0x03);
        let t = v.timing();
        assert_eq!((t.h_active, t.v_active), (720, 400));
        assert_eq!(v.text_geometry(), Some((80, 25, 16)));
    }

    #[test]
    fn ega_200_line_mode_runs_at_ntsc_rates() {
        let mut v = Vga::new(VgaKind::Ega);
        assert!(v.set_mode(0x0D));
        let t = v.timing();
        assert_eq!((t.h_total, t.h_active, t.v_active), (912, 640, 200));
        assert!(approx(t.line_rate_hz(), 15_700.0, 50.0));
        assert!(approx(t.refresh_hz(), 60.0, 0.3), "{}", t.refresh_hz());
        assert!(!v.set_mode(0x13), "no 256-color mode on EGA");
    }

    #[test]
    fn chain4_puts_pixels_in_planes() {
        let mut v = Vga::new(VgaKind::Vga);
        v.set_mode(0x13);
        for i in 0..8u32 {
            v.mem_write(0xA0000 + i, i as u8 + 1);
        }
        assert_eq!(v.plane_byte(0, 0), 1);
        assert_eq!(v.plane_byte(1, 0), 2);
        assert_eq!(v.plane_byte(3, 0), 4);
        assert_eq!(v.plane_byte(0, 4), 5);
        assert_eq!(v.mem_read(0xA0005), Some(6));
    }

    #[test]
    fn write_mode_1_copies_latches() {
        let mut v = Vga::new(VgaKind::Vga);
        v.set_mode(0x12);
        for p in 0..4 {
            v.set_plane_byte(p, 10, 0x10 + p as u8);
        }
        v.mem_read(0xA000A);
        v.gc[5] = 0x01;
        v.mem_write(0xA0020, 0);
        for p in 0..4 {
            assert_eq!(v.plane_byte(p, 0x20), 0x10 + p as u8);
        }
    }

    #[test]
    fn set_reset_and_bit_mask() {
        let mut v = Vga::new(VgaKind::Vga);
        v.set_mode(0x12);
        v.gc[0] = 0x05; // color 5
        v.gc[1] = 0x0F;
        v.gc[8] = 0x80; // leftmost pixel only
        v.mem_read(0xA0000);
        v.mem_write(0xA0000, 0xFF);
        assert_eq!(v.plane_byte(0, 0), 0x80);
        assert_eq!(v.plane_byte(1, 0), 0x00);
        assert_eq!(v.plane_byte(2, 0), 0x80);
        // Read mode 1: which pixels have color 5?
        v.gc[5] = 0x08;
        v.gc[2] = 0x05;
        v.gc[7] = 0x0F;
        assert_eq!(v.mem_read(0xA0000), Some(0x80));
    }

    #[test]
    fn crtc_protect_bit_blocks_timing_writes() {
        let mut v = Vga::new(VgaKind::Vga);
        v.set_mode(0x13);
        v.port_out(0x3D4, 0x06);
        v.port_out(0x3D5, 0x0D);
        assert_eq!(v.crtc[6], 0xBF, "protected by CRTC 0x11 bit 7");
        v.port_out(0x3D4, 0x11);
        v.port_out(0x3D5, 0x0E);
        v.port_out(0x3D4, 0x06);
        v.port_out(0x3D5, 0x0D);
        assert_eq!(v.crtc[6], 0x0D);
    }

    #[test]
    fn attribute_flip_flop() {
        let mut v = Vga::new(VgaKind::Vga);
        v.set_mode(0x13);
        let b = Beam::default();
        v.port_in(0x3DA, &b);
        v.port_out(0x3C0, 0x11 | 0x20);
        v.port_out(0x3C0, 0x2A);
        assert_eq!(v.attr[0x11], 0x2A);
        // Without the reset, the next write is taken as an index.
        v.port_out(0x3C0, 0x31);
        assert_eq!(v.attr_index, 0x31);
    }
}
