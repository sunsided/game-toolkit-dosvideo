//! Tandy 1000 / PCjr graphics: 16 colors at 320x200, from 1984 on, years before EGA was
//! affordable (and what Sierra's and many other games' "Tandy" option meant).
//!
//! The memory is packed pixels, two per byte, but *four-way interleaved*: scanline `y` lives
//! in bank `y & 3` at `0x2000 * (y & 3) + (y >> 2) * 160`, because the 6845 runs with four
//! scanlines per character row and the row-scan counter selects the bank. The video gate
//! array adds 16 palette registers (index written to port 0x3DA, data to 0x3DE) that map the
//! pixel values to the 16 RGBI colors, so palette tricks work as on EGA.
//!
//! Only the rows the sprites touched are copied each frame: a full 32 KiB screen would take
//! about 32 ms on the 8-bit bus of a Tandy 1000.
//!
//! Keys: `M` mode 9 (320x200x16) / 8 (160x200x16) / 0Ah (640x200x4), `P` palette register
//! cycling, `F` flash (all palette registers to one color for a frame), `Esc` quit.

use game_toolkit_dosvideo::prelude::*;

const MODES: [u8; 3] = [9, 8, 0x0A];

struct Ball {
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
    color: u8,
}

struct Tandy {
    mode: usize,
    background: Bitmap,
    back: Bitmap,
    balls: Vec<Ball>,
    dirty: Vec<(i32, i32)>,
    cycle: bool,
    flash: u32,
    t: u32,
}

impl Tandy {
    fn bios_mode(&self) -> u8 {
        MODES[self.mode]
    }

    fn width(&self) -> u32 {
        match self.bios_mode() {
            8 => 160,
            9 => 320,
            _ => 640,
        }
    }

    fn colors(&self) -> u8 {
        if self.bios_mode() == 0x0A { 4 } else { 16 }
    }

    fn enter(&mut self, dos: &mut Dos) {
        dos.set_mode(self.bios_mode());
        let w = self.width();
        self.background = scene(w, self.colors());
        self.back = self.background.clone();
        for y in 0..200 {
            write_row(dos, self.bios_mode(), &self.back, y);
        }
        let n = 5;
        self.balls = (0..n)
            .map(|i| Ball {
                x: (20 + i * 50) as f32 * w as f32 / 320.0,
                y: (30 + i * 25) as f32,
                dx: (1.3 + i as f32 * 0.4) * w as f32 / 320.0,
                dy: 0.8 + i as f32 * 0.3,
                color: if self.colors() == 4 {
                    3
                } else {
                    [15, 14, 13, 11, 12][i as usize]
                },
            })
            .collect();
        self.dirty.clear();
    }

    /// Write palette register `i` through the gate array.
    fn palette(dos: &mut Dos, i: u8, v: u8) {
        dos.outb(0x3DA, 0x10 + i);
        dos.outb(0x3DE, v);
    }
}

/// A landscape: sky bands, sun, hills, sea. `colors` is 16 or 4.
fn scene(w: u32, colors: u8) -> Bitmap {
    let mut b = Bitmap::new(w, 200);
    let sx = w as f32 / 320.0;
    let pick = |c16: u8, c4: u8| if colors == 4 { c4 } else { c16 };
    let sky = [1u8, 1, 9, 9, 3, 11, 11, 7];
    for y in 0..120 {
        let band = (y / 15) as usize;
        for x in 0..w as i32 {
            // Dither between two bands near the edge.
            let next = (band + 1).min(7);
            let edge = y % 15 >= 11 && (x + y) % 2 == 0;
            b.pset(x, y, pick(if edge { sky[next] } else { sky[band] }, 1));
        }
    }
    b.circle((240.0 * sx) as i32, 60, 22, pick(14, 2), true);
    b.circle((240.0 * sx) as i32, 60, 16, pick(15, 3), true);
    for x in 0..w as i32 {
        let fx = x as f32 / sx;
        let h1 = 100.0 + (fx * 0.02).sin() * 15.0 + (fx * 0.07).sin() * 5.0;
        let h2 = 125.0 + (fx * 0.035 + 1.0).sin() * 10.0;
        for y in h1 as i32..140 {
            b.pset(x, y, pick(if y < h1 as i32 + 3 { 10 } else { 2 }, 2));
        }
        for y in h2 as i32..150 {
            b.pset(x, y, pick(6, 2));
        }
        for y in 150..200 {
            let wave = ((fx * 0.2 + y as f32 * 0.7).sin() > 0.6) as u8;
            b.pset(
                x,
                y,
                pick(if wave == 1 { 9 } else { 1 }, if wave == 1 { 3 } else { 1 }),
            );
        }
    }
    let font = Font::cga();
    let title = if w >= 320 {
        "TANDY 1000 GRAPHICS"
    } else {
        "TANDY"
    };
    let tx = (w as i32 - title.len() as i32 * 8) / 2;
    b.text(tx, 6, title, pick(15, 3), None, font);
    b
}

/// Encode one row of `bmp` in the memory format of `mode` and copy it to video memory.
fn write_row(dos: &mut Dos, mode: u8, bmp: &Bitmap, y: u32) {
    let row = bmp.row(y);
    let bank = 0xB8000 + 0x2000 * (y & 3);
    match mode {
        8 | 9 => {
            let bytes: Vec<u8> = row.chunks(2).map(|c| (c[0] << 4) | (c[1] & 0x0F)).collect();
            let pitch = if mode == 9 { 160 } else { 80 };
            dos.poke_slice(bank + (y >> 2) * pitch, &bytes);
        }
        _ => {
            // 640x200x4: per 8 pixels, a byte of bit 0s then a byte of bit 1s.
            let bytes: Vec<u8> = row
                .chunks(8)
                .flat_map(|c| {
                    let plane = |p: u8| {
                        c.iter()
                            .enumerate()
                            .fold(0u8, |a, (i, &v)| a | (((v >> p) & 1) << (7 - i)))
                    };
                    [plane(0), plane(1)]
                })
                .collect();
            dos.poke_slice(bank + (y >> 2) * 160, &bytes);
        }
    }
}

impl DosGame for Tandy {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        let mut t = Self {
            mode: 0,
            background: Bitmap::new(1, 1),
            back: Bitmap::new(1, 1),
            balls: Vec::new(),
            dirty: Vec::new(),
            cycle: false,
            flash: 0,
            t: 0,
        };
        t.enter(dos);
        Ok(t)
    }

    fn step(&mut self, dos: &mut Dos) {
        while let Some(k) = dos.getch() {
            match k.scan {
                sc::ESC => dos.exit(),
                sc::M => {
                    self.mode = (self.mode + 1) % MODES.len();
                    self.enter(dos);
                }
                sc::P => self.cycle = !self.cycle,
                sc::F => self.flash = 2,
                _ => {}
            }
        }
        let w = self.width() as i32;
        let r = (6 * w / 320).max(3);

        // Move the balls in the back buffer, remembering which rows changed.
        let mut rows: Vec<(i32, i32)> = std::mem::take(&mut self.dirty);
        for b in &mut self.balls {
            b.x += b.dx;
            b.y += b.dy;
            if b.x < r as f32 || b.x > (w - r) as f32 {
                b.dx = -b.dx;
            }
            if b.y < 20.0 || b.y > 190.0 {
                b.dy = -b.dy;
            }
            let (y0, y1) = (b.y as i32 - 8, b.y as i32 + 8);
            rows.push((y0, y1));
            self.dirty.push((y0, y1));
        }
        for (y0, y1) in &rows {
            for y in (*y0).max(0)..(*y1).min(200) {
                let w = self.back.width as usize;
                let o = y as usize * w;
                self.back.data[o..o + w].copy_from_slice(&self.background.data[o..o + w]);
            }
        }
        for b in &self.balls {
            // Ellipses: the pixels are wide in mode 8 and narrow in 0Ah.
            for dy in -6i32..=6 {
                let half = (((36 - dy * dy).max(0) as f32).sqrt() * r as f32 / 6.0) as i32;
                self.back.hline(
                    b.x as i32 - half,
                    b.x as i32 + half,
                    b.y as i32 + dy,
                    b.color,
                );
            }
            self.back.pset(
                b.x as i32 - r / 3,
                b.y as i32 - 3,
                if self.colors() == 4 { 0 } else { 7 },
            );
        }

        dos.wait_vretrace();
        let mut todo: Vec<u32> = rows
            .iter()
            .flat_map(|(a, b)| (*a).max(0)..(*b).min(200))
            .map(|y| y as u32)
            .collect();
        todo.sort_unstable();
        todo.dedup();
        let mode = self.bios_mode();
        for y in todo {
            write_row(dos, mode, &self.back, y);
        }

        // Palette register tricks.
        if self.flash > 0 {
            for i in 0..16 {
                Self::palette(dos, i, if self.flash == 2 { 15 } else { i });
            }
            self.flash -= 1;
        } else if self.cycle && self.colors() == 16 && self.t.is_multiple_of(6) {
            // Rotate the sky and sea colors through the registers: the pixels never change.
            let k = (self.t / 6) as u8;
            for i in 1..15u8 {
                Self::palette(dos, i, 1 + (i + k) % 14);
            }
        } else if !self.cycle && self.colors() == 16 && self.t.is_multiple_of(6) {
            for i in 0..16 {
                Self::palette(dos, i, i);
            }
        }
        self.t += 1;
    }

    fn overlay(&mut self, _dos: &Dos, p: &mut Painter<'_>, screen: [f32; 4]) {
        let (w, c) = (self.width(), self.colors());
        let s = format!(
            "Tandy mode {:02X}h: {w}x200x{c}   M mode  P palette cycling: {}  F flash  Esc quit",
            self.bios_mode(),
            if self.cycle { "on" } else { "off" }
        );
        p.text(
            [screen[0] + 8.0, screen[1] + screen[3] + 8.0],
            &s,
            16.0,
            [0.7, 0.7, 0.7, 1.0],
        );
    }
}

const KEYS: &[(&str, &str)] = &[
    (
        "M",
        "mode 9 (320x200x16) / 8 (160x200x16) / 0Ah (640x200x4)",
    ),
    ("P", "palette register cycling"),
    ("F", "flash"),
    ("Esc", "quit"),
];

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<Tandy>(DosConfig::new("Tandy 1000", Adapter::Tandy).with_keys(KEYS))
}
