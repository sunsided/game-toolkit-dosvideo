//! The PC speaker: a paper cone wired to PIT channel 2 and two bits of port 0x61.
//!
//! - **Tones**: program channel 2 as a square wave (`OUT 43h, B6h`, then the divisor low and
//!   high byte to port 0x42), and set bits 0 (timer gate) and 1 (speaker data) of port 0x61.
//!   The pitch is `1193182 Hz / divisor`. [`Dos::sound`] and [`Dos::nosound`] do that.
//! - **Bit-banging**: with the gate off, bit 1 of port 0x61 *is* the cone position. Toggling it
//!   fast enough, with varying duty cycles, plays (rough) digitized sound: the trick behind
//!   RealSound and countless title screens that froze the game while they talked.
//!   [`Dos::speaker_pwm`] does it.
//! - **Music**: [`Dos::play_speaker`] sequences a [`Melody`] (QBasic `PLAY`) or a prepared
//!   PWM stream ([`game_toolkit_dosaudio::speaker_pwm`]) in the background, tick-exact.
//!
//! The hardware is [game-toolkit-dosaudio](https://docs.rs/game-toolkit-dosaudio)'s speaker:
//! every port access is timestamped on the machine clock and rendered to PCM later by
//! integrating the cone position over each output sample, so ultrasonic carriers average out
//! like they did in the real cone, and pulse widths become amplitudes.

use game_toolkit_dosaudio::{AudioEngine, DosAudio, PreparedMusic, SoundConfig, Tick};

pub use game_toolkit_dosaudio::{Melody, PlayOptions, SpeakerHandle};

use crate::machine::Dos;

/// Output rate of [`Dos::speaker_render`], in Hz.
pub const SPEAKER_RATE: u32 = 44_100;

/// Audio kept when the command queue fills up between two [`Dos::speaker_render`] calls
/// (nobody renders, or a long bit-banged sample): older samples are dropped.
const BACKLOG_MAX: usize = SPEAKER_RATE as usize;

/// The speaker's port half and its synthesis half, both run on the machine clock.
pub(crate) struct PcSpeaker {
    ports: DosAudio,
    engine: AudioEngine,
    /// Rendered early to make room in the command queue, not yet taken.
    backlog: Vec<f32>,
}

impl PcSpeaker {
    pub fn new() -> Self {
        let config = SoundConfig::none()
            .speaker(true)
            .sample_rate(SPEAKER_RATE)
            .latency(Tick::ZERO);
        let (ports, engine) = DosAudio::split(config).expect("PC speaker configuration is valid");
        Self {
            ports,
            engine,
            backlog: Vec::new(),
        }
    }

    /// Ports 0x42, 0x43 (channel 2 commands only) and 0x61.
    pub fn outb(&mut self, at: Tick, port: u16, v: u8) {
        if self.ports.try_outb_at(at, port, v).is_err() {
            // The queue is full: render what it holds, then write again.
            self.make_room(at);
            self.ports
                .try_outb_at(at, port, v)
                .expect("speaker command queue has room after rendering");
        }
    }

    pub fn inb(&mut self, at: Tick, port: u16) -> u8 {
        self.ports
            .try_inb_at(at, port)
            .expect("machine time runs forward")
    }

    /// Render up to `at`, appending mono samples to `out`.
    fn render(&mut self, at: Tick, out: &mut Vec<f32>) {
        let target =
            (u128::from(at.0) * u128::from(SPEAKER_RATE) * u128::from(Tick::HZ_DENOMINATOR)
                / u128::from(Tick::HZ_NUMERATOR)) as u64;
        let mut frames = [[0.0f32; 2]; 512];
        let mut left = target.saturating_sub(self.engine.stats().frames);
        while left > 0 {
            let n = left.min(frames.len() as u64) as usize;
            self.engine.render(&mut frames[..n]);
            // The speaker is mono: both channels carry it.
            out.extend(frames[..n].iter().map(|f| f[0]));
            left -= n as u64;
        }
    }

    fn make_room(&mut self, at: Tick) {
        let mut backlog = std::mem::take(&mut self.backlog);
        self.render(at, &mut backlog);
        let excess = backlog.len().saturating_sub(BACKLOG_MAX);
        backlog.drain(..excess);
        self.backlog = backlog;
    }

    pub fn take(&mut self, at: Tick, out: &mut Vec<f32>) {
        out.append(&mut self.backlog);
        self.render(at, out);
    }

    pub fn skip(&mut self, at: Tick) {
        // The engine still has to run: it consumes the queued port writes.
        self.make_room(at);
        self.backlog.clear();
    }

    pub fn play(
        &mut self,
        at: Tick,
        music: PreparedMusic,
        options: PlayOptions,
    ) -> game_toolkit_dosaudio::Result<SpeakerHandle> {
        // The engine renders behind the machine clock: start at machine time, not its own.
        self.engine.play_speaker_at(music, options, at)
    }

    /// The machine's only 8253 (dosaudio owns it; channel 2 drives the cone).
    pub fn pit(&self) -> &game_toolkit_dosbus::Pit {
        self.ports.pit().expect("the speaker installs the PIT")
    }

    pub fn tone(&self) -> Option<f64> {
        self.ports.speaker_tone()
    }
}

impl Dos {
    /// Play a tone on the PC speaker until [`Dos::nosound`] (Turbo Pascal's `Sound`): program
    /// PIT channel 2 for a square wave at `hz` and connect it to the speaker.
    pub fn sound(&mut self, hz: f64) {
        let d = (crate::PIT_HZ / hz.max(19.0)).round().clamp(1.0, 65535.0) as u16;
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
    /// does not change meanwhile. To play samples in the background instead, prepare them
    /// with [`game_toolkit_dosaudio::speaker_pwm`] and hand them to [`Dos::play_speaker`].
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

    /// Sequence a [`Melody`] (or other prepared speaker music) in the background, starting now
    /// and replacing what [`Dos::play_speaker`] played before. Notes switch at their exact
    /// machine times, independent of the main loop. Port writes ([`Dos::sound`], ...) drive the
    /// same speaker, so pause the handle while the program beeps on its own; dropping the last
    /// clone of the handle stops the music.
    ///
    /// ```
    /// # use game_toolkit_dosvideo::{Dos, MachineConfig, speaker::{Melody, PlayOptions}};
    /// let mut dos = Dos::new(MachineConfig::default());
    /// let tune = dos.play_speaker(Melody::parse("T180 O3 L8 CEG>C")?, PlayOptions::default())?;
    /// dos.delay_us(1_000_000.0);
    /// let mut pcm = Vec::new();
    /// dos.speaker_render(&mut pcm);
    /// assert!(tune.is_finished());
    /// # Ok::<(), game_toolkit_dosvideo::game_toolkit_dosaudio::Error>(())
    /// ```
    pub fn play_speaker(
        &mut self,
        music: impl Into<PreparedMusic>,
        options: PlayOptions,
    ) -> game_toolkit_dosaudio::Result<SpeakerHandle> {
        let at = self.now_tick();
        self.speaker.play(at, music.into(), options)
    }

    /// Render the speaker output from where the last call stopped up to now, at
    /// [`SPEAKER_RATE`], appending mono samples in -1..1 to `out`. The host calls this; call it
    /// yourself to capture sound without the game-toolkit host.
    pub fn speaker_render(&mut self, out: &mut Vec<f32>) {
        let at = self.now_tick();
        self.speaker.take(at, out);
    }

    /// Drop speaker output up to now without rendering it.
    pub fn speaker_skip(&mut self) {
        let at = self.now_tick();
        self.speaker.skip(at);
    }

    /// The tone the ports currently play, in Hz (`None` when the speaker is off or bit-banged).
    /// Music from [`Dos::play_speaker`] does not show here: ask its handle
    /// ([`SpeakerHandle::tone`]).
    pub fn speaker_tone(&self) -> Option<f64> {
        self.speaker.tone()
    }
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
        d.speaker_render(&mut out);
        assert!((out.len() as i64 - 22_050).abs() < 5);
        let tail = &out[4410..];
        assert!((crossings_hz(tail, SPEAKER_RATE) - 440.0).abs() < 5.0);
        assert!(tail.iter().any(|&v| v > 0.1));
        d.nosound();
        d.delay_us(500_000.0);
        let mut quiet = Vec::new();
        d.speaker_render(&mut quiet);
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
        d.nosound();
        assert!(d.speaker_tone().is_none());
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
        d.speaker_render(&mut out);
        // Smooth out what is left of the 16 kHz carrier, then count.
        let smooth: Vec<f32> = out
            .windows(8)
            .map(|w| w.iter().sum::<f32>() / 8.0)
            .collect();
        let tail = &smooth[4410..smooth.len() - 100];
        let hz = crossings_hz(tail, SPEAKER_RATE);
        assert!((hz - 500.0).abs() < 15.0, "{hz}");
    }

    #[test]
    fn long_bit_banging_overflows_into_the_backlog() {
        let mut d = dos();
        // 3 s at 16 kHz: two port writes per sample, far beyond the command queue.
        d.speaker_pwm(&vec![128; 48_000], 16_000);
        let mut out = Vec::new();
        d.speaker_render(&mut out);
        assert!(out.len() >= BACKLOG_MAX, "{}", out.len());
    }

    #[test]
    fn melody_plays_in_the_background() {
        let mut d = dos();
        // O3 A is 440 Hz; T120 L4 makes each note half a second.
        let tune = d
            .play_speaker(
                Melody::parse("T120 L4 ML O3 A").unwrap(),
                PlayOptions::default(),
            )
            .unwrap();
        d.delay_us(400_000.0);
        let mut out = Vec::new();
        d.speaker_render(&mut out);
        let hz = crossings_hz(&out[4410..], SPEAKER_RATE);
        assert!((hz - 440.0).abs() < 5.0, "{hz}");
        assert!(d.speaker_tone().is_none(), "music does not touch the ports");
        assert!(tune.tone().is_some_and(|f| (f - 440.0).abs() < 1.0));
        d.delay_us(200_000.0);
        d.speaker_render(&mut out);
        assert!(tune.is_finished());
    }
}
