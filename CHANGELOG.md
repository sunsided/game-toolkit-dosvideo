# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `ratatui` feature: `tui::DosBackend`, a ratatui backend that writes cells into text memory
  (CP437 characters, attribute bytes, the hardware cursor), with `tui::draw` to run a frame
  inside `DosGame::step`, and `tui::mouse_cell` / `tui::mouse_cursor` for the `INT 33h` text
  cursor.
- `Mouse::show` / `Mouse::hide` (`INT 33h` functions 01h/02h): while a program shows the mouse
  cursor, the host hides the OS pointer over the emulated display. A mode set hides it.
- Headless runs take `DOSVIDEO_MOUSE=frame:x:y:buttons,...` to script the mouse, like
  `DOSVIDEO_KEYS` for keys.

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

[Unreleased]: https://github.com/sunsided/game-toolkit-dosvideo/compare/v0.2.2...HEAD
[0.2.2]: https://github.com/sunsided/game-toolkit-dosvideo/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/sunsided/game-toolkit-dosvideo/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/sunsided/game-toolkit-dosvideo/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/sunsided/game-toolkit-dosvideo/compare/fd5895d47b319a72d91e75651ac3ad8bdd12e2cc...v0.1.0
