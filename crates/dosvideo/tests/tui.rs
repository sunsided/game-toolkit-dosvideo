//! The ratatui backend, checked through the public API: widgets end up as CP437 cells and
//! attribute bytes in text memory.
#![cfg(feature = "ratatui")]

use game_toolkit_dosvideo::prelude::*;
use game_toolkit_dosvideo::tui::{self, DosBackend};
use ratatui::Terminal;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, BorderType, Paragraph};

fn text_mode() -> Dos {
    let mut d = Dos::new(MachineConfig::new(Adapter::Vga).with_bus(BusTiming::INSTANT));
    d.set_mode(0x03);
    d
}

#[test]
fn widgets_become_text_cells() {
    let mut d = text_mode();
    let mut term = Terminal::new(DosBackend::new(&d)).unwrap();
    tui::draw(&mut term, &mut d, |f| {
        let block = Block::bordered()
            .border_type(BorderType::Double)
            .style(Style::new().fg(Color::LightYellow).bg(Color::Blue));
        f.render_widget(Paragraph::new("Hi").block(block), Rect::new(2, 1, 6, 3));
    });
    assert_eq!(d.text_get(2, 1), (0xC9, 0x1E)); // ╔, yellow on blue
    assert_eq!(d.text_get(3, 2), (b'H', 0x1E));
    assert_eq!(d.text_get(7, 3), (0xBC, 0x1E)); // ╝
    assert_eq!(d.text_get(0, 0), (b' ', 0x07)); // Reset: light gray on black

    // A second frame writes only what changed, and the change shows.
    tui::draw(&mut term, &mut d, |f| {
        f.render_widget(Paragraph::new("Yo"), Rect::new(3, 2, 2, 1));
    });
    assert_eq!(d.text_get(3, 2), (b'Y', 0x07));
}

#[test]
fn terminal_follows_the_text_mode_size() {
    let mut d = text_mode();
    let mut term = Terminal::new(DosBackend::new(&d)).unwrap();
    let mut area = Rect::default();
    tui::draw(&mut term, &mut d, |f| area = f.area());
    assert_eq!((area.width, area.height), (80, 25));
    d.crtc(0x09, 0x47); // max scan line 7: 50 rows
    tui::draw(&mut term, &mut d, |f| {
        area = f.area();
        f.render_widget(Paragraph::new("bottom"), Rect::new(0, 49, 6, 1));
    });
    assert_eq!((area.width, area.height), (80, 50));
    assert_eq!(d.text_get(0, 49).0, b'b');
}
