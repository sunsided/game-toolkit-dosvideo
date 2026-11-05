//! IBM CGA, five ways.
//!
//! 1. **Mode 4**, 320x200 in 4 colors: two hard-wired palettes (green/red/brown or
//!    cyan/magenta/white), an intensity bit, and a freely chosen background color, all in the
//!    color select register at port 0x3D9.
//! 2. **Mode 5**: the same mode with the B/W bit set, which on an RGB monitor gives the third,
//!    "secret" palette: cyan, red and white.
//! 3. **Mode 6**, 640x200 in 2 colors. On an RGB monitor it is black and white, but on a
//!    composite monitor (or TV) with the color burst switched on, every 4-pixel group becomes
//!    one of 16 *artifact colors*: the pixels are as narrow as one cycle of the NTSC color
//!    subcarrier, so the TV decodes the pattern as chroma. The bands show all 16 patterns.
//! 4. **160x100 in 16 colors**: not a graphics mode at all. It is 80-column text with the CRTC
//!    reprogrammed to two scanlines per character row, filled with character 0xDE (right half
//!    block), so each cell's background and foreground colors are two "pixels".
//! 5. **Snow**: in 80-column text, any CPU access to video memory during the display steals
//!    the bus from the CRTC, which then shows the byte on the bus instead of the character it
//!    wanted. Polling port 0x3DA and only writing while the display is off avoids it, at the
//!    price of a lot of waiting.
//!
//! Keys: `Space` next scene, `1`-`5` pick a scene, `Esc` quit.
//! Modes 4/5: `P` palette, `I` intensity, `B` background color.
//! Mode 6: `M` RGB / composite monitor, `K` color burst on/off, `H` composite hue.
//! 160x100 and text: `S` write only while the display is off (no snow).

use game_toolkit_dosvideo::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scene {
    Mode4,
    Mode5,
    Mode6,
    LowRes,
    Snow,
}

const SCENES: [Scene; 5] = [
    Scene::Mode4,
    Scene::Mode5,
    Scene::Mode6,
    Scene::LowRes,
    Scene::Snow,
];

impl Scene {
    fn name(self) -> &'static str {
        match self {
            Scene::Mode4 => "mode 4: 320x200x4",
            Scene::Mode5 => "mode 5: the cyan/red/white palette",
            Scene::Mode6 => "mode 6: 640x200x2, artifact colors",
            Scene::LowRes => "160x100x16 tweaked text mode",
            Scene::Snow => "80x25 text: snow",
        }
    }
}

struct Showcase {
    scene: usize,
    palette: bool,
    intensity: bool,
    background: u8,
    composite: bool,
    burst: bool,
    hue: f32,
    safe: bool,
    t: u32,
    rng: u32,
}

impl Showcase {
    fn scene(&self) -> Scene {
        SCENES[self.scene]
    }

    fn color_select(&self) -> u8 {
        (u8::from(self.palette) << 5) | (u8::from(self.intensity) << 4) | (self.background & 0x0F)
    }

    fn rand(&mut self) -> u32 {
        self.rng = self.rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
        self.rng >> 16
    }

    fn enter(&mut self, dos: &mut Dos) {
        dos.cga_set_monitor(if self.composite && self.scene() == Scene::Mode6 {
            CgaMonitor::Composite
        } else {
            CgaMonitor::Rgb
        });
        match self.scene() {
            Scene::Mode4 | Scene::Mode5 => {
                dos.set_mode(if self.scene() == Scene::Mode4 { 4 } else { 5 });
                dos.outb(0x3D9, self.color_select());
                dos.cga_blit(&picture4());
            }
            Scene::Mode6 => {
                dos.set_mode(6);
                dos.cga_set_composite_hue(self.hue);
                draw_mode6(dos);
                self.apply_burst(dos);
            }
            Scene::LowRes => {
                dos.set_mode(3);
                // The tweak: 100 character rows of 2 scanlines (R9 = 1), 128 rows total.
                for (r, v) in [(4u8, 0x7Fu8), (6, 0x64), (7, 0x70), (9, 0x01), (10, 0x20)] {
                    dos.outb(0x3D4, r);
                    dos.outb(0x3D5, v);
                }
                // 80 columns + video on, blink off: bit 7 of the attribute is a bright
                // background, so both "pixels" of a cell get all 16 colors.
                dos.outb(0x3D8, 0x09);
                dos.fill(0xB8000, 0x4000, 0);
            }
            Scene::Snow => {
                dos.set_mode(3);
                dos.hide_cursor();
                for y in 0..25 {
                    for x in 0..80 {
                        dos.text_put(x, y, 0xB1, 0x01);
                    }
                }
                dos.text_box(18, 9, 44, 7, 0x1F, game_toolkit_dosvideo::text::BOX_DOUBLE);
                dos.text_center(11, "CPU writes during display = SNOW", 0x1E);
                dos.text_center(13, "press S to write only during retrace", 0x1B);
            }
        }
    }

    fn apply_burst(&self, dos: &mut Dos) {
        // BIOS mode 6 sets the B/W bit (no color burst); clear it to let a composite monitor
        // decode the artifact colors.
        dos.outb(0x3D8, if self.burst { 0x1A } else { 0x1E });
    }

    /// Wait until the display is off (horizontal or vertical blank), if writing safely.
    fn wait_safe(&self, dos: &mut Dos) {
        if self.safe {
            while dos.inb(0x3DA) & 0x01 == 0 {}
        }
    }

    fn step_lowres(&mut self, dos: &mut Dos) {
        // A plasma, 20 of the 100 rows per frame (a full 8000-cell refresh would take 16 ms on
        // an 8-bit bus).
        let t = self.t as f32 * 0.15;
        let band = (self.t % 5) * 20;
        for y in band..band + 20 {
            for x in 0..80u32 {
                let px = |xx: f32| {
                    let fy = y as f32;
                    let v = (xx * 0.09 + t).sin()
                        + (fy * 0.11 - t * 0.7).sin()
                        + ((xx + fy) * 0.05 + t * 0.4).sin();
                    const RAMP: [u8; 8] = [0, 1, 9, 11, 15, 14, 12, 4];
                    RAMP[(((v + 3.0) / 6.0 * 7.99) as usize).min(7)]
                };
                let l = px(x as f32 * 2.0);
                let r = px(x as f32 * 2.0 + 1.0);
                self.wait_safe(dos);
                dos.pokew(
                    0xB8000 + (y * 80 + x) * 2,
                    0xDE | ((((l << 4) | r) as u16) << 8),
                );
            }
        }
    }

    fn step_snow(&mut self, dos: &mut Dos) {
        // Rain of random characters, written for as long as the beam draws the picture (a
        // text-heavy program updating its screen). Every write that lands while the display is
        // active leaves a streak of garbage on the scanline the beam is on, wherever that is.
        dos.wait_frame_start();
        let frame = dos.beam().frame;
        while dos.beam().frame == frame && dos.beam().line < 200 {
            let x = self.rand() % 80;
            let y = self.rand() % 25;
            if (18..62).contains(&x) && (9..16).contains(&y) {
                continue;
            }
            let ch = 0x21 + (self.rand() % 90) as u8;
            let at = [0x02, 0x0A, 0x03, 0x0B][(self.rand() % 4) as usize];
            self.wait_safe(dos);
            dos.text_put(x, y, ch, at);
        }
    }
}

/// The mode 4/5 test picture, colors 0-3.
fn picture4() -> Bitmap {
    let mut b = Bitmap::new(320, 200);
    let font = Font::cga();
    b.text(96, 8, "IBM COLOR GRAPHICS", 3, None, font);
    for c in 0..4u8 {
        let x = 20 + c as i32 * 72;
        b.fill_rect(x, 30, 64, 50, c);
        b.rect(x, 30, 64, 50, 3);
        b.text(x + 28, 84, &c.to_string(), 3, None, font);
    }
    for i in 0..6 {
        b.circle(60 + i * 40, 140, 18, (i % 3) as u8 + 1, true);
        b.circle(60 + i * 40, 140, 18, 3, false);
    }
    for x in 0..320 {
        let y = 180 + ((x as f32 * 0.05).sin() * 8.0) as i32;
        b.fill_rect(x, y, 1, 200 - y, 1 + (x / 40 % 3) as u8);
    }
    b
}

/// The mode 6 picture: 16 bands with every 4-pixel nibble pattern, and some dithers.
fn draw_mode6(dos: &mut Dos) {
    let mut b = Bitmap::new(640, 200);
    for y in 0..120 {
        for x in 0..640i32 {
            let pattern = (x / 40) as u8;
            let bit = (pattern >> (3 - (x & 3))) & 1;
            b.pset(x, y, bit);
        }
    }
    let font = Font::cga();
    for n in 0..16 {
        b.text(n * 40 + 12, 124, &format!("{n:X}"), 1, None, font);
    }
    for y in 140..200 {
        for x in 0..640i32 {
            let v = match x / 160 {
                0 => (x + y) & 1,                         // checkerboard
                1 => (x >> 1) & 1,                        // 2-pixel stripes
                2 => ((x + y) >> 1) & 1,                  // diagonal
                _ => i32::from(x % 4 == 0 && y % 2 == 0), // sparse dots
            };
            b.pset(x, y, v as u8);
        }
    }
    b.text(232, 188, " ARTIFACT COLORS ", 1, Some(0), font);
    for y in 0..200u32 {
        let bytes: Vec<u8> = b
            .row(y)
            .chunks(8)
            .map(|c| {
                c.iter()
                    .enumerate()
                    .fold(0u8, |a, (i, &p)| a | ((p & 1) << (7 - i)))
            })
            .collect();
        dos.poke_slice(0xB8000 + 0x2000 * (y & 1) + (y >> 1) * 80, &bytes);
    }
}

impl DosGame for Showcase {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        let mut s = Self {
            scene: 0,
            palette: true,
            intensity: true,
            background: 1,
            composite: true,
            burst: true,
            // -60 degrees matches the well-known palette of the old CGA.
            hue: 300.0,
            safe: false,
            t: 0,
            rng: 7,
        };
        s.enter(dos);
        Ok(s)
    }

    fn step(&mut self, dos: &mut Dos) {
        while let Some(k) = dos.getch() {
            let scene = self.scene();
            let mut redo = false;
            match k.scan {
                sc::ESC => dos.exit(),
                sc::SPACE => {
                    self.scene = (self.scene + 1) % SCENES.len();
                    redo = true;
                }
                s @ sc::N1..=sc::N5 => {
                    self.scene = (s - sc::N1) as usize;
                    redo = true;
                }
                sc::P if matches!(scene, Scene::Mode4 | Scene::Mode5) => {
                    self.palette = !self.palette;
                    dos.outb(0x3D9, self.color_select());
                }
                sc::I if matches!(scene, Scene::Mode4 | Scene::Mode5) => {
                    self.intensity = !self.intensity;
                    dos.outb(0x3D9, self.color_select());
                }
                sc::B if matches!(scene, Scene::Mode4 | Scene::Mode5) => {
                    self.background = (self.background + 1) & 0x0F;
                    dos.outb(0x3D9, self.color_select());
                }
                sc::M if scene == Scene::Mode6 => {
                    self.composite = !self.composite;
                    dos.cga_set_monitor(if self.composite {
                        CgaMonitor::Composite
                    } else {
                        CgaMonitor::Rgb
                    });
                }
                sc::K if scene == Scene::Mode6 => {
                    self.burst = !self.burst;
                    self.apply_burst(dos);
                }
                sc::H if scene == Scene::Mode6 => {
                    self.hue = (self.hue + 15.0) % 360.0;
                    dos.cga_set_composite_hue(self.hue);
                }
                sc::S => self.safe = !self.safe,
                _ => {}
            }
            if redo {
                self.enter(dos);
            }
        }
        dos.wait_vretrace();
        match self.scene() {
            Scene::LowRes => self.step_lowres(dos),
            Scene::Snow => self.step_snow(dos),
            _ => {}
        }
        self.t += 1;
    }

    fn overlay(&mut self, _dos: &Dos, p: &mut Painter<'_>, screen: [f32; 4]) {
        let keys = match self.scene() {
            Scene::Mode4 | Scene::Mode5 => format!(
                "P palette {}  I intensity {}  B background {}",
                u8::from(self.palette),
                on(self.intensity),
                self.background
            ),
            Scene::Mode6 => format!(
                "M monitor: {}  K color burst: {}  H hue: {:.0}",
                if self.composite { "composite" } else { "RGB" },
                on(self.burst),
                self.hue
            ),
            _ => format!("S write only while display is off: {}", on(self.safe)),
        };
        let s = format!(
            "{}/5 {}   Space next scene, 1-5, Esc quit\n{keys}",
            self.scene + 1,
            self.scene().name()
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

const KEYS: &[(&str, &str)] = &[
    ("Space", "next scene"),
    ("1-5", "pick a scene"),
    ("P", "modes 4/5: palette"),
    ("I", "modes 4/5: intensity"),
    ("B", "modes 4/5: background color"),
    ("M", "mode 6: RGB / composite monitor"),
    ("K", "mode 6: color burst on/off"),
    ("H", "mode 6: composite hue"),
    (
        "S",
        "160x100 / text: write only while the display is off (no snow)",
    ),
    ("Esc", "quit"),
];

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<Showcase>(DosConfig::new("CGA showcase", Adapter::Cga(CgaMonitor::Rgb)).with_keys(KEYS))
}
