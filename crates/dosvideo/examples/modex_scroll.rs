//! Mode X: hardware scrolling, a split-screen status bar, and page-flipped sprites.
//!
//! The playfield is a 448x216 virtual screen, wider than the 320 pixels on display: the CRTC
//! offset register is set to 56 words (112 bytes per row and plane), so a row of video memory
//! is longer than a row on screen. Scrolling then costs nothing - the program only moves the
//! CRTC start address (in 4-pixel steps) and the attribute controller's pel panning register
//! (the remaining 0-3 pixels). The start address is latched at vertical retrace and pel
//! panning acts at once, so the start address is written during the display and pel panning
//! right after the retrace begins.
//!
//! The status bar is a split screen: the line compare register makes the CRTC restart at
//! address 0 for the bottom 24 rows, where the bar is stored. With attribute mode bit 5 set,
//! pel panning is ignored below the split, so the bar stands still while the playfield
//! scrolls. Turn that bit off and the bar wobbles along.
//!
//! Two copies of the playfield are double buffered: the sprites are erased (restored from a
//! copy in system memory) and redrawn on the hidden page, then the pages flip.
//!
//! Keys: `T` tearing mode (draw on the visible page, no waiting for retrace: sprites flicker,
//! scrolling jitters), `S` split screen on/off, `R` pel-panning reset below the split on/off,
//! `Esc` quit.

use game_toolkit_dosvideo::prelude::*;

/// Virtual screen width in pixels.
const W: i32 = 448;
/// Bytes per row in each plane.
const PITCH: u32 = W as u32 / 4;
const VIEW_W: i32 = 320;
const PLAY_H: i32 = 216;
const STATUS_H: i32 = 24;
/// The status bar lives at address 0, where the CRTC restarts after the split.
const STATUS_BASE: u32 = 0;
/// The two playfield pages, right after the status bar.
const PAGE_BASE: [u32; 2] = [
    STATUS_H as u32 * PITCH,
    STATUS_H as u32 * PITCH + PLAY_H as u32 * PITCH,
];
const MAX_SCROLL: i32 = W - VIEW_W;
/// Mode X is double scanned (480 scanlines for 240 rows), and the split starts on the line
/// *after* the line compare value.
const SPLIT_LINE: u16 = ((240 - STATUS_H) * 2 - 1) as u16;
const SPLIT_OFF: u16 = 0x3FF;
const SPRITE: i32 = 16;
const SKY: u8 = 200;

#[derive(Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
}

struct Scroller {
    field: Bitmap,
    status: Bitmap,
    balls: Vec<Bitmap>,
    /// Sprite rectangles drawn on each page, to erase next time that page is drawn.
    dirty: [Vec<Rect>; 2],
    draw_page: usize,
    tearing: bool,
    split: bool,
    pan_reset: bool,
    scroll_x: i32,
}

/// Copy a `w` x `h` region of `src` at `(sx, sy)` into planar video memory at `base`
/// (a `PITCH`-wide surface `max_h` rows high) at `(dx, dy)`. One map mask write per plane,
/// then runs of the pixels that live in that plane (every fourth column).
#[allow(clippy::too_many_arguments)]
fn put(
    dos: &mut Dos,
    base: u32,
    max_h: i32,
    src: &Bitmap,
    (sx, sy, w, h): (i32, i32, i32, i32),
    dx: i32,
    dy: i32,
    transparent: Option<u8>,
) {
    for plane in 0..4i32 {
        dos.map_mask(1 << plane);
        for row in 0..h {
            let y = dy + row;
            if y < 0 || y >= max_h {
                continue;
            }
            let row_addr = 0xA0000 + base + y as u32 * PITCH;
            let mut run: Vec<u8> = Vec::with_capacity(w as usize / 4 + 1);
            let mut start = None;
            let mut xx = (plane - dx).rem_euclid(4);
            while xx < w {
                let x = dx + xx;
                let c = src.get(sx + xx, sy + row);
                if (0..W).contains(&x) && Some(c) != transparent {
                    start.get_or_insert(x as u32 / 4);
                    run.push(c);
                } else if let Some(s) = start.take() {
                    dos.poke_slice(row_addr + s, &run);
                    run.clear();
                }
                xx += 4;
            }
            if let Some(s) = start {
                dos.poke_slice(row_addr + s, &run);
            }
        }
    }
    dos.map_mask(0x0F);
}

fn playfield() -> Bitmap {
    let mut b = Bitmap::new(W as u32, PLAY_H as u32);
    // Sky gradient in custom palette entries.
    for y in 0..120 {
        b.hline(0, W - 1, y, SKY + (y / 8) as u8);
    }
    // Two layers of hills.
    for x in 0..W {
        let far = 95.0 + (x as f32 * 0.035).sin() * 14.0 + (x as f32 * 0.09).cos() * 5.0;
        for y in far as i32..140 {
            b.pset(x, y, 120);
        }
        let near = 120.0 + (x as f32 * 0.06 + 1.0).sin() * 10.0;
        for y in near as i32..140 {
            b.pset(x, y, 2);
        }
    }
    // Checkered ground.
    for y in 140..PLAY_H {
        for x in 0..W {
            let c = if ((x / 16) + (y / 16)) % 2 == 0 {
                6
            } else {
                114
            };
            b.pset(x, y, c);
        }
    }
    // Posts with their x coordinate, so the scrolling is easy to read.
    let font = Font::cga();
    for x in (0..W).step_by(32) {
        b.fill_rect(x, 128, 2, 30, 15);
        b.text(x + 3, 128, &format!("{x}"), 14, None, font);
    }
    b.text(
        8,
        8,
        "MODE X 320x240 - VIRTUAL SCREEN 448 WIDE",
        15,
        None,
        font,
    );
    b
}

fn status_bar(scroll_x: i32) -> Bitmap {
    let mut b = Bitmap::new(W as u32, STATUS_H as u32);
    b.clear(1);
    b.hline(0, W - 1, 0, 9);
    let font = Font::cga();
    b.text(4, 4, "SCORE 001990  LIVES 3  SPLIT SCREEN", 14, None, font);
    b.text(
        4,
        14,
        &format!("LINE COMPARE {SPLIT_LINE}  SCROLL X {scroll_x:3}"),
        11,
        None,
        font,
    );
    b
}

fn ball(color: u8) -> Bitmap {
    let mut b = Bitmap::new(SPRITE as u32, SPRITE as u32);
    b.circle(7, 7, 7, color, true);
    b.circle(5, 5, 2, 15, true);
    b
}

/// A triangle wave bouncing between 0 and `span`.
fn bounce(t: f64, speed: f64, span: i32) -> i32 {
    let p = (t * speed).rem_euclid(2.0 * span as f64);
    (if p > span as f64 {
        2.0 * span as f64 - p
    } else {
        p
    }) as i32
}

fn scroll_x(t: f64) -> i32 {
    ((0.5 - 0.5 * (t * 0.7).cos()) * MAX_SCROLL as f64).round() as i32
}

impl Scroller {
    fn sprites(&self, t: f64) -> Vec<(i32, i32, usize)> {
        (0..self.balls.len())
            .map(|i| {
                let fi = i as f64;
                let x = bounce(t + fi * 1.3, 90.0 + fi * 23.0, W - SPRITE);
                let y = bounce(t + fi * 0.7, 60.0 + fi * 17.0, PLAY_H - SPRITE);
                (x, y, i)
            })
            .collect()
    }

    fn erase(&mut self, dos: &mut Dos, page: usize) {
        for r in std::mem::take(&mut self.dirty[page]) {
            let region = (r.x, r.y, SPRITE, SPRITE);
            put(
                dos,
                PAGE_BASE[page],
                PLAY_H,
                &self.field,
                region,
                r.x,
                r.y,
                None,
            );
        }
    }

    fn draw_sprites(&mut self, dos: &mut Dos, page: usize, t: f64) {
        for (x, y, i) in self.sprites(t) {
            let region = (0, 0, SPRITE, SPRITE);
            put(
                dos,
                PAGE_BASE[page],
                PLAY_H,
                &self.balls[i],
                region,
                x,
                y,
                Some(0),
            );
            self.dirty[page].push(Rect { x, y });
        }
    }

    fn update_status(&mut self, dos: &mut Dos) {
        self.status = status_bar(self.scroll_x);
        let region = (0, 0, VIEW_W, STATUS_H);
        put(dos, STATUS_BASE, STATUS_H, &self.status, region, 0, 0, None);
    }

    fn apply_split(&self, dos: &mut Dos) {
        dos.set_line_compare(if self.split { SPLIT_LINE } else { SPLIT_OFF });
        // Attribute mode control: 8-bit color + graphics, bit 5 = no pel panning below split.
        dos.attr(0x10, if self.pan_reset { 0x61 } else { 0x41 });
    }
}

impl DosGame for Scroller {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        ModeX::set(dos, 240).ok_or_else(|| anyhow::anyhow!("Mode X needs a VGA"))?;
        // Widen the virtual screen: offset is in words, two bytes per word in byte mode.
        dos.set_offset((PITCH / 2) as u8);
        for i in 0..16u8 {
            dos.set_palette(SKY + i, [8 + i * 2, 16 + i * 2, 40 + i]);
        }
        let field = playfield();
        for base in PAGE_BASE {
            put(dos, base, PLAY_H, &field, (0, 0, W, PLAY_H), 0, 0, None);
        }
        let mut s = Self {
            field,
            status: status_bar(0),
            balls: [40, 48, 32, 44, 36].into_iter().map(ball).collect(),
            dirty: [Vec::new(), Vec::new()],
            draw_page: 1,
            tearing: false,
            split: true,
            pan_reset: true,
            scroll_x: 0,
        };
        s.update_status(dos);
        s.apply_split(dos);
        dos.set_start_address(PAGE_BASE[0] as u16);
        Ok(s)
    }

    fn step(&mut self, dos: &mut Dos) {
        while let Some(k) = dos.getch() {
            match k.scan {
                sc::ESC => dos.exit(),
                sc::T => {
                    self.tearing = !self.tearing;
                    // Start clean: both pages without sprites, draw on page 0.
                    self.erase(dos, 0);
                    self.erase(dos, 1);
                    self.draw_page = if self.tearing { 0 } else { 1 };
                }
                sc::S => {
                    self.split = !self.split;
                    self.apply_split(dos);
                }
                sc::R => {
                    self.pan_reset = !self.pan_reset;
                    self.apply_split(dos);
                }
                _ => {}
            }
        }

        let t = dos.now();
        let x = scroll_x(t);
        let page = self.draw_page;
        if self.tearing {
            // Single buffered and unsynchronized: erase, "think", draw, all while the beam
            // may be passing over the sprites; then scroll whenever we get to it.
            self.erase(dos, page);
            dos.cpu(120_000);
            self.draw_sprites(dos, page, t);
            dos.set_start_address((PAGE_BASE[page] + x as u32 / 4) as u16);
            dos.set_pel_pan(((x & 3) * 2) as u8);
            self.scroll_x = x;
            dos.cpu(100_000);
        } else {
            self.erase(dos, page);
            self.draw_sprites(dos, page, t);
            // Start address during the display: latched at the next retrace...
            dos.set_start_address((PAGE_BASE[page] + x as u32 / 4) as u16);
            dos.wait_vretrace();
            // ...and pel panning right after it, so both change for the same frame.
            dos.set_pel_pan(((x & 3) * 2) as u8);
            self.scroll_x = x;
            self.draw_page ^= 1;
        }
        if dos.frames().is_multiple_of(8) {
            self.update_status(dos);
        }
    }

    fn overlay(&mut self, dos: &Dos, p: &mut Painter<'_>, screen: [f32; 4]) {
        let lines = [
            format!(
                "Mode X {}x{}  {:.2} Hz   scroll x {:3} = start +{} pan {}",
                VIEW_W,
                240,
                dos.refresh_hz(),
                self.scroll_x,
                self.scroll_x / 4,
                self.scroll_x & 3
            ),
            format!(
                "T tearing:{}  S split:{}  R pan reset below split:{}  Esc quit",
                on(self.tearing),
                on(self.split),
                on(self.pan_reset)
            ),
        ];
        for (i, s) in lines.iter().enumerate() {
            let y = screen[1] + screen[3] + 6.0 + i as f32 * 18.0;
            p.text([screen[0] + 8.0, y], s, 16.0, [0.7, 0.7, 0.7, 1.0]);
        }
    }
}

fn on(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<Scroller>(DosConfig::new("Mode X scroll", Adapter::Vga))
}
