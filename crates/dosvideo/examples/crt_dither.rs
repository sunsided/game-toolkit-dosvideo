//! Dithering, and why it looked better back then.
//!
//! A true-color scene is reduced to a small palette, the way artwork was prepared for 16- and
//! 4-color modes: nearest color (banding), ordered Bayer dithering (the crosshatch of countless
//! EGA games) or Floyd-Steinberg error diffusion. On a sharp modern screen the patterns are
//! obvious. Press `Alt+C` to cycle the CRT filter: the beam blur and phosphor glow melt the
//! patterns back into the in-between colors the artists were after.
//!
//! Keys: `D` dither method, `P` palette (EGA 16 / CGA 4 / 256-color cube), `G` captions in
//! the Standard Galactic Alphabet, `Alt+C` CRT filter (host), `Esc` quit.

use game_toolkit_dosvideo::draw::Dither;
use game_toolkit_dosvideo::palette::CGA;
use game_toolkit_dosvideo::prelude::*;

const DITHERS: [(Dither, &str); 5] = [
    (Dither::None, "NEAREST COLOR"),
    (Dither::Bayer2, "BAYER 2X2"),
    (Dither::Bayer4, "BAYER 4X4"),
    (Dither::Bayer8, "BAYER 8X8"),
    (Dither::FloydSteinberg, "FLOYD-STEINBERG"),
];

const PALETTES: [&str; 3] = ["EGA 16 COLORS", "CGA 4 COLORS", "256 COLOR CUBE"];

struct Demo {
    scene: Vec<u8>,
    dither: usize,
    palette: usize,
    galactic: bool,
}

/// A sunset over water with a shaded sphere: smooth gradients that a small palette can't hold.
fn scene() -> Vec<u8> {
    let mut rgba = Vec::with_capacity(320 * 184 * 4);
    for y in 0..184 {
        for x in 0..320 {
            let (fx, fy) = (x as f32, y as f32);
            let mut c = if fy < 110.0 {
                // Sky: deep blue to orange.
                let k = fy / 110.0;
                [30.0 + 210.0 * k * k, 50.0 + 100.0 * k, 170.0 - 120.0 * k]
            } else {
                // Water: darker, with sun glitter bands.
                let k = (fy - 110.0) / 74.0;
                let glitter = ((fy * 1.7).sin() * (fx * 0.05 + fy * 0.3).sin()).max(0.0);
                let near_sun = (1.0 - ((fx - 230.0) / 60.0).abs()).max(0.0);
                [
                    40.0 + 60.0 * (1.0 - k) + 200.0 * glitter * near_sun,
                    40.0 + 40.0 * (1.0 - k) + 120.0 * glitter * near_sun,
                    80.0 + 30.0 * k + 40.0 * glitter * near_sun,
                ]
            };
            // Sun with a soft glow, cut by the horizon.
            let d = ((fx - 230.0).powi(2) + (fy - 95.0).powi(2)).sqrt();
            if fy < 110.0 {
                let glow = (1.0 - d / 70.0).max(0.0);
                c[0] += 120.0 * glow;
                c[1] += 80.0 * glow * glow;
                if d < 26.0 {
                    c = [255.0, 230.0 - d * 2.0, 120.0];
                }
            }
            // A shaded sphere on the left.
            let (sx, sy, r) = (80.0, 120.0, 46.0);
            let (dx, dy) = ((fx - sx) / r, (fy - sy) / r);
            let dd = dx * dx + dy * dy;
            if dd < 1.0 {
                let nz = (1.0 - dd).sqrt();
                let light = (dx * 0.6 - dy * 0.3 + nz * 0.75).max(0.0);
                let spec = light.powf(24.0) * 255.0;
                c = [
                    40.0 + 180.0 * light + spec,
                    90.0 * light + spec,
                    150.0 * light + 40.0 + spec,
                ];
            }
            rgba.extend(c.map(|v| v.clamp(0.0, 255.0) as u8));
            rgba.push(255);
        }
    }
    rgba
}

fn palette(which: usize) -> Vec<[u8; 3]> {
    match which {
        0 => CGA.to_vec(),
        1 => vec![CGA[0], CGA[11], CGA[13], CGA[15]],
        _ => {
            let mut p = Vec::with_capacity(256);
            for r in 0..6u8 {
                for g in 0..6u8 {
                    for b in 0..6u8 {
                        p.push([r * 51, g * 51, b * 51]);
                    }
                }
            }
            for i in 0..40u8 {
                let v = 6 + i * 6;
                p.push([v, v, v]);
            }
            p
        }
    }
}

impl Demo {
    fn redraw(&self, dos: &mut Dos) {
        let pal = palette(self.palette);
        let mut dac = [[0u8; 3]; 256];
        for (i, c) in pal.iter().enumerate() {
            dac[i] = [c[0] >> 2, c[1] >> 2, c[2] >> 2];
        }
        // Caption colors live at the top of the DAC, outside every test palette.
        dac[254] = [0, 0, 0];
        dac[255] = [63, 63, 63];
        let (dither, dname) = DITHERS[self.dither];
        let pic = Bitmap::from_rgba(320, 184, &self.scene, &pal, dither);
        let mut screen = Bitmap::new(320, 200);
        screen.clear(254);
        screen.blit(&pic, 0, 0, None);
        let font = if self.galactic {
            Font::sga()
        } else {
            Font::vga()
        };
        let caption = format!("{dname} / {}", PALETTES[self.palette]);
        let x = (320 - caption.len() as i32 * 8) / 2;
        screen.text(x, 184, &caption, 255, Some(254), font);
        dos.wait_vretrace();
        dos.set_palette_range(0, &dac);
        dos.blit13(&screen);
    }
}

impl DosGame for Demo {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        dos.set_mode(0x13);
        let d = Self {
            scene: scene(),
            dither: 2,
            palette: 0,
            galactic: false,
        };
        d.redraw(dos);
        Ok(d)
    }

    fn step(&mut self, dos: &mut Dos) {
        let mut dirty = false;
        while let Some(k) = dos.getch() {
            match k.scan {
                sc::ESC => dos.exit(),
                sc::D => {
                    self.dither = (self.dither + 1) % DITHERS.len();
                    dirty = true;
                }
                sc::P => {
                    self.palette = (self.palette + 1) % PALETTES.len();
                    dirty = true;
                }
                sc::G => {
                    self.galactic = !self.galactic;
                    dirty = true;
                }
                _ => {}
            }
        }
        if dirty {
            self.redraw(dos);
        }
        dos.wait_vretrace();
    }

    fn overlay(&mut self, _dos: &Dos, p: &mut Painter<'_>, screen: [f32; 4]) {
        p.text(
            [screen[0] + 8.0, screen[1] + screen[3] + 8.0],
            "D dither   P palette   G galactic   Alt+C CRT filter   Esc quit",
            16.0,
            [0.7, 0.7, 0.7, 1.0],
        );
    }
}

const KEYS: &[(&str, &str)] = &[
    ("D", "dither: nearest, Bayer 2/4/8, Floyd-Steinberg"),
    ("P", "palette: EGA 16 / CGA 4 / 256 cube"),
    ("G", "galactic captions"),
    ("Esc", "quit"),
];

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<Demo>(
        DosConfig::new("dithering on a CRT", Adapter::Vga)
            .with_crt(CrtFilter::tv())
            .with_keys(KEYS),
    )
}
