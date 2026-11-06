# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/sunsided/game-toolkit-dosvideo/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/sunsided/game-toolkit-dosvideo/compare/fd5895d47b319a72d91e75651ac3ad8bdd12e2cc...v0.1.0
