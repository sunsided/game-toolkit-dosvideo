//! DOS-era PC video for [game-toolkit](https://docs.rs/game-toolkit): VGA (mode 13h, Mode X,
//! planar 16-color, text), EGA, CGA (with composite artifact colors and snow), Hercules,
//! Tandy 1000 and VESA SVGA, emulated at the register level with a beam-accurate CRT.
//!
//! # The model
//!
//! A [`Dos`] is a machine with a clock. Everything a program does costs time: a port write
//! ([`Dos::outb`]), a video memory write ([`Dos::poke`], priced by the configured
//! [`BusTiming`]), computation you choose to charge ([`Dos::cpu`]), and waiting for the beam
//! ([`Dos::wait_vretrace`], [`Dos::wait_line`]) or polling the status port like DOS programs
//! did (`while dos.inb(0x3DA) & 8 == 0 {}` works).
//!
//! The CRT paints the picture as the beam sweeps, from the adapter's state *at that moment*.
//! So all the tricks of the era fall out by themselves: palette changes between scanlines
//! (copper bars), changing the start address or line compare for page flipping and split
//! screens, pel panning for smooth scrolling, palette rotation, and the failure modes too:
//! tearing when a copy is overtaken by the beam, a start address that only latches at
//! vertical retrace, the attribute controller flip-flop, CRTC write protection, CGA snow.
//!
//! # Running a program
//!
//! With the default `toolkit` feature, implement [`DosGame`] and hand it to [`run`]. `step` is
//! one pass of your main loop; the host calls it as often as emulated time allows and shows the
//! frames the CRT produces.
//!
//! ```no_run
//! use game_toolkit_dosvideo::prelude::*;
//!
//! struct Hello;
//!
//! impl DosGame for Hello {
//!     fn init(dos: &mut Dos) -> anyhow::Result<Self> {
//!         dos.set_mode(0x13);
//!         Ok(Self)
//!     }
//!
//!     fn step(&mut self, dos: &mut Dos) {
//!         dos.wait_vretrace();
//!         let t = dos.frames() as u32;
//!         for y in 0..200u32 {
//!             let row: Vec<u8> = (0..320u32).map(|x| ((x ^ y) + t) as u8).collect();
//!             dos.poke_slice(0xA0000 + y * 320, &row);
//!         }
//!         if dos.key(sc::ESC) {
//!             dos.exit();
//!         }
//!     }
//! }
//!
//! fn main() -> anyhow::Result<()> {
//!     run::<Hello>(DosConfig::new("hello", Adapter::Vga))
//! }
//! ```

#![forbid(unsafe_code)]

pub mod cga;
mod crt;
pub mod draw;
pub mod font;
pub mod hercules;
pub mod input;
mod machine;
pub mod palette;
mod png;
pub mod speaker;
#[cfg(feature = "synth")]
pub mod speaker_synth;
pub mod svga;
pub mod text;
#[cfg(feature = "ratatui")]
pub mod tui;
pub mod vga;

#[cfg(feature = "toolkit")]
mod crt_filter;
#[cfg(feature = "toolkit")]
mod host;

pub use cga::CgaMonitor;
pub use crt::{Beam, FrameView, Timing};
pub use font::Font;
pub use input::{KeyPress, Keyboard, Mouse, sc};
pub use machine::{Adapter, BusTiming, Dos, MIN_IO_NS, MachineConfig, PIT_HZ, Video, cpu};
pub use palette::Phosphor;
pub use svga::VbeMode;

#[cfg(feature = "toolkit")]
pub use crt_filter::{CrtFilter, PhosphorMask};
#[cfg(feature = "toolkit")]
pub use host::{Aspect, DosConfig, DosGame, DosScreen, run};

/// `use game_toolkit_dosvideo::prelude::*;`
pub mod prelude {
    pub use crate::draw::{Bitmap, Dither, ModeX};
    pub use crate::palette::color;
    pub use crate::speaker::Melody;
    pub use crate::{
        Adapter, Beam, BusTiming, CgaMonitor, Dos, Font, MachineConfig, Phosphor, Timing, cpu, sc,
    };
    #[cfg(feature = "toolkit")]
    pub use crate::{Aspect, CrtFilter, DosConfig, DosGame, DosScreen, PhosphorMask, run};
    #[cfg(feature = "toolkit")]
    pub use game_toolkit::prelude::{Painter, anyhow};
}

#[cfg(feature = "toolkit")]
pub use game_toolkit;
/// The sound hardware behind [`speaker`]: prepared speaker music, the crystal clock.
pub use game_toolkit_dosaudio;
