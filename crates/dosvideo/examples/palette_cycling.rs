//! Palette rotation: animation without touching a single pixel.
//!
//! In a 256-color mode a pixel holds an index, not a color. Draw a picture once with
//! carefully laid-out index ranges, then rotate those DAC entries every frame and the picture
//! moves: water flows, the sun pulses, a plasma swirls. A few hundred port writes per frame
//! instead of 64000 bytes of video memory - which is why DOS and Amiga artists loved it.
//!
//! Scenes (keys `1`-`4`):
//! 1. A landscape: waterfall, lake ripples and sun glow each cycle at their own speed and
//!    direction.
//! 2. A plasma drawn once, set in motion by rotating a 255-entry gradient.
//! 3. Fades and flashes: the whole picture faded through the palette; `Space` flashes white.
//! 4. The same rotation on an 8-bit ISA bus. `T` moves the DAC writes from the vertical blank
//!    into the middle of the frame: they take longer than a scanline, the beam overtakes them,
//!    and a band of half-old, half-new colors appears.
//!
//! `Esc` quits.

use game_toolkit_dosvideo::prelude::*;

const SKY: u8 = 32; // 32..=63 static gradient
const GLOW: (u8, u8) = (64, 79);
const SUN: u8 = 80;
const ROCK: u8 = 100; // 100..=107
const FALL: (u8, u8) = (112, 127);
const LAKE: (u8, u8) = (128, 143);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scene {
    Landscape,
    Plasma,
    Fade,
    Seam,
}

struct Cycling {
    scene: Scene,
    t: u32,
    /// The palette as set up for the scene, for fades.
    base: [[u8; 3]; 256],
    flash: u8,
    mid_frame: bool,
    vram_bytes: usize,
    dac_writes: u32,
}

/// Rotate DAC entries `first..=last` by one step; `down` moves colors to higher indices.
fn rotate(dos: &mut Dos, (first, last): (u8, u8), down: bool, writes: &mut u32) {
    let Some(dac) = dos.dac() else { return };
    let mut seg: Vec<[u8; 3]> = dac[first as usize..=last as usize].to_vec();
    if down {
        seg.rotate_right(1);
    } else {
        seg.rotate_left(1);
    }
    dos.set_palette_range(first, &seg);
    *writes += 1 + 3 * seg.len() as u32;
}

fn lerp(a: [u8; 3], b: [u8; 3], k: f32) -> [u8; 3] {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

fn landscape(dos: &mut Dos) -> usize {
    let mut pal = [[0u8; 3]; 256];
    // Sky: night blue at the top to orange at the horizon.
    for i in 0..32 {
        pal[(SKY + i) as usize] = lerp([4, 6, 20], [60, 30, 12], i as f32 / 31.0);
    }
    // Glow rings: one bright band travelling through a soft falloff.
    for i in 0..16u8 {
        let k = i as f32 / 15.0;
        let mut c = lerp([63, 48, 20], [50, 25, 12], k);
        if i < 3 {
            c = [63, 60, 40];
        }
        pal[(GLOW.0 + i) as usize] = c;
    }
    pal[SUN as usize] = [63, 63, 48];
    for i in 0..8u8 {
        let v = 4 + i * 2;
        pal[(ROCK + i) as usize] = [v, v / 2 + 2, v / 3 + 2];
    }
    // Waterfall: blues with three foam highlights.
    for i in 0..16u8 {
        pal[(FALL.0 + i) as usize] = match i {
            0 | 5 | 11 => [56, 60, 63],
            1 | 6 | 12 => [30, 44, 60],
            _ => [8, 20 + (i % 3) * 4, 44 + (i % 4) * 3],
        };
    }
    // Lake: dark water with a couple of sky reflections.
    for i in 0..16u8 {
        pal[(LAKE.0 + i) as usize] = match i {
            2 | 9 => [40, 30, 22],
            3 | 10 => [20, 20, 30],
            _ => [3, 8, 18 + i % 3],
        };
    }
    dos.set_palette_range(0, &pal);

    let mut b = Bitmap::new(320, 200);
    for y in 0..150 {
        let c = SKY + (y * 31 / 149) as u8;
        b.hline(0, 319, y, c);
    }
    // Sun with glow rings: ring index by distance, so rotating the range pulses outward.
    for y in 0..150i32 {
        for x in 170..320i32 {
            let d = (((x - 250).pow(2) + (y - 70).pow(2)) as f32).sqrt();
            if d < 16.0 {
                b.pset(x, y, SUN);
            } else if d < 56.0 {
                b.pset(x, y, GLOW.0 + ((d - 16.0) as u8 / 2) % 16);
            }
        }
    }
    // Cliffs on both sides of the waterfall.
    for x in 0..320i32 {
        let left = 40.0 + (x as f32 * 0.07).sin() * 10.0;
        let top = if (96..148).contains(&x) {
            200.0
        } else if x < 96 {
            left + (96 - x) as f32 * 0.1
        } else {
            62.0 + ((x - 148) as f32 * 0.05).sin() * 12.0 + (x - 148) as f32 * 0.25
        };
        for y in top as i32..150 {
            b.pset(x, y, ROCK + (((x * 7 + y * 3) % 23) as u8 % 8));
        }
    }
    // Waterfall: diagonal stripes so rotation reads as falling water.
    for y in 44..150i32 {
        for x in 96..148i32 {
            let wobble = ((x as f32 * 0.9).sin() * 2.0) as i32;
            b.pset(x, y, FALL.0 + ((y + wobble + x / 5).rem_euclid(16)) as u8);
        }
    }
    // Lake with horizontal ripples.
    for y in 150..200i32 {
        for x in 0..320i32 {
            let r =
                ((x as f32 / (6.0 + (y - 150) as f32 * 0.3)) + (y as f32 * 0.8).sin() * 2.0) as i32;
            b.pset(x, y, LAKE.0 + r.rem_euclid(16) as u8);
        }
    }
    dos.blit13(&b);
    b.data.len()
}

fn plasma(dos: &mut Dos) -> usize {
    let mut pal = [[0u8; 3]; 256];
    for (i, p) in pal.iter_mut().enumerate().skip(1) {
        let h = i as f32 / 255.0 * std::f32::consts::TAU;
        let c = |o: f32| ((h + o).sin() * 0.5 + 0.5) * 63.0;
        *p = [c(0.0) as u8, c(2.1) as u8, c(4.2) as u8];
    }
    dos.set_palette_range(0, &pal);
    let mut b = Bitmap::new(320, 200);
    for y in 0..200 {
        for x in 0..320 {
            let (fx, fy) = (x as f32, y as f32);
            let v = (fx / 16.0).sin()
                + (fy / 11.0).sin()
                + ((fx + fy) / 23.0).sin()
                + ((fx - 160.0).hypot(fy - 100.0) / 9.0).sin();
            b.pset(x, y, 1 + (((v + 4.0) / 8.0 * 254.0) as u8));
        }
    }
    dos.blit13(&b);
    b.data.len()
}

impl Cycling {
    fn enter(&mut self, dos: &mut Dos, scene: Scene) {
        self.scene = scene;
        self.flash = 0;
        dos.set_bus(if scene == Scene::Seam {
            BusTiming::ISA8
        } else {
            BusTiming::VLB
        });
        dos.wait_vretrace();
        self.vram_bytes = match scene {
            Scene::Landscape | Scene::Fade => landscape(dos),
            Scene::Plasma | Scene::Seam => plasma(dos),
        };
        self.base = dos.dac().unwrap_or([[0; 3]; 256]);
    }
}

impl DosGame for Cycling {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        dos.set_mode(0x13);
        let mut s = Self {
            scene: Scene::Landscape,
            t: 0,
            base: [[0; 3]; 256],
            flash: 0,
            mid_frame: true,
            vram_bytes: 0,
            dac_writes: 0,
        };
        s.enter(dos, Scene::Landscape);
        Ok(s)
    }

    fn step(&mut self, dos: &mut Dos) {
        while let Some(k) = dos.getch() {
            let next = match k.scan {
                sc::ESC => {
                    dos.exit();
                    None
                }
                sc::N1 => Some(Scene::Landscape),
                sc::N2 => Some(Scene::Plasma),
                sc::N3 => Some(Scene::Fade),
                sc::N4 => Some(Scene::Seam),
                sc::SPACE => {
                    self.flash = 12;
                    None
                }
                sc::T => {
                    self.mid_frame = !self.mid_frame;
                    None
                }
                _ => None,
            };
            if let Some(s) = next {
                self.enter(dos, s);
                return;
            }
        }

        let mut writes = 0u32;
        match self.scene {
            Scene::Landscape => {
                dos.wait_vretrace();
                rotate(dos, FALL, true, &mut writes);
                if self.t.is_multiple_of(4) {
                    rotate(dos, LAKE, false, &mut writes);
                }
                if self.t.is_multiple_of(3) {
                    rotate(dos, GLOW, true, &mut writes);
                }
            }
            Scene::Plasma => {
                dos.wait_vretrace();
                rotate(dos, (1, 255), true, &mut writes);
            }
            Scene::Fade => {
                dos.wait_vretrace();
                // Keep the water moving in the faded picture: rotate the base copy too.
                for (range, down, every) in [(FALL, true, 1), (LAKE, false, 4), (GLOW, true, 3)] {
                    if self.t.is_multiple_of(every) {
                        let seg = &mut self.base[range.0 as usize..=range.1 as usize];
                        if down {
                            seg.rotate_right(1);
                        } else {
                            seg.rotate_left(1);
                        }
                    }
                }
                let pal: Vec<[u8; 3]> = if self.flash > 0 {
                    self.flash -= 1;
                    let k = self.flash as f32 / 12.0;
                    self.base
                        .iter()
                        .map(|&c| lerp(c, [63, 63, 63], k))
                        .collect()
                } else {
                    // Triangle wave: 2 s down to black, 2 s back up.
                    let phase = (self.t % 280) as f32 / 140.0;
                    let level = if phase < 1.0 {
                        1.0 - phase
                    } else {
                        phase - 1.0
                    };
                    self.base
                        .iter()
                        .map(|&c| lerp([0, 0, 0], c, level))
                        .collect()
                };
                dos.set_palette_range(0, &pal);
                writes += 1 + 3 * 256;
            }
            Scene::Seam => {
                if self.mid_frame {
                    dos.wait_line(200);
                } else {
                    dos.wait_vretrace();
                }
                // Rotate several steps at once so the jump between old and new is visible.
                let Some(dac) = dos.dac() else { return };
                let mut seg = dac[1..=255].to_vec();
                seg.rotate_right(6);
                dos.set_palette_range(1, &seg);
                writes += 1 + 3 * 255;
                if self.mid_frame {
                    dos.wait_vretrace();
                }
            }
        }
        self.dac_writes = writes;
        self.vram_bytes = 0;
        self.t += 1;
    }

    fn overlay(&mut self, dos: &Dos, p: &mut Painter<'_>, screen: [f32; 4]) {
        let name = match self.scene {
            Scene::Landscape => "1 landscape",
            Scene::Plasma => "2 plasma",
            Scene::Fade => "3 fade (Space: flash)",
            Scene::Seam => {
                if self.mid_frame {
                    "4 ISA8, rotate at line 200 (T: in vblank)"
                } else {
                    "4 ISA8, rotate in vblank (T: mid-frame)"
                }
            }
        };
        let io_us =
            self.dac_writes as f64 * dos.bus().io_ns.max(game_toolkit_dosvideo::MIN_IO_NS) / 1000.0;
        let line_us = dos.timing().line_ns() / 1000.0;
        let s = format!(
            "{name}   VRAM bytes/frame: {}   DAC port writes/frame: {} ({:.0} us = {:.1} scanlines)",
            self.vram_bytes,
            self.dac_writes,
            io_us,
            io_us / line_us
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
    ("1", "landscape: waterfall, ripples, sun glow"),
    ("2", "plasma"),
    ("3", "fades (Space: flash white)"),
    ("4", "mid-frame rotation on an ISA8 bus"),
    ("T", "scene 4: rotate in vblank / mid-frame"),
    ("Esc", "quit"),
];

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<Cycling>(DosConfig::new("palette cycling", Adapter::Vga).with_keys(KEYS))
}
