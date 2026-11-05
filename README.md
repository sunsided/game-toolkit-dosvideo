# game-toolkit-dosvideo

DOS-era PC video for [game-toolkit](https://github.com/sunsided/game-toolkit): VGA (mode 13h,
Mode X, 16-color planar, text), EGA, CGA (with composite artifact colors and snow), Hercules,
Tandy 1000 and VESA SVGA, emulated at the register level with a beam-accurate CRT.

It is not a PC emulator: your game is ordinary Rust. What is emulated is the part DOS
programmers fought with, the display adapter: its memory layout, its registers, its timing,
and the electron beam that reads it out while you are writing to it. Program it through I/O
ports and memory pokes like in 1992, or through the helpers that do the same thing with less
typing.

```rust
use game_toolkit_dosvideo::prelude::*;

struct Hello;

impl DosGame for Hello {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        dos.set_mode(0x13); // INT 10h, AX=0013h
        Ok(Self)
    }

    fn step(&mut self, dos: &mut Dos) {
        // while (inp(0x3DA) & 8); while (!(inp(0x3DA) & 8));
        dos.wait_vretrace();
        dos.set_palette(1, [63, 0, 0]); // OUT 3C8h / 3C9h
        dos.fill(0xA0000 + 100 * 320, 320, 1); // a red line in A000:7D00
        if dos.key(sc::ESC) {
            dos.exit();
        }
    }
}

fn main() -> anyhow::Result<()> {
    run::<Hello>(DosConfig::new("hello", Adapter::Vga))
}
```

## The model: a clock, a bus and a beam

A `Dos` is a machine with a clock. Everything costs time:

- port I/O (`outb`, `inb`) and video memory access (`poke`, `peek`, `poke_slice`, `fill`),
  priced by the configured `BusTiming`. An 8-bit ISA card moves about 1 MB/s, a 16-bit ISA VGA
  2–4 MB/s, a VLB or PCI card 30–60 MB/s.
- CPU work you choose to charge (`dos.cpu(cycles)` at the configured clock rate).
- waiting for the beam: `wait_vretrace`, `wait_vblank`, `wait_line(n)`, `wait_hblank`,
  `wait_beam(line, dot)`, or polling the status port yourself.

The CRT paints the picture as the beam sweeps, dot by dot, from the adapter's state *at that
moment*. Nothing is special-cased, so the tricks of the era just work:

- copper bars: DAC writes between scanlines (at 400-line resolution in mode 13h, into the
  overscan border too).
- palette cycling and fades.
- page flipping with the start address, which latches at vertical retrace, like the real CRTC.
- split screens with line compare, smooth scrolling with pel panning, virtual screens with the
  offset register.
- tweaked modes (Mode X, 320x400, 360x240, …): timing and resolution are derived from the
  CRTC registers, so programming them is enough.
- latch copies (write mode 1) for four planes per byte, set/reset and the bit mask, and read
  mode 1 color compare.

So do the failure modes:

- tearing when a copy is overtaken by the beam (on a slow bus a full mode 13h frame takes
  longer than the vertical blank).
- the attribute controller flip-flop, and a blank screen if the palette address source bit
  isn't set again.
- CRTC registers 0–7 being write-protected.
- leftover garbage after unchaining mode 13h.
- CGA snow in 80-column text.
- the inverted Hercules retrace bit.
- EGA's write-only registers and the 16-color limit of its 200-line modes.
- the mouse driver reporting 640x200 coordinates in mode 13h.

## Adapters

| `Adapter` | Memory | BIOS modes | Highlights |
|-----------|--------|-----------|------------|
| `Vga` | 256 KiB, 4 planes | 00h–03h, 0Dh, 0Eh, 10h, 12h, 13h | Mode X (`ModeX`), DAC, line compare, pel panning, chain-4 / odd-even / planar addressing, 9-dot text with fonts in plane 2 |
| `Ega` | 256 KiB, 4 planes | 02h, 03h, 0Dh, 0Eh, 10h | write-only registers, 64-color palette (RGBI in 200-line modes), 14.318/16.257 MHz clocks |
| `Cga(CgaMonitor)` | 16 KiB | 00h–06h | 2 palettes + intensity, mode 5, 640x200, composite artifact colors, snow, 160x100 via 6845 tweaks |
| `Tandy` | 32 KiB | 00h–06h, 08h, 09h, 0Ah | 160x200x16, 320x200x16, 640x200x4, video gate array palette |
| `Hercules(Phosphor)` | 64 KiB, 2 pages | 07h (+ `hercules_graphics`) | 720x348, 4-way interleave, config switch, MDA attributes, 9th-column line graphics |
| `Svga { memory_kib }` | as configured | VGA modes + VBE 100h–118h | 64 KiB bank window with switch cost, linear framebuffer at `0xE0000000`, display start, scanline length, 8-bit DAC, 8/15/16/32 bpp |

SVGA is period-correct for DOS: VBE 1.2 (1991) and 2.0 (1994) were how DOS games reached
640x480 and above.

## Layers of API

From most to least authentic, freely mixed:

1. **Ports and memory**: `outb`, `outw`, `inb`, `poke`, `peek`, `pokew`, `poke32`,
   `poke_slice`, `fill`, `copy_vram`, at physical addresses (`0xA0000`, `0xB8000`, …).
2. **BIOS**: `set_mode` (`INT 10h`), `vbe_set_mode`, `vbe_set_bank`,
   `vbe_set_display_start`, `bios_print` (teletype with scrolling), `load_font`, `getch` /
   `kbhit` (`INT 16h`), `mouse()` (`INT 33h`), `set_timer_hz` / `take_timer_ticks` (the PIT),
   `exit` (`INT 21h, 4Ch`).
3. **Register helpers**: `set_palette`, `cycle_palette`, `fade_palette`, `seq`, `gc`, `crtc`,
   `attr`, `map_mask`, `write_mode`, `bit_mask`, `set_start_address`, `set_offset`,
   `set_line_compare`, `set_pel_pan`, `set_border`, `screen_on`.
4. **Drawing**: `pset` (any graphics mode, in that mode's memory layout), `pset13`, `blit13`,
   `planar_pset`, `planar_fill_rect`, `planar_blit`, `cga_blit`, `tandy_blit`, `herc_pset`,
   `vbe_pset_banked`, `vbe_pset_lfb`, `vbe_write_banked`, `ModeX` (set, pset, fill_rect, blit,
   VRAM-to-VRAM latch copy, flip), text helpers (`text_print`, `text_box`, `set_cursor`,
   `set_blink`, …).
5. **`Bitmap`**: an 8-bit back buffer in system memory with lines, rects, circles, blits and
   bitmap-font text, for double buffering the way most games did.

Keyboard input comes both ways: `dos.key(sc::LEFT)` is the held-key table a custom `INT 9`
handler would fill, and `getch()` / `kbhit()` is the BIOS buffer.

## Running

`run::<G>(DosConfig)` opens a game-toolkit window. `DosGame::step` is one iteration of your
main loop; the host calls it as often as emulated time allows, so a loop that waits for the
retrace once runs at the mode's refresh rate (70 Hz in mode 13h, 60 Hz in Mode X, ~50 Hz on a
Hercules card). Alt+Enter toggles fullscreen. `DosConfig` picks the aspect (`Aspect::Crt` by
default, so the active area is 4:3 and mode 13h pixels are taller than wide), the overscan
border, integer scaling and a scanline effect.

To put a DOS screen into your own `Game`, use `DosScreen` directly: `feed_input`, then step
your `Dos`, `upload` and `draw`.

Headless, for tests and screenshots: `DOSVIDEO_SHOT=out.png DOSVIDEO_SHOT_FRAMES=120
cargo run --example copper_bars` writes the frame as PNG instead of opening a window, and
`DOSVIDEO_KEYS=30:0x30,90:0x24` taps keys (by scancode) at given frames. `dos.frame()`
gives the last complete frame as RGBA anywhere, with `to_png()`.

Without the default `toolkit` feature the crate has no dependencies at all: just the machine,
for tests or for driving it from another frontend.

## Demos

```sh
task examples            # list them
task run EXAMPLE=copper_bars
task shots               # screenshot every demo headless into target/shots/
```

| Demo | Shows |
|------|-------|
| `copper_bars` | Mode 13h raster bars (DAC entry 0 rewritten on every scanline, into the border too), palette cycling, a sine scroller copied as a band during vblank. `J` sloppy raster timing. |
| `palette_cycling` | Animation by palette rotation alone: a landscape with flowing water, lake ripples and a pulsing sun; a plasma; fades and flashes; and on an ISA8 bus, the seam left when DAC writes happen mid-frame (`T`). The overlay counts VRAM bytes per frame: 0. |
| `modex_scroll` | Mode X 320x240: a 448-pixel-wide virtual screen with smooth scrolling (start address + pel panning), a split-screen status bar via line compare, and page-flipped sprites. `T` draws without flipping, `R` shows why pel-panning reset below the split matters. |
| `tearing_lab` | A full mode 13h copy every frame. `1`-`5` pick the bus (ISA8 to PCI), `S` picks no wait / vsync / racing the beam. Shows why ISA tears and local bus doesn't. |
| `ega_scroller` | EGA mode 0Dh side-scroller: virtual screen via the offset register, per-pixel scrolling, tiles copied with latches (write mode 1), day/night via the attribute palette. `J` shows the jitter from setting pan and start in the wrong order. |
| `cga_showcase` | Five scenes: mode 4 palettes, intensity and background; mode 5; mode 6 patterns on RGB vs. composite (artifact colors, hue knob); the 160x100x16 tweak; 80-column snow and the retrace-wait cure. |
| `tandy` | Mode 9 320x200x16 with dirty-row updates in the 4-bank interleave, gate-array palette effects, and modes 8 and 0Ah. |
| `hercules` | 720x348 wireframes with two-page flipping on the inverted retrace bit, phosphor colors, and MDA text attributes with the 9th-column rule. |
| `svga` | VBE 640x480x8 fill benchmark (banked by pixel, column or row vs. LFB, with bank-switch counts), 640x480 hi-color, and 800x600 panning over a 1600x1200 virtual screen. |
| `text_mode` | An 80x25 TUI: boxes and shadows, blink vs. bright backgrounds, cursor, a custom tile character set loaded into plane 2 (with animated water), and 80x50. |

## Accuracy

The goal is "feels like the hardware, behaves like the hardware where a program can tell", not
cycle accuracy. Simplifications worth knowing:

- Memory writes become visible to the beam immediately; the CRTC's character-clock prefetch
  is not modeled.
- 6845 boards (CGA, Tandy, Hercules) have no blanking registers, so their border width is an
  approximation.
- CGA composite output is a straightforward NTSC encode/decode of RGBI colors, not a model of
  the CGA's chroma circuitry; tune it with `cga_set_composite_hue`.
- EGA and VGA 8-bit mode pass both nibbles through the attribute palette, as the hardware
  does; per-card quirks of SVGA chipsets are not modeled (it is a generic VBE 2.0 card).
- There is no interrupt-driven code: instead of a timer ISR, count ticks with
  `take_timer_ticks()`, and instead of raster IRQs, wait for the beam.

## Tasks

`task` (go-task, `Taskfile.dist.yaml`) wraps the usual: `build`, `run`, `test`, `lint`,
`check` (fmt, clippy with and without `toolkit`, wasm32), `ci`, `docs`, `shots`, `font`.

## License

EUPL-1.2. The 8x16 font in `crates/dosvideo/fonts/` is derived from `u_vga16.bdf` by Dmitry
Bolkhovityanov (MIT, see `fonts/LICENSE-vga16.txt`); the 26 glyphs it lacks were drawn for
this project.
