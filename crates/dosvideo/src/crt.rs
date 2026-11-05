//! The CRT: raster timing, the electron beam, and the "phosphor" image it paints.
//!
//! Scanout is *lazy*: the beam position is a pure function of the machine clock, and the image
//! is only painted when something observable happens (a VRAM write, a register write, the host
//! asking for a frame). [`Crt::advance`] then paints every dot between the last painted position
//! and the current beam position using the card's *current* state. That is exactly what makes
//! racing the beam work: a palette write half-way through a line changes the rest of that line
//! and nothing before it, and a VRAM write just behind the beam only shows up next frame.

/// Raster timing of the current video mode, in dots (pixel clock ticks) and scanlines.
///
/// Coordinates are relative to the first active dot of the first active line. A line runs
/// `0..h_total`: active display `0..h_active`, right border up to `h_blank_start`, blanking
/// (with horizontal retrace inside it) up to `h_blank_end`, and the *next* line's left border up
/// to `h_total`. Vertically the same: active `0..v_active`, bottom border up to
/// `v_blank_start`, blanking (with vertical retrace) up to `v_blank_end`, then the top border of
/// the next frame up to `v_total`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing {
    /// Dot clock in Hz (25.175 MHz for most VGA modes, 14.318 MHz for CGA, ...).
    pub dot_clock_hz: f64,
    pub h_total: u32,
    pub h_active: u32,
    pub h_blank_start: u32,
    pub h_blank_end: u32,
    pub h_retrace_start: u32,
    pub h_retrace_end: u32,
    pub v_total: u32,
    pub v_active: u32,
    pub v_blank_start: u32,
    pub v_blank_end: u32,
    pub v_retrace_start: u32,
    pub v_retrace_end: u32,
}

impl Timing {
    /// Clamp the values into a consistent order (registers can be programmed with nonsense).
    pub fn normalized(mut self) -> Self {
        self.h_total = self.h_total.max(8);
        self.h_active = self.h_active.clamp(1, self.h_total);
        self.h_blank_start = self.h_blank_start.clamp(self.h_active, self.h_total);
        self.h_blank_end = self.h_blank_end.clamp(self.h_blank_start, self.h_total);
        self.h_retrace_start = self.h_retrace_start.min(self.h_total);
        self.h_retrace_end = self.h_retrace_end.clamp(self.h_retrace_start, self.h_total);
        self.v_total = self.v_total.max(2);
        self.v_active = self.v_active.clamp(1, self.v_total);
        self.v_blank_start = self.v_blank_start.clamp(self.v_active, self.v_total);
        self.v_blank_end = self.v_blank_end.clamp(self.v_blank_start, self.v_total);
        self.v_retrace_start = self.v_retrace_start.min(self.v_total);
        self.v_retrace_end = self.v_retrace_end.clamp(self.v_retrace_start, self.v_total);
        if self.dot_clock_hz.is_nan() || self.dot_clock_hz <= 1.0 {
            self.dot_clock_hz = 1.0;
        }
        self
    }

    /// Horizontal scan rate in Hz.
    pub fn line_rate_hz(&self) -> f64 {
        self.dot_clock_hz / self.h_total as f64
    }

    /// Vertical refresh rate in Hz.
    pub fn refresh_hz(&self) -> f64 {
        self.line_rate_hz() / self.v_total as f64
    }

    /// Duration of one scanline in nanoseconds.
    pub fn line_ns(&self) -> f64 {
        1e9 / self.line_rate_hz()
    }

    /// Duration of one frame in nanoseconds.
    pub fn frame_ns(&self) -> f64 {
        1e9 / self.refresh_hz()
    }

    pub(crate) fn frame_dots(&self) -> u64 {
        self.h_total as u64 * self.v_total as u64
    }

    fn left_border(&self) -> u32 {
        self.h_total - self.h_blank_end
    }

    fn top_border(&self) -> u32 {
        self.v_total - self.v_blank_end
    }

    /// Size of the visible picture (active area plus overscan border) in dots x lines.
    pub fn visible_size(&self) -> (u32, u32) {
        (
            self.left_border() + self.h_blank_start,
            self.top_border() + self.v_blank_start,
        )
    }
}

/// Where the beam is at some instant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Beam {
    /// Frames started since the mode was set.
    pub frame: u64,
    /// Scanline, `0` = first active line.
    pub line: u32,
    /// Dot within the line, `0` = first active dot.
    pub dot: u32,
    /// Inside the active display area (not border, not blanking).
    pub display_active: bool,
    pub h_retrace: bool,
    pub v_retrace: bool,
    pub h_blank: bool,
    pub v_blank: bool,
}

/// The hooks a video card implements so the CRT can paint it.
pub(crate) trait Scanout {
    /// Called at dot 0 of line 0, before [`Scanout::line_start`] for that line.
    fn frame_start(&mut self) {}
    /// Called at dot 0 of every line (`0..v_total`).
    fn line_start(&mut self, _line: u32) {}
    /// Called at the start of the line on which vertical retrace begins.
    fn vretrace_start(&mut self) {}
    /// Paint active-display dots `x0 .. x0 + out.len()` of `line`.
    fn render_active(&mut self, line: u32, x0: u32, out: &mut [[u8; 4]]);
    /// Current overscan (border) color.
    fn border(&self) -> [u8; 4];
}

/// One finished frame, as RGBA8 in visible-area dots.
#[derive(Clone, Copy, Debug)]
pub struct FrameView<'a> {
    /// Width of the visible area (active + border) in dots.
    pub width: u32,
    pub height: u32,
    /// RGBA8 rows, `width * 4` bytes each.
    pub rgba: &'a [u8],
    /// Active display rectangle inside the image: `[x, y, w, h]`.
    pub active: [u32; 4],
    /// Number of the frame (counts completed frames since power-on).
    pub number: u64,
}

pub(crate) struct Crt {
    timing: Timing,
    dots_per_ns: f64,
    /// Machine time (ns) of global dot 0.
    epoch_ns: f64,
    /// Next global dot to paint.
    painted: u64,
    back: Vec<[u8; 4]>,
    front: Vec<[u8; 4]>,
    width: u32,
    height: u32,
    frames: u64,
}

impl Crt {
    pub fn new(timing: Timing, now_ns: f64) -> Self {
        let timing = timing.normalized();
        let (width, height) = timing.visible_size();
        let n = (width * height) as usize;
        Self {
            dots_per_ns: timing.dot_clock_hz / 1e9,
            timing,
            epoch_ns: now_ns,
            painted: 0,
            back: vec![[0, 0, 0, 255]; n],
            front: vec![[0, 0, 0, 255]; n],
            width,
            height,
            frames: 0,
        }
    }

    pub fn timing(&self) -> &Timing {
        &self.timing
    }

    /// Global dot index at machine time `now_ns`.
    pub fn dot_at(&self, now_ns: f64) -> u64 {
        let d = (now_ns - self.epoch_ns) * self.dots_per_ns;
        if d <= 0.0 { 0 } else { d as u64 }
    }

    /// Machine time of global dot `dot`.
    pub fn time_of(&self, dot: u64) -> f64 {
        self.epoch_ns + dot as f64 / self.dots_per_ns
    }

    pub fn beam_at(&self, now_ns: f64) -> Beam {
        self.beam_of(self.dot_at(now_ns))
    }

    pub fn beam_of(&self, g: u64) -> Beam {
        let t = &self.timing;
        let fd = t.frame_dots();
        let in_frame = g % fd;
        let line = (in_frame / t.h_total as u64) as u32;
        let dot = (in_frame % t.h_total as u64) as u32;
        Beam {
            frame: g / fd,
            line,
            dot,
            display_active: line < t.v_active && dot < t.h_active,
            h_retrace: dot >= t.h_retrace_start && dot < t.h_retrace_end,
            v_retrace: line >= t.v_retrace_start && line < t.v_retrace_end,
            h_blank: dot >= t.h_blank_start && dot < t.h_blank_end,
            v_blank: line >= t.v_blank_start && line < t.v_blank_end,
        }
    }

    /// Global dot of the next time the beam reaches `(line, dot)` at or after global dot `from`.
    pub fn next_dot(&self, from: u64, line: u32, dot: u32) -> u64 {
        let t = &self.timing;
        let fd = t.frame_dots();
        let target =
            line.min(t.v_total - 1) as u64 * t.h_total as u64 + dot.min(t.h_total - 1) as u64;
        let base = from - from % fd;
        let cand = base + target;
        if cand >= from { cand } else { cand + fd }
    }

    /// Switch to new timing without losing the beam position (clamped into the new raster).
    pub fn retime<C: Scanout>(&mut self, card: &mut C, timing: Timing, now_ns: f64) {
        let timing = timing.normalized();
        if timing == self.timing {
            return;
        }
        self.advance(card, self.dot_at(now_ns));
        let b = self.beam_at(now_ns);
        let line = b.line.min(timing.v_total - 1);
        let dot = b.dot.min(timing.h_total - 1);
        self.timing = timing;
        self.dots_per_ns = timing.dot_clock_hz / 1e9;
        let g = line as u64 * timing.h_total as u64 + dot as u64;
        self.epoch_ns = now_ns - g as f64 / self.dots_per_ns;
        self.painted = g;
        let (w, h) = timing.visible_size();
        if (w, h) != (self.width, self.height) {
            self.width = w;
            self.height = h;
            let n = (w * h) as usize;
            self.back = vec![[0, 0, 0, 255]; n];
            self.front = vec![[0, 0, 0, 255]; n];
        }
    }

    /// Restart the raster at line 0 (mode set).
    pub fn reset<C: Scanout>(&mut self, card: &mut C, timing: Timing, now_ns: f64) {
        let frames = self.frames;
        *self = Crt::new(timing, now_ns);
        self.frames = frames;
        let _ = card;
    }

    pub fn frame(&self) -> FrameView<'_> {
        let t = &self.timing;
        FrameView {
            width: self.width,
            height: self.height,
            rgba: self.front.as_flattened(),
            active: [t.left_border(), t.top_border(), t.h_active, t.v_active],
            number: self.frames,
        }
    }

    /// The image as painted so far, including the half-finished current frame.
    pub fn live(&self) -> FrameView<'_> {
        let mut f = self.frame();
        f.rgba = self.back.as_flattened();
        f
    }

    pub fn frames_completed(&self) -> u64 {
        self.frames
    }

    /// Paint everything from the last painted dot up to (not including) global dot `target`.
    pub fn advance<C: Scanout>(&mut self, card: &mut C, target: u64) {
        if target <= self.painted {
            return;
        }
        let t = self.timing;
        let fd = t.frame_dots();
        // Far behind (the program idled for a while): only the last two frames can be seen.
        if target - self.painted > 2 * fd {
            let start = (target / fd).saturating_sub(1) * fd;
            if start > self.painted {
                self.painted = start;
            }
        }
        let ht = t.h_total as u64;
        while self.painted < target {
            let in_frame = self.painted % fd;
            let line = (in_frame / ht) as u32;
            let x = (in_frame % ht) as u32;
            if x == 0 {
                if line == 0 {
                    card.frame_start();
                }
                card.line_start(line);
                if line == t.v_retrace_start {
                    card.vretrace_start();
                }
                if line == t.v_blank_start {
                    std::mem::swap(&mut self.back, &mut self.front);
                    self.frames += 1;
                }
            }
            let x1 = (x as u64 + (target - self.painted)).min(ht) as u32;
            self.paint_segment(card, line, x, x1);
            self.painted += (x1 - x) as u64;
        }
    }

    fn row_of_line(&self, line: u32) -> Option<u32> {
        let t = &self.timing;
        if line < t.v_blank_start {
            Some(t.top_border() + line)
        } else if line >= t.v_blank_end {
            Some(line - t.v_blank_end)
        } else {
            None
        }
    }

    fn paint_segment<C: Scanout>(&mut self, card: &mut C, line: u32, x0: u32, x1: u32) {
        let t = self.timing;
        let w = self.width as usize;
        let lb = t.left_border();
        // Active display + right border, on this line's row.
        if x0 < t.h_blank_start
            && let Some(row) = self.row_of_line(line)
        {
            let row_off = row as usize * w + lb as usize;
            let a0 = x0;
            let a1 = x1.min(t.h_blank_start);
            if line < t.v_active {
                let e = a1.min(t.h_active);
                if a0 < e {
                    let out = &mut self.back[row_off + a0 as usize..row_off + e as usize];
                    card.render_active(line, a0, out);
                }
                let b0 = a0.max(t.h_active);
                if b0 < a1 {
                    let c = card.border();
                    self.back[row_off + b0 as usize..row_off + a1 as usize].fill(c);
                }
            } else if a0 < a1 {
                let c = card.border();
                self.back[row_off + a0 as usize..row_off + a1 as usize].fill(c);
            }
        }
        // Left border of the next line.
        if x1 > t.h_blank_end {
            let next = (line + 1) % t.v_total;
            if let Some(row) = self.row_of_line(next) {
                let s = x0.max(t.h_blank_end) - t.h_blank_end;
                let e = x1 - t.h_blank_end;
                let off = row as usize * w;
                let c = card.border();
                self.back[off + s as usize..off + e as usize].fill(c);
            }
        }
    }
}
