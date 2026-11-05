//! game-toolkit integration: run a [`DosGame`] in a window, or put a [`Dos`] screen into your
//! own [`Game`] with [`DosScreen`].

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use game_toolkit::prelude::*;

use crate::crt_filter::{CrtFilter, CrtPass};
use crate::input::sc;
use crate::machine::{Adapter, BusTiming, Dos, MachineConfig};

/// How the emulated picture is fitted into the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Aspect {
    /// Like a CRT: the active display area is 4:3, whatever its resolution. Mode 13h's
    /// 320x200 pixels come out taller than wide, as the artists drew them.
    #[default]
    Crt,
    /// Every pixel of the current mode is square (mode 13h looks 16:10).
    SquarePixels,
    /// Fill the window.
    Stretch,
}

/// Configuration for [`run`].
#[derive(Clone, Debug)]
pub struct DosConfig {
    pub title: String,
    pub machine: MachineConfig,
    /// Initial window size in logical pixels.
    pub window: (u32, u32),
    pub aspect: Aspect,
    /// Show the overscan border around the active display.
    pub show_border: bool,
    /// Scale by whole multiples vertically when the window is big enough.
    pub integer_scale: bool,
    /// CRT post-filter (`None`: sharp pixels). Alt+C cycles the presets at run time.
    pub crt: Option<CrtFilter>,
    /// Upper bound on [`DosGame::step`] calls per host frame.
    pub max_steps_per_frame: u32,
    /// The program's keys as `(key, what it does)`, printed to stdout by [`run`] together
    /// with the host keys, so people know what to press.
    pub keys: Vec<(String, String)>,
}

impl DosConfig {
    pub fn new(title: impl Into<String>, adapter: Adapter) -> Self {
        Self {
            title: title.into(),
            machine: MachineConfig::new(adapter),
            window: (1024, 768),
            aspect: Aspect::Crt,
            show_border: true,
            integer_scale: false,
            crt: None,
            max_steps_per_frame: 2000,
            keys: Vec::new(),
        }
    }

    pub fn with_machine(mut self, m: MachineConfig) -> Self {
        self.machine = m;
        self
    }

    pub fn with_bus(mut self, bus: BusTiming) -> Self {
        self.machine.bus = bus;
        self
    }

    /// Document the program's keys; [`run`] prints them at startup.
    pub fn with_keys(mut self, keys: &[(&str, &str)]) -> Self {
        self.keys = keys
            .iter()
            .map(|(k, d)| ((*k).to_string(), (*d).to_string()))
            .collect();
        self
    }

    pub fn with_window(mut self, w: u32, h: u32) -> Self {
        self.window = (w, h);
        self
    }

    pub fn with_aspect(mut self, a: Aspect) -> Self {
        self.aspect = a;
        self
    }

    pub fn with_border(mut self, on: bool) -> Self {
        self.show_border = on;
        self
    }

    /// Scanlines only: the [`CrtFilter::crisp`] preset at the given strength (0..1).
    pub fn with_scanlines(mut self, strength: f32) -> Self {
        self.crt = Some(CrtFilter {
            scanlines: strength.clamp(0.0, 1.0),
            ..CrtFilter::crisp()
        });
        self
    }

    /// A CRT post-filter, e.g. [`CrtFilter::vga_monitor`] or [`CrtFilter::tv`].
    pub fn with_crt(mut self, filter: CrtFilter) -> Self {
        self.crt = Some(filter);
        self
    }
}

/// A DOS program.
///
/// `step` is one iteration of your main loop. Write it like a DOS loop: wait for the retrace
/// (or don't), draw, read the keyboard. The host calls it as often as emulated time allows: a
/// loop that waits for vertical retrace once runs at the mode's refresh rate (70 Hz in mode
/// 13h), one that waits for nothing runs as fast as its bus and CPU costs allow.
pub trait DosGame: Sized + 'static {
    fn init(dos: &mut Dos) -> Result<Self>;
    fn step(&mut self, dos: &mut Dos);
    /// Draw host-side extras over the picture (debug text, help). `screen` is the rectangle
    /// the emulated display occupies in the window, `[x, y, w, h]`.
    fn overlay(&mut self, _dos: &Dos, _p: &mut Painter<'_>, _screen: [f32; 4]) {}
}

static PENDING: Mutex<Option<DosConfig>> = Mutex::new(None);

/// Open a window and run `G` on an emulated PC.
///
/// Headless screenshots (native only): with `DOSVIDEO_SHOT=out.png` set, no window opens; the
/// program runs for `DOSVIDEO_SHOT_FRAMES` frames (default 120) and the last finished frame is
/// written as a PNG. `DOSVIDEO_KEYS=frame:scancode,...` taps keys (scancodes in hex or
/// decimal) at the given frames, to drive a demo into a particular state.
pub fn run<G: DosGame>(config: DosConfig) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(path) = std::env::var("DOSVIDEO_SHOT") {
        return headless::<G>(config, &path);
    }
    print_keys(&config);
    let app = AppConfig {
        title: config.title.clone(),
        width: config.window.0,
        height: config.window.1,
        ..Default::default()
    };
    *PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(config);
    game_toolkit::prelude::run::<Runner<G>>(app)
}

/// The key table printed at startup.
fn print_keys(config: &DosConfig) {
    let host = [
        ("Alt+Enter", "fullscreen"),
        (
            "Alt+C",
            "CRT filter: off, crisp, VGA monitor, TV, green screen, amber screen",
        ),
    ];
    let width = config
        .keys
        .iter()
        .map(|(k, _)| k.chars().count())
        .chain(host.iter().map(|(k, _)| k.len()))
        .max()
        .unwrap_or(0);
    println!("{}", config.title);
    for (k, d) in &config.keys {
        println!("  {k:<width$}  {d}");
    }
    println!("host:");
    for (k, d) in host {
        println!("  {k:<width$}  {d}");
    }
}

/// `DOSVIDEO_KEYS=frame:scancode,...` (scancodes in hex with `0x` or decimal).
fn scripted_keys() -> Vec<(u64, u8)> {
    let parse = |s: &str| {
        s.strip_prefix("0x")
            .map_or_else(|| s.parse().ok(), |h| u8::from_str_radix(h, 16).ok())
    };
    std::env::var("DOSVIDEO_KEYS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|e| {
            let (f, k) = e.split_once(':')?;
            Some((f.trim().parse().ok()?, parse(k.trim())?))
        })
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn headless<G: DosGame>(config: DosConfig, path: &str) -> Result<()> {
    let frames: u64 = std::env::var("DOSVIDEO_SHOT_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    let keys = scripted_keys();
    let mut dos = Dos::new(config.machine);
    let mut game = G::init(&mut dos)?;
    let start = dos.frames();
    let mut held: Vec<u8> = Vec::new();
    let mut last_frame = u64::MAX;
    let mut fired = vec![false; keys.len()];
    let mut guard = 0u64;
    // DOSVIDEO_WAV=out.wav: also record the PC speaker.
    let wav_path = std::env::var("DOSVIDEO_WAV").ok();
    let mut audio: Vec<f32> = Vec::new();
    while dos.frames() - start < frames && !dos.exit_requested() {
        let f = dos.frames() - start;
        if f != last_frame {
            for k in held.drain(..) {
                dos.keyboard_mut().release(k);
            }
            // Fire every key that is due, even if a long step skipped its exact frame.
            for (i, &(kf, k)) in keys.iter().enumerate() {
                if kf <= f && !fired[i] {
                    fired[i] = true;
                    let ascii = crate::input::Keyboard::ascii_for(k, false);
                    dos.keyboard_mut().press(k, ascii);
                    held.push(k);
                }
            }
            last_frame = f;
        }
        let before = dos.now_ns();
        game.step(&mut dos);
        if dos.now_ns() <= before {
            let t = dos.now_ns() + dos.timing().frame_ns();
            dos.idle_until(t);
        }
        guard += 1;
        anyhow::ensure!(guard < 50_000_000, "program makes no progress");
        if wav_path.is_some() {
            dos.speaker_render(AUDIO_RATE, &mut audio);
        } else {
            dos.speaker_skip();
        }
    }
    dos.sync();
    std::fs::write(path, dos.frame().to_png())?;
    if let Some(p) = wav_path {
        std::fs::write(p, wav16(&audio, AUDIO_RATE))?;
    }
    Ok(())
}

/// Open an audio stream for the PC speaker (deferred until audio may start, on the web).
fn start_speaker(ctx: &mut Context) -> Option<(SpeakerQueue, StreamHandle)> {
    let audio = ctx.audio.as_mut()?;
    let q = SpeakerQueue::default();
    q.push(&vec![0.0; SpeakerQueue::PRIME]);
    let feed = q.clone();
    let opts = PlayOptions {
        deferred: true,
        ..Default::default()
    };
    match audio.play_stream_with(move |out: &mut [f32]| feed.fill(out), AUDIO_RATE, opts) {
        Ok(h) => Some((q, h)),
        Err(e) => {
            eprintln!("PC speaker: no audio ({e})");
            None
        }
    }
}

/// Output rate of the PC speaker.
const AUDIO_RATE: u32 = 44_100;

/// Mono 16-bit PCM WAV.
#[cfg(not(target_arch = "wasm32"))]
fn wav16(samples: &[f32], rate: u32) -> Vec<u8> {
    let data = samples.len() as u32 * 2;
    let mut w = Vec::with_capacity(44 + data as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    w
}

/// PC speaker samples handed from the emulation (rendered up to the machine clock each host
/// frame) to the audio thread.
#[derive(Clone, Default)]
struct SpeakerQueue(Arc<Mutex<VecDeque<f32>>>);

impl SpeakerQueue {
    /// Keep at most this much queued: emulated and real time drift a little, and a backlog
    /// would only add latency.
    const MAX: usize = AUDIO_RATE as usize / 6;
    /// Prime the queue with silence so the first host frames don't underrun.
    const PRIME: usize = AUDIO_RATE as usize / 20;

    fn push(&self, samples: &[f32]) {
        let mut q = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        q.extend(samples);
        let excess = q.len().saturating_sub(Self::MAX);
        q.drain(..excess);
    }

    fn fill(&self, out: &mut [f32]) {
        let mut q = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for o in out {
            *o = q.pop_front().unwrap_or(0.0);
        }
    }
}

struct Runner<G: DosGame> {
    dos: Dos,
    game: G,
    screen: DosScreen,
    deadline: f64,
    max_steps: u32,
    host_frames: u64,
    window_shot: Option<(String, u64)>,
    /// Scripted key taps by host frame (`DOSVIDEO_KEYS`), for window screenshots.
    keys: Vec<(u64, u8)>,
    held: Vec<u8>,
    /// Where the speaker goes, if audio is available.
    audio: Option<(SpeakerQueue, StreamHandle)>,
    audio_buf: Vec<f32>,
}

impl<G: DosGame> Game for Runner<G> {
    fn init(ctx: &mut Context) -> Result<Self> {
        let config = PENDING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .unwrap_or_else(|| DosConfig::new("DOS", Adapter::Vga));
        let mut dos = Dos::new(config.machine);
        let game = G::init(&mut dos)?;
        let mut screen = DosScreen::new();
        screen.aspect = config.aspect;
        screen.show_border = config.show_border;
        screen.integer_scale = config.integer_scale;
        screen.crt = config.crt;
        // DOSVIDEO_CRT=0..6 picks a preset (0 = off), for trying them without code changes.
        if let Some(i) = std::env::var("DOSVIDEO_CRT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        {
            screen.set_crt_preset(i);
        }
        screen.upload(&mut ctx.gfx, &dos);
        // DOSVIDEO_WINDOW_SHOT=out.png [DOSVIDEO_WINDOW_SHOT_FRAMES=N]: capture the window
        // (with the GPU filter) after N host frames, then quit.
        let window_shot = std::env::var("DOSVIDEO_WINDOW_SHOT").ok().map(|p| {
            let n = std::env::var("DOSVIDEO_WINDOW_SHOT_FRAMES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(120);
            (p, n)
        });
        Ok(Self {
            deadline: dos.now_ns(),
            dos,
            game,
            screen,
            max_steps: config.max_steps_per_frame.max(1),
            host_frames: 0,
            keys: scripted_keys(),
            held: Vec::new(),
            audio: start_speaker(ctx),
            audio_buf: Vec::new(),
            window_shot,
        })
    }

    fn update(&mut self, ctx: &mut Context, dt: f32) {
        for k in self.held.drain(..) {
            self.dos.keyboard_mut().release(k);
        }
        let f = self.host_frames;
        for &(_, k) in self.keys.iter().filter(|(kf, _)| *kf == f) {
            let ascii = crate::input::Keyboard::ascii_for(k, false);
            self.dos.keyboard_mut().press(k, ascii);
            self.held.push(k);
        }
        let alt = ctx.input.key_held(Key::AltLeft) || ctx.input.key_held(Key::AltRight);
        if alt && ctx.input.key_pressed(Key::KeyC) {
            self.screen.cycle_crt();
        } else if alt && ctx.input.key_pressed(Key::Enter) {
            let fs = ctx.window.fullscreen().is_some();
            ctx.window
                .set_fullscreen((!fs).then_some(winit::window::Fullscreen::Borderless(None)));
        } else {
            self.screen
                .feed_input(&ctx.input, &mut self.dos, ctx.gfx.size());
        }

        self.deadline += dt.clamp(0.0, 0.1) as f64 * 1e9;
        let mut steps = 0;
        while self.dos.now_ns() < self.deadline && steps < self.max_steps {
            let before = self.dos.now_ns();
            self.game.step(&mut self.dos);
            steps += 1;
            if self.dos.exit_requested() {
                ctx.quit();
                return;
            }
            if self.dos.now_ns() <= before {
                // The step spent no emulated time: treat it as running flat out until the
                // next host frame.
                self.dos.idle_until(self.deadline);
                break;
            }
        }
        // Hand the speaker output of this frame to the audio thread.
        if let Some((q, _)) = &self.audio {
            self.audio_buf.clear();
            self.dos.speaker_render(AUDIO_RATE, &mut self.audio_buf);
            q.push(&self.audio_buf);
        } else {
            self.dos.speaker_skip();
        }
        // Emulation can't keep up (or a step took very long): don't try to catch up later.
        if self.deadline - self.dos.now_ns() > 0.25e9 {
            self.deadline = self.dos.now_ns();
        }
        if self.dos.now_ns() - self.deadline > 0.5e9 {
            self.deadline = self.dos.now_ns();
        }
    }

    fn render(&mut self, ctx: &mut Context, frame: &mut Frame) {
        self.dos.sync();
        self.screen.upload(&mut ctx.gfx, &self.dos);
        let size = ctx.gfx.size();
        {
            let mut p = frame.painter(&mut ctx.gfx);
            p.clear([0.0, 0.0, 0.0, 1.0]);
            let rect = self.screen.draw(&mut p, &self.dos, size);
            self.game.overlay(&self.dos, &mut p, rect);
        }
        // The CRT pass goes on top of the flushed painter output.
        self.screen.draw_crt(&ctx.gfx, frame);

        self.host_frames += 1;
        if let Some((path, n)) = &self.window_shot {
            if self.host_frames == *n {
                ctx.gfx.request_screenshot(path.clone());
            } else if self.host_frames > *n + 2 {
                ctx.quit();
            }
        }
    }
}

/// Shows a [`Dos`] picture through game-toolkit: uploads finished frames into a texture, fits
/// them into a rectangle with the right aspect ratio, and feeds the keyboard and mouse.
pub struct DosScreen {
    tex: Option<(TextureId, u32, u32)>,
    uploaded: u64,
    pub aspect: Aspect,
    pub show_border: bool,
    pub integer_scale: bool,
    /// CRT post-filter; `None` shows sharp pixels. Drawn by [`DosScreen::draw_crt`].
    pub crt: Option<CrtFilter>,
    crt_index: usize,
    crt_pass: Option<CrtPass>,
    crt_uploaded: u64,
    layout: ([f32; 4], [f32; 4]),
    /// Where the active display area was drawn last, `[x, y, w, h]`.
    pub active_rect: [f32; 4],
}

impl Default for DosScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl DosScreen {
    pub fn new() -> Self {
        Self {
            tex: None,
            uploaded: u64::MAX,
            aspect: Aspect::Crt,
            show_border: true,
            integer_scale: false,
            crt: None,
            crt_index: 0,
            crt_pass: None,
            crt_uploaded: u64::MAX,
            layout: ([0.0; 4], [0.0; 4]),
            active_rect: [0.0; 4],
        }
    }

    /// Select a CRT preset: 0 = off, 1.. = [`CrtFilter::presets`].
    pub fn set_crt_preset(&mut self, i: usize) {
        let presets = CrtFilter::presets();
        self.crt_index = i % (presets.len() + 1);
        self.crt = self.crt_index.checked_sub(1).map(|k| presets[k].1);
    }

    /// Next CRT preset (off → presets → off).
    pub fn cycle_crt(&mut self) {
        self.set_crt_preset(self.crt_index + 1);
    }

    /// Name of the current CRT preset.
    pub fn crt_name(&self) -> &'static str {
        match self.crt_index.checked_sub(1) {
            None if self.crt.is_some() => "custom",
            None => "off",
            Some(k) => CrtFilter::presets()[k].0,
        }
    }

    /// Upload the last finished frame if it is new.
    pub fn upload(&mut self, gfx: &mut Graphics, dos: &Dos) {
        let f = dos.frame();
        if self.crt.is_some() {
            let pass = self.crt_pass.get_or_insert_with(|| CrtPass::new(gfx));
            if self.crt_uploaded != f.number {
                pass.upload(gfx, f.width, f.height, f.rgba);
                self.crt_uploaded = f.number;
            }
            return;
        }
        self.crt_uploaded = u64::MAX;
        match self.tex {
            Some((id, w, h)) if w == f.width && h == f.height => {
                if self.uploaded != f.number {
                    gfx.update_texture_rgba(id, f.rgba);
                }
            }
            _ => {
                let id = gfx.create_texture_rgba(f.width, f.height, f.rgba, Some("dos screen"));
                self.tex = Some((id, f.width, f.height));
            }
        }
        self.uploaded = f.number;
    }

    /// The texture holding the picture (visible area including the border).
    pub fn texture(&self) -> Option<TextureId> {
        self.tex.map(|t| t.0)
    }

    /// Compute where the picture goes in a `window`-sized target: `(image rect, source
    /// region [x, y, w, h] in dots)`.
    pub fn layout(&self, dos: &Dos, window: (u32, u32)) -> ([f32; 4], [f32; 4]) {
        let f = dos.frame();
        let [ax, ay, aw, ah] = f.active.map(|v| v as f32);
        let src = if self.show_border {
            [0.0, 0.0, f.width as f32, f.height as f32]
        } else {
            [ax, ay, aw, ah]
        };
        let (ww, wh) = (window.0 as f32, window.1 as f32);
        // Width of one dot in units of one scanline's height.
        let dot_w = match self.aspect {
            Aspect::Crt | Aspect::Stretch => (4.0 / 3.0) * ah / aw,
            Aspect::SquarePixels => {
                let (lw, lh) = dos.resolution();
                let dots_per_px = aw / lw.max(1) as f32;
                let lines_per_px = ah / lh.max(1) as f32;
                lines_per_px / dots_per_px
            }
        };
        let (dw, dh) = (src[2] * dot_w, src[3]);
        let (w, h) = if self.aspect == Aspect::Stretch {
            (ww, wh)
        } else {
            let mut s = (ww / dw).min(wh / dh);
            if self.integer_scale && s >= 1.0 {
                s = s.floor();
            }
            (dw * s, dh * s)
        };
        (
            [((ww - w) / 2.0).floor(), ((wh - h) / 2.0).floor(), w, h],
            src,
        )
    }

    /// Draw the picture centered into a `window`-sized target. Returns the rectangle of the
    /// active display area.
    pub fn draw(&mut self, p: &mut Painter<'_>, dos: &Dos, window: (u32, u32)) -> [f32; 4] {
        let (rect, src) = self.layout(dos, window);
        self.layout = (rect, src);
        if let (None, Some((tex, _, _))) = (&self.crt, self.tex) {
            p.sprite_region(
                tex,
                [src[0], src[1]],
                [src[2], src[3]],
                [rect[0], rect[1]],
                [rect[2], rect[3]],
            );
        }
        let sy = rect[3] / src[3];
        let f = dos.frame();
        let sx = rect[2] / src[2];
        let active = [
            rect[0] + (f.active[0] as f32 - src[0]) * sx,
            rect[1] + (f.active[1] as f32 - src[1]) * sy,
            f.active[2] as f32 * sx,
            f.active[3] as f32 * sy,
        ];
        self.active_rect = active;
        active
    }

    /// Draw the picture through the CRT filter, if one is set. Call after the painter for the
    /// frame was dropped (the filter is its own render pass on top); [`DosScreen::draw`] must
    /// have run this frame to lay the picture out. Anything painted inside the picture's
    /// rectangle is covered.
    pub fn draw_crt(&self, gfx: &Graphics, frame: &mut Frame) {
        if let (Some(filter), Some(pass)) = (&self.crt, &self.crt_pass) {
            let (rect, src) = self.layout;
            pass.draw(gfx, frame, filter, rect, src);
        }
    }

    /// Forward host keyboard and mouse state: key presses and releases become scancodes in
    /// the keyboard table and the BIOS buffer; the mouse position over the active display
    /// becomes `INT 33h` coordinates.
    pub fn feed_input(&self, input: &Input, dos: &mut Dos, window: (u32, u32)) {
        let shift = input.key_held(Key::ShiftLeft) || input.key_held(Key::ShiftRight);
        for &(key, scan) in KEYMAP {
            if input.key_pressed(key) {
                let ascii = crate::input::Keyboard::ascii_for(scan, shift);
                dos.keyboard_mut().press(scan, ascii);
            }
            if input.key_released(key) {
                dos.keyboard_mut().release(scan);
            }
        }
        let _ = window;
        let [x, y, w, h] = self.active_rect;
        if w > 0.0 && h > 0.0 {
            let (mx, my) = input.mouse_pos();
            dos.mouse_mut().set_normalized((mx - x) / w, (my - y) / h);
        }
        let mut b = 0;
        if input.mouse_held(MouseButton::Left) {
            b |= 1;
        }
        if input.mouse_held(MouseButton::Right) {
            b |= 2;
        }
        if input.mouse_held(MouseButton::Middle) {
            b |= 4;
        }
        dos.mouse_mut().set_buttons(b);
    }
}

/// Host key to XT scancode.
pub(crate) const KEYMAP: &[(Key, u8)] = &[
    (Key::Escape, sc::ESC),
    (Key::Digit1, sc::N1),
    (Key::Digit2, sc::N2),
    (Key::Digit3, sc::N3),
    (Key::Digit4, sc::N4),
    (Key::Digit5, sc::N5),
    (Key::Digit6, sc::N6),
    (Key::Digit7, sc::N7),
    (Key::Digit8, sc::N8),
    (Key::Digit9, sc::N9),
    (Key::Digit0, sc::N0),
    (Key::Minus, sc::MINUS),
    (Key::Equal, sc::EQUALS),
    (Key::Backspace, sc::BACKSPACE),
    (Key::Tab, sc::TAB),
    (Key::KeyQ, sc::Q),
    (Key::KeyW, sc::W),
    (Key::KeyE, sc::E),
    (Key::KeyR, sc::R),
    (Key::KeyT, sc::T),
    (Key::KeyY, sc::Y),
    (Key::KeyU, sc::U),
    (Key::KeyI, sc::I),
    (Key::KeyO, sc::O),
    (Key::KeyP, sc::P),
    (Key::BracketLeft, sc::LBRACKET),
    (Key::BracketRight, sc::RBRACKET),
    (Key::Enter, sc::ENTER),
    (Key::NumpadEnter, sc::ENTER),
    (Key::ControlLeft, sc::CTRL),
    (Key::ControlRight, sc::CTRL),
    (Key::KeyA, sc::A),
    (Key::KeyS, sc::S),
    (Key::KeyD, sc::D),
    (Key::KeyF, sc::F),
    (Key::KeyG, sc::G),
    (Key::KeyH, sc::H),
    (Key::KeyJ, sc::J),
    (Key::KeyK, sc::K),
    (Key::KeyL, sc::L),
    (Key::Semicolon, sc::SEMICOLON),
    (Key::Quote, sc::APOSTROPHE),
    (Key::Backquote, sc::BACKTICK),
    (Key::ShiftLeft, sc::LSHIFT),
    (Key::Backslash, sc::BACKSLASH),
    (Key::KeyZ, sc::Z),
    (Key::KeyX, sc::X),
    (Key::KeyC, sc::C),
    (Key::KeyV, sc::V),
    (Key::KeyB, sc::B),
    (Key::KeyN, sc::N),
    (Key::KeyM, sc::M),
    (Key::Comma, sc::COMMA),
    (Key::Period, sc::PERIOD),
    (Key::Slash, sc::SLASH),
    (Key::ShiftRight, sc::RSHIFT),
    (Key::NumpadMultiply, sc::KP_STAR),
    (Key::AltLeft, sc::ALT),
    (Key::AltRight, sc::ALT),
    (Key::Space, sc::SPACE),
    (Key::CapsLock, sc::CAPSLOCK),
    (Key::F1, sc::F1),
    (Key::F2, sc::F2),
    (Key::F3, sc::F3),
    (Key::F4, sc::F4),
    (Key::F5, sc::F5),
    (Key::F6, sc::F6),
    (Key::F7, sc::F7),
    (Key::F8, sc::F8),
    (Key::F9, sc::F9),
    (Key::F10, sc::F10),
    (Key::NumLock, sc::NUMLOCK),
    (Key::ScrollLock, sc::SCROLLLOCK),
    (Key::Home, sc::HOME),
    (Key::Numpad7, sc::HOME),
    (Key::ArrowUp, sc::UP),
    (Key::Numpad8, sc::UP),
    (Key::PageUp, sc::PGUP),
    (Key::Numpad9, sc::PGUP),
    (Key::NumpadSubtract, sc::KP_MINUS),
    (Key::ArrowLeft, sc::LEFT),
    (Key::Numpad4, sc::LEFT),
    (Key::Numpad5, sc::KP5),
    (Key::ArrowRight, sc::RIGHT),
    (Key::Numpad6, sc::RIGHT),
    (Key::NumpadAdd, sc::KP_PLUS),
    (Key::End, sc::END),
    (Key::Numpad1, sc::END),
    (Key::ArrowDown, sc::DOWN),
    (Key::Numpad2, sc::DOWN),
    (Key::PageDown, sc::PGDN),
    (Key::Numpad3, sc::PGDN),
    (Key::Insert, sc::INSERT),
    (Key::Numpad0, sc::INSERT),
    (Key::Delete, sc::DELETE),
    (Key::NumpadDecimal, sc::DELETE),
    (Key::F11, sc::F11),
    (Key::F12, sc::F12),
];
