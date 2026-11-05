//! An EGA side-scroller in mode 0Dh (320x200, 16 colors), the way Commander Keen made smooth
//! scrolling possible on a card with no hardware sprites or scroll registers worth the name.
//!
//! * **A virtual screen wider than the display.** The CRTC offset register sets the row pitch
//!   to 44 bytes (352 pixels) while only 40 are shown, so there is room for the next tile
//!   column just off the right edge.
//! * **Start address + pel panning.** The start address moves the picture in 8-pixel steps;
//!   the attribute controller's pel panning register shifts it by 0-7 more pixels. The start
//!   address is latched at vertical retrace, pel panning takes effect immediately, so the start
//!   address is written during the display and the panning *after* waiting for retrace.
//!   `J` does it in the wrong order: the two get out of step and the picture jitters.
//! * **Endless scrolling with a moving buffer.** Scrolling right adds 1 to the start address
//!   per 8 pixels. The byte just right of row `y` is the byte just left of row `y + 1`, which
//!   has already scrolled out of view, so new tile columns can be drawn there. When the buffer
//!   reaches the end of its area, the visible part is copied back to the start in one go.
//! * **Latch copies.** The tiles live in off-screen video memory at `A800:0000`. Reading a byte
//!   loads all four planes into the latches, and a write in write mode 1 stores all four, so a
//!   16x16 tile moves with 32 reads and 32 writes instead of 128 pixel operations. That
//!   bandwidth (the bus is a 16-bit ISA bus here) is what made EGA scrollers possible at all.
//! * **The 200-line palette.** `P` swaps the attribute palette to a night scene. In 200-line
//!   modes an EGA monitor reads palette values as CGA-style RGBI (bit 4 = intensity), so only
//!   the 16 CGA colors exist even though the card could generate 64.
//!
//! Keys: `←` / `→` scroll speed, `J` jitter (wrong register order), `P` day / night palette,
//! `Esc` quit.

use game_toolkit_dosvideo::prelude::*;

/// Row pitch in bytes (the offset register holds half of it).
const PITCH: u32 = 44;
/// Tiles live here (offset into the 64 KiB plane window).
const TILE_BASE: u32 = 0x8000;
/// The scrolling buffer may use addresses up to here (Keen used nearly all of the 64 KiB;
/// a smaller area makes the jump back happen often enough to watch it in the overlay).
const BUFFER_END: u32 = 0x4000;
const ROWS: u32 = 13; // 12.5 tile rows fill 200 lines

#[derive(Clone, Copy)]
enum Tile {
    Sky = 0,
    Cloud,
    Grass,
    Dirt,
    Brick,
    Block,
    Bush,
    CloudEdge,
}

struct Scroller {
    /// Scroll position in pixels (world coordinates).
    scroll: u32,
    speed: u32,
    /// World byte at video byte 0 (grows on every jump back).
    shift: u32,
    /// World byte up to which tile columns are drawn.
    drawn: u32,
    jumps: u32,
    jitter: bool,
    night: bool,
    vram_ns: f64,
}

fn tile_bitmap(t: Tile) -> Bitmap {
    let mut b = Bitmap::new(16, 16);
    let sky = color::LIGHT_BLUE;
    match t {
        Tile::Sky => b.clear(sky),
        Tile::Cloud | Tile::CloudEdge => {
            b.clear(sky);
            let r = if matches!(t, Tile::Cloud) { 9 } else { 6 };
            b.circle(8, 10, r, color::WHITE, true);
            b.circle(4, 12, 4, color::LIGHT_GRAY, true);
            b.circle(8, 10, r - 2, color::WHITE, true);
        }
        Tile::Grass => {
            b.clear(color::BROWN);
            b.fill_rect(0, 0, 16, 5, color::GREEN);
            for x in (0..16).step_by(3) {
                b.pset(x, 5, color::GREEN);
                b.pset(x + 1, 0, color::LIGHT_GREEN);
            }
            b.pset(4, 10, color::DARK_GRAY);
            b.pset(11, 13, color::DARK_GRAY);
        }
        Tile::Dirt => {
            b.clear(color::BROWN);
            for (x, y) in [(2, 3), (9, 1), (13, 7), (5, 11), (11, 14), (1, 9)] {
                b.pset(x, y, color::DARK_GRAY);
            }
        }
        Tile::Brick => {
            b.clear(color::RED);
            for y in [0, 4, 8, 12] {
                b.hline(0, 15, y, color::LIGHT_GRAY);
            }
            for y in 0..16 {
                let off = if (y / 4) % 2 == 0 { 3 } else { 11 };
                b.pset(off, y, color::LIGHT_GRAY);
            }
        }
        Tile::Block => {
            b.clear(color::YELLOW);
            b.rect(0, 0, 16, 16, color::BROWN);
            b.text(4, 0, "?", color::BROWN, None, Font::vga());
        }
        Tile::Bush => {
            b.clear(sky);
            b.circle(5, 12, 5, color::GREEN, true);
            b.circle(11, 11, 5, color::GREEN, true);
            b.circle(8, 9, 4, color::LIGHT_GREEN, true);
        }
    }
    b
}

/// Deterministic noise for the level generator.
fn hash(k: u32) -> u32 {
    let mut x = k.wrapping_mul(0x9E37_79B9) ^ 0x5bd1_e995;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2c1b_3c6d);
    x ^ (x >> 12)
}

/// The tile at world column `k`, row `j`.
fn level(k: u32, j: u32) -> Tile {
    let ground = 9 + (hash(k / 4) % 3);
    let h = hash(k);
    if j >= ground {
        return if j == ground { Tile::Grass } else { Tile::Dirt };
    }
    if j == ground - 1 && h.is_multiple_of(5) {
        return Tile::Bush;
    }
    // Floating platforms every so often.
    let seg = k / 12;
    if hash(seg + 1000).is_multiple_of(2) && (3..8).contains(&(k % 12)) && j == 5 {
        return if k % 12 == 5 {
            Tile::Block
        } else {
            Tile::Brick
        };
    }
    // Clouds.
    let cs = k / 7;
    if (j == 1 || j == 2) && hash(cs + 7).is_multiple_of(3) {
        let pos = k % 7;
        let row = 1 + hash(cs + 9) % 2;
        if j == row && (2..5).contains(&pos) {
            return if pos == 3 {
                Tile::Cloud
            } else {
                Tile::CloudEdge
            };
        }
    }
    Tile::Sky
}

impl Scroller {
    /// Write the tiles into off-screen video memory, plane by plane (once, at startup).
    fn upload_tiles(dos: &mut Dos) {
        let all = [
            Tile::Sky,
            Tile::Cloud,
            Tile::Grass,
            Tile::Dirt,
            Tile::Brick,
            Tile::Block,
            Tile::Bush,
            Tile::CloudEdge,
        ];
        for t in all {
            let bmp = tile_bitmap(t);
            for plane in 0..4u8 {
                dos.map_mask(1 << plane);
                let mut bytes = [0u8; 32];
                for y in 0..16u32 {
                    for x in 0..16u32 {
                        let c = bmp.get(x as i32, y as i32);
                        if (c >> plane) & 1 != 0 {
                            bytes[(y * 2 + x / 8) as usize] |= 0x80 >> (x % 8);
                        }
                    }
                }
                dos.poke_slice(0xA0000 + TILE_BASE + t as u32 * 32, &bytes);
            }
        }
        dos.map_mask(0x0F);
    }

    /// Copy world tile column `k` into the buffer with latch copies.
    fn draw_column(&mut self, dos: &mut Dos, k: u32) {
        let vb = k * 2 - self.shift;
        let t0 = dos.now_ns();
        dos.map_mask(0x0F);
        dos.write_mode(1);
        for j in 0..ROWS {
            let tile = level(k, j) as u32;
            let lines = if j == ROWS - 1 { 8 } else { 16 };
            for r in 0..lines {
                let src = 0xA0000 + TILE_BASE + tile * 32 + r * 2;
                let dst = 0xA0000 + vb + (j * 16 + r) * PITCH;
                dos.copy_vram(src, dst, 2);
            }
        }
        dos.write_mode(0);
        self.vram_ns += dos.now_ns() - t0;
    }

    fn start_byte(&self) -> u32 {
        self.scroll / 8 - self.shift
    }
}

impl DosGame for Scroller {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        dos.set_mode(0x0D);
        dos.set_offset((PITCH / 2) as u8);
        Self::upload_tiles(dos);
        let mut s = Self {
            scroll: 0,
            speed: 1,
            shift: 0,
            drawn: 0,
            jumps: 0,
            jitter: false,
            night: false,
            vram_ns: 0.0,
        };
        while s.drawn < PITCH {
            s.draw_column(dos, s.drawn / 2);
            s.drawn += 2;
        }
        Ok(s)
    }

    fn step(&mut self, dos: &mut Dos) {
        while let Some(k) = dos.getch() {
            match k.scan {
                sc::ESC => dos.exit(),
                sc::RIGHT => self.speed = (self.speed + 1).min(8),
                sc::LEFT => self.speed = self.speed.saturating_sub(1),
                sc::J => self.jitter = !self.jitter,
                sc::P => {
                    self.night = !self.night;
                    // 200-line EGA: palette values are RGBI with intensity in bit 4.
                    let rgbi = |c: u8| (c & 7) | ((c & 8) << 1);
                    let night: [u8; 16] = [0, 0, 0, 1, 4, 5, 6, 8, 0, 1, 2, 3, 4, 5, 6, 7];
                    for i in 0..16u8 {
                        let c = if self.night { night[i as usize] } else { i };
                        dos.set_attr_palette(i, rgbi(c));
                    }
                }
                _ => {}
            }
        }
        self.vram_ns = 0.0;

        self.scroll += self.speed;
        // Draw the tile columns that will be needed next, just off the right edge.
        while self.drawn < self.scroll / 8 + PITCH {
            let k = self.drawn / 2;
            self.draw_column(dos, k);
            self.drawn += 2;
        }
        // At the end of the buffer area: copy the visible part back to the start.
        if self.start_byte() + (200 + 16) * PITCH > BUFFER_END {
            let start = self.start_byte();
            let t0 = dos.now_ns();
            dos.map_mask(0x0F);
            dos.write_mode(1);
            dos.copy_vram(0xA0000 + start, 0xA0000, (200 * PITCH) as usize);
            dos.write_mode(0);
            self.vram_ns += dos.now_ns() - t0;
            self.shift += start;
            self.jumps += 1;
        }

        let start = self.start_byte() as u16;
        let pan = (self.scroll % 8) as u8;
        dos.set_start_address(start);
        if self.jitter {
            // Wrong: the panning changes now, the start address only at the next retrace.
            dos.set_pel_pan(pan);
            dos.wait_vretrace();
        } else {
            dos.wait_vretrace();
            dos.set_pel_pan(pan);
        }
    }

    fn overlay(&mut self, dos: &Dos, p: &mut Painter<'_>, screen: [f32; 4]) {
        let s = format!(
            "EGA 0Dh {:.2} Hz  scroll {:>6} px  start {:04X}h pan {}  jumps {}  VRAM {:.2} ms  speed {}  J jitter:{}  P {}",
            dos.refresh_hz(),
            self.scroll,
            dos.start_address(),
            self.scroll % 8,
            self.jumps,
            self.vram_ns / 1e6,
            self.speed,
            if self.jitter { "on" } else { "off" },
            if self.night { "night" } else { "day" },
        );
        p.text(
            [screen[0] + 8.0, screen[1] + screen[3] + 8.0],
            &s,
            15.0,
            [0.7, 0.7, 0.7, 1.0],
        );
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<Scroller>(DosConfig::new("EGA scroller", Adapter::Ega))
}
