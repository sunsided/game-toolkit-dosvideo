//! Behavior a DOS program can observe, checked through the public API.

use game_toolkit_dosvideo::prelude::*;
use game_toolkit_dosvideo::{FrameView, Video};

fn instant(adapter: Adapter) -> Dos {
    Dos::new(MachineConfig::new(adapter).with_bus(BusTiming::INSTANT))
}

/// RGB of active-area dot `(x, line)` in the last finished frame.
fn px(f: &FrameView<'_>, x: u32, line: u32) -> [u8; 3] {
    let i = (((f.active[1] + line) * f.width + f.active[0] + x) * 4) as usize;
    [f.rgba[i], f.rgba[i + 1], f.rgba[i + 2]]
}

fn full_frame(dos: &mut Dos) {
    dos.wait_vretrace();
    dos.wait_vretrace();
}

#[test]
fn start_address_latches_at_vertical_retrace() {
    let mut d = instant(Adapter::Vga);
    d.set_mode(0x13);
    // Row 0 red, row 1 green.
    d.fill(0xA0000, 320, 4);
    d.fill(0xA0000 + 320, 320, 2);
    full_frame(&mut d);
    assert_eq!(px(&d.frame(), 0, 0), [0xAA, 0, 0]);
    // Mid-frame the new start address (one row down = 80 units) must not show yet...
    d.wait_line(100);
    d.set_start_address(80);
    d.wait_line(300);
    d.wait_vblank();
    assert_eq!(px(&d.frame(), 0, 0), [0xAA, 0, 0]);
    // ...but from the next frame on it does.
    full_frame(&mut d);
    assert_eq!(px(&d.frame(), 0, 0), [0, 0xAA, 0]);
}

#[test]
fn line_compare_splits_the_screen() {
    let mut d = instant(Adapter::Vga);
    d.set_mode(0x13);
    d.fill(0xA0000, 64000, 1); // blue everywhere
    d.fill(0xA0000, 320, 4); // row 0 red
    d.set_start_address(320 * 50 / 4);
    d.set_line_compare(299);
    full_frame(&mut d);
    let f = d.frame();
    assert_eq!(px(&f, 0, 0), [0, 0, 0xAA], "top shows memory from row 50");
    assert_eq!(
        px(&f, 0, 300),
        [0xAA, 0, 0],
        "below the split: memory from address 0"
    );
    assert_eq!(px(&f, 0, 299), [0, 0, 0xAA]);
}

#[test]
fn mode_x_planes_and_pages() {
    let mut d = instant(Adapter::Vga);
    let mut mx = ModeX::set(&mut d, 240).expect("mode x");
    assert_eq!(d.resolution(), (320, 240));
    assert!((d.refresh_hz() - 60.0).abs() < 0.5, "{}", d.refresh_hz());
    mx.active = 0;
    mx.pset(&mut d, 5, 0, 4);
    mx.fill_rect(&mut d, 1, 1, 6, 1, 2);
    full_frame(&mut d);
    let f = d.frame();
    // 320 pixels on 640 dots, 2 scanlines per row.
    assert_eq!(px(&f, 10, 0), [0xAA, 0, 0]);
    assert_eq!(px(&f, 8, 0), [0, 0, 0]);
    assert_eq!(px(&f, 2, 2), [0, 0xAA, 0]);
    assert_eq!(px(&f, 12, 2), [0, 0xAA, 0]);
    assert_eq!(px(&f, 14, 2), [0, 0, 0]);
    // Page 1 is not visible until flipped.
    mx.active = 1;
    mx.clear(&mut d, 1);
    full_frame(&mut d);
    assert_eq!(px(&d.frame(), 100, 100), [0, 0, 0]);
    mx.flip(&mut d);
    full_frame(&mut d);
    assert_eq!(px(&d.frame(), 100, 100), [0, 0, 0xAA]);
}

#[test]
fn unchaining_mode_13h_shows_the_gaps() {
    // Chained, pixel x sits in plane x&3 at address x&!3; unchained, the CRTC reads plane
    // addresses consecutively, so the old picture shows up spread out with holes.
    let mut d = instant(Adapter::Vga);
    d.set_mode(0x13);
    d.fill(0xA0000, 8, 4);
    d.seq(4, 0x06);
    d.crtc(0x14, 0x00);
    d.crtc(0x17, 0xE3);
    full_frame(&mut d);
    let f = d.frame();
    let row: Vec<bool> = (0..16).map(|x| px(&f, x * 2, 0) == [0xAA, 0, 0]).collect();
    assert_eq!(
        &row[..8],
        &[true, true, true, true, false, false, false, false]
    );
    assert_eq!(&row[8..12], &[false; 4]);
    assert_eq!(&row[12..16], &[false; 4]);
    let red = (0..80).filter(|x| px(&f, x * 2, 0) == [0xAA, 0, 0]).count();
    assert_eq!(red, 8, "two plane addresses with four pixels each");
}

#[test]
fn attribute_index_without_pas_blanks_the_screen() {
    let mut d = instant(Adapter::Vga);
    d.set_mode(0x13);
    d.fill(0xA0000, 64000, 15);
    d.inb(0x3DA);
    d.outb(0x3C0, 0x11); // index without bit 5
    full_frame(&mut d);
    assert_eq!(px(&d.frame(), 10, 10), [0, 0, 0]);
    d.outb(0x3C0, 0x00);
    d.outb(0x3C0, 0x20);
    full_frame(&mut d);
    assert_eq!(px(&d.frame(), 10, 10), [0xFF, 0xFF, 0xFF]);
}

#[test]
fn cga_snow_only_during_display() {
    let mut d = Dos::new(MachineConfig::new(Adapter::Cga(CgaMonitor::Rgb)));
    d.set_mode(3);
    d.wait_frame_start();
    for i in 0..400u32 {
        d.poke(0xB8000 + (i % 2000) * 2, 0xB1);
    }
    full_frame(&mut d);
    // A clean frame after writes stopped...
    let clean = d.frame().rgba.to_vec();
    // ...vs. the frame during which we wrote.
    d.wait_frame_start();
    for i in 0..400u32 {
        d.poke(0xB8000 + (i % 2000) * 2 + 1, 0x07);
    }
    d.wait_vblank();
    let snowy = d.frame().rgba.to_vec();
    assert_ne!(clean, snowy, "writes during display leave snow");

    let mut d = Dos::new(MachineConfig::new(Adapter::Cga(CgaMonitor::Rgb)));
    d.set_mode(3);
    full_frame(&mut d);
    let before = d.frame().rgba.to_vec();
    d.wait_vretrace();
    for _ in 0..20 {
        d.poke(0xB8000 + 3998, 0x20); // same char, during retrace
    }
    d.wait_vblank();
    assert_eq!(before, d.frame().rgba.to_vec(), "no snow during retrace");
}

#[test]
fn hercules_retrace_bit_is_inverted() {
    let mut d = instant(Adapter::Hercules(Phosphor::Amber));
    d.wait_vretrace();
    assert_eq!(d.inb(0x3BA) & 0x80, 0);
    d.wait_frame_start();
    assert_eq!(d.inb(0x3BA) & 0x80, 0x80);
    assert_eq!(d.inb(0x3DA), 0xFF, "no CGA status port on a Hercules card");
}

#[test]
fn mouse_in_mode_13h_reports_doubled_x() {
    let mut d = instant(Adapter::Vga);
    d.set_mode(0x13);
    d.mouse_mut().set_normalized(1.0, 1.0);
    assert_eq!((d.mouse().x, d.mouse().y), (639, 199));
    assert_eq!(d.mouse_pixel(), (319, 199));
}

#[test]
fn text_mode_shows_characters() {
    let mut d = instant(Adapter::Vga);
    d.set_mode(0x03);
    d.hide_cursor();
    d.text_put(0, 0, 0xDB, 0x0E); // full block, yellow
    full_frame(&mut d);
    let f = d.frame();
    assert_eq!(px(&f, 0, 0), [0xFF, 0xFF, 0x55]);
    assert_eq!(px(&f, 7, 15), [0xFF, 0xFF, 0x55]);
    // 0xDB is in the line-graphics range C0-DF: the 9th column repeats the 8th, so blocks
    // tile without gaps. A shade character (0xB1) keeps the blank 9th column.
    assert_eq!(px(&f, 8, 5), [0xFF, 0xFF, 0x55]);
    d.text_put(1, 0, 0xB1, 0x0E);
    full_frame(&mut d);
    assert_eq!(px(&d.frame(), 9 + 8, 5), [0, 0, 0]);
    assert!(matches!(d.video(), Video::Vga(_)));
}

#[test]
fn ega_200_line_palette_is_rgbi() {
    let mut d = instant(Adapter::Ega);
    assert!(d.set_mode(0x0D));
    d.planar_fill_rect(0, 0, 8, 1, 1);
    // 0x09 is blue plus secondary blue: on a 350-line monitor light blue, but a 200-line
    // monitor only has RGBI inputs, ignores the secondary blue bit and shows plain blue.
    d.set_attr_palette(1, 0x09);
    full_frame(&mut d);
    assert_eq!(px(&d.frame(), 0, 0), [0, 0, 0xAA]);
}

#[test]
fn bitmap_clips_offscreen_rects() {
    let mut b = Bitmap::new(320, 200);
    b.fill_rect(330, 0, 14, 200, 1);
    b.fill_rect(-50, -50, 10, 10, 1);
    b.fill_rect(310, 190, 50, 50, 2);
    assert_eq!(b.get(319, 199), 2);
    assert_eq!(b.data.iter().filter(|&&p| p == 1).count(), 0);
}

#[test]
fn dithering_mixes_two_colors_for_a_middle_gray() {
    use game_toolkit_dosvideo::draw::Dither;
    let pal = [[0u8, 0, 0], [255, 255, 255]];
    let rgba: Vec<u8> = (0..16 * 16).flat_map(|_| [128u8, 128, 128, 255]).collect();
    for d in [Dither::Bayer4, Dither::Bayer8, Dither::FloydSteinberg] {
        let b = Bitmap::from_rgba(16, 16, &rgba, &pal, d);
        let white = b.data.iter().filter(|&&p| p == 1).count();
        assert!((96..=160).contains(&white), "{d:?}: {white} of 256 white");
    }
    let flat = Bitmap::from_rgba(16, 16, &rgba, &pal, Dither::None);
    assert!(flat.data.iter().all(|&p| p == flat.data[0]));
}

#[test]
fn standard_vga_modes_have_a_centered_border() {
    // Blanking lags the CRTC compare by one character clock, so the overscan border is
    // equal on both sides (8 or 9 dots), not all on the left.
    let mut d = instant(Adapter::Vga);
    for (mode, side) in [(0x03, 9), (0x10, 8), (0x12, 8), (0x13, 8)] {
        d.set_mode(mode);
        full_frame(&mut d);
        let f = d.frame();
        let right = f.width - f.active[0] - f.active[2];
        assert_eq!((f.active[0], right), (side, side), "mode {mode:02X}h");
    }
}
