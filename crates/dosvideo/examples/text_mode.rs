//! VGA text mode as a user interface *and* a graphics mode.
//!
//! An 80x25 screen of character/attribute cells at `B800:0000`: a menu bar, windows drawn with
//! the CP437 box characters, drop shadows (just the attribute of the cells next to a window
//! set to dark gray), a scrolling log, and the hardware cursor.
//!
//! The graphics trick: the VGA character generator reads its glyphs from plane 2 of video
//! memory, so a program can redefine characters. Text-mode games replaced rarely used
//! characters (here `Ç ü é â ä à`) with tiles: a brick wall, a tree, a coin, a hero, and water
//! whose glyph is rewritten every few frames, which animates every water cell on screen at
//! once without touching the text buffer.
//!
//! Attribute bit 7 either blinks the character or, with blinking off, selects one of 16
//! background colors instead of 8.
//!
//! The same trick can swap the whole alphabet: `G` loads the Standard Galactic Alphabet, the
//! alien script from the Commander Keen games, and every letter on screen turns galactic at
//! once (digits, punctuation and box drawing stay).
//!
//! Keys: arrows move the cursor (and the hero), `B` blink vs. bright backgrounds, `F` custom
//! font on/off, `G` galactic alphabet, `H` 80x25 / 80x50 (max scan line 7 and the 8x8 font),
//! `Esc` quit.

use game_toolkit_dosvideo::prelude::*;
use game_toolkit_dosvideo::text::{BOX_DOUBLE, BOX_SINGLE, attr};

const WALL: u8 = 0x80;
const TREE: u8 = 0x81;
const WATER: u8 = 0x82;
const HERO: u8 = 0x83;
const COIN: u8 = 0x84;
const GRASS: u8 = 0x85;

const MAP: [&str; 9] = [
    "##################################",
    "#..T...~~~~..........T....$......#",
    "#.....~~~~~~....###.......T......#",
    "#..$...~~~~.....#$#..........~~~.#",
    "#.T.............#.#....T....~~~~.#",
    "#.......T..........$........~~~..#",
    "#...###########...........T......#",
    "#.$............T......$..........#",
    "##################################",
];

fn tile(rows16: [u16; 16]) -> [u8; 16] {
    rows16.map(|r| r as u8)
}

/// Tile glyphs, 8x16. Written as binary rows.
fn tiles(water_phase: u32) -> Vec<(u8, [u8; 16])> {
    let wall = tile([
        0xFF, 0x08, 0x08, 0x08, 0xFF, 0x80, 0x80, 0x80, 0xFF, 0x08, 0x08, 0x08, 0xFF, 0x80, 0x80,
        0x80,
    ]);
    let tree = tile([
        0x18, 0x3C, 0x7E, 0x7E, 0xFF, 0xFF, 0xFF, 0x7E, 0x7E, 0x3C, 0x18, 0x18, 0x18, 0x18, 0x3C,
        0x00,
    ]);
    let hero = tile([
        0x00, 0x3C, 0x7E, 0x5A, 0x7E, 0x66, 0x3C, 0x18, 0xFF, 0x3C, 0x3C, 0x3C, 0x24, 0x24, 0x66,
        0x00,
    ]);
    let coin = tile([
        0x00, 0x00, 0x00, 0x3C, 0x7E, 0x66, 0xDB, 0xDB, 0xDB, 0xDB, 0x66, 0x7E, 0x3C, 0x00, 0x00,
        0x00,
    ]);
    let grass = tile([
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x04, 0x00,
        0x00,
    ]);
    // Water: a wave pattern, shifted sideways with the phase.
    let mut water = [0u8; 16];
    for (r, w) in water.iter_mut().enumerate() {
        let base: u8 = match r % 8 {
            1 => 0b0011_0000,
            2 => 0b0100_1000,
            3 => 0b1000_0110,
            5 => 0b0000_0011,
            6 => 0b0000_1100,
            _ => 0,
        };
        *w = base.rotate_right(water_phase % 8);
    }
    vec![
        (WALL, wall),
        (TREE, tree),
        (WATER, water),
        (HERO, hero),
        (COIN, coin),
        (GRASS, grass),
    ]
}

/// Build the font: VGA 8x16 or CGA 8x8 with the tiles patched in.
fn make_font(tall: bool, custom: bool, galactic: bool, phase: u32) -> Font {
    let base = match (tall, galactic) {
        (true, false) => Font::vga(),
        (false, false) => Font::cga(),
        (true, true) => Font::sga(),
        (false, true) => Font::sga_cga(),
    };
    let h = base.height() as usize;
    let mut raw = base.raw().to_vec();
    if custom {
        for (ch, g) in tiles(phase) {
            for r in 0..h {
                // 8-line font: take every other row of the 16-line tile.
                raw[ch as usize * h + r] = if tall { g[r] } else { g[r * 2] | g[r * 2 + 1] };
            }
        }
    }
    Font::from_raw(h as u8, &raw).expect("font size")
}

struct TextUi {
    blink: bool,
    custom: bool,
    galactic: bool,
    tall: bool,
    cursor: (u32, u32),
    log: Vec<String>,
    t: u32,
    phase: u32,
}

impl TextUi {
    fn rows(dos: &Dos) -> u32 {
        dos.text_size().1
    }

    fn shadow(dos: &mut Dos, x: u32, y: u32, w: u32, h: u32) {
        for yy in y + 1..=y + h {
            for xx in [x + w, x + w + 1] {
                let (c, _) = dos.text_get(xx, yy);
                dos.text_put(xx, yy, c, attr(color::DARK_GRAY, color::BLACK));
            }
        }
        for xx in x + 2..x + w + 2 {
            let (c, _) = dos.text_get(xx, y + h);
            dos.text_put(xx, y + h, c, attr(color::DARK_GRAY, color::BLACK));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn window(dos: &mut Dos, x: u32, y: u32, w: u32, h: u32, a: u8, chars: [u8; 6], title: &str) {
        dos.text_box(x, y, w, h, a, chars);
        Self::shadow(dos, x, y, w, h);
        let t = format!(" {title} ");
        dos.text_print(x + (w - t.len() as u32) / 2, y, &t, a);
    }

    /// Draw the whole screen.
    fn draw(&self, dos: &mut Dos) {
        let rows = Self::rows(dos);
        // Desktop: the classic light shade pattern.
        dos.text_fill(
            0,
            1,
            80,
            rows - 2,
            0xB1,
            attr(color::BLUE, color::LIGHT_GRAY),
        );
        // Menu bar.
        let bar = attr(color::BLACK, color::LIGHT_GRAY);
        dos.text_fill(0, 0, 80, 1, b' ', bar);
        let mut x = 2;
        for item in ["File", "Edit", "View", "Options", "Help"] {
            dos.text_print(x, 0, item, bar);
            dos.text_put(
                x,
                0,
                item.as_bytes()[0],
                attr(color::RED, color::LIGHT_GRAY),
            );
            x += item.len() as u32 + 3;
        }
        dos.text_print(64, 0, "DOSVIDEO 1.0", bar);

        // The map window, in custom characters.
        let map_a = attr(color::YELLOW, color::BLUE);
        Self::window(dos, 1, 2, 38, 12, map_a, BOX_DOUBLE, "Character set");
        self.draw_map(dos);

        // Attribute table: 16 backgrounds x 8 foreground samples, with bit 7 set on the
        // right half (blinks, or bright background).
        let a = attr(color::BLACK, color::CYAN);
        Self::window(dos, 42, 2, 36, 12, a, BOX_SINGLE, "Attributes");
        for bg in 0..8u8 {
            for fg in 0..8u8 {
                let at = (bg << 4) | (fg * 2 + 1);
                dos.text_put(44 + fg as u32 * 2, 3 + bg as u32, b'A' + fg, at);
                dos.text_put(60 + fg as u32 * 2, 3 + bg as u32, b'A' + fg, at | 0x80);
            }
        }
        dos.text_print(44, 12, "bit 7 off      bit 7 on", a);

        // Log window.
        let la = attr(color::LIGHT_GREEN, color::BLACK);
        let log_h = rows - 18;
        Self::window(dos, 1, 15, 77, log_h, la, BOX_SINGLE, "Log");
        self.draw_log(dos);

        // Status line.
        let sa = attr(color::BLACK, color::CYAN);
        dos.text_fill(0, rows - 1, 80, 1, b' ', sa);
        let mode = if self.tall { "80x25" } else { "80x50" };
        dos.text_print(
            1,
            rows - 1,
            &format!(
                "\u{2191}\u{2193}\u{2190}\u{2192} move  B blink:{}  F font:{}  G galactic  H {}  Esc quit",
                if self.blink { "on " } else { "off" },
                if self.custom { "on " } else { "off" },
                mode
            ),
            sa,
        );
    }

    fn draw_map(&self, dos: &mut Dos) {
        for (y, row) in MAP.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                let (c, a) = match ch {
                    '#' => (WALL, attr(color::LIGHT_RED, color::RED)),
                    'T' => (TREE, attr(color::LIGHT_GREEN, color::GREEN)),
                    '~' => (WATER, attr(color::LIGHT_CYAN, color::BLUE)),
                    '$' => (COIN, attr(color::YELLOW, color::GREEN)),
                    _ => (GRASS, attr(color::LIGHT_GREEN, color::GREEN)),
                };
                dos.text_put(3 + x as u32, 4 + y as u32, c, a);
            }
        }
        let (cx, cy) = self.cursor;
        if (3..37).contains(&cx) && (4..13).contains(&cy) {
            dos.text_put(cx, cy, HERO, attr(color::WHITE, color::GREEN));
        }
    }

    fn draw_log(&self, dos: &mut Dos) {
        let rows = Self::rows(dos);
        let inner = (rows - 18).saturating_sub(2) as usize;
        let la = attr(color::LIGHT_GREEN, color::BLACK);
        let start = self.log.len().saturating_sub(inner);
        for i in 0..inner {
            let line = self.log.get(start + i).map_or("", String::as_str);
            let s = format!("{line:<75}");
            dos.text_print(2, 16 + i as u32, &s[..75], la);
        }
    }

    fn log(&mut self, s: String) {
        self.log.push(s);
        if self.log.len() > 100 {
            self.log.remove(0);
        }
    }

    fn apply_font(&self, dos: &mut Dos) {
        dos.load_font(
            &make_font(self.tall, self.custom, self.galactic, self.phase),
            0,
        );
    }

    fn set_height(&mut self, dos: &mut Dos, tall: bool) {
        self.tall = tall;
        // Max scan line: 16 lines per row for 80x25, 8 for 80x50. Bit 6 is bit 9 of line
        // compare, keep it. The display stays 400 scanlines tall either way.
        dos.crtc(0x09, if tall { 0x4F } else { 0x47 });
        if tall {
            dos.cursor_shape(13, 14);
        } else {
            dos.cursor_shape(6, 7);
        }
        self.apply_font(dos);
        self.cursor.1 = self.cursor.1.min(Self::rows(dos) - 2);
        self.draw(dos);
        let (x, y) = self.cursor;
        dos.set_cursor(x, y);
    }
}

impl DosGame for TextUi {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        dos.set_mode(0x03);
        let mut ui = Self {
            blink: true,
            custom: true,
            galactic: false,
            tall: true,
            cursor: (10, 8),
            log: Vec::new(),
            t: 0,
            phase: 0,
        };
        ui.log("C:\\> DOSVIDEO.EXE".into());
        ui.log("VGA text mode 03h: 720x400, 9-dot characters, 70 Hz".into());
        ui.log("Font loaded into plane 2; characters 80h-85h redefined as tiles".into());
        ui.apply_font(dos);
        ui.draw(dos);
        dos.set_cursor(10, 8);
        Ok(ui)
    }

    fn step(&mut self, dos: &mut Dos) {
        dos.wait_vretrace();
        self.t += 1;
        let mut moved = false;
        while let Some(k) = dos.getch() {
            let (mut x, mut y) = self.cursor;
            match k.scan {
                sc::ESC => dos.exit(),
                sc::UP => y = y.saturating_sub(1).max(1),
                sc::DOWN => y = (y + 1).min(Self::rows(dos) - 2),
                sc::LEFT => x = x.saturating_sub(1),
                sc::RIGHT => x = (x + 1).min(79),
                sc::B => {
                    self.blink = !self.blink;
                    dos.set_blink(self.blink);
                    self.log(format!(
                        "Attribute bit 7 now means {}",
                        if self.blink {
                            "blink"
                        } else {
                            "bright background"
                        }
                    ));
                    self.draw(dos);
                }
                sc::F => {
                    self.custom = !self.custom;
                    self.apply_font(dos);
                    self.log(format!(
                        "Custom tiles {}",
                        if self.custom {
                            "loaded"
                        } else {
                            "removed: the plain CP437 glyphs are back"
                        }
                    ));
                    self.draw_log(dos);
                }
                sc::G => {
                    self.galactic = !self.galactic;
                    self.apply_font(dos);
                    self.log(format!(
                        "Standard Galactic Alphabet {}",
                        if self.galactic { "loaded" } else { "unloaded" }
                    ));
                    self.draw_log(dos);
                }
                sc::H => {
                    let tall = !self.tall;
                    self.log(format!(
                        "Switching to {}",
                        if tall {
                            "80x25 (8x16 font)"
                        } else {
                            "80x50 (8x8 font, max scan line 7)"
                        }
                    ));
                    self.set_height(dos, tall);
                }
                _ => {}
            }
            if (x, y) != self.cursor {
                self.cursor = (x, y);
                moved = true;
            }
        }
        if moved {
            self.draw_map(dos);
            let (x, y) = self.cursor;
            dos.set_cursor(x, y);
        }
        // Animate the water: rewrite one glyph's bitmap, every water cell changes at once.
        if self.custom && self.t.is_multiple_of(8) {
            self.phase += 1;
            self.apply_font(dos);
        }
        if self.t.is_multiple_of(70) {
            let ticks = dos.bios_ticks();
            self.log(format!(
                "tick {:>5}  BIOS timer {ticks:>6}  cursor at {:?}",
                self.t, self.cursor
            ));
            self.draw_log(dos);
            let (x, y) = self.cursor;
            dos.set_cursor(x, y);
        }
    }

    fn overlay(&mut self, dos: &Dos, p: &mut Painter<'_>, screen: [f32; 4]) {
        let (c, r) = dos.text_size();
        let s = format!(
            "text {c}x{r}  {:.2} Hz   B blink:{}  F font:{}  H 25/50 rows",
            dos.refresh_hz(),
            if self.blink { "on" } else { "off" },
            if self.custom { "on" } else { "off" }
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
    ("Arrows", "move the cursor and the hero"),
    ("B", "blink vs. bright backgrounds"),
    ("F", "custom tile font on/off"),
    ("G", "Standard Galactic Alphabet"),
    ("H", "80x25 / 80x50"),
    ("Esc", "quit"),
];

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<TextUi>(DosConfig::new("text mode", Adapter::Vga).with_keys(KEYS))
}
