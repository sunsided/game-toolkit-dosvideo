//! The PC speaker: a paper cone wired to PIT channel 2 and two bits of port 0x61.
//!
//! - **Tones**: program channel 2 as a square wave (`OUT 43h, B6h`, then the divisor low and
//!   high byte to port 0x42), and set bits 0 (timer gate) and 1 (speaker data) of port 0x61.
//!   The pitch is `1193182 Hz / divisor`. [`Dos::sound`] and [`Dos::nosound`] do that.
//! - **Bit-banging**: with the gate off, bit 1 of port 0x61 *is* the cone position. Toggling it
//!   fast enough, with varying duty cycles, plays (rough) digitized sound: the trick behind
//!   RealSound and countless title screens that froze the game while they talked.
//!   [`Dos::speaker_pwm`] does it.
//!
//! Every change is timestamped on the machine clock and rendered to PCM later by integrating
//! the cone position over each output sample, so ultrasonic carriers average out like they did
//! in the real cone, and pulse widths become amplitudes.
//!
//! [`Melody`] plays QBasic `PLAY` strings in the background, one call per main-loop pass.

use std::collections::VecDeque;

use crate::machine::{Dos, PIT_HZ};

/// The speaker's state from a point in time on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct State {
    /// Port 0x61 bit 0: PIT channel 2 gate.
    gate: bool,
    /// Port 0x61 bit 1: speaker data (cone driven by the timer output, or held low).
    data: bool,
    /// Channel 2 divisor.
    divisor: u32,
    /// Machine time (ns) the counter was loaded.
    epoch: f64,
}

impl State {
    /// Average cone position (0..1) over `[ta, tb)` in ns.
    fn average(&self, ta: f64, tb: f64) -> f64 {
        if !self.data {
            return 0.0;
        }
        if !self.gate {
            // Mode 3 with the gate low holds the output high.
            return 1.0;
        }
        let d = self.divisor as f64;
        let h = (self.divisor as f64 / 2.0).ceil();
        let high = |x: f64| {
            let x = x.max(0.0);
            (x / d).floor() * h + (x % d).min(h)
        };
        let k = PIT_HZ * 1e-9;
        let x0 = (ta - self.epoch) * k;
        let x1 = (tb - self.epoch) * k;
        if x1 <= x0 {
            return 0.0;
        }
        (high(x1) - high(x0)) / (x1 - x0)
    }

    /// Output level at time `t` (for reading port 0x61 bit 5).
    fn out2(&self, t: f64) -> bool {
        if !self.gate {
            return true;
        }
        let x = ((t - self.epoch) * PIT_HZ * 1e-9).max(0.0);
        x % self.divisor as f64 <= (self.divisor as f64 / 2.0).ceil()
    }
}

/// Speaker hardware state and its change log.
pub(crate) struct PcSpeaker {
    cur: State,
    /// Changes not yet rendered, with the time they take effect.
    pending: VecDeque<(f64, State)>,
    /// Rendered up to this machine time.
    rendered: f64,
    write_lo: Option<u8>,
    hp_x: f64,
    hp_y: f64,
    lp: f64,
    lp2: f64,
    pub(crate) port61: u8,
}

/// Changes kept when nobody renders audio (no host audio, headless): older ones are dropped.
const MAX_PENDING: usize = 1 << 16;

impl PcSpeaker {
    pub fn new() -> Self {
        Self {
            cur: State {
                gate: false,
                data: false,
                divisor: 65536,
                epoch: 0.0,
            },
            pending: VecDeque::new(),
            rendered: 0.0,
            write_lo: None,
            hp_x: 0.0,
            hp_y: 0.0,
            lp: 0.0,
            lp2: 0.0,
            port61: 0,
        }
    }

    fn latest(&self) -> State {
        self.pending.back().map_or(self.cur, |p| p.1)
    }

    fn change(&mut self, t: f64, f: impl FnOnce(&mut State)) {
        let mut s = self.latest();
        f(&mut s);
        if s != self.latest() {
            if self.pending.len() >= MAX_PENDING
                && let Some((t0, s0)) = self.pending.pop_front()
            {
                self.cur = s0;
                self.rendered = self.rendered.max(t0);
            }
            self.pending.push_back((t, s));
        }
    }

    /// Port 0x43 with channel 2 selected.
    pub fn command(&mut self, _v: u8) {
        self.write_lo = None;
    }

    /// Port 0x42: the divisor, low byte then high byte.
    pub fn write_counter(&mut self, t: f64, v: u8) {
        match self.write_lo.take() {
            None => self.write_lo = Some(v),
            Some(lo) => {
                let d = (lo as u32) | ((v as u32) << 8);
                let d = if d == 0 { 65536 } else { d };
                self.change(t, |s| {
                    s.divisor = d;
                    s.epoch = t;
                });
            }
        }
    }

    pub fn write_port61(&mut self, t: f64, v: u8) {
        self.port61 = v;
        self.change(t, |s| {
            s.gate = v & 1 != 0;
            s.data = v & 2 != 0;
        });
    }

    pub fn read_port61(&self, t: f64) -> u8 {
        // Bit 4 toggles with every DRAM refresh cycle (~15 µs): a free-running timer that
        // delay loops counted. Bit 5 is the channel 2 output.
        let refresh = ((t / 15_085.0) as u64 & 1) as u8;
        let s = self.state_at(t);
        (self.port61 & 0x0F) | (refresh << 4) | (u8::from(s.out2(t)) << 5)
    }

    fn state_at(&self, t: f64) -> State {
        let mut s = self.cur;
        for &(pt, ps) in &self.pending {
            if pt <= t {
                s = ps;
            } else {
                break;
            }
        }
        s
    }

    /// Is the speaker currently making a sound (gate and data on)?
    pub fn tone_on(&self) -> bool {
        let s = self.latest();
        s.gate && s.data
    }

    pub fn divisor(&self) -> u32 {
        self.latest().divisor
    }

    /// Render samples from the last rendered time up to `until` (ns) at `rate` Hz, appending
    /// to `out` (mono, -1..1).
    pub fn render(&mut self, until: f64, rate: u32, out: &mut Vec<f32>) {
        let dt = 1e9 / rate as f64;
        // High-pass (cone and coupling: no DC) ~20 Hz; two-pole low-pass (a small cone's
        // inertia) ~7 kHz, which also tames PWM carriers.
        let a_hp = 1.0 / (1.0 + 2.0 * std::f64::consts::PI * 20.0 / rate as f64);
        let a_lp = 1.0 - (-2.0 * std::f64::consts::PI * 7_000.0 / rate as f64).exp();
        while self.rendered + dt <= until {
            let t0 = self.rendered;
            let t1 = t0 + dt;
            let mut acc = 0.0;
            let mut t = t0;
            while t < t1 {
                let next = self.pending.front().map_or(f64::INFINITY, |p| p.0);
                if next <= t {
                    if let Some((_, s)) = self.pending.pop_front() {
                        self.cur = s;
                    }
                    continue;
                }
                let e = t1.min(next);
                acc += self.cur.average(t, e) * (e - t);
                t = e;
            }
            let x = acc / dt;
            let y = a_hp * (self.hp_y + x - self.hp_x);
            self.hp_x = x;
            self.hp_y = y;
            self.lp += a_lp * (y - self.lp);
            self.lp2 += a_lp * (self.lp - self.lp2);
            out.push((self.lp2 * 0.5) as f32);
            self.rendered = t1;
        }
    }

    /// Forget everything before `t` (skip rendering).
    pub fn skip_to(&mut self, t: f64) {
        while let Some(&(pt, s)) = self.pending.front() {
            if pt > t {
                break;
            }
            self.cur = s;
            self.pending.pop_front();
        }
        self.rendered = self.rendered.max(t);
    }
}

impl Dos {
    /// Play a tone on the PC speaker until [`Dos::nosound`] (Turbo Pascal's `Sound`): program
    /// PIT channel 2 for a square wave at `hz` and connect it to the speaker.
    pub fn sound(&mut self, hz: f64) {
        let d = (PIT_HZ / hz.max(19.0)).round().clamp(1.0, 65535.0) as u16;
        self.outb(0x43, 0xB6);
        self.outb(0x42, d as u8);
        self.outb(0x42, (d >> 8) as u8);
        let v = self.inb(0x61);
        self.outb(0x61, v | 0x03);
    }

    /// Silence the speaker.
    pub fn nosound(&mut self) {
        let v = self.inb(0x61);
        self.outb(0x61, v & !0x03);
    }

    /// Play 8-bit unsigned samples by bit-banging the speaker (pulse-width modulation with the
    /// timer gate off). Like on the real thing this takes all the CPU: the call returns when
    /// the sound is over, `samples.len() / rate` seconds of machine time later, and the screen
    /// does not change meanwhile.
    pub fn speaker_pwm(&mut self, samples: &[u8], rate: u32) {
        let period = 1e9 / rate.max(1) as f64;
        let base = self.inb(0x61) & !0x03;
        let start = self.now_ns();
        for (i, &s) in samples.iter().enumerate() {
            let t0 = start + i as f64 * period;
            self.idle_until(t0);
            let high = period * s as f64 / 255.0;
            if high > 0.0 {
                self.outb(0x61, base | 0x02);
                self.idle_until(t0 + high);
            }
            self.outb(0x61, base);
        }
        self.idle_until(start + samples.len() as f64 * period);
    }

    /// Render the speaker output from where the last call stopped up to now, at `rate` Hz,
    /// appending mono samples in -1..1 to `out`. The host calls this; call it yourself to
    /// capture sound without the game-toolkit host.
    pub fn speaker_render(&mut self, rate: u32, out: &mut Vec<f32>) {
        let now = self.now_ns();
        self.speaker.render(now, rate, out);
    }

    /// Drop speaker output up to now without rendering it.
    pub fn speaker_skip(&mut self) {
        let now = self.now_ns();
        self.speaker.skip_to(now);
    }

    /// The tone currently playing, in Hz (`None` when the speaker is off or bit-banged).
    pub fn speaker_tone(&self) -> Option<f64> {
        self.speaker
            .tone_on()
            .then(|| PIT_HZ / self.speaker.divisor() as f64)
    }
}

/// Note lengths are in whole notes; articulation is the fraction of a note that sounds.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Event {
    /// Frequency in Hz, `None` for a rest.
    hz: Option<f64>,
    /// Duration in seconds.
    dur: f64,
    /// Part of the duration that sounds.
    on: f64,
}

/// A tune in QBasic `PLAY` notation, played in the background on the PC speaker.
///
/// Octave 3 starts at middle C (`O3 A` is 440 Hz), the default octave is 4, the default tempo
/// 120 and the default length a quarter note, as in QBasic.
///
/// Supported: notes `A`-`G` with `#`/`+` (sharp) and `-` (flat), a length after the note (`C8`),
/// dots (`C4.`), `N0`-`N84` (note number), `O0`-`O6` (octave), `<` / `>` (octave down / up),
/// `L1`-`L64` (default length), `T32`-`T255` (tempo, quarter notes per minute), `P` / `R`
/// (pause, same lengths), `MS` / `MN` / `ML` (staccato, normal, legato). `MF` / `MB` are
/// accepted and ignored: this always plays in the background.
///
/// Call [`Melody::update`] once per pass of the main loop; it switches the speaker at the right
/// machine times (as precisely as the loop runs).
#[derive(Clone, Debug)]
pub struct Melody {
    events: Vec<Event>,
    start: Option<f64>,
    index: usize,
    sounding: bool,
    looped: bool,
}

impl Melody {
    /// Parse a `PLAY` string. Unknown characters are skipped.
    pub fn new(play: &str) -> Self {
        Self {
            events: parse_play(play),
            start: None,
            index: 0,
            sounding: false,
            looped: false,
        }
    }

    /// Start over when the end is reached.
    pub fn looped(mut self, on: bool) -> Self {
        self.looped = on;
        self
    }

    /// Length in seconds.
    pub fn duration(&self) -> f64 {
        self.events.iter().map(|e| e.dur).sum()
    }

    pub fn is_done(&self) -> bool {
        !self.looped && self.start.is_some() && self.index >= self.events.len()
    }

    /// Restart from the beginning at the next update.
    pub fn restart(&mut self) {
        self.start = None;
        self.index = 0;
    }

    /// Stop and silence the speaker.
    pub fn stop(&mut self, dos: &mut Dos) {
        self.index = self.events.len();
        self.looped = false;
        if self.sounding {
            dos.nosound();
            self.sounding = false;
        }
    }

    /// Advance to the machine's current time, switching tones as needed.
    pub fn update(&mut self, dos: &mut Dos) {
        if self.events.is_empty() {
            return;
        }
        let now = dos.now();
        let start = *self.start.get_or_insert(now);
        let mut t = start
            + self.events[..self.index.min(self.events.len())]
                .iter()
                .map(|e| e.dur)
                .sum::<f64>();
        loop {
            if self.index >= self.events.len() {
                if self.looped {
                    self.index = 0;
                    self.start = Some(t);
                    continue;
                }
                if self.sounding {
                    dos.nosound();
                    self.sounding = false;
                }
                return;
            }
            let e = self.events[self.index];
            let want = if now < t + e.on { e.hz } else { None };
            if now >= t + e.dur {
                self.index += 1;
                t += e.dur;
                continue;
            }
            match want {
                Some(hz) => {
                    if !self.sounding
                        || dos
                            .speaker_tone()
                            .is_none_or(|f| (f - hz).abs() > hz * 0.01)
                    {
                        dos.sound(hz);
                        self.sounding = true;
                    }
                }
                None => {
                    if self.sounding {
                        dos.nosound();
                        self.sounding = false;
                    }
                }
            }
            return;
        }
    }
}

fn parse_play(s: &str) -> Vec<Event> {
    let b: Vec<u8> = s.to_ascii_uppercase().into_bytes();
    let mut i = 0;
    let mut octave: i32 = 4;
    let mut length = 4.0f64;
    let mut tempo = 120.0f64;
    let mut art = 7.0 / 8.0;
    let mut out = Vec::new();
    let num = |i: &mut usize| -> Option<u32> {
        let st = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        std::str::from_utf8(&b[st..*i]).ok()?.parse().ok()
    };
    let dots = |i: &mut usize| -> f64 {
        let mut m = 1.0;
        let mut add = 0.5;
        while *i < b.len() && b[*i] == b'.' {
            m += add;
            add /= 2.0;
            *i += 1;
        }
        m
    };
    // Seconds of a note of length `l` (4 = quarter).
    let secs = |l: f64, tempo: f64| 60.0 / tempo * 4.0 / l;
    // Note number n: 1 = C in octave 0 ... 84; A in octave 3 (n = 46) is 440 Hz.
    let freq = |n: i32| 440.0 * 2f64.powf((n - 46) as f64 / 12.0);
    while i < b.len() {
        let c = b[i];
        i += 1;
        match c {
            b'A'..=b'G' => {
                let semis = [9, 11, 0, 2, 4, 5, 7][(c - b'A') as usize];
                let mut n = octave * 12 + semis + 1;
                if i < b.len() && (b[i] == b'#' || b[i] == b'+') {
                    n += 1;
                    i += 1;
                } else if i < b.len() && b[i] == b'-' {
                    n -= 1;
                    i += 1;
                }
                let l = num(&mut i).map_or(length, |v| v.clamp(1, 64) as f64);
                let d = secs(l, tempo) * dots(&mut i);
                out.push(Event {
                    hz: Some(freq(n)),
                    dur: d,
                    on: d * art,
                });
            }
            b'N' => {
                let n = num(&mut i).unwrap_or(0) as i32;
                let d = secs(length, tempo) * dots(&mut i);
                out.push(Event {
                    hz: (n > 0).then(|| freq(n)),
                    dur: d,
                    on: d * art,
                });
            }
            b'P' | b'R' => {
                let l = num(&mut i).map_or(length, |v| v.clamp(1, 64) as f64);
                let d = secs(l, tempo) * dots(&mut i);
                out.push(Event {
                    hz: None,
                    dur: d,
                    on: 0.0,
                });
            }
            b'O' => octave = num(&mut i).unwrap_or(4).min(6) as i32,
            b'<' => octave = (octave - 1).max(0),
            b'>' => octave = (octave + 1).min(6),
            b'L' => length = num(&mut i).unwrap_or(4).clamp(1, 64) as f64,
            b'T' => tempo = num(&mut i).unwrap_or(120).clamp(32, 255) as f64,
            b'M' if i < b.len() => {
                match b[i] {
                    b'S' => art = 3.0 / 4.0,
                    b'N' => art = 7.0 / 8.0,
                    b'L' => art = 1.0,
                    _ => {}
                }
                i += 1;
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{BusTiming, MachineConfig};

    fn dos() -> Dos {
        Dos::new(MachineConfig::default().with_bus(BusTiming::INSTANT))
    }

    /// Dominant frequency by counting zero crossings.
    fn crossings_hz(s: &[f32], rate: u32) -> f64 {
        let n = s.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        n as f64 * rate as f64 / s.len() as f64
    }

    #[test]
    fn a_tone_comes_out_at_its_pitch() {
        let mut d = dos();
        d.sound(440.0);
        d.delay_us(500_000.0);
        let mut out = Vec::new();
        d.speaker_render(44_100, &mut out);
        assert!((out.len() as i64 - 22_050).abs() < 5);
        let tail = &out[4410..];
        assert!((crossings_hz(tail, 44_100) - 440.0).abs() < 5.0);
        assert!(tail.iter().any(|&v| v > 0.1));
        d.nosound();
        d.delay_us(500_000.0);
        let mut quiet = Vec::new();
        d.speaker_render(44_100, &mut quiet);
        assert!(
            quiet[8000..].iter().all(|v| v.abs() < 0.01),
            "silence after nosound"
        );
    }

    #[test]
    fn port61_reflects_gate_and_refresh() {
        let mut d = dos();
        let a = d.inb(0x61) & 0x10;
        d.delay_us(15.085);
        let b = d.inb(0x61) & 0x10;
        assert_ne!(a, b, "refresh bit toggles every ~15 us");
        d.sound(1000.0);
        assert_eq!(d.inb(0x61) & 0x03, 0x03);
        assert!(d.speaker_tone().is_some_and(|f| (f - 1000.0).abs() < 1.0));
    }

    #[test]
    fn pwm_plays_amplitudes() {
        let mut d = dos();
        // A 500 Hz sine as 8-bit samples at 16 kHz.
        let s: Vec<u8> = (0..8000)
            .map(|i| {
                (128.0 + 100.0 * (i as f64 * 500.0 / 16_000.0 * std::f64::consts::TAU).sin()) as u8
            })
            .collect();
        let t0 = d.now();
        d.speaker_pwm(&s, 16_000);
        assert!((d.now() - t0 - 0.5).abs() < 0.01);
        let mut out = Vec::new();
        d.speaker_render(44_100, &mut out);
        // Smooth out what is left of the 16 kHz carrier, then count.
        let smooth: Vec<f32> = out
            .windows(8)
            .map(|w| w.iter().sum::<f32>() / 8.0)
            .collect();
        let tail = &smooth[4410..smooth.len() - 100];
        let hz = crossings_hz(tail, 44_100);
        assert!((hz - 500.0).abs() < 15.0, "{hz}");
    }

    #[test]
    fn play_strings() {
        let e = parse_play("T120 L4 O3 A P4 >C8. MS N46"); // QBasic: O3 A = 440 Hz
        assert_eq!(e.len(), 4);
        assert!((e[0].hz.unwrap() - 440.0).abs() < 0.01);
        assert!((e[0].dur - 0.5).abs() < 1e-9);
        assert_eq!(e[1].hz, None);
        assert!((e[2].hz.unwrap() - 523.25).abs() < 0.1);
        assert!((e[2].dur - 0.375).abs() < 1e-9);
        assert!((e[3].on / e[3].dur - 0.75).abs() < 1e-9);
        let m = Melody::new("T240 L8 CDEFGAB>C");
        assert!((m.duration() - 1.0).abs() < 1e-9); // 8 eighths at 240 bpm
    }

    #[test]
    fn melody_drives_the_speaker() {
        let mut d = dos();
        let mut m = Melody::new("T120 L4 O3 A B");
        m.update(&mut d);
        assert!(d.speaker_tone().is_some_and(|f| (f - 440.0).abs() < 1.0));
        d.delay_us(600_000.0);
        m.update(&mut d);
        assert!(d.speaker_tone().is_some_and(|f| (f - 493.9).abs() < 1.0));
        d.delay_us(600_000.0);
        m.update(&mut d);
        assert!(m.is_done());
        assert!(d.speaker_tone().is_none());
    }
}
