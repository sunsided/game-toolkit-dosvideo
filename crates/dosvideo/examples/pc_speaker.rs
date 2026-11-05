//! The PC speaker: one square wave, no volume control, and all the tricks to get more out of it.
//!
//! - **Piano**: hold `A`-`K` (white keys) and `W` `E` `T` `Y` `U` (black keys): `sound()` /
//!   `nosound()`, PIT channel 2 and port 0x61.
//! - **Tracker music** (`M`): a [synthie](https://docs.rs/synthie) song played on the speaker. One
//!   voice for a whole band: chords become a fast arpeggio (one note per frame), drums become
//!   pitch sweeps and noise bursts. `V` changes the voicing, `O` the octave.
//! - **QBasic `PLAY`** (`Q`): a melody in `PLAY` notation, playing in the background.
//! - **Sound effects** (`Z` laser, `X` explosion, `C` coin): pitch changed once per frame.
//! - **Digitized sound** (`P`): bit-banging the speaker with pulse-width modulation. It sounds
//!   rough and takes all of the CPU: the screen freezes until it is done, like it did.
//!
//! `Esc` quits.

use game_toolkit_dosvideo::palette::color::*;
use game_toolkit_dosvideo::prelude::*;
use game_toolkit_dosvideo::speaker_synth::{SpeakerTracker, Voicing};
use game_toolkit_dosvideo::text::{BOX_DOUBLE, BOX_SINGLE, attr};

/// Beethoven, Ode to Joy (public domain), in QBasic PLAY notation.
const ODE_TO_JOY: &str = "T140 O3 L4 EEFG GFED CCDE E.D8D2 EEFG GFED CCDE D.C8C2 \
                          DDEC DE8F8EC DE8F8ED CDG<G> EEFG GFED CCDE D.C8C2";

/// Piano keys: (scancode, label, semitones above C4).
const PIANO: [(u8, &str, i32); 13] = [
    (sc::A, "A", 0),
    (sc::W, "W", 1),
    (sc::S, "S", 2),
    (sc::E, "E", 3),
    (sc::D, "D", 4),
    (sc::F, "F", 5),
    (sc::T, "T", 6),
    (sc::G, "G", 7),
    (sc::Y, "Y", 8),
    (sc::H, "H", 9),
    (sc::U, "U", 10),
    (sc::J, "J", 11),
    (sc::K, "K", 12),
];

#[derive(Clone, Copy, PartialEq)]
enum Sfx {
    Laser,
    Explosion,
    Coin,
}

struct Speaker {
    tracker: Option<SpeakerTracker>,
    voicing: Voicing,
    octave: i32,
    melody: Option<Melody>,
    sfx: Option<(Sfx, u32)>,
    rng: u32,
    history: Vec<Option<f64>>,
    status: String,
}

fn piano_hz(semis: i32) -> f64 {
    261.63 * 2f64.powf(semis as f64 / 12.0)
}

/// A synthesized "zap-boing" for the PWM demo: 8-bit unsigned at 11 kHz.
fn sample() -> Vec<u8> {
    let rate = 11_025.0;
    (0..(rate * 0.9) as usize)
        .map(|i| {
            let t = i as f64 / rate;
            let f = 900.0 * (-t * 3.0).exp() + 120.0;
            let env = (-t * 2.5).exp();
            let wobble = (t * 18.0).sin() * 0.3 + 1.0;
            let v = (t * f * std::f64::consts::TAU * wobble).sin() * env;
            (128.0 + v * 120.0).clamp(0.0, 255.0) as u8
        })
        .collect()
}

impl Speaker {
    fn random(&mut self) -> u32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng
    }

    fn draw_static(dos: &mut Dos) {
        dos.hide_cursor();
        // Attribute bit 7 = bright background, not blink: white keys need it.
        dos.set_blink(false);
        dos.text_clear(attr(LIGHT_GRAY, BLUE));
        dos.text_box(0, 0, 80, 25, attr(YELLOW, BLUE), BOX_DOUBLE);
        dos.text_center(0, " PC SPEAKER ", attr(BLACK, YELLOW));
        dos.text_print(
            3,
            2,
            "PIT channel 2 \u{2192} port 61h bits 0+1 \u{2192} a 2\" paper cone",
            attr(WHITE, BLUE),
        );
        // Piano.
        dos.text_box(3, 4, 46, 9, attr(LIGHT_CYAN, BLUE), BOX_SINGLE);
        dos.text_print(5, 4, " piano (hold the keys) ", attr(LIGHT_CYAN, BLUE));
        let menu = [
            ("M", "tracker music"),
            ("V", "voicing"),
            ("O", "octave shift"),
            ("Q", "PLAY: Ode to Joy"),
            ("Z X C", "laser, boom, coin"),
            ("P", "PWM sample"),
            ("Esc", "quit"),
        ];
        dos.text_box(51, 4, 27, 9, attr(LIGHT_CYAN, BLUE), BOX_SINGLE);
        for (i, (k, d)) in menu.iter().enumerate() {
            dos.text_print(53, 5 + i as u32, k, attr(YELLOW, BLUE));
            dos.text_print(59, 5 + i as u32, d, attr(LIGHT_GRAY, BLUE));
        }
        dos.text_box(3, 14, 75, 9, attr(LIGHT_CYAN, BLUE), BOX_SINGLE);
        dos.text_print(
            5,
            14,
            " pitch over the last frames ",
            attr(LIGHT_CYAN, BLUE),
        );
    }

    fn draw_piano(&self, dos: &mut Dos) {
        for (i, &(scan, label, semis)) in PIANO.iter().enumerate() {
            let black = matches!(semis % 12, 1 | 3 | 6 | 8 | 10);
            let down = dos.key(scan);
            let x = 6 + i as u32 * 3;
            let (fg, bg) = match (black, down) {
                (_, true) => (BLACK, LIGHT_RED),
                (true, false) => (WHITE, BLACK),
                (false, false) => (BLACK, WHITE),
            };
            let y0 = if black { 6 } else { 8 };
            for y in 6..11 {
                let a = if y >= y0 {
                    attr(fg, bg)
                } else {
                    attr(LIGHT_GRAY, BLUE)
                };
                dos.text_put(x, y, b' ', a);
                dos.text_put(x + 1, y, b' ', a);
            }
            dos.text_print(x, 10, label, attr(fg, bg));
        }
    }

    fn draw_scope(&self, dos: &mut Dos) {
        // One column per frame; height = log pitch.
        let cols = 71usize;
        let start = self.history.len().saturating_sub(cols);
        for (i, f) in self.history[start..].iter().enumerate() {
            let h = f
                .map_or(0, |hz| ((hz.max(55.0) / 55.0).log2() * 1.2).round() as u32)
                .min(7);
            for row in 0..7u32 {
                let lit = 7 - row <= h && h > 0;
                let (ch, a) = if lit {
                    (0xDB, attr(LIGHT_GREEN, BLUE))
                } else {
                    (b' ', attr(LIGHT_GRAY, BLUE))
                };
                dos.text_put(5 + i as u32, 15 + row, ch, a);
            }
        }
    }

    fn draw_status(&self, dos: &mut Dos) {
        let tone = dos
            .speaker_tone()
            .map_or("  silent  ".to_string(), |f| format!("{f:7.1} Hz"));
        dos.text_fill(3, 23, 75, 1, b' ', attr(LIGHT_GRAY, BLUE));
        dos.text_print(
            3,
            23,
            &format!("{tone}   {}", self.status),
            attr(WHITE, BLUE),
        );
        let music = format!(
            "music:{}  voicing:{:<8} octave:{:+}",
            if self.tracker.is_some() { "on " } else { "off" },
            format!("{:?}", self.voicing).to_lowercase(),
            self.octave
        );
        dos.text_print(5, 11, &format!("{music:<42}"), attr(LIGHT_GRAY, BLUE));
    }

    fn sfx_pitch(&mut self) -> Option<f64> {
        let (kind, age) = self.sfx?;
        let r = self.random();
        let hz = match kind {
            Sfx::Laser => (age < 12).then(|| 2400.0 * 0.78f64.powi(age as i32)),
            Sfx::Explosion => (age < 24).then_some(60.0 + (r % (900 - age * 30).max(40)) as f64),
            Sfx::Coin => [988.0, 988.0, 1319.0, 1319.0, 1319.0, 1319.0]
                .get(age as usize)
                .copied(),
        };
        self.sfx = hz.map(|_| (kind, age + 1));
        hz
    }
}

impl DosGame for Speaker {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        dos.set_mode(0x03);
        Self::draw_static(dos);
        Ok(Self {
            tracker: None,
            voicing: Voicing::Arpeggio,
            octave: 0,
            melody: None,
            sfx: None,
            rng: 0xC0FFEE,
            history: Vec::new(),
            status: "hold A..K to play".into(),
        })
    }

    fn step(&mut self, dos: &mut Dos) {
        dos.wait_vretrace();
        while let Some(k) = dos.getch() {
            match k.scan {
                sc::ESC => dos.exit(),
                sc::M => {
                    if let Some(mut t) = self.tracker.take() {
                        t.mute(dos);
                        self.status = "music off".into();
                    } else {
                        let mut t = SpeakerTracker::new(synthie::songs::lazy_jones());
                        t.voicing = self.voicing;
                        t.octave_shift = self.octave;
                        self.tracker = Some(t);
                        self.status = "synthie's lazy_jones() on one square wave".into();
                    }
                }
                sc::V => {
                    self.voicing = match self.voicing {
                        Voicing::Arpeggio => Voicing::Lead,
                        Voicing::Lead => Voicing::Highest,
                        Voicing::Highest => Voicing::Arpeggio,
                    };
                    if let Some(t) = &mut self.tracker {
                        t.voicing = self.voicing;
                    }
                }
                sc::O => {
                    self.octave = if self.octave >= 1 {
                        -1
                    } else {
                        self.octave + 1
                    };
                    if let Some(t) = &mut self.tracker {
                        t.octave_shift = self.octave;
                    }
                }
                sc::Q => {
                    self.melody = Some(Melody::new(ODE_TO_JOY));
                    self.status = "PLAY \"T140 O3 L4 EEFG GFED ...\"".into();
                }
                sc::Z => self.sfx = Some((Sfx::Laser, 0)),
                sc::X => self.sfx = Some((Sfx::Explosion, 0)),
                sc::C => self.sfx = Some((Sfx::Coin, 0)),
                sc::P => {
                    if let Some(t) = &mut self.tracker {
                        t.mute(dos);
                    }
                    dos.text_print(
                        3,
                        23,
                        &format!(
                            "{:<75}",
                            "   PWM sample playing: the CPU does nothing else..."
                        ),
                        attr(YELLOW, RED),
                    );
                    dos.speaker_pwm(&sample(), 11_025);
                    self.status = "PWM done: 0.9 s of CPU time for 0.9 s of sound".into();
                }
                _ => {}
            }
        }

        // Priority: the piano, then effects, then the melody, then the tracker.
        let piano = PIANO
            .iter()
            .rev()
            .find(|p| dos.key(p.0))
            .map(|p| piano_hz(p.2));
        let sfx = self.sfx_pitch();
        if let Some(hz) = piano.or(sfx) {
            if let Some(t) = &mut self.tracker {
                // Keep the song's position moving underneath, but silenced.
                t.update(dos);
                t.mute(dos);
            }
            if dos.speaker_tone().is_none_or(|f| (f - hz).abs() > 0.5) {
                dos.sound(hz);
            }
        } else if let Some(m) = &mut self.melody {
            m.update(dos);
            if m.is_done() {
                self.melody = None;
            }
        } else if let Some(t) = &mut self.tracker {
            t.update(dos);
        } else if dos.speaker_tone().is_some() {
            dos.nosound();
        }

        self.history.push(dos.speaker_tone());
        if self.history.len() > 200 {
            self.history.drain(..100);
        }
        self.draw_piano(dos);
        self.draw_scope(dos);
        self.draw_status(dos);
    }
}

const KEYS: &[(&str, &str)] = &[
    ("A-K, W E T Y U", "piano (hold)"),
    ("M", "tracker music on/off (synthie song on the speaker)"),
    ("V", "voicing: arpeggio / lead / highest note"),
    ("O", "octave shift"),
    ("Q", "QBasic PLAY: Ode to Joy"),
    ("Z X C", "sound effects: laser, explosion, coin"),
    ("P", "digitized sound by PWM (freezes the screen)"),
    ("Esc", "quit"),
];

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<Speaker>(DosConfig::new("PC speaker", Adapter::Vga).with_keys(KEYS))
}
