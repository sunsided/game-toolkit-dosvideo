//! Mode 13h raster bars, palette cycling and a sine scroller.
//!
//! The "sky" is color 0. Every scanline the program rewrites DAC entry 0, so the bars show
//! wherever color 0 is, at 400-line resolution (mode 13h is double-scanned: each pixel row is
//! two scanlines, and the DAC does not care).
//!
//! *When* the write happens is everything. `T` cycles three versions of the raster loop:
//!
//! - **hblank** (the proper one): write during the horizontal blank of the *previous* line.
//!   The three port writes to 0x3C9 are done before the line starts: clean bars.
//! - **line start** (naive): wait for the line, then write. The writes take over a
//!   microsecond, the beam is already drawing, and the first ~15 pixels of every line still
//!   show the old color: a ragged stripe down the left side.
//! - **jittery** (too slow): the routine does some work before each write that takes a
//!   varying amount of time, so every line changes color somewhere else: frayed edges.
//!
//! The overscan border normally shows a black DAC entry. Point it at entry 0 (`O`) and the
//! bars run into the border, a classic demo trick. Note the left border is scanned at the end
//! of the previous line, so there each bar starts one scanline late, as on real hardware.
//!
//! Keys: `B` bars on/off, `T` raster timing, `O` bars in the overscan border, `P` palette
//! cycling on/off, `Esc` quit.

use game_toolkit_dosvideo::prelude::*;

const FLOOR_FIRST: u8 = 64;
const FLOOR_LAST: u8 = 79;
const LOGO_FIRST: u8 = 96;
const LOGO_LAST: u8 = 127;
const SCROLL_TOP: u32 = 150;
/// A DAC entry the default palette leaves black, for the border.
const BLACK: u8 = 255;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RasterTiming {
    Hblank,
    LineStart,
    Jittery,
}

impl RasterTiming {
    fn next(self) -> Self {
        match self {
            Self::Hblank => Self::LineStart,
            Self::LineStart => Self::Jittery,
            Self::Jittery => Self::Hblank,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Hblank => "hblank (proper)",
            Self::LineStart => "line start (naive)",
            Self::Jittery => "jittery (too slow)",
        }
    }
}

struct Copper {
    background: Bitmap,
    back: Bitmap,
    t: u32,
    bars: bool,
    timing: RasterTiming,
    border_bars: bool,
    cycle: bool,
    rng: u32,
}

fn bar_color(line: u32, t: u32) -> [u8; 3] {
    // Three bars bouncing on sines, each a little gradient.
    let mut c = [0u8; 3];
    let tints = [[63, 20, 10], [10, 50, 63], [50, 63, 20]];
    for (i, tint) in tints.iter().enumerate() {
        let phase = t as f32 * 0.035 + i as f32 * 2.1;
        let center = 150.0 + phase.sin() * 110.0;
        let d = (line as f32 - center).abs();
        if d < 14.0 {
            let k = 1.0 - d / 14.0;
            // Every other scanline a shade darker: proof the bars have 400-line resolution
            // in a 200-line mode, and any write that lands a line late flips the stripes.
            let k = if line % 2 == 1 { k * 0.7 } else { k };
            for ch in 0..3 {
                c[ch] = c[ch].max((tint[ch] as f32 * k) as u8);
            }
        }
    }
    c
}

impl DosGame for Copper {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        dos.set_mode(0x13);
        dos.set_border(BLACK);
        // Floor stripes: two shades alternating in 16 entries, cycled every frame.
        for i in 0..16u8 {
            let v = if i < 8 { 12 + i * 3 } else { 8 };
            dos.set_palette(FLOOR_FIRST + i, [v / 3, v / 2, v]);
        }
        // Logo: a 32-entry rainbow that the cycling runs through.
        for i in 0..32u8 {
            let h = i as f32 / 32.0 * std::f32::consts::TAU;
            let ch = |o: f32| ((h + o).sin() * 0.5 + 0.5) * 63.0;
            dos.set_palette(
                LOGO_FIRST + i,
                [ch(0.0) as u8, ch(2.1) as u8, ch(4.2) as u8],
            );
        }
        // Mountains in grays 16..31 of the default palette.
        let mut bg = Bitmap::new(320, 200);
        for x in 0..320i32 {
            let h = 110.0 + (x as f32 * 0.03).sin() * 18.0 + (x as f32 * 0.11).sin() * 7.0;
            for y in h as i32..140 {
                bg.pset(x, y, 18 + ((y - h as i32) / 6).min(10) as u8);
            }
        }
        // A floor in perspective: rows further away get thinner stripes.
        for y in 140..200i32 {
            let z = 600.0 / (y - 130) as f32;
            for x in 0..320i32 {
                let u = ((x - 160) as f32 * z / 60.0).floor() as i32;
                let v = (z * 4.0) as i32;
                let stripe = ((u + v).rem_euclid(16)) as u8;
                bg.pset(x, y, FLOOR_FIRST + stripe);
            }
        }
        // The logo: the 8x16 font blown up 4x, each column its own rainbow entry.
        let font = Font::vga();
        let text = "DOSVIDEO";
        for (i, ch) in text.bytes().enumerate() {
            let g = font.glyph(ch);
            for (r, row) in g.iter().enumerate() {
                for b in 0..8 {
                    if (row >> (7 - b)) & 1 != 0 {
                        let x0 = 32 + (i as i32 * 8 + b) * 4;
                        let y0 = 20 + r as i32 * 4;
                        let c = LOGO_FIRST + ((x0 / 4) % 32) as u8;
                        bg.fill_rect(x0, y0, 4, 4, c);
                    }
                }
            }
        }
        dos.blit13(&bg);
        Ok(Self {
            back: bg.clone(),
            background: bg,
            t: 0,
            bars: true,
            timing: RasterTiming::Hblank,
            border_bars: false,
            cycle: true,
            rng: 1,
        })
    }

    fn step(&mut self, dos: &mut Dos) {
        while let Some(k) = dos.getch() {
            match k.scan {
                sc::ESC => dos.exit(),
                sc::B => self.bars = !self.bars,
                sc::T => self.timing = self.timing.next(),
                sc::O => {
                    self.border_bars = !self.border_bars;
                    dos.set_border(if self.border_bars { 0 } else { BLACK });
                }
                sc::P => self.cycle = !self.cycle,
                _ => {}
            }
        }

        // 1. Race the beam: a new color 0 for each of the 400 scanlines.
        let timing = dos.timing();
        if self.bars {
            for line in 0..timing.v_active {
                let prev = if line == 0 {
                    timing.v_total - 1
                } else {
                    line - 1
                };
                match self.timing {
                    // Hblank of the line before: the color is in place when this line starts.
                    RasterTiming::Hblank => dos.wait_beam(prev, timing.h_active),
                    // Too late: the beam is already drawing this line while we write.
                    RasterTiming::LineStart => dos.wait_line(line),
                    // Some work of varying length between the wait and the write.
                    RasterTiming::Jittery => {
                        dos.wait_beam(prev, timing.h_active);
                        self.rng = self.rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
                        dos.cpu(80 + ((self.rng >> 16) % 900) as u64);
                    }
                }
                let c = bar_color(line, self.t);
                dos.set_palette(0, c);
            }
            dos.set_palette(0, [0, 0, 0]);
        }

        // 2. Below the picture: cycle palettes and update the scroller. Only the band the
        //    scroller lives in is copied - a whole 64000-byte frame would not fit into the
        //    vertical blank, but the rest of the picture is animated by palette cycling alone.
        dos.wait_vblank();
        if self.cycle {
            dos.cycle_palette(FLOOR_FIRST, FLOOR_LAST);
            dos.cycle_palette(LOGO_FIRST, LOGO_LAST);
        }
        self.back.data.copy_from_slice(&self.background.data);
        let msg =
            "   GREETINGS FROM THE VGA DAC ... 400 SCANLINES OF COLOR 0 ... PRESS B, J, P ...   ";
        let font = Font::vga();
        let scroll = (self.t * 2) as i32;
        let w = msg.len() as i32 * 8;
        for (i, ch) in msg.chars().enumerate() {
            let x = i as i32 * 8 - scroll % w;
            for xx in [x, x + w] {
                if (-8..320).contains(&xx) {
                    let y = 168 + ((xx as f32 * 0.04 + self.t as f32 * 0.08).sin() * 10.0) as i32;
                    self.back.text(xx, y, &ch.to_string(), 15, None, font);
                }
            }
        }
        let band = (SCROLL_TOP * 320) as usize..(200 * 320) as usize;
        dos.poke_slice(0xA0000 + SCROLL_TOP * 320, &self.back.data[band]);
        self.t += 1;
    }

    fn overlay(&mut self, dos: &Dos, p: &mut Painter<'_>, screen: [f32; 4]) {
        let s = format!(
            "mode 13h  {:.2} Hz   B bars:{}  T timing: {}  O border:{}  P cycle:{}",
            dos.refresh_hz(),
            on(self.bars),
            self.timing.name(),
            on(self.border_bars),
            on(self.cycle)
        );
        p.text(
            [screen[0] + 8.0, screen[1] + screen[3] + 8.0],
            &s,
            16.0,
            [0.7, 0.7, 0.7, 1.0],
        );
    }
}

fn on(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<Copper>(DosConfig::new("copper bars", Adapter::Vga))
}
