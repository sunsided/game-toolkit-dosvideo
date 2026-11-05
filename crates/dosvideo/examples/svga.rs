//! VESA SVGA: resolutions and color depths beyond VGA, and the price of reaching them from
//! real mode.
//!
//! A real-mode program sees video memory only through a 64 KiB window at `A000:0000`. On a
//! 640x480x256 screen (300 KiB) it has to switch *banks* (VBE function 05h) to reach the rest,
//! and a 640-byte scanline does not divide 65536, so some rows straddle two banks. VBE 2.0 (1994)
//! added a linear framebuffer for protected-mode programs (DOS/4GW and friends): all of video
//! memory at one flat address, no banks.
//!
//! * **Scene 1** (`1`): 640x480x8 (mode 101h). Fill the screen four ways and compare the
//!   emulated time: pixel by pixel through the bank window in row order (`X`) or in column
//!   order (`C`, a bank switch every 100-odd pixels), whole rows through the window (`R`,
//!   switching only at bank boundaries), and the linear framebuffer (`L`).
//! * **Scene 2** (`2`): 640x480 with 65536 colors (mode 111h, 5:6:5), a gradient and a
//!   plasma window: no palette, every pixel is its own color.
//! * **Scene 3** (`3`): 800x600x8 (mode 103h) on a 1600x1200 virtual screen (VBE function 06h
//!   sets the scanline length), panned around with the display start (function 07h), which
//!   like the VGA start address takes effect at vertical retrace. Arrows pan, `A` auto-pan.
//!
//! `B` cycles the bus (PCI, VLB, 16-bit ISA), `Esc` quits.

use game_toolkit_dosvideo::game_toolkit::prelude::BlendMode;
use game_toolkit_dosvideo::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scene {
    Fill,
    HiColor,
    Pan,
}

#[derive(Clone, Copy, Debug)]
enum Method {
    PixelRows,
    PixelColumns,
    RowsBanked,
    Lfb,
}

impl Method {
    fn name(self) -> &'static str {
        match self {
            Method::PixelRows => "pixels, banked, row order",
            Method::PixelColumns => "pixels, banked, column order",
            Method::RowsBanked => "rows, banked",
            Method::Lfb => "rows, linear framebuffer",
        }
    }
}

struct FillResult {
    method: Method,
    ms: f64,
    switches: u32,
}

const BUSES: [(&str, BusTiming); 3] = [
    ("PCI", BusTiming::PCI),
    ("VLB", BusTiming::VLB),
    ("ISA16", BusTiming::ISA16),
];

struct SvgaDemo {
    scene: Scene,
    results: Vec<FillResult>,
    runs: u32,
    bus: usize,
    t: u32,
    pan: (i32, i32),
    auto_pan: bool,
}

fn pattern(x: u32, y: u32, run: u32) -> u8 {
    let d = ((x as i32 - 320).pow(2) + (y as i32 - 240).pow(2)) as f32;
    32 + ((d.sqrt() / 6.0) as u32 + run * 5 + ((x / 32) ^ (y / 32))) as u8 % 72
}

impl SvgaDemo {
    fn fill(&mut self, dos: &mut Dos, method: Method) {
        let pitch = dos.vbe_pitch();
        let t0 = dos.now_ns();
        let mut switches = 0;
        let mut bank = dos.vbe_bank();
        let run = self.runs;
        self.runs += 1;
        match method {
            Method::Lfb => {
                for y in 0..480 {
                    let row: Vec<u8> = (0..640).map(|x| pattern(x, y, run)).collect();
                    dos.vbe_write_lfb(y * pitch, &row);
                }
            }
            Method::RowsBanked => {
                for y in 0..480 {
                    let row: Vec<u8> = (0..640).map(|x| pattern(x, y, run)).collect();
                    let off = y * pitch;
                    let (b0, b1) = (off >> 16, (off + 639) >> 16);
                    switches += u32::from(b0 != bank) + u32::from(b1 != b0);
                    bank = b1;
                    dos.vbe_write_banked(off, &row);
                }
            }
            Method::PixelRows | Method::PixelColumns => {
                let columns = matches!(method, Method::PixelColumns);
                for a in 0..if columns { 640 } else { 480 } {
                    for b in 0..if columns { 480 } else { 640 } {
                        let (x, y) = if columns { (a, b) } else { (b, a) };
                        let need = (y * pitch + x) >> 16;
                        if need != bank {
                            switches += 1;
                            bank = need;
                        }
                        dos.vbe_pset_banked(x, y, pattern(x, y, run) as u32);
                    }
                }
            }
        }
        let ms = (dos.now_ns() - t0) / 1e6;
        self.results.retain(|r| r.method as u8 != method as u8);
        self.results.push(FillResult {
            method,
            ms,
            switches,
        });
    }

    fn enter(&mut self, dos: &mut Dos, scene: Scene) {
        self.scene = scene;
        match scene {
            Scene::Fill => {
                dos.vbe_set_mode(0x101);
                self.fill(dos, Method::RowsBanked);
            }
            Scene::HiColor => {
                dos.vbe_set_mode(0x111);
                let pitch = dos.vbe_pitch();
                for y in 0..480u32 {
                    let row: Vec<u8> = (0..640u32)
                        .flat_map(|x| {
                            let r = x * 31 / 639;
                            let g = y * 63 / 479;
                            let b = 31 - (x + y) * 31 / 1118;
                            ((r << 11 | g << 5 | b) as u16).to_le_bytes()
                        })
                        .collect();
                    dos.vbe_write_lfb(y * pitch, &row);
                }
            }
            Scene::Pan => {
                dos.vbe_set_mode(0x103);
                dos.vbe_set_scanline(1600);
                let mut img = Bitmap::new(1600, 1200);
                for ty in 0..12 {
                    for tx in 0..16 {
                        let c = 32 + ((tx * 5 + ty * 7) % 72) as u8;
                        img.fill_rect(tx * 100, ty * 100, 100, 100, c);
                        img.rect(tx * 100, ty * 100, 100, 100, 15);
                        let label = format!("{},{}", tx * 100, ty * 100);
                        img.text(tx * 100 + 6, ty * 100 + 6, &label, 15, None, Font::vga());
                    }
                }
                for r in (50..600).step_by(50) {
                    img.circle(800, 600, r, 14, false);
                }
                img.text(
                    700,
                    590,
                    "1600 x 1200 virtual screen",
                    15,
                    Some(0),
                    Font::vga(),
                );
                let pitch = dos.vbe_pitch();
                for y in 0..1200 {
                    dos.vbe_write_lfb(y * pitch, img.row(y));
                }
            }
        }
    }
}

impl DosGame for SvgaDemo {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        let mut d = Self {
            scene: Scene::Fill,
            results: Vec::new(),
            runs: 0,
            bus: 0,
            t: 0,
            pan: (400, 300),
            auto_pan: true,
        };
        d.enter(dos, Scene::Fill);
        Ok(d)
    }

    fn step(&mut self, dos: &mut Dos) {
        while let Some(k) = dos.getch() {
            match k.scan {
                sc::ESC => dos.exit(),
                sc::N1 => self.enter(dos, Scene::Fill),
                sc::N2 => self.enter(dos, Scene::HiColor),
                sc::N3 => self.enter(dos, Scene::Pan),
                sc::B => {
                    self.bus = (self.bus + 1) % BUSES.len();
                    dos.set_bus(BUSES[self.bus].1);
                }
                sc::A => self.auto_pan = !self.auto_pan,
                sc::X if self.scene == Scene::Fill => self.fill(dos, Method::PixelRows),
                sc::C if self.scene == Scene::Fill => self.fill(dos, Method::PixelColumns),
                sc::R if self.scene == Scene::Fill => self.fill(dos, Method::RowsBanked),
                sc::L if self.scene == Scene::Fill => self.fill(dos, Method::Lfb),
                _ => {}
            }
        }
        self.t += 1;
        match self.scene {
            Scene::Fill => dos.wait_vretrace(),
            Scene::HiColor => {
                // A 192x144 plasma window; the rest of the screen stays as drawn.
                let pitch = dos.vbe_pitch();
                let t = self.t as f32 * 0.05;
                for y in 0..144u32 {
                    let row: Vec<u8> = (0..192u32)
                        .flat_map(|x| {
                            let (fx, fy) = (x as f32 / 20.0, y as f32 / 20.0);
                            let v = (fx + t).sin() + (fy - t).sin() + ((fx + fy) * 0.7 + t).sin();
                            let c = |o: f32| (v + o).sin() * 0.5 + 0.5;
                            let r = (c(0.0) * 31.0) as u32;
                            let g = (c(2.1) * 63.0) as u32;
                            let b = (c(4.2) * 31.0) as u32;
                            ((r << 11 | g << 5 | b) as u16).to_le_bytes()
                        })
                        .collect();
                    dos.vbe_write_lfb((168 + y) * pitch + 224 * 2, &row);
                }
                dos.wait_vretrace();
            }
            Scene::Pan => {
                if self.auto_pan {
                    let t = self.t as f32 * 0.01;
                    self.pan = (
                        (400.0 + 400.0 * t.sin()) as i32,
                        (300.0 + 300.0 * (t * 1.3).cos()) as i32,
                    );
                } else {
                    let s = 6;
                    if dos.key(sc::LEFT) {
                        self.pan.0 -= s;
                    }
                    if dos.key(sc::RIGHT) {
                        self.pan.0 += s;
                    }
                    if dos.key(sc::UP) {
                        self.pan.1 -= s;
                    }
                    if dos.key(sc::DOWN) {
                        self.pan.1 += s;
                    }
                }
                self.pan = (self.pan.0.clamp(0, 800), self.pan.1.clamp(0, 600));
                dos.vbe_set_display_start(self.pan.0 as u32, self.pan.1 as u32, true);
            }
        }
    }

    fn overlay(&mut self, dos: &Dos, p: &mut Painter<'_>, screen: [f32; 4]) {
        let mode = dos.vbe_mode().map_or(0, |m| m.number);
        let (w, h) = dos.resolution();
        let mut lines = vec![format!(
            "VBE mode {mode:03X}h {w}x{h} {:.2} Hz   bus {} (B)   1 fill  2 hi-color  3 panning   Esc quit",
            dos.refresh_hz(),
            BUSES[self.bus].0
        )];
        match self.scene {
            Scene::Fill => {
                lines.push("fill 640x480: X pixels/rows  C pixels/columns  R rows banked  L linear framebuffer".into());
                for r in &self.results {
                    lines.push(format!(
                        "  {:<30} {:>8.2} ms  {:>6} bank switches",
                        r.method.name(),
                        r.ms,
                        r.switches
                    ));
                }
            }
            Scene::HiColor => lines.push(
                "5:6:5 direct color: gradient drawn once, plasma window redrawn every frame".into(),
            ),
            Scene::Pan => lines.push(format!(
                "display start ({}, {})   A auto-pan:{}   arrows pan",
                self.pan.0,
                self.pan.1,
                if self.auto_pan { "on" } else { "off" }
            )),
        }
        let y0 =
            (screen[1] + screen[3] + 6.0).min(p.visible_rect().1[1] - 20.0 * lines.len() as f32);
        for (i, l) in lines.iter().enumerate() {
            let pos = [screen[0] + 8.0, y0 + i as f32 * 18.0];
            p.rect_ex(
                [pos[0] - 4.0, pos[1] - 2.0],
                [l.len() as f32 * 8.5 + 8.0, 18.0],
                [0.0, 0.0, 0.0, 0.7],
                2,
                BlendMode::Alpha,
            );
            p.text(pos, l, 15.0, [0.85, 0.85, 0.85, 1.0]);
        }
    }
}

const KEYS: &[(&str, &str)] = &[
    (
        "1",
        "640x480x8 fill benchmark (X/C/R/L: pixel rows, pixel columns, banked rows, LFB)",
    ),
    ("2", "640x480 hi-color"),
    ("3", "800x600 panning (arrows, A auto-pan)"),
    ("B", "bus: PCI / VLB / 16-bit ISA"),
    ("Esc", "quit"),
];

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<SvgaDemo>(
        DosConfig::new("SVGA", Adapter::Svga { memory_kib: 2048 })
            .with_bus(BusTiming::PCI)
            .with_keys(KEYS),
    )
}
