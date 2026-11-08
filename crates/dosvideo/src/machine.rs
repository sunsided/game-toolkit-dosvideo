//! The machine: a clock, a bus, one display adapter and the bits of a PC a game talks to.

use crate::cga::{Cga, CgaKind, CgaMonitor};
use crate::crt::{Beam, Crt, FrameView, Scanout, Timing};
use crate::hercules::Hercules;
use crate::input::{Keyboard, Mouse};
use crate::palette::Phosphor;
use crate::speaker::PcSpeaker;
use crate::svga::{Svga, VbeMode};
use crate::vga::{Vga, VgaKind};
use game_toolkit_dosaudio::Tick;

/// Which display adapter is installed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Adapter {
    /// IBM VGA (1987): mode 13h, Mode X, 16-color planar modes, text.
    Vga,
    /// IBM EGA (1984) with 256 KiB.
    Ega,
    /// IBM CGA (1981).
    Cga(CgaMonitor),
    /// Tandy 1000 / PCjr graphics (1983/84).
    Tandy,
    /// Hercules Graphics Card (1982) on a monochrome monitor.
    Hercules(Phosphor),
    /// A VESA VBE 2.0 SVGA card with this much video memory.
    Svga { memory_kib: u32 },
}

/// How long the CPU waits for the video card on each access, in nanoseconds.
///
/// Video memory on the ISA bus was *slow*, and how slow decided whether a full-screen copy fit
/// into the vertical blank or tore. Every [`Dos::poke`] / [`Dos::outb`] advances the clock by
/// these amounts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusTiming {
    /// One 8-bit video memory access.
    pub mem8_ns: f64,
    /// One 16-bit access (two 8-bit cycles on an 8-bit bus).
    pub mem16_ns: f64,
    /// One 32-bit access.
    pub mem32_ns: f64,
    /// One I/O port access.
    pub io_ns: f64,
}

impl BusTiming {
    /// Free: video memory as fast as system RAM. Tearing then only comes from *when* you draw.
    /// Port accesses still cost a little ([`MIN_IO_NS`]), so status polling loops terminate.
    pub const INSTANT: Self = Self {
        mem8_ns: 0.0,
        mem16_ns: 0.0,
        mem32_ns: 0.0,
        io_ns: MIN_IO_NS,
    };
    /// An 8-bit card in a 4.77 MHz PC/XT (CGA, Hercules, early EGA): about 1 MB/s.
    pub const ISA8: Self = Self {
        mem8_ns: 1000.0,
        mem16_ns: 2000.0,
        mem32_ns: 4000.0,
        io_ns: 1000.0,
    };
    /// A 16-bit ISA VGA in a 286/386: 2-4 MB/s, a full mode 13h frame takes longer than the
    /// vertical blank.
    pub const ISA16: Self = Self {
        mem8_ns: 500.0,
        mem16_ns: 500.0,
        mem32_ns: 1000.0,
        io_ns: 800.0,
    };
    /// A VESA Local Bus card in a 486: ~30 MB/s.
    pub const VLB: Self = Self {
        mem8_ns: 100.0,
        mem16_ns: 100.0,
        mem32_ns: 120.0,
        io_ns: 300.0,
    };
    /// A PCI card in a Pentium: ~60 MB/s.
    pub const PCI: Self = Self {
        mem8_ns: 50.0,
        mem16_ns: 50.0,
        mem32_ns: 60.0,
        io_ns: 200.0,
    };
}

/// The least an I/O port access costs, whatever the [`BusTiming`] says. Without it a
/// `while (inp(0x3DA) & 8)` loop would never see time pass.
pub const MIN_IO_NS: f64 = 100.0;

/// Common CPU clock rates, for [`Dos::cpu`].
pub mod cpu {
    /// IBM PC / XT, 8088.
    pub const XT_8088: f64 = 4_772_727.0;
    /// IBM AT, 286.
    pub const AT_286: f64 = 8_000_000.0;
    pub const I386DX_33: f64 = 33_000_000.0;
    pub const I486DX2_66: f64 = 66_000_000.0;
    pub const PENTIUM_100: f64 = 100_000_000.0;
}

/// Machine configuration.
#[derive(Clone, Copy, Debug)]
pub struct MachineConfig {
    pub adapter: Adapter,
    pub bus: BusTiming,
    /// CPU clock in Hz, for [`Dos::cpu`].
    pub cpu_hz: f64,
    /// Snow on a CGA in 80-column text when touching memory during display.
    pub cga_snow: bool,
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            adapter: Adapter::Vga,
            bus: BusTiming::VLB,
            cpu_hz: cpu::I486DX2_66,
            cga_snow: true,
        }
    }
}

impl MachineConfig {
    pub fn new(adapter: Adapter) -> Self {
        let mut c = Self {
            adapter,
            ..Default::default()
        };
        if matches!(
            adapter,
            Adapter::Cga(_) | Adapter::Tandy | Adapter::Hercules(_)
        ) {
            c.bus = BusTiming::ISA8;
            c.cpu_hz = cpu::XT_8088;
        }
        if adapter == Adapter::Ega {
            c.bus = BusTiming::ISA16;
            c.cpu_hz = cpu::AT_286;
        }
        c
    }

    pub fn with_bus(mut self, bus: BusTiming) -> Self {
        self.bus = bus;
        self
    }

    pub fn with_cpu(mut self, hz: f64) -> Self {
        self.cpu_hz = hz;
        self
    }
}

/// The installed adapter, for read-only inspection.
pub enum Video {
    Vga(Box<Vga>),
    Cga(Box<Cga>),
    Hercules(Box<Hercules>),
    Svga(Box<Svga>),
}

macro_rules! with_card {
    ($video:expr, $c:ident => $e:expr) => {
        match $video {
            Video::Vga($c) => $e,
            Video::Cga($c) => $e,
            Video::Hercules($c) => $e,
            Video::Svga($c) => $e,
        }
    };
}

impl Video {
    fn timing(&mut self) -> Timing {
        with_card!(self, c => c.timing())
    }

    fn take_dirty(&mut self) -> bool {
        match self {
            Video::Vga(c) => std::mem::take(&mut c.timing_dirty),
            Video::Cga(c) => std::mem::take(&mut c.timing_dirty),
            Video::Hercules(c) => std::mem::take(&mut c.timing_dirty),
            Video::Svga(c) => c.take_dirty(),
        }
    }

    /// Pixel resolution of the current mode.
    pub fn resolution(&self) -> (u32, u32) {
        with_card!(self, c => c.logical_resolution())
    }

    /// `(columns, rows, scanlines per row)` in a text mode.
    pub fn text_geometry(&self) -> Option<(u32, u32, u32)> {
        match self {
            Video::Vga(c) => c.text_geometry(),
            Video::Cga(c) => c.text_geometry(),
            Video::Hercules(c) => c.text_geometry(),
            Video::Svga(c) => {
                if c.vbe.is_some() {
                    None
                } else {
                    c.vga.text_geometry()
                }
            }
        }
    }

    pub fn as_vga(&self) -> Option<&Vga> {
        match self {
            Video::Vga(v) => Some(v),
            Video::Svga(s) => Some(&s.vga),
            _ => None,
        }
    }

    pub fn as_cga(&self) -> Option<&Cga> {
        match self {
            Video::Cga(c) => Some(c),
            _ => None,
        }
    }

    pub fn as_hercules(&self) -> Option<&Hercules> {
        match self {
            Video::Hercules(h) => Some(h),
            _ => None,
        }
    }

    pub fn as_svga(&self) -> Option<&Svga> {
        match self {
            Video::Svga(s) => Some(s),
            _ => None,
        }
    }

    fn bios_mode(&self) -> u8 {
        match self {
            Video::Vga(v) => v.bios_mode,
            Video::Cga(c) => c.bios_mode,
            Video::Hercules(h) => h.bios_mode,
            Video::Svga(s) => s.vga.bios_mode,
        }
    }
}

/// The 8253 programmable interval timer, channel 0.
#[derive(Clone, Copy, Debug)]
struct Pit {
    divisor: u32,
    epoch_ns: f64,
    taken: u64,
    write_lo: Option<u8>,
    latch: Option<u16>,
    read_hi: bool,
}

/// 1.193182 MHz: the PC crystal (315/22 MHz) divided by 12.
pub const PIT_HZ: f64 = Tick::HZ_NUMERATOR as f64 / (Tick::HZ_DENOMINATOR * 12) as f64;

/// A DOS PC with one display adapter.
///
/// All interaction goes through the clock: port and memory accesses cost bus time, [`Dos::cpu`]
/// charges CPU cycles, and the `wait_*` functions busy-wait (in zero real time) until the beam
/// gets somewhere. The picture is painted as the beam passes, so *when* you change something
/// decides where on screen the change appears.
pub struct Dos {
    now: f64,
    crt: Crt,
    video: Video,
    bus: BusTiming,
    cpu_hz: f64,
    keyboard: Keyboard,
    mouse: Mouse,
    pit: Pit,
    exit: bool,
    adapter: Adapter,
    /// BIOS teletype cursor (column, row).
    pub(crate) tty: (u32, u32),
    pub(crate) speaker: PcSpeaker,
}

impl Dos {
    pub fn new(config: MachineConfig) -> Self {
        let mut video = match config.adapter {
            Adapter::Vga => Video::Vga(Box::new(Vga::new(VgaKind::Vga))),
            Adapter::Ega => Video::Vga(Box::new(Vga::new(VgaKind::Ega))),
            Adapter::Cga(m) => {
                let mut c = Cga::new(CgaKind::Cga);
                c.set_monitor(m);
                c.snow = config.cga_snow;
                Video::Cga(Box::new(c))
            }
            Adapter::Tandy => Video::Cga(Box::new(Cga::new(CgaKind::Tandy))),
            Adapter::Hercules(p) => Video::Hercules(Box::new(Hercules::new(p))),
            Adapter::Svga { memory_kib } => {
                Video::Svga(Box::new(Svga::new(memory_kib as usize * 1024)))
            }
        };
        let timing = video.timing();
        video.take_dirty();
        let mut dos = Self {
            now: 0.0,
            crt: Crt::new(timing, 0.0),
            video,
            bus: config.bus,
            cpu_hz: config.cpu_hz,
            keyboard: Keyboard::default(),
            mouse: Mouse::default(),
            pit: Pit {
                divisor: 65536,
                epoch_ns: 0.0,
                taken: 0,
                write_lo: None,
                latch: None,
                read_hi: false,
            },
            exit: false,
            adapter: config.adapter,
            tty: (0, 0),
            speaker: PcSpeaker::new(),
        };
        let mode = dos.video.bios_mode();
        dos.mouse
            .reset_for_mode(mode, matches!(config.adapter, Adapter::Hercules(_)));
        dos
    }

    pub fn adapter(&self) -> Adapter {
        self.adapter
    }

    /// The display adapter, for inspection (registers, video memory).
    pub fn video(&self) -> &Video {
        &self.video
    }

    // ---------------------------------------------------------------------------------------
    // Time

    /// Machine time in nanoseconds since power-on.
    pub fn now_ns(&self) -> f64 {
        self.now
    }

    /// Machine time in crystal ticks, for the sound hardware.
    pub(crate) fn now_tick(&self) -> Tick {
        Tick::from_secs_f64(self.now * 1e-9)
    }

    /// Machine time in seconds since power-on.
    pub fn now(&self) -> f64 {
        self.now * 1e-9
    }

    /// Spend `cycles` CPU clock cycles (your code's cost, if you want it modeled).
    pub fn cpu(&mut self, cycles: u64) {
        self.now += cycles as f64 * 1e9 / self.cpu_hz;
    }

    pub fn cpu_hz(&self) -> f64 {
        self.cpu_hz
    }

    pub fn set_cpu_hz(&mut self, hz: f64) {
        self.cpu_hz = hz.max(1.0);
    }

    pub fn bus(&self) -> BusTiming {
        self.bus
    }

    /// Swap the bus timing at run time (to compare ISA vs. local bus).
    pub fn set_bus(&mut self, bus: BusTiming) {
        self.bus = bus;
    }

    /// Busy-wait for `us` microseconds.
    pub fn delay_us(&mut self, us: f64) {
        self.now += us * 1000.0;
    }

    /// Advance the clock to `t_ns` if it is in the future (an idle CPU).
    pub fn idle_until(&mut self, t_ns: f64) {
        if t_ns > self.now {
            self.now = t_ns;
        }
        self.sync();
    }

    // ---------------------------------------------------------------------------------------
    // Beam

    /// Paint the screen up to the current beam position.
    pub fn sync(&mut self) {
        let target = self.crt.dot_at(self.now);
        with_card!(&mut self.video, c => self.crt.advance(&mut **c, target));
    }

    fn check_timing(&mut self) {
        if self.video.take_dirty() {
            let t = self.video.timing();
            with_card!(&mut self.video, c => self.crt.retime(&mut **c, t, self.now));
        }
    }

    /// Where the beam is right now.
    pub fn beam(&self) -> Beam {
        self.crt.beam_at(self.now)
    }

    /// Raster timing of the current mode.
    pub fn timing(&self) -> Timing {
        *self.crt.timing()
    }

    /// Refresh rate of the current mode.
    pub fn refresh_hz(&self) -> f64 {
        self.crt.timing().refresh_hz()
    }

    /// Pixel resolution of the current mode.
    pub fn resolution(&self) -> (u32, u32) {
        self.video.resolution()
    }

    fn wait_dot(&mut self, line: u32, dot: u32) {
        let from = self.crt.dot_at(self.now) + 1;
        let g = self.crt.next_dot(from, line, dot);
        self.now = self
            .now
            .max(self.crt.time_of(g) + 0.5 / (self.crt.timing().dot_clock_hz * 1e-9));
        self.sync();
    }

    /// The classic `while (inp(0x3DA) & 8); while (!(inp(0x3DA) & 8));`: wait until the *start*
    /// of the next vertical retrace (if one is in progress, wait for the one after it).
    pub fn wait_vretrace(&mut self) {
        let t = *self.crt.timing();
        self.wait_dot(t.v_retrace_start, 0);
    }

    /// Wait until vertical retrace ends.
    pub fn wait_vretrace_end(&mut self) {
        if self.beam().v_retrace {
            let t = *self.crt.timing();
            self.wait_dot(t.v_retrace_end, 0);
        }
    }

    /// Wait for the start of vertical *blanking* (the bottom border ends), a little earlier than
    /// retrace: the earliest moment to start changing the picture invisibly.
    pub fn wait_vblank(&mut self) {
        let t = *self.crt.timing();
        self.wait_dot(t.v_blank_start, 0);
    }

    /// Wait for the first active line of the next frame.
    pub fn wait_frame_start(&mut self) {
        self.wait_dot(0, 0);
    }

    /// Wait until the beam starts scanline `line` (0 = first active line).
    pub fn wait_line(&mut self, line: u32) {
        self.wait_dot(line, 0);
    }

    /// Wait until the beam reaches the end of the active part of the current line (the start of
    /// horizontal blanking, where a palette change is invisible).
    pub fn wait_hblank(&mut self) {
        let b = self.beam();
        let t = *self.crt.timing();
        self.wait_dot(b.line, t.h_active);
    }

    /// Wait until the beam reaches dot `dot` of line `line`.
    pub fn wait_beam(&mut self, line: u32, dot: u32) {
        self.wait_dot(line, dot);
    }

    /// Frames scanned out since power-on.
    pub fn frames(&self) -> u64 {
        self.crt.frames_completed()
    }

    // ---------------------------------------------------------------------------------------
    // Output

    /// The last completely scanned-out frame.
    pub fn frame(&self) -> FrameView<'_> {
        self.crt.frame()
    }

    /// The picture as painted so far, including the half-painted current frame (call
    /// [`Dos::sync`] first).
    pub fn live_frame(&self) -> FrameView<'_> {
        self.crt.live()
    }

    // ---------------------------------------------------------------------------------------
    // Ports

    /// `OUT port, AL`.
    pub fn outb(&mut self, port: u16, v: u8) {
        self.now += self.bus.io_ns.max(MIN_IO_NS);
        self.sync();
        match port {
            0x40 => self.pit_write(v),
            // Port 0x43 is shared: channel 2 commands go to the speaker.
            0x42 | 0x61 => self.speaker.outb(self.now_tick(), port, v),
            0x43 if v >> 6 == 2 => self.speaker.outb(self.now_tick(), port, v),
            0x43 => self.pit_command(v),
            _ => with_card!(&mut self.video, c => c.port_out(port, v)),
        }
        self.check_timing();
    }

    /// `OUT port, AX`: low byte to `port`, high byte to `port + 1`. With an index/data pair
    /// this sets a register in one go: `outw(0x3C4, 0x0F02)` writes 0x0F to sequencer reg 2.
    pub fn outw(&mut self, port: u16, v: u16) {
        self.outb(port, v as u8);
        self.outb(port + 1, (v >> 8) as u8);
    }

    /// `IN AL, port`.
    pub fn inb(&mut self, port: u16) -> u8 {
        self.now += self.bus.io_ns.max(MIN_IO_NS);
        let beam = self.crt.beam_at(self.now);
        match port {
            0x40 => self.pit_read(),
            0x60 => self.keyboard.port60(),
            0x42 | 0x61 => self.speaker.inb(self.now_tick(), port),
            _ => {
                self.sync();
                with_card!(&mut self.video, c => c.port_in(port, &beam))
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // Memory

    fn mem_write_raw(&mut self, addr: u32, v: u8) {
        let beam = self.crt.beam_at(self.now);
        match &mut self.video {
            Video::Vga(c) => {
                c.mem_write(addr, v);
            }
            Video::Cga(c) => {
                c.mem_write(addr, v, &beam);
            }
            Video::Hercules(c) => {
                c.mem_write(addr, v);
            }
            Video::Svga(c) => {
                c.mem_write(addr, v);
            }
        }
    }

    fn mem_read_raw(&mut self, addr: u32) -> u8 {
        let beam = self.crt.beam_at(self.now);
        match &mut self.video {
            Video::Vga(c) => c.mem_read(addr),
            Video::Cga(c) => c.mem_read(addr, &beam),
            Video::Hercules(c) => c.mem_read(addr),
            Video::Svga(c) => c.mem_read(addr),
        }
        .unwrap_or(0xFF)
    }

    /// Write a byte to a physical address (`0xA0000` is `A000:0000`).
    pub fn poke(&mut self, addr: u32, v: u8) {
        self.now += self.bus.mem8_ns;
        self.sync();
        self.mem_write_raw(addr, v);
    }

    /// Read a byte. On EGA/VGA this also loads the latches.
    pub fn peek(&mut self, addr: u32) -> u8 {
        self.now += self.bus.mem8_ns;
        self.sync();
        self.mem_read_raw(addr)
    }

    /// Write a little-endian word in one bus cycle.
    pub fn pokew(&mut self, addr: u32, v: u16) {
        self.now += self.bus.mem16_ns;
        self.sync();
        self.mem_write_raw(addr, v as u8);
        self.mem_write_raw(addr + 1, (v >> 8) as u8);
    }

    pub fn peekw(&mut self, addr: u32) -> u16 {
        self.now += self.bus.mem16_ns;
        self.sync();
        let lo = self.mem_read_raw(addr) as u16;
        lo | (self.mem_read_raw(addr + 1) as u16) << 8
    }

    /// Write a little-endian dword in one bus cycle (a 386's `MOV [mem], EAX`).
    pub fn poke32(&mut self, addr: u32, v: u32) {
        self.now += self.bus.mem32_ns;
        self.sync();
        for (i, b) in v.to_le_bytes().into_iter().enumerate() {
            self.mem_write_raw(addr + i as u32, b);
        }
    }

    /// Copy bytes into video memory using 32-bit moves (`REP MOVSD`), with bus timing.
    pub fn poke_slice(&mut self, addr: u32, data: &[u8]) {
        self.write_block(addr, data.len(), |i| data[i]);
    }

    /// Fill video memory with a byte (`REP STOSD`).
    pub fn fill(&mut self, addr: u32, len: usize, v: u8) {
        self.write_block(addr, len, |_| v);
    }

    fn write_block(&mut self, addr: u32, len: usize, byte: impl Fn(usize) -> u8) {
        // Timing granularity: 16 bytes per step, so the beam can overtake a copy mid-way.
        const STEP: usize = 16;
        let per_byte = self.bus.mem32_ns / 4.0;
        if per_byte == 0.0 {
            self.sync();
            for i in 0..len {
                self.mem_write_raw(addr + i as u32, byte(i));
            }
            return;
        }
        let mut i = 0;
        while i < len {
            let n = STEP.min(len - i);
            self.now += per_byte * n as f64;
            self.sync();
            for k in i..i + n {
                self.mem_write_raw(addr + k as u32, byte(k));
            }
            i += n;
        }
    }

    /// Read bytes from video memory, with bus timing.
    pub fn peek_slice(&mut self, addr: u32, out: &mut [u8]) {
        self.now += self.bus.mem32_ns / 4.0 * out.len() as f64;
        self.sync();
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.mem_read_raw(addr + i as u32);
        }
    }

    /// Copy within video memory one byte at a time (`REP MOVSB` from video to video memory).
    /// On EGA/VGA with write mode 1 each byte read loads all four latches and each write stores
    /// them, so this moves four planes per byte: the fast tile copy of EGA-era scrollers.
    pub fn copy_vram(&mut self, src: u32, dst: u32, len: usize) {
        for i in 0..len as u32 {
            self.now += self.bus.mem8_ns * 2.0;
            self.sync();
            let v = self.mem_read_raw(src + i);
            self.mem_write_raw(dst + i, v);
        }
    }

    // ---------------------------------------------------------------------------------------
    // BIOS

    /// `INT 10h, AH=00h`: set a BIOS video mode. Bit 7 keeps video memory. Returns `false` if
    /// the adapter has no such mode (the real BIOS would silently do something odd).
    ///
    /// | Adapter | Modes |
    /// |---------|-------|
    /// | VGA / SVGA | 00h-03h, 0Dh, 0Eh, 10h, 12h, 13h |
    /// | EGA | 02h, 03h, 0Dh, 0Eh, 10h |
    /// | CGA | 00h-06h |
    /// | Tandy | 00h-06h, 08h, 09h, 0Ah |
    /// | Hercules | 07h (graphics: [`Dos::hercules_graphics`]) |
    pub fn set_mode(&mut self, mode: u8) -> bool {
        self.now += 50_000.0; // the BIOS takes its time
        self.sync();
        let ok = match &mut self.video {
            Video::Vga(v) => v.set_mode(mode),
            Video::Cga(c) => c.set_mode(mode),
            Video::Hercules(h) => h.set_mode(mode),
            Video::Svga(s) => s.set_vga_mode(mode),
        };
        if ok {
            self.after_mode_set();
        }
        ok
    }

    fn after_mode_set(&mut self) {
        self.video.take_dirty();
        let t = self.video.timing();
        with_card!(&mut self.video, c => {
            self.crt.reset(&mut **c, t, self.now);
            c.vretrace_start();
        });
        self.tty = (0, 0);
        let mode = self.video.bios_mode();
        self.mouse
            .reset_for_mode(mode, matches!(self.adapter, Adapter::Hercules(_)));
        if mode == 0xFF {
            let (w, h) = self.video.resolution();
            self.mouse.range = [0, w as i32 - 1, 0, h as i32 - 1];
        }
    }

    /// The current BIOS mode number (`INT 10h, AH=0Fh`), `0xFF` in a VBE mode.
    pub fn mode(&self) -> u8 {
        self.video.bios_mode()
    }

    /// VBE function 02h: set an SVGA mode. Returns `None` without an SVGA card or for an
    /// unknown mode.
    pub fn vbe_set_mode(&mut self, number: u16) -> Option<VbeMode> {
        self.now += 100_000.0;
        self.sync();
        let Video::Svga(s) = &mut self.video else {
            return None;
        };
        let m = s.set_vbe_mode(number)?;
        self.after_mode_set();
        Some(m)
    }

    /// VBE function 05h: select the 64 KiB bank visible at `A000:0000`. Through the BIOS this
    /// is a far call and costs a few microseconds, which adds up per scanline.
    pub fn vbe_set_bank(&mut self, bank: u32) {
        self.now += 2_000.0;
        self.sync();
        if let Video::Svga(s) = &mut self.video
            && let Some(v) = &mut s.vbe
        {
            v.bank = bank;
        }
    }

    /// Current VBE bank.
    pub fn vbe_bank(&self) -> u32 {
        match &self.video {
            Video::Svga(s) => s.vbe.as_ref().map_or(0, |v| v.bank),
            _ => 0,
        }
    }

    /// Bytes per scanline in the current VBE mode.
    pub fn vbe_pitch(&self) -> u32 {
        match &self.video {
            Video::Svga(s) => s.vbe.as_ref().map_or(0, |v| v.pitch),
            _ => 0,
        }
    }

    /// VBE function 06h: set the logical scanline length in pixels (for a virtual screen wider
    /// than the display).
    pub fn vbe_set_scanline(&mut self, pixels: u32) {
        self.now += 2_000.0;
        self.sync();
        if let Video::Svga(s) = &mut self.video
            && let Some(v) = &mut s.vbe
        {
            v.pitch = pixels * v.mode.bytes_per_pixel();
        }
    }

    /// VBE function 07h: set the display start (first visible pixel) for panning and page
    /// flipping. Takes effect at the next vertical retrace; `wait` waits for it (subfunction
    /// 80h), like a page flip should.
    pub fn vbe_set_display_start(&mut self, x: u32, y: u32, wait: bool) {
        self.now += 2_000.0;
        self.sync();
        if let Video::Svga(s) = &mut self.video
            && let Some(v) = &mut s.vbe
        {
            v.start = y * v.pitch + x * v.mode.bytes_per_pixel();
        }
        if wait {
            self.wait_vretrace();
        }
    }

    /// VBE function 08h: switch the DAC between 6 and 8 bits per component.
    pub fn vbe_set_dac_width(&mut self, bits: u8) -> u8 {
        self.sync();
        if let Video::Svga(s) = &mut self.video {
            s.vga.dac_8bit = bits >= 8;
            s.vga.refresh_dac_all();
            return if s.vga.dac_8bit { 8 } else { 6 };
        }
        6
    }

    /// The current VBE mode.
    pub fn vbe_mode(&self) -> Option<VbeMode> {
        match &self.video {
            Video::Svga(s) => s.vbe.as_ref().map(|v| v.mode),
            _ => None,
        }
    }

    /// Program the Hercules card for 720x348 graphics on `page` (0 or 1). The configuration
    /// switch is set to allow graphics and (for page 1) the second page; there is no BIOS
    /// support, so this is what every Hercules game shipped its own version of.
    pub fn hercules_graphics(&mut self, page: u8) -> bool {
        if !matches!(self.video, Video::Hercules(_)) {
            return false;
        }
        self.outb(0x3BF, 0x03);
        self.outb(0x3B8, 0x02);
        for (i, v) in crate::hercules::HGC_GRAPHICS_CRTC.iter().enumerate() {
            self.outb(0x3B4, i as u8);
            self.outb(0x3B5, *v);
        }
        self.outb(0x3B8, 0x0A | if page & 1 != 0 { 0x80 } else { 0 });
        if let Video::Hercules(h) = &mut self.video {
            h.bios_mode = 0x7F;
        }
        self.mouse.range = [0, 719, 0, 347];
        true
    }

    /// Back to Hercules text mode.
    pub fn hercules_text(&mut self) -> bool {
        if !matches!(self.video, Video::Hercules(_)) {
            return false;
        }
        self.outb(0x3B8, 0x00);
        for (i, v) in crate::hercules::HGC_TEXT_CRTC.iter().enumerate() {
            self.outb(0x3B4, i as u8);
            self.outb(0x3B5, *v);
        }
        self.outb(0x3B8, 0x28);
        if let Video::Hercules(h) = &mut self.video {
            h.bios_mode = 7;
        }
        true
    }

    /// Change the CGA monitor (RGB vs. composite) at run time.
    pub fn cga_set_monitor(&mut self, m: CgaMonitor) {
        self.sync();
        if let Video::Cga(c) = &mut self.video {
            c.set_monitor(m);
        }
    }

    /// Composite tint knob.
    pub fn cga_set_composite_hue(&mut self, degrees: f32) {
        self.sync();
        if let Video::Cga(c) = &mut self.video {
            c.composite_hue = degrees;
        }
    }

    /// Change the Hercules phosphor color.
    pub fn hercules_set_phosphor(&mut self, p: Phosphor) {
        self.sync();
        if let Video::Hercules(h) = &mut self.video {
            h.phosphor = p;
        }
    }

    /// Load a font into VGA/EGA character map `map` (like `INT 10h, AX=1110h`), for custom
    /// character sets in text mode. The font height should match the mode's character height.
    pub fn load_font(&mut self, font: &crate::font::Font, map: usize) {
        self.now += 20_000.0;
        self.sync();
        match &mut self.video {
            Video::Vga(v) => v.load_font(font, map),
            Video::Svga(s) => s.vga.load_font(font, map),
            _ => {}
        }
    }

    // ---------------------------------------------------------------------------------------
    // Timer

    fn pit_command(&mut self, v: u8) {
        if v >> 6 != 0 {
            return; // only channel 0
        }
        if (v >> 4) & 3 == 0 {
            self.pit.latch = Some(self.pit_counter());
            self.pit.read_hi = false;
        } else {
            self.pit.write_lo = None;
        }
    }

    fn pit_write(&mut self, v: u8) {
        match self.pit.write_lo.take() {
            None => self.pit.write_lo = Some(v),
            Some(lo) => {
                let d = (lo as u32) | ((v as u32) << 8);
                self.set_timer_divisor(if d == 0 { 65536 } else { d });
            }
        }
    }

    fn pit_counter(&self) -> u16 {
        let ticks = ((self.now - self.pit.epoch_ns) * PIT_HZ * 1e-9) as u64;
        let d = self.pit.divisor as u64;
        (d - ticks % d) as u16
    }

    fn pit_read(&mut self) -> u8 {
        let v = self.pit.latch.unwrap_or_else(|| self.pit_counter());
        let out = if self.pit.read_hi {
            self.pit.latch = None;
            (v >> 8) as u8
        } else {
            v as u8
        };
        self.pit.read_hi = !self.pit.read_hi;
        out
    }

    fn set_timer_divisor(&mut self, d: u32) {
        self.pit.divisor = d.clamp(1, 65536);
        self.pit.epoch_ns = self.now;
        self.pit.taken = 0;
    }

    /// Reprogram the timer interrupt rate (the PIT channel 0 divisor), like games did for a
    /// steady game tick.
    pub fn set_timer_hz(&mut self, hz: f64) {
        let d = (PIT_HZ / hz.max(18.2)).round() as u32;
        self.set_timer_divisor(d);
    }

    /// Current timer interrupt rate.
    pub fn timer_hz(&self) -> f64 {
        PIT_HZ / self.pit.divisor as f64
    }

    /// Timer interrupts since the timer was last programmed.
    pub fn timer_ticks(&self) -> u64 {
        ((self.now - self.pit.epoch_ns) * PIT_HZ * 1e-9) as u64 / self.pit.divisor as u64
    }

    /// Timer interrupts since the last call: run that many fixed game ticks, as an `INT 8`
    /// handler would have.
    pub fn take_timer_ticks(&mut self) -> u64 {
        let t = self.timer_ticks();
        let n = t - self.pit.taken.min(t);
        self.pit.taken = t;
        n
    }

    /// BIOS tick count (18.2 Hz since midnight, here since power-on).
    pub fn bios_ticks(&self) -> u64 {
        (self.now * 1e-9 * PIT_HZ / 65536.0) as u64
    }

    // ---------------------------------------------------------------------------------------
    // Input

    pub fn keyboard(&self) -> &Keyboard {
        &self.keyboard
    }

    pub fn keyboard_mut(&mut self) -> &mut Keyboard {
        &mut self.keyboard
    }

    /// Is the key with this scancode held?
    pub fn key(&self, scan: u8) -> bool {
        self.keyboard.down(scan)
    }

    /// `kbhit()`.
    pub fn kbhit(&self) -> bool {
        self.keyboard.hit()
    }

    /// `getch()`, without blocking.
    pub fn getch(&mut self) -> Option<crate::input::KeyPress> {
        self.keyboard.read()
    }

    pub fn mouse(&self) -> &Mouse {
        &self.mouse
    }

    pub fn mouse_mut(&mut self) -> &mut Mouse {
        &mut self.mouse
    }

    /// The mouse position converted from driver coordinates to pixels of the current mode
    /// (the conversion a program has to do itself).
    pub fn mouse_pixel(&self) -> (i32, i32) {
        let (w, h) = self.video.resolution();
        let [x0, x1, y0, y1] = self.mouse.range;
        let sx = (self.mouse.x - x0) as i64 * w as i64 / (x1 - x0 + 1).max(1) as i64;
        let sy = (self.mouse.y - y0) as i64 * h as i64 / (y1 - y0 + 1).max(1) as i64;
        (sx as i32, sy as i32)
    }

    // ---------------------------------------------------------------------------------------
    // Program control

    /// Ask the host to end the program (`INT 21h, AH=4Ch`).
    pub fn exit(&mut self) {
        self.exit = true;
    }

    pub fn exit_requested(&self) -> bool {
        self.exit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vga() -> Dos {
        let mut d = Dos::new(MachineConfig::default().with_bus(BusTiming::INSTANT));
        assert!(d.set_mode(0x13));
        d
    }

    #[test]
    fn status_register_polling_terminates() {
        // Even on the INSTANT bus a port read costs time, so this cannot spin forever.
        let mut d = vga();
        while d.inb(0x3DA) & 8 != 0 {}
        while d.inb(0x3DA) & 8 == 0 {}
        assert!(d.beam().v_retrace);
    }

    #[test]
    fn polling_with_bus_cost_finds_retrace() {
        let mut d = Dos::new(MachineConfig::default());
        d.set_mode(0x13);
        while d.inb(0x3DA) & 8 != 0 {}
        while d.inb(0x3DA) & 8 == 0 {}
        assert!(d.beam().v_retrace);
        assert_eq!(d.beam().line, d.timing().v_retrace_start);
    }

    #[test]
    fn wait_vretrace_lands_on_retrace_start() {
        let mut d = vga();
        d.wait_vretrace();
        let b = d.beam();
        assert!(b.v_retrace);
        assert_eq!(b.line, d.timing().v_retrace_start);
        let f = b.frame;
        d.wait_vretrace();
        assert_eq!(d.beam().frame, f + 1);
    }

    #[test]
    fn mode13h_pixel_reaches_the_screen() {
        let mut d = vga();
        d.poke(0xA0000 + 100 * 320 + 160, 4); // red
        d.wait_vretrace();
        d.wait_vretrace();
        let f = d.frame();
        let [ax, ay, ..] = f.active;
        let x = ax + 160 * 2;
        let y = ay + 100 * 2;
        let i = ((y * f.width + x) * 4) as usize;
        assert_eq!(&f.rgba[i..i + 3], &[0xAA, 0, 0]);
        let i2 = (((y + 1) * f.width + x + 1) * 4) as usize;
        assert_eq!(
            &f.rgba[i2..i2 + 3],
            &[0xAA, 0, 0],
            "double-scanned and doubled"
        );
    }

    #[test]
    fn palette_change_mid_frame_splits_the_screen() {
        let mut d = vga();
        d.fill(0xA0000, 64000, 1);
        d.wait_frame_start();
        d.outb(0x3C8, 1);
        d.outb(0x3C9, 63);
        d.outb(0x3C9, 0);
        d.outb(0x3C9, 0);
        d.wait_line(200);
        d.outb(0x3C8, 1);
        d.outb(0x3C9, 0);
        d.outb(0x3C9, 63);
        d.outb(0x3C9, 0);
        d.wait_vretrace();
        let f = d.frame();
        let [ax, ay, ..] = f.active;
        let px = |y: u32| {
            let i = (((ay + y) * f.width + ax + 10) * 4) as usize;
            [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2]]
        };
        assert_eq!(px(10), [255, 0, 0]);
        assert_eq!(px(390), [0, 255, 0]);
    }

    #[test]
    fn slow_bus_copy_tears() {
        // Copy a full frame of color 2 over color 1 on an ISA bus, starting at the top of the
        // display: the beam overtakes the copy, so the finished frame shows both.
        let mut d = Dos::new(MachineConfig::default().with_bus(BusTiming::ISA16));
        d.set_mode(0x13);
        d.wait_vretrace();
        d.wait_frame_start();
        let data = vec![2u8; 64000];
        d.poke_slice(0xA0000, &data);
        assert!(
            d.now_ns() > 14e6,
            "64000 bytes on ISA16 take longer than a frame"
        );
    }

    #[test]
    fn timer_ticks() {
        let mut d = vga();
        d.set_timer_hz(100.0);
        d.delay_us(105_000.0);
        assert_eq!(d.take_timer_ticks(), 10);
        assert_eq!(d.take_timer_ticks(), 0);
    }

    #[test]
    fn mode_support_per_adapter() {
        let mut d = Dos::new(MachineConfig::new(Adapter::Cga(CgaMonitor::Rgb)));
        assert!(d.set_mode(4));
        assert!(!d.set_mode(0x13));
        let mut d = Dos::new(MachineConfig::new(Adapter::Svga { memory_kib: 2048 }));
        assert_eq!(d.vbe_set_mode(0x101).map(|m| m.width), Some(640));
        assert_eq!(d.resolution(), (640, 480));
        assert!((d.refresh_hz() - 59.94).abs() < 0.05);
    }
}
