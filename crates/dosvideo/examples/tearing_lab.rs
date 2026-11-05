//! Why DOS games tore: bus speed against the beam.
//!
//! A mode 13h game drew into a back buffer in system memory and copied the whole 64000 bytes
//! to `A000:0000` every frame. How long that copy takes depends on the bus the video card sits
//! on: about 64 ms on an 8-bit ISA card, 16 ms on a 16-bit ISA VGA - longer than a 70 Hz
//! frame (14.3 ms) - and 2 ms on a VESA Local Bus card. Whenever the beam overtakes the copy,
//! the top of the screen shows the new frame and the bottom the old one: a tear.
//!
//! Three copy strategies:
//! - *no wait*: copy whenever the frame is ready; tears land anywhere.
//! - *vsync*: wait for vertical retrace, then copy. Clean only if the copy outruns the beam
//!   (a fast bus); on ISA the beam catches up a third of the way down.
//! - *race the beam*: wait until the beam has started the frame, then copy *behind* it. The
//!   beam is faster than an ISA copy, so it never meets the writer: tear-free on a 16-bit
//!   card, one frame later. On an 8-bit card the copy is so slow that nothing helps.
//!
//! Keys: `1`-`5` bus (8-bit ISA, 16-bit ISA, VLB, PCI, instant), `S` next strategy, `Esc` quit.

use game_toolkit_dosvideo::prelude::*;

const BUSES: [(&str, BusTiming); 5] = [
    ("8-bit ISA", BusTiming::ISA8),
    ("16-bit ISA", BusTiming::ISA16),
    ("VLB", BusTiming::VLB),
    ("PCI", BusTiming::PCI),
    ("instant", BusTiming::INSTANT),
];

const STRATEGIES: [&str; 3] = ["no wait", "vsync, then copy", "race the beam"];

/// Bar speed in pixels per second.
const SPEED: f64 = 960.0;

struct Lab {
    back: Bitmap,
    bus: usize,
    strategy: usize,
    copy_ns: f64,
    step_ns: f64,
    last_step: f64,
}

impl Lab {
    fn draw(&mut self, t: f64) {
        let b = &mut self.back;
        // Horizontal bands with row labels, so the tear line can be read off.
        for y in 0..200 {
            let c = if (y / 20) % 2 == 0 { 17 } else { 19 };
            b.hline(0, 319, y, c);
        }
        let font = Font::cga();
        for y in (0..200).step_by(20) {
            b.text(2, y + 6, &format!("{y:3}"), 7, None, font);
        }
        // Three tall bars racing across.
        let x0 = (t * SPEED).rem_euclid(320.0 + 40.0) as i32 - 20;
        for (i, c) in [(0, 15u8), (110, 40), (220, 48)] {
            let x = (x0 + i).rem_euclid(360) - 20;
            if x < 320 {
                b.fill_rect(x, 0, 14, 200, c);
            }
        }
    }
}

impl DosGame for Lab {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        dos.set_mode(0x13);
        let bus = 1;
        dos.set_bus(BUSES[bus].1);
        Ok(Self {
            back: Bitmap::new(320, 200),
            bus,
            strategy: 1,
            copy_ns: 0.0,
            step_ns: 0.0,
            last_step: dos.now_ns(),
        })
    }

    fn step(&mut self, dos: &mut Dos) {
        while let Some(k) = dos.getch() {
            match k.scan {
                sc::ESC => dos.exit(),
                sc::S => self.strategy = (self.strategy + 1) % STRATEGIES.len(),
                s @ sc::N1..=sc::N5 => {
                    self.bus = (s - sc::N1) as usize;
                    dos.set_bus(BUSES[self.bus].1);
                }
                _ => {}
            }
        }

        // The game's own work for the frame: logic, drawing into system memory.
        dos.cpu(300_000);
        self.draw(dos.now());

        match self.strategy {
            0 => {}
            1 => dos.wait_vretrace(),
            // A few scanlines into the frame, so the beam is safely ahead of the first row.
            _ => dos.wait_beam(4, 0),
        }
        let t0 = dos.now_ns();
        dos.blit13(&self.back);
        self.copy_ns = dos.now_ns() - t0;
        let now = dos.now_ns();
        self.step_ns = now - self.last_step;
        self.last_step = now;
    }

    fn overlay(&mut self, dos: &Dos, p: &mut Painter<'_>, screen: [f32; 4]) {
        let frame_ms = dos.timing().frame_ns() / 1e6;
        let copy_ms = self.copy_ns / 1e6;
        let rate = if self.copy_ns > 0.0 {
            format!("{:.1} MB/s", 64000.0 / self.copy_ns * 1e3)
        } else {
            "unlimited".into()
        };
        let lines = [
            format!(
                "bus {} ({rate})   copy {copy_ms:.1} ms vs frame {frame_ms:.1} ms   {:.0} fps",
                BUSES[self.bus].0,
                1e9 / self.step_ns.max(1.0)
            ),
            format!("strategy: {}", STRATEGIES[self.strategy]),
            "1-5 bus (ISA8, ISA16, VLB, PCI, instant)   S strategy   Esc quit".into(),
        ];
        for (i, s) in lines.iter().enumerate() {
            let y = screen[1] + screen[3] + 6.0 + i as f32 * 18.0;
            p.text([screen[0] + 8.0, y], s, 16.0, [0.7, 0.7, 0.7, 1.0]);
        }
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<Lab>(DosConfig::new("tearing lab", Adapter::Vga))
}
