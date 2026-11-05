//! Hercules Graphics Card: 720x348 monochrome with two pages, and MDA text.
//!
//! There is no BIOS mode for Hercules graphics: the program flips the configuration switch at
//! port 0x3BF (allow graphics, enable page 1), loads the 6845 itself and selects graphics and
//! the displayed page through the mode register at 0x3B8. Scanlines are interleaved four ways
//! (`0x2000` apart), 90 bytes each.
//!
//! The wireframe is drawn on the hidden page, erasing last frame's lines by drawing them again
//! in black (cheaper than clearing 32 KiB over an 8-bit bus), then the pages are flipped
//! during vertical retrace. Careful: the retrace bit of the Hercules status port (0x3BA bit 7)
//! is *inverted* compared to CGA: it reads 0 while the retrace is in progress.
//!
//! Text mode shows the MDA attributes (underline, bright, inverse, blink) and the 9th pixel
//! column: line-drawing characters C0h-DFh repeat their 8th column into it, so `═` and `─` join
//! up while `░` does not.
//!
//! Keys: `T` text / graphics, `P` phosphor (green, amber, white), `V` wait for retrace before
//! flipping on/off, `Esc` quit.

use game_toolkit_dosvideo::prelude::*;
use game_toolkit_dosvideo::text::{BOX_DOUBLE, BOX_SINGLE};

type Line = (i32, i32, i32, i32);

struct Herc {
    text: bool,
    phosphor: usize,
    vsync: bool,
    /// Page being shown; we draw on the other one.
    shown: u32,
    /// Lines currently drawn on each page, to erase them next time.
    drawn: [Vec<Line>; 2],
    angle: f32,
}

const PHOSPHORS: [Phosphor; 3] = [Phosphor::Green, Phosphor::Amber, Phosphor::White];

fn line(dos: &mut Dos, (x0, y0, x1, y1): Line, on: bool, page: u32) {
    let (mut x, mut y) = (x0, y0);
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        if x >= 0 && y >= 0 {
            dos.herc_pset(x as u32, y as u32, on, page);
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// A cube and an octahedron, rotated and projected. Hercules pixels are tall (720x348 shown at
/// 4:3), so X is stretched by 1.55 to keep the shapes round.
fn wireframe(angle: f32) -> Vec<Line> {
    let cube: Vec<[f32; 3]> = (0..8)
        .map(|i| {
            [
                if i & 1 != 0 { 1.0 } else { -1.0 },
                if i & 2 != 0 { 1.0 } else { -1.0 },
                if i & 4 != 0 { 1.0 } else { -1.0 },
            ]
        })
        .collect();
    let cube_edges: Vec<(usize, usize)> = (0..8)
        .flat_map(|a| {
            [1, 2, 4]
                .into_iter()
                .filter(move |b| a & b == 0)
                .map(move |b| (a, a | b))
        })
        .collect();
    let octa: Vec<[f32; 3]> = vec![
        [1.6, 0.0, 0.0],
        [-1.6, 0.0, 0.0],
        [0.0, 1.6, 0.0],
        [0.0, -1.6, 0.0],
        [0.0, 0.0, 1.6],
        [0.0, 0.0, -1.6],
    ];
    let octa_edges: Vec<(usize, usize)> = (0..6)
        .flat_map(|a| ((a | 1) + 1..6).map(move |b| (a, b)))
        .collect();
    let mut lines = Vec::new();
    let mut add = |pts: &[[f32; 3]],
                   edges: &[(usize, usize)],
                   a: f32,
                   b: f32,
                   cx: f32,
                   cy: f32,
                   size: f32| {
        let p: Vec<(i32, i32)> = pts
            .iter()
            .map(|v| {
                let (sa, ca) = a.sin_cos();
                let (sb, cb) = b.sin_cos();
                let x = v[0] * ca - v[2] * sa;
                let z = v[0] * sa + v[2] * ca;
                let y = v[1] * cb - z * sb;
                let z = v[1] * sb + z * cb;
                let k = size / (z + 5.0);
                ((cx + x * k * 1.55) as i32, (cy + y * k) as i32)
            })
            .collect();
        for &(i, j) in edges {
            lines.push((p[i].0, p[i].1, p[j].0, p[j].1));
        }
    };
    add(&cube, &cube_edges, angle, angle * 0.7, 220.0, 175.0, 170.0);
    add(
        &octa,
        &octa_edges,
        -angle * 1.3,
        angle * 0.5,
        510.0,
        175.0,
        150.0,
    );
    lines
}

impl Herc {
    /// Static parts of both pages: frame, title, page number.
    fn enter_graphics(&mut self, dos: &mut Dos) {
        dos.hercules_graphics(0);
        dos.fill(0xB0000, 0x10000, 0);
        let font = Font::vga();
        for page in 0..2u32 {
            let mut b = Bitmap::new(720, 348);
            b.rect(0, 0, 720, 348, 1);
            b.rect(3, 3, 714, 342, 1);
            b.text(16, 12, "HERCULES GRAPHICS CARD  720x348", 1, None, font);
            b.text(560, 12, &format!("PAGE {page}"), 1, None, font);
            b.text(
                16,
                320,
                "drawn on the hidden page, flipped in vertical retrace",
                1,
                None,
                font,
            );
            let packed: Vec<u8> = b
                .data
                .chunks(8)
                .map(|c| {
                    c.iter()
                        .enumerate()
                        .fold(0u8, |a, (i, &p)| a | ((p & 1) << (7 - i)))
                })
                .collect();
            dos.herc_blit(&packed, page);
        }
        self.drawn = [Vec::new(), Vec::new()];
        self.shown = 0;
    }

    fn enter_text(&mut self, dos: &mut Dos) {
        dos.hercules_text();
        dos.text_clear(0x07);
        dos.text_box(0, 0, 80, 25, 0x07, BOX_DOUBLE);
        dos.text_center(
            1,
            " MDA / Hercules text mode, 80x25, 9x14 character cells ",
            0x0F,
        );
        let samples: [(u8, &str); 7] = [
            (0x07, "07h  normal"),
            (0x01, "01h  underline"),
            (0x0F, "0Fh  bright"),
            (0x09, "09h  bright underline"),
            (0x70, "70h  inverse"),
            (0x87, "87h  blinking"),
            (0x00, "00h  invisible"),
        ];
        for (i, (a, s)) in samples.iter().enumerate() {
            dos.text_print(4, 4 + i as u32 * 2, s, *a);
        }
        dos.text_print(4, 18, "(the invisible line is above)", 0x07);
        dos.text_box(40, 3, 36, 9, 0x07, BOX_SINGLE);
        dos.text_print(42, 4, "The 9th column:", 0x0F);
        dos.text_print(42, 6, "C4h ────────────────", 0x07);
        dos.text_print(42, 7, "CDh ════════════════", 0x07);
        dos.text_print(42, 8, "DBh ████████████████", 0x07);
        dos.text_print(42, 9, "B1h ▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒▒", 0x07);
        dos.text_print(42, 10, "B3h ││││││││││││││││", 0x07);
        dos.text_print(40, 14, "C0h-DFh repeat column 8 into 9:", 0x07);
        dos.text_print(40, 15, "boxes and lines join, shades don't.", 0x07);
        dos.text_print(4, 22, "T graphics   P phosphor   Esc quit", 0x70);
        dos.hide_cursor();
    }
}

impl DosGame for Herc {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        let mut h = Self {
            text: false,
            phosphor: 0,
            vsync: true,
            shown: 0,
            drawn: [Vec::new(), Vec::new()],
            angle: 0.0,
        };
        h.enter_graphics(dos);
        Ok(h)
    }

    fn step(&mut self, dos: &mut Dos) {
        while let Some(k) = dos.getch() {
            match k.scan {
                sc::ESC => dos.exit(),
                sc::P => {
                    self.phosphor = (self.phosphor + 1) % PHOSPHORS.len();
                    dos.hercules_set_phosphor(PHOSPHORS[self.phosphor]);
                }
                sc::V => self.vsync = !self.vsync,
                sc::T => {
                    self.text = !self.text;
                    if self.text {
                        self.enter_text(dos);
                    } else {
                        self.enter_graphics(dos);
                    }
                }
                _ => {}
            }
        }
        if self.text {
            // The card blinks by itself; just idle until the next frame.
            while dos.inb(0x3BA) & 0x80 == 0 {}
            while dos.inb(0x3BA) & 0x80 != 0 {}
            return;
        }

        // Draw on the hidden page: erase the old lines, draw the new ones.
        let page = 1 - self.shown;
        let old = std::mem::take(&mut self.drawn[page as usize]);
        for l in &old {
            line(dos, *l, false, page);
        }
        let new = wireframe(self.angle);
        for l in &new {
            line(dos, *l, true, page);
        }
        self.drawn[page as usize] = new;
        self.angle += 0.04;

        if self.vsync {
            // Bit 7 of 0x3BA is LOW during vertical retrace. First let a retrace in progress
            // end (bit goes high), then wait for the next one to start (bit goes low).
            while dos.inb(0x3BA) & 0x80 == 0 {}
            while dos.inb(0x3BA) & 0x80 != 0 {}
        }
        // Mode register: graphics (bit 1), video on (bit 3), page (bit 7).
        dos.outb(0x3B8, 0x0A | if page == 1 { 0x80 } else { 0 });
        self.shown = page;
    }

    fn overlay(&mut self, dos: &Dos, p: &mut Painter<'_>, screen: [f32; 4]) {
        let s = format!(
            "{}  {:.1} Hz   T text/graphics  P phosphor  V wait for retrace: {}  Esc quit",
            if self.text {
                "text 80x25"
            } else {
                "graphics 720x348"
            },
            dos.refresh_hz(),
            if self.vsync { "on" } else { "off" }
        );
        p.text(
            [screen[0] + 8.0, screen[1] + screen[3] + 8.0],
            &s,
            16.0,
            [0.7, 0.7, 0.7, 1.0],
        );
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<Herc>(DosConfig::new(
        "Hercules",
        Adapter::Hercules(Phosphor::Green),
    ))
}
