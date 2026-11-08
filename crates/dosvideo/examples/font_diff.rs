//! Compare the built-in VGA fonts against a reference font, glyph by glyph.
//!
//! Pass a raw font file (256 glyphs, one byte per row, MSB leftmost: 4096 bytes for 8x16,
//! 3584 for 8x14, 2048 for 8x8), e.g. a BIOS font dumped from DOSBox through INT 43h:
//!
//! ```text
//! cargo run --example font_diff -- path/to/vga8x16.bin
//! ```
//!
//! Only the glyphs that differ are shown, at 2x, in one of four views (`1`-`4`, or `Tab` to
//! cycle):
//! - *comparison*: built-in left, reference right. Pixels both agree on are white (set) or
//!   gray (clear); pixels they disagree on are red, bright where that font sets the pixel,
//!   dark where it leaves it clear.
//! - *overlay*: one cell per glyph. White where both set the pixel, red where only the
//!   built-in font does, cyan where only the reference does.
//! - *built-in* / *reference*: just that font, plain white on gray.

use game_toolkit_dosvideo::prelude::*;

const SCALE: i32 = 2;
const VIEWS: [&str; 4] = ["comparison", "overlay", "built-in", "reference"];

struct FontDiff {
    ours: &'static Font,
    reference: Font,
    differing: Vec<u8>,
    view: usize,
    screen: Bitmap,
}

impl FontDiff {
    fn draw(&mut self) {
        let s = &mut self.screen;
        s.clear(0);
        let label = Font::cga();
        let h = i32::from(self.reference.height());
        s.text(
            2,
            2,
            &format!("8x{h}: {} of 256 glyphs differ", self.differing.len()),
            15,
            None,
            label,
        );
        s.text(
            2,
            11,
            &format!("view: {}   1-4/Tab view, Esc quit", VIEWS[self.view]),
            7,
            None,
            label,
        );

        // Cells per glyph in this view.
        let panels = if self.view == 0 { 2 } else { 1 };
        let cell_w = 8 * SCALE + 2;
        let entry_w = panels * cell_w + 18;
        let entry_h = h * SCALE + 12;
        let per_row = (318 / entry_w) as usize;
        for (i, &code) in self.differing.iter().enumerate() {
            let x = 2 + (i % per_row) as i32 * entry_w;
            let y = 24 + (i / per_row) as i32 * entry_h;
            if y + entry_h > 200 {
                s.text(2, 192, "(more glyphs than fit)", 40, None, label);
                break;
            }
            s.text(x, y, &format!("{code:02X}"), 7, None, label);
            let (a, b) = (self.ours.glyph(code), self.reference.glyph(code));
            for panel in 0..panels {
                let px = x + panel * cell_w;
                for row in 0..h as usize {
                    for col in 0..8 {
                        let bit = 0x80 >> col;
                        let (on_a, on_b) = (a[row] & bit != 0, b[row] & bit != 0);
                        // Default palette: 15 white, 20 gray, 40 bright red, 4 dark red,
                        // 52 cyan.
                        let c = match self.view {
                            0 => {
                                let on = if panel == 0 { on_a } else { on_b };
                                match (on_a == on_b, on) {
                                    (true, true) => 15,
                                    (true, false) => 20,
                                    (false, true) => 40,
                                    (false, false) => 4,
                                }
                            }
                            1 => match (on_a, on_b) {
                                (true, true) => 15,
                                (true, false) => 40,
                                (false, true) => 52,
                                (false, false) => 20,
                            },
                            2 if on_a => 15,
                            3 if on_b => 15,
                            _ => 20,
                        };
                        s.fill_rect(
                            px + col * SCALE,
                            y + 9 + row as i32 * SCALE,
                            SCALE,
                            SCALE,
                            c,
                        );
                    }
                }
            }
        }
    }
}

impl DosGame for FontDiff {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        let path = std::env::args()
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("usage: font_diff <reference font .bin>"))?;
        let data = std::fs::read(&path)?;
        let height = u8::try_from(data.len() / 256)?;
        let reference = Font::from_raw(height, &data)
            .ok_or_else(|| anyhow::anyhow!("{path}: not 256 glyphs of whole rows"))?;
        let ours = match height {
            16 => Font::vga(),
            14 => Font::ega(),
            8 => Font::cga(),
            h => anyhow::bail!("no built-in 8x{h} font to compare with"),
        };
        let differing = (0..=255)
            .filter(|&c| ours.glyph(c) != reference.glyph(c))
            .collect();
        dos.set_mode(0x13);
        let mut app = Self {
            ours,
            reference,
            differing,
            view: 0,
            screen: Bitmap::new(320, 200),
        };
        app.draw();
        Ok(app)
    }

    fn step(&mut self, dos: &mut Dos) {
        while let Some(k) = dos.getch() {
            let view = match k.scan {
                sc::ESC => {
                    dos.exit();
                    None
                }
                sc::TAB => Some((self.view + 1) % VIEWS.len()),
                s @ sc::N1..=sc::N4 => Some((s - sc::N1) as usize),
                _ => None,
            };
            if let Some(v) = view {
                self.view = v;
                self.draw();
            }
        }
        dos.wait_vretrace();
        dos.blit13(&self.screen);
    }
}

const KEYS: &[(&str, &str)] = &[
    ("1-4", "view: comparison, overlay, built-in, reference"),
    ("Tab", "next view"),
    ("Esc", "quit"),
];

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<FontDiff>(DosConfig::new("font diff", Adapter::Vga).with_keys(KEYS))
}
