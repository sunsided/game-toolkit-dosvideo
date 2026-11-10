# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- The web build no longer fetches `assets/manifest.txt` on start: a DOS program loads no
  files through the toolkit's assets, so `run` now preloads nothing instead of asking for a
  manifest, which was a 404 for every game and on every host page.

## [0.3.1] - 2026-11-10

### Fixed

- A mouse click whose press and release both fall between two frames (a quick click, a
  trackpad tap) was lost: the `INT 33h` driver counted presses from the held state sampled once
  per frame. A button pressed during the frame now counts as held for it, so `take_presses`
  sees the press.

## [0.3.0] - 2026-11-08

### Added

- `ratatui` feature: `tui::DosBackend`, a ratatui backend that writes cells into text memory
  (CP437 characters, attribute bytes, the hardware cursor), with `tui::draw` to run a frame
  inside `DosGame::step`, and `tui::mouse_cell` / `tui::mouse_cursor` for the `INT 33h` text
  cursor.
- `file_shell` example: a DOS-style file shell built from ratatui widgets (directory tree,
  file table, pull-down menus, dialogs, 80x50, mouse).
- `Mouse::show` / `Mouse::hide` (`INT 33h` functions 01h/02h): while a program shows the mouse
  cursor, the host hides the OS pointer over the emulated display. A mode set hides it.
- Headless runs take `DOSVIDEO_MOUSE=frame:x:y:buttons,...` to script the mouse, like
  `DOSVIDEO_KEYS` for keys.

### Changed

- The `text_mode` example draws its windows with ratatui: drag them by the title bar, click
  to bring one to the front, pull-down menus by mouse or `F10`. It now needs the `ratatui`
  feature.
- The PC speaker is [game-toolkit-dosaudio](https://crates.io/crates/game-toolkit-dosaudio)'s
  (PIT channel 2 and port `61h` on the 315/22 MHz crystal clock); the crate now depends on it,
  also without default features. `PIT_HZ` derives from the crystal (1193181.8 Hz, was
  1193182).
- The 8253 PIT is [game-toolkit-dosbus](https://crates.io/crates/game-toolkit-dosbus)'s, owned
  by dosaudio: ports `40h`-`43h` reach all three channels (latch, lo/hi flip-flop, modes 0, 2
  and 3), port `61h` bit 4 follows channel 1. `set_timer_hz` programs channel 0 through the
  ports (it now costs I/O time), and `timer_ticks` / `take_timer_ticks` count its edges.
- **Breaking:** `speaker::Melody` is dosaudio's QBasic `PLAY` melody, sequenced tick-exact
  in the background: `Dos::play_speaker(Melody::parse(..)?, PlayOptions::default())` returns
  a `SpeakerHandle` (pause, resume, stop, volume, the sounding note's `tone`). `Melody::new`, `update`, `looped`,
  `restart` and `stop` are gone. `Melody::parse` rejects unknown commands, including `R`.
- **Breaking:** `Dos::speaker_render(out)` renders at the fixed `speaker::SPEAKER_RATE`
  (44.1 kHz) and loses its `rate` argument.

## [0.2.2] - 2026-11-08

### Added

- `DosConfig::with_canvas_id`: render into an existing `<canvas>` on the web.
- `font_diff` example: compares the built-in fonts with a raw font file (side by side,
  overlay, either font alone).

### Changed

- `Font::vga`, `Font::ega` and `Font::cga` are the VGA BIOS 8x16, 8x14 and 8x8 fonts from
  Joseph Gil's public-domain fntcol16 collection, replacing the `u_vga16`-derived 8x16 font and
  the 8x14/8x8 sets squashed from it. The 8x14 font now matches IBM's pixel for pixel,
  including its vertical placement; the SGA variants keep the ROM glyphs outside the letters.

### Fixed

- The VGA/EGA overscan border is centered: horizontal blanking lags the CRTC compare by one
  character clock, as on real hardware. Mode 10h had 16 border dots on the left and none on
  the right; now it has 8 on each side.
- PC speaker crackle, mostly on the web: the runner keeps about 100 ms of audio queued,
  rebuilds that cushion after an underrun, and runs the machine up to 5% faster or slower to
  follow the audio device's clock. Audio queued while the canvas was off the page is dropped.
- Sound played in the first second after startup was lost on the web: the runner now holds
  the machine for up to 1.6 s until audio has started.

Thanks to the gorillas.rs port for finding and measuring these.

## [0.2.1] - 2026-11-06

### Fixed

- The README's screenshots use absolute URLs, so they render on crates.io (relative paths
  resolved against the package directory, where the images are not packaged).

## [0.2.0] - 2026-11-06

### Changed

- Require game-toolkit 0.11 (device limits, virtual resolution, post-processing, bitmap
  fonts, indexed colour); no API changes in dosvideo.

### Fixed

- The CRT filter draws into the toolkit's drawing target instead of the window surface, so
  it stays visible in games that set a virtual resolution or add post effects, and those
  effects apply on top of it.
- With the toolkit's indexed colour on, the CRT filter is skipped (with a warning) instead
  of writing colours into the canvas of palette indices.

## [0.1.0] - 2026-11-06

First release.

### Added

- `Dos`, a machine with a clock, a bus cost model (`BusTiming`: ISA8, ISA16, VLB, PCI,
  instant) and one display adapter. Port I/O, video memory access, CPU cycles and beam waits
  advance the clock; the CRT paints the picture dot by dot as the beam passes.
- Display adapters at the register level: VGA (mode 13h, Mode X via `ModeX`, 16-color planar
  modes, text with fonts in plane 2), EGA, CGA (palettes, mode 5, 640x200, composite artifact
  colors, snow, the 160x100 tweak), Tandy 1000, Hercules and a VESA VBE 2.0 SVGA card (banked
  window and linear framebuffer, 8/15/16/32 bpp).
- Register and drawing helpers: palette programming and cycling, CRTC start address, offset,
  line compare and pel panning, planar set/reset and latch copies, per-layout `pset`, text mode
  (boxes, cursor, blink, teletype with scrolling), an 8-bit `Bitmap` back buffer with ordered
  (Bayer) and Floyd-Steinberg dithering.
- CP437 fonts (8x16, 8x14, 8x8) and the Standard Galactic Alphabet.
- Keyboard (held-key table, BIOS buffer, port 0x60), `INT 33h` mouse, PIT timer ticks.
- PC speaker: PIT channel 2 and port 0x61 rendered to PCM, `sound`/`nosound`, PWM sample
  playback, QBasic `PLAY` strings (`Melody`), and with the `synth` feature synthie tracker
  songs on the speaker (`SpeakerTracker`).
- game-toolkit host (`toolkit` feature, default): `run` and `DosGame`, the embeddable
  `DosScreen`, aspect-correct scaling, a GPU CRT filter with presets (scanlines, beam blur,
  phosphor masks, bloom, curvature, monochrome phosphors), PC speaker audio, key help on
  stdout, and headless PNG/WAV capture for tests.
- Demos: `copper_bars`, `palette_cycling`, `modex_scroll`, `tearing_lab`, `ega_scroller`,
  `cga_showcase`, `tandy`, `hercules`, `svga`, `text_mode`, `crt_dither`, `pc_speaker`.

[Unreleased]: https://github.com/sunsided/game-toolkit-dosvideo/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/sunsided/game-toolkit-dosvideo/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/sunsided/game-toolkit-dosvideo/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/sunsided/game-toolkit-dosvideo/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/sunsided/game-toolkit-dosvideo/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/sunsided/game-toolkit-dosvideo/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/sunsided/game-toolkit-dosvideo/compare/fd5895d47b319a72d91e75651ac3ad8bdd12e2cc...v0.1.0
