//! A VESA BIOS Extensions 2.0 SVGA card: a full VGA, plus high-resolution and high-color modes
//! reached through a 64 KiB bank window at `A000:0000` (VBE 1.2 style, switch banks with
//! function 05h) or a linear framebuffer at [`LFB_BASE`] (VBE 2.0, protected-mode programs).
//!
//! SVGA is not anachronistic for DOS: VBE 1.2 dates from 1991 and VBE 2.0 from 1994, and plenty
//! of DOS games ran at 640x480 and above. The pain point of the era is reproduced: in banked
//! modes a scanline can straddle two banks, and every bank switch costs time.

use crate::crt::{Beam, Scanout, Timing};
use crate::vga::{Vga, VgaKind};

/// Physical address of the linear framebuffer.
pub const LFB_BASE: u32 = 0xE000_0000;
/// Size of the bank window at `A0000` (and its granularity).
pub const BANK_SIZE: u32 = 0x10000;

/// A VBE mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VbeMode {
    /// VESA mode number (`0x101` = 640x480x8, ...).
    pub number: u16,
    pub width: u32,
    pub height: u32,
    /// 8 (palettized through the DAC), 15 (5:5:5), 16 (5:6:5) or 32 (X:8:8:8).
    pub bpp: u8,
}

impl VbeMode {
    /// Bytes per pixel.
    pub fn bytes_per_pixel(&self) -> u32 {
        match self.bpp {
            8 => 1,
            15 | 16 => 2,
            _ => 4,
        }
    }
}

/// The modes this card offers (the standard VESA numbers).
pub const VBE_MODES: &[VbeMode] = &[
    VbeMode {
        number: 0x100,
        width: 640,
        height: 400,
        bpp: 8,
    },
    VbeMode {
        number: 0x101,
        width: 640,
        height: 480,
        bpp: 8,
    },
    VbeMode {
        number: 0x103,
        width: 800,
        height: 600,
        bpp: 8,
    },
    VbeMode {
        number: 0x105,
        width: 1024,
        height: 768,
        bpp: 8,
    },
    VbeMode {
        number: 0x107,
        width: 1280,
        height: 1024,
        bpp: 8,
    },
    VbeMode {
        number: 0x10D,
        width: 320,
        height: 200,
        bpp: 15,
    },
    VbeMode {
        number: 0x10E,
        width: 320,
        height: 200,
        bpp: 16,
    },
    VbeMode {
        number: 0x10F,
        width: 320,
        height: 200,
        bpp: 32,
    },
    VbeMode {
        number: 0x110,
        width: 640,
        height: 480,
        bpp: 15,
    },
    VbeMode {
        number: 0x111,
        width: 640,
        height: 480,
        bpp: 16,
    },
    VbeMode {
        number: 0x112,
        width: 640,
        height: 480,
        bpp: 32,
    },
    VbeMode {
        number: 0x113,
        width: 800,
        height: 600,
        bpp: 15,
    },
    VbeMode {
        number: 0x114,
        width: 800,
        height: 600,
        bpp: 16,
    },
    VbeMode {
        number: 0x115,
        width: 800,
        height: 600,
        bpp: 32,
    },
    VbeMode {
        number: 0x116,
        width: 1024,
        height: 768,
        bpp: 15,
    },
    VbeMode {
        number: 0x117,
        width: 1024,
        height: 768,
        bpp: 16,
    },
    VbeMode {
        number: 0x118,
        width: 1024,
        height: 768,
        bpp: 32,
    },
];

pub(crate) struct VbeState {
    pub mode: VbeMode,
    pub pitch: u32,
    pub start: u32,
    start_latched: u32,
    pub bank: u32,
    line_div: u32,
    timing: Timing,
}

pub struct Svga {
    pub(crate) vga: Vga,
    pub(crate) mem: Vec<u8>,
    pub(crate) vbe: Option<VbeState>,
    pub(crate) timing_dirty: bool,
}

fn vesa_timing(w: u32, h: u32) -> (Timing, u32) {
    let t = |clk: f64, ha, ht, hrs, hre, va, vt, vrs, vre| Timing {
        dot_clock_hz: clk,
        h_total: ht,
        h_active: ha,
        h_blank_start: ha,
        h_blank_end: ht,
        h_retrace_start: hrs,
        h_retrace_end: hre,
        v_total: vt,
        v_active: va,
        v_blank_start: va,
        v_blank_end: vt,
        v_retrace_start: vrs,
        v_retrace_end: vre,
    };
    match (w, h) {
        (320, 200) => (t(12_587_500.0, 320, 400, 328, 376, 400, 449, 412, 414), 2),
        (640, 400) => (t(25_175_000.0, 640, 800, 656, 752, 400, 449, 412, 414), 1),
        (640, 480) => (t(25_175_000.0, 640, 800, 656, 752, 480, 525, 490, 492), 1),
        (800, 600) => (t(40_000_000.0, 800, 1056, 840, 968, 600, 628, 601, 605), 1),
        (1024, 768) => (
            t(65_000_000.0, 1024, 1344, 1048, 1184, 768, 806, 771, 777),
            1,
        ),
        _ => (
            t(
                108_000_000.0,
                1280,
                1688,
                1328,
                1440,
                1024,
                1066,
                1025,
                1028,
            ),
            1,
        ),
    }
}

impl Svga {
    pub fn new(mem_bytes: usize) -> Self {
        Self {
            vga: Vga::new(VgaKind::Vga),
            mem: vec![0; mem_bytes.next_power_of_two().max(0x40000)],
            vbe: None,
            timing_dirty: true,
        }
    }

    pub(crate) fn timing(&mut self) -> Timing {
        match &self.vbe {
            Some(v) => v.timing,
            None => self.vga.timing(),
        }
    }

    pub(crate) fn take_dirty(&mut self) -> bool {
        let d = self.timing_dirty || self.vga.timing_dirty;
        self.timing_dirty = false;
        self.vga.timing_dirty = false;
        d
    }

    pub(crate) fn set_vga_mode(&mut self, mode: u8) -> bool {
        let ok = self.vga.set_mode(mode);
        if ok {
            self.vbe = None;
            self.timing_dirty = true;
        }
        ok
    }

    /// VBE function 02h. Bit 15 of `number` keeps memory, bit 14 asks for the LFB (accepted
    /// either way: the bank window and the LFB both stay mapped).
    pub(crate) fn set_vbe_mode(&mut self, number: u16) -> Option<VbeMode> {
        let n = number & 0x1FF;
        if n < 0x100 {
            return self
                .set_vga_mode(n as u8 | if number & 0x8000 != 0 { 0x80 } else { 0 })
                .then_some(VbeMode {
                    number: n,
                    width: 0,
                    height: 0,
                    bpp: 0,
                });
        }
        let mode = *VBE_MODES.iter().find(|m| m.number == n)?;
        let pitch = mode.width * mode.bytes_per_pixel();
        if (pitch * mode.height) as usize > self.mem.len() {
            return None;
        }
        if number & 0x8000 == 0 {
            self.mem.fill(0);
        }
        let (timing, line_div) = vesa_timing(mode.width, mode.height);
        if mode.bpp == 8 {
            self.vga.dac = crate::palette::vga_default_dac();
            self.vga.dac_8bit = false;
            self.vga.refresh_dac_all();
        }
        self.vga.pel_mask = 0xFF;
        self.vbe = Some(VbeState {
            mode,
            pitch,
            start: 0,
            start_latched: 0,
            bank: 0,
            line_div,
            timing: timing.normalized(),
        });
        self.vga.bios_mode = 0xFF;
        self.timing_dirty = true;
        Some(mode)
    }

    fn vbe_offset(&self, phys: u32) -> Option<usize> {
        if phys >= LFB_BASE {
            let o = (phys - LFB_BASE) as usize;
            return (o < self.mem.len()).then_some(o);
        }
        let v = self.vbe.as_ref()?;
        if (0xA0000..0xB0000).contains(&phys) {
            let o = (v.bank * BANK_SIZE + (phys - 0xA0000)) as usize;
            return (o < self.mem.len()).then_some(o);
        }
        None
    }

    pub(crate) fn mem_write(&mut self, phys: u32, val: u8) -> bool {
        if let Some(o) = self.vbe_offset(phys) {
            self.mem[o] = val;
            return true;
        }
        self.vbe.is_none() && self.vga.mem_write(phys, val)
    }

    pub(crate) fn mem_read(&mut self, phys: u32) -> Option<u8> {
        if let Some(o) = self.vbe_offset(phys) {
            return Some(self.mem[o]);
        }
        if self.vbe.is_none() {
            self.vga.mem_read(phys)
        } else {
            None
        }
    }

    pub(crate) fn port_out(&mut self, port: u16, v: u8) {
        self.vga.port_out(port, v);
    }

    pub(crate) fn port_in(&mut self, port: u16, beam: &Beam) -> u8 {
        self.vga.port_in(port, beam)
    }

    pub(crate) fn logical_resolution(&self) -> (u32, u32) {
        match &self.vbe {
            Some(v) => (v.mode.width, v.mode.height),
            None => self.vga.logical_resolution(),
        }
    }
}

impl Scanout for Svga {
    fn frame_start(&mut self) {
        if self.vbe.is_none() {
            self.vga.frame_start();
        }
    }

    fn line_start(&mut self, line: u32) {
        if self.vbe.is_none() {
            self.vga.line_start(line);
        }
    }

    fn vretrace_start(&mut self) {
        match &mut self.vbe {
            Some(v) => v.start_latched = v.start,
            None => self.vga.vretrace_start(),
        }
    }

    fn render_active(&mut self, line: u32, x0: u32, out: &mut [[u8; 4]]) {
        let Some(v) = &self.vbe else {
            self.vga.render_active(line, x0, out);
            return;
        };
        let y = line / v.line_div;
        let bpp = v.mode.bytes_per_pixel() as usize;
        let mlen = self.mem.len();
        let row = v.start_latched as usize + y as usize * v.pitch as usize;
        for (i, px) in out.iter_mut().enumerate() {
            let x = x0 as usize + i;
            let a = (row + x * bpp) % mlen;
            *px = match v.mode.bpp {
                8 => self.vga.dac_rgba(self.mem[a]),
                15 => {
                    let w = u16::from_le_bytes([self.mem[a], self.mem[(a + 1) % mlen]]);
                    let c = |s: u16| ((((w >> s) & 31) * 255 + 15) / 31) as u8;
                    [c(10), c(5), c(0), 255]
                }
                16 => {
                    let w = u16::from_le_bytes([self.mem[a], self.mem[(a + 1) % mlen]]);
                    let r = (((w >> 11) & 31) * 255 / 31) as u8;
                    let g = (((w >> 5) & 63) * 255 / 63) as u8;
                    let b = ((w & 31) * 255 / 31) as u8;
                    [r, g, b, 255]
                }
                _ => [
                    self.mem[(a + 2) % mlen],
                    self.mem[(a + 1) % mlen],
                    self.mem[a],
                    255,
                ],
            };
        }
    }

    fn border(&self) -> [u8; 4] {
        if self.vbe.is_some() {
            [0, 0, 0, 255]
        } else {
            self.vga.border()
        }
    }
}
