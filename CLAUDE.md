# CLAUDE.md - working on game-toolkit-dosvideo

An extension crate for [game-toolkit](../game-toolkit) (crates.io `game-toolkit = "0.11"`):
DOS display adapters emulated at register level with a lazily painted, beam-accurate CRT.
User-facing docs: `README.md`; API docs in `crates/dosvideo/src/lib.rs`.

## Layout (`crates/dosvideo/src`)

| File | Role |
|------|------|
| `crt.rs` | `Timing`, `Beam`, `Crt` (time→beam, lazy `advance` painting, front/back frame swap at vblank start), `Scanout` trait |
| `machine.rs` | `Dos`: clock, `BusTiming` cost model, ports/memory dispatch, waits, BIOS mode set, VBE calls, PIT, input state |
| `vga.rs` | EGA/VGA core: planes, sequencer, GC (latches, write/read modes), CRTC-derived timing, attribute ctrl, DAC, mode tables |
| `cga.rs` | `Mc6845` + CGA/Tandy (snow, composite decode, gate array) |
| `hercules.rs`, `svga.rs` | Hercules; VBE 2.0 card wrapping a `Vga` |
| `draw.rs`, `text.rs` | helpers on `Dos` (palette, registers, pset per layout, `ModeX`, `Bitmap`, text mode) |
| `input.rs` | scancodes, BIOS keyboard buffer, INT 33h mouse |
| `host.rs` | `toolkit` feature: `run`, `DosGame`, `DosScreen`, key map, headless screenshots |
| `font.rs`, `png.rs`, `palette.rs` | CP437 fonts, PNG encoder, standard palettes |

## Invariants

- **Every observable action syncs first.** Port writes, memory writes and waits call
  `Dos::sync()` (paint up to `now`) *before* changing card state; otherwise changes leak
  backwards in time on screen. A new mutating path on `Dos` must do the same, and must call
  `check_timing()` after anything that can change CRTC timing.
- **I/O always costs time** (`MIN_IO_NS`), so status-port polling loops terminate.
- **No special cases for tweaked modes**: timing and addressing come from the registers.
  If a mode looks wrong, fix the register table or the derivation, not the renderer.
- The core (`--no-default-features`) has zero dependencies and must stay that way; it must
  also build for `wasm32-unknown-unknown`.
- `#![forbid(unsafe_code)]`.

## Checking work

- `task test` (unit + `tests/hardware.rs` + doctests), `task check` (fmt, clippy with and
  without `toolkit`, wasm32), `task docs`.
- Visual checks without a window: `DOSVIDEO_SHOT=x.png DOSVIDEO_SHOT_FRAMES=N
  DOSVIDEO_KEYS=frame:0xSC,... cargo run --release --example NAME`, or `task shots`.
- Write a test for each hardware behavior you add, against the public API, in the style of
  `tests/hardware.rs` (render frames, sample pixels).

## Releasing

Tag-driven, see `release.toml` and `.github/workflows/release.yml`:

- Write the release's notes under `## [Unreleased]` in `CHANGELOG.md`.
- `task release VERSION=x.y.z` (`cargo release x.y.z --execute --no-publish`): bumps the
  version, stamps the changelog, commits, tags `vx.y.z` and pushes.
- The tag runs the release workflow: it publishes to crates.io through trusted publishing
  (OIDC, environment `crates.io`, restricted to `v*` tags), skipping versions that are already
  published, and creates the GitHub release from the changelog section.
- Workflows are linted by actionlint and zizmor (`task lint:actions`, and in CI); actions are
  pinned to commit SHAs, Dependabot keeps them current.
