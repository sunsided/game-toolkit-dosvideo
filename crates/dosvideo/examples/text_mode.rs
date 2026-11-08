//! VGA text mode as a user interface *and* a graphics mode.
//!
//! An 80x25 screen of character/attribute cells at `B800:0000`: a menu bar, windows drawn with
//! the CP437 box characters, drop shadows (just the attribute of the cells next to a window
//! set to dark gray), a scrolling log, and the hardware cursor. The windows are ratatui
//! widgets, drawn through `tui::DosBackend` (`ratatui` feature): ratatui diffs each frame
//! against the last one and only the changed cells go to text memory.
//!
//! The graphics trick: the VGA character generator reads its glyphs from plane 2 of video
//! memory, so a program can redefine characters. Text-mode games replaced rarely used
//! characters (here `Ç ü é â ä à`) with tiles: a brick wall, a tree, a coin, a hero, and water
//! whose glyph is rewritten every few frames, which animates every water cell on screen at
//! once without touching the text buffer.
//!
//! Attribute bit 7 either blinks the character or, with blinking off, selects one of 16
//! background colors instead of 8.
//!
//! The same trick can swap the whole alphabet: `G` loads the Standard Galactic Alphabet, the
//! alien script from the Commander Keen games, and every letter on screen turns galactic at
//! once (digits, punctuation and box drawing stay).
//!
//! The windows are real windows: drag them by the title bar with the mouse, click one to bring
//! it to the front (or `Tab`), and open the menus with a click or `F10`. The mouse pointer is
//! the `INT 33h` text cursor, an inverted cell.
//!
//! Keys: arrows move the hero, `F10` menus, `Tab` next window, `B` blink vs. bright
//! backgrounds, `F` custom font on/off, `G` galactic alphabet, `H` 80x25 / 80x50 (max scan
//! line 7 and the 8x8 font), `Esc` quit.

use game_toolkit_dosvideo::KeyPress;
use game_toolkit_dosvideo::prelude::*;
use game_toolkit_dosvideo::tui::{self, DosBackend};
use ratatui::Terminal;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};

const WALL: u8 = 0x80;
const TREE: u8 = 0x81;
const WATER: u8 = 0x82;
const HERO: u8 = 0x83;
const COIN: u8 = 0x84;
const GRASS: u8 = 0x85;

const MAP: [&str; 9] = [
    "##################################",
    "#..T...~~~~..........T....$......#",
    "#.....~~~~~~....###.......T......#",
    "#..$...~~~~.....#$#..........~~~.#",
    "#.T.............#.#....T....~~~~.#",
    "#.......T..........$........~~~..#",
    "#...###########...........T......#",
    "#.$............T......$..........#",
    "##################################",
];

fn tile(rows16: [u16; 16]) -> [u8; 16] {
    rows16.map(|r| r as u8)
}

/// Tile glyphs, 8x16. Written as binary rows.
fn tiles(water_phase: u32) -> Vec<(u8, [u8; 16])> {
    let wall = tile([
        0xFF, 0x08, 0x08, 0x08, 0xFF, 0x80, 0x80, 0x80, 0xFF, 0x08, 0x08, 0x08, 0xFF, 0x80, 0x80,
        0x80,
    ]);
    let tree = tile([
        0x18, 0x3C, 0x7E, 0x7E, 0xFF, 0xFF, 0xFF, 0x7E, 0x7E, 0x3C, 0x18, 0x18, 0x18, 0x18, 0x3C,
        0x00,
    ]);
    let hero = tile([
        0x00, 0x3C, 0x7E, 0x5A, 0x7E, 0x66, 0x3C, 0x18, 0xFF, 0x3C, 0x3C, 0x3C, 0x24, 0x24, 0x66,
        0x00,
    ]);
    let coin = tile([
        0x00, 0x00, 0x00, 0x3C, 0x7E, 0x66, 0xDB, 0xDB, 0xDB, 0xDB, 0x66, 0x7E, 0x3C, 0x00, 0x00,
        0x00,
    ]);
    let grass = tile([
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x04, 0x00,
        0x00,
    ]);
    // Water: a wave pattern, shifted sideways with the phase.
    let mut water = [0u8; 16];
    for (r, w) in water.iter_mut().enumerate() {
        let base: u8 = match r % 8 {
            1 => 0b0011_0000,
            2 => 0b0100_1000,
            3 => 0b1000_0110,
            5 => 0b0000_0011,
            6 => 0b0000_1100,
            _ => 0,
        };
        *w = base.rotate_right(water_phase % 8);
    }
    vec![
        (WALL, wall),
        (TREE, tree),
        (WATER, water),
        (HERO, hero),
        (COIN, coin),
        (GRASS, grass),
    ]
}

/// Build the font: VGA 8x16 or CGA 8x8 with the tiles patched in.
fn make_font(tall: bool, custom: bool, galactic: bool, phase: u32) -> Font {
    let base = match (tall, galactic) {
        (true, false) => Font::vga(),
        (false, false) => Font::cga(),
        (true, true) => Font::sga(),
        (false, true) => Font::sga_cga(),
    };
    let h = base.height() as usize;
    let mut raw = base.raw().to_vec();
    if custom {
        for (ch, g) in tiles(phase) {
            for r in 0..h {
                // 8-line font: take every other row of the 16-line tile.
                raw[ch as usize * h + r] = if tall { g[r] } else { g[r * 2] | g[r * 2 + 1] };
            }
        }
    }
    Font::from_raw(h as u8, &raw).expect("font size")
}

/// The tile characters as Unicode (CP437 0x80..0x85), the form ratatui cells hold them in.
fn ch(code: u8) -> &'static str {
    ["Ç", "ü", "é", "â", "ä", "à"][(code - 0x80) as usize]
}

/// The 16 text-mode colors in attribute order.
const IBM: [Color; 16] = [
    Color::Black,
    Color::Blue,
    Color::Green,
    Color::Cyan,
    Color::Red,
    Color::Magenta,
    Color::Yellow, // brown
    Color::Gray,
    Color::DarkGray,
    Color::LightBlue,
    Color::LightGreen,
    Color::LightCyan,
    Color::LightRed,
    Color::LightMagenta,
    Color::LightYellow,
    Color::White,
];

fn style(fg: Color, bg: Color) -> Style {
    Style::new().fg(fg).bg(bg)
}

/// Fill an area with one character.
fn fill(buf: &mut Buffer, area: Rect, sym: &str, st: Style) {
    for p in area.positions() {
        buf[p].set_symbol(sym).set_style(st);
    }
}

/// Drop shadow: the cells right of and below a window keep their character but turn dark gray
/// on black.
fn shadow(buf: &mut Buffer, r: Rect) {
    let right = Rect::new(r.right(), r.y + 1, 2, r.height);
    let below = Rect::new(r.x + 2, r.bottom(), r.width, 1);
    for area in [right, below] {
        for p in area.intersection(buf.area).positions() {
            buf[p].set_style(style(Color::DarkGray, Color::Black));
        }
    }
}

/// A window: cleared interior, frame (double when focused), centered title, shadow. Returns
/// the inner area.
fn window(buf: &mut Buffer, r: Rect, st: Style, title: &str, focused: bool) -> Rect {
    Clear.render(r, buf);
    let block = Block::bordered()
        .border_type(if focused {
            BorderType::Double
        } else {
            BorderType::Plain
        })
        .title(Line::from(format!(" {title} ")).centered())
        .style(st);
    let inner = block.inner(r);
    block.render(r, buf);
    shadow(buf, r);
    inner
}

/// What a menu item or a hot key does.
#[derive(Clone, Copy)]
enum Act {
    Quit,
    ClearLog,
    Rows,
    Blink,
    Font,
    Galactic,
    Keys,
}

const MENUS: [(&str, &[(&str, Act)]); 5] = [
    ("File", &[("Exit             Esc", Act::Quit)]),
    ("Edit", &[("Clear log", Act::ClearLog)]),
    (
        "View",
        &[
            ("80x25 / 80x50      H", Act::Rows),
            ("Blink / bright bg  B", Act::Blink),
        ],
    ),
    (
        "Options",
        &[
            ("Custom tiles       F", Act::Font),
            ("Galactic alphabet  G", Act::Galactic),
        ],
    ),
    ("Help", &[("Keys", Act::Keys)]),
];

/// Column of menu title `i` on the menu bar.
fn menu_x(i: usize) -> u16 {
    2 + MENUS[..i]
        .iter()
        .map(|(n, _)| n.len() as u16 + 3)
        .sum::<u16>()
}

/// The drop-down box of menu `m`.
fn dropdown(m: usize) -> Rect {
    let items = MENUS[m].1;
    let w = items.iter().map(|(s, _)| s.len()).max().unwrap_or(0) as u16 + 4;
    Rect::new(menu_x(m) - 1, 1, w, items.len() as u16 + 2)
}

const TITLES: [&str; 3] = ["Character set", "Attributes", "Log"];
const MAP_W: u16 = 34;
const MAP_H: u16 = 9;

struct TextUi {
    term: Terminal<DosBackend>,
    blink: bool,
    custom: bool,
    galactic: bool,
    tall: bool,
    /// Window frames, by id: map, attributes, log.
    wins: [Rect; 3],
    /// Stacking order, bottom to top; the top window has the focus.
    z: Vec<usize>,
    /// Window being dragged by its title bar, and where on the bar it was grabbed.
    drag: Option<(usize, u16)>,
    /// Open menu and highlighted item.
    menu: Option<(usize, usize)>,
    /// The hero, in map cells.
    hero: (u16, u16),
    log: Vec<String>,
    t: u32,
    phase: u32,
}

impl TextUi {
    fn rows(dos: &Dos) -> u16 {
        dos.text_size().1 as u16
    }

    fn top(&self) -> usize {
        self.z[self.z.len() - 1]
    }

    fn raise(&mut self, id: usize) {
        self.z.retain(|&w| w != id);
        self.z.push(id);
    }

    /// Keep a window between the menu bar and the status line.
    fn place(r: Rect, x: i32, y: i32, rows: u16) -> Rect {
        let x = x.clamp(0, 80 - r.width as i32) as u16;
        let y = y.clamp(1, (rows - 1 - r.height) as i32) as u16;
        Rect { x, y, ..r }
    }

    /// Screen position of the hero (the map sits one cell inside the window's frame).
    fn hero_pos(&self) -> Position {
        let r = self.wins[0];
        Position::new(r.x + 2 + self.hero.0, r.y + 2 + self.hero.1)
    }

    /// Render the whole screen into ratatui's buffer; the backend writes what changed.
    fn draw(&mut self, dos: &mut Dos) {
        let rows = Self::rows(dos);
        let mouse = tui::mouse_cell(dos);
        let (blink, custom, tall, menu) = (self.blink, self.custom, self.tall, self.menu);
        let (wins, z, top, hero) = (self.wins, self.z.clone(), self.top(), self.hero_pos());
        let log = &self.log;
        tui::draw(&mut self.term, dos, |f| {
            let area = f.area();
            let buf = f.buffer_mut();
            // Desktop: the classic shade pattern.
            fill(buf, area, "▒", style(Color::Blue, Color::Gray));

            // Windows, bottom to top.
            for &id in &z {
                match id {
                    0 => {
                        let a = style(Color::LightYellow, Color::Blue);
                        let inner = window(buf, wins[0], a, TITLES[0], id == top);
                        for (y, row) in MAP.iter().enumerate() {
                            for (x, c) in row.chars().enumerate() {
                                let (code, st) = match c {
                                    '#' => (WALL, style(Color::LightRed, Color::Red)),
                                    'T' => (TREE, style(Color::LightGreen, Color::Green)),
                                    '~' => (WATER, style(Color::LightCyan, Color::Blue)),
                                    '$' => (COIN, style(Color::LightYellow, Color::Green)),
                                    _ => (GRASS, style(Color::LightGreen, Color::Green)),
                                };
                                let p =
                                    Position::new(inner.x + 1 + x as u16, inner.y + 1 + y as u16);
                                buf[p].set_symbol(ch(code)).set_style(st);
                            }
                        }
                        buf[hero]
                            .set_symbol(ch(HERO))
                            .set_style(style(Color::White, Color::Green));
                    }
                    1 => {
                        // 8 backgrounds x 8 foregrounds, and again with bit 7 set (blinking,
                        // or with blinking off a bright background).
                        let a = style(Color::Black, Color::Cyan);
                        let inner = window(buf, wins[1], a, TITLES[1], id == top);
                        for bg in 0..8u16 {
                            for fg in 0..8u16 {
                                let st = style(IBM[fg as usize * 2 + 1], IBM[bg as usize]);
                                let c = char::from(b'A' + fg as u8).to_string();
                                let (x, y) = (inner.x + 1 + fg * 2, inner.y + bg);
                                buf[(x, y)].set_symbol(&c).set_style(st);
                                buf[(x + 16, y)]
                                    .set_symbol(&c)
                                    .set_style(st.add_modifier(Modifier::SLOW_BLINK));
                            }
                        }
                        let caption =
                            Rect::new(inner.x + 1, inner.bottom() - 1, inner.width - 1, 1);
                        Line::from("bit 7 off      bit 7 on").render(caption, buf);
                    }
                    _ => {
                        let a = style(Color::LightGreen, Color::Black);
                        let inner = window(buf, wins[2], a, TITLES[2], id == top);
                        let skip = log.len().saturating_sub(inner.height as usize);
                        let lines: Vec<Line<'_>> =
                            log[skip..].iter().map(|l| Line::raw(l.as_str())).collect();
                        Paragraph::new(lines).render(inner, buf);
                    }
                }
            }

            // Menu bar, hot keys in red, the open menu inverted.
            let bar = style(Color::Black, Color::Gray);
            fill(buf, Rect::new(0, 0, area.width, 1), " ", bar);
            for (i, (name, _)) in MENUS.iter().enumerate() {
                let open = menu.is_some_and(|(m, _)| m == i);
                let s = if open {
                    style(Color::White, Color::Black)
                } else {
                    bar
                };
                let (first, rest) = name.split_at(1);
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(
                        first,
                        s.fg(if open { Color::LightYellow } else { Color::Red }),
                    ),
                    Span::raw(rest),
                    Span::raw(" "),
                ])
                .style(s)
                .render(Rect::new(menu_x(i) - 1, 0, name.len() as u16 + 2, 1), buf);
            }
            Line::styled("DOSVIDEO 1.0", bar).render(Rect::new(64, 0, 12, 1), buf);

            // Status line.
            let status = format!(
                " F10 menu  Tab window  B blink:{}  F font:{}  G alien  H {}  Esc quit",
                if blink { "on " } else { "off" },
                if custom { "on " } else { "off" },
                if tall { "80x25" } else { "80x50" },
            );
            Paragraph::new(status)
                .style(style(Color::Black, Color::Cyan))
                .render(Rect::new(0, rows - 1, area.width, 1), buf);

            // The open drop-down.
            if let Some((m, sel)) = menu {
                let r = dropdown(m);
                Clear.render(r, buf);
                Block::bordered().style(bar).render(r, buf);
                for (i, (item, _)) in MENUS[m].1.iter().enumerate() {
                    let s = if i == sel {
                        style(Color::White, Color::Black)
                    } else {
                        bar
                    };
                    let row = Rect::new(r.x + 1, r.y + 1 + i as u16, r.width - 2, 1);
                    Paragraph::new(format!(" {item}")).style(s).render(row, buf);
                }
                shadow(buf, r);
            }

            // The hardware cursor marks the hero while the map has the focus.
            if top == 0 && menu.is_none() {
                f.set_cursor_position(hero);
            }
            tui::mouse_cursor(f.buffer_mut(), mouse);
        });
    }

    fn log(&mut self, s: String) {
        self.log.push(s);
        if self.log.len() > 100 {
            self.log.remove(0);
        }
    }

    fn apply_font(&self, dos: &mut Dos) {
        dos.load_font(
            &make_font(self.tall, self.custom, self.galactic, self.phase),
            0,
        );
    }

    fn act(&mut self, dos: &mut Dos, a: Act) {
        self.menu = None;
        match a {
            Act::Quit => dos.exit(),
            Act::ClearLog => self.log.clear(),
            Act::Blink => {
                self.blink = !self.blink;
                dos.set_blink(self.blink);
                self.log(format!(
                    "Attribute bit 7 now means {}",
                    if self.blink {
                        "blink"
                    } else {
                        "bright background"
                    }
                ));
            }
            Act::Font => {
                self.custom = !self.custom;
                self.apply_font(dos);
                self.log(format!(
                    "Custom tiles {}",
                    if self.custom {
                        "loaded"
                    } else {
                        "removed: the plain CP437 glyphs are back"
                    }
                ));
            }
            Act::Galactic => {
                self.galactic = !self.galactic;
                self.apply_font(dos);
                self.log(format!(
                    "Standard Galactic Alphabet {}",
                    if self.galactic { "loaded" } else { "unloaded" }
                ));
            }
            Act::Rows => {
                self.tall = !self.tall;
                self.log(format!(
                    "Switching to {}",
                    if self.tall {
                        "80x25 (8x16 font)"
                    } else {
                        "80x50 (8x8 font, max scan line 7)"
                    }
                ));
                // Max scan line: 16 lines per row for 80x25, 8 for 80x50. Bit 6 is bit 9 of
                // line compare, keep it. The display stays 400 scanlines tall either way. The
                // next draw sees the new row count and redraws everything.
                dos.crtc(0x09, if self.tall { 0x4F } else { 0x47 });
                self.apply_font(dos);
                let rows = Self::rows(dos);
                self.wins[2].height = rows - 18;
                for r in &mut self.wins {
                    *r = Self::place(*r, r.x as i32, r.y as i32, rows);
                }
            }
            Act::Keys => {
                for (k, d) in KEYS {
                    self.log(format!("{k:<8} {d}"));
                }
            }
        }
    }

    fn key(&mut self, dos: &mut Dos, k: KeyPress) {
        if let Some((m, i)) = self.menu {
            let n = MENUS[m].1.len();
            self.menu = match k.scan {
                sc::LEFT => Some(((m + MENUS.len() - 1) % MENUS.len(), 0)),
                sc::RIGHT => Some(((m + 1) % MENUS.len(), 0)),
                sc::UP => Some((m, (i + n - 1) % n)),
                sc::DOWN => Some((m, (i + 1) % n)),
                sc::ENTER => return self.act(dos, MENUS[m].1[i].1),
                sc::ESC | sc::F10 => None,
                _ => Some((m, i)),
            };
            return;
        }
        let (x, y) = self.hero;
        match k.scan {
            sc::ESC => dos.exit(),
            sc::F10 => self.menu = Some((0, 0)),
            sc::TAB => self.raise(self.z[0]),
            sc::UP => self.hero.1 = y.saturating_sub(1),
            sc::DOWN => self.hero.1 = (y + 1).min(MAP_H - 1),
            sc::LEFT => self.hero.0 = x.saturating_sub(1),
            sc::RIGHT => self.hero.0 = (x + 1).min(MAP_W - 1),
            sc::B => self.act(dos, Act::Blink),
            sc::F => self.act(dos, Act::Font),
            sc::G => self.act(dos, Act::Galactic),
            sc::H => self.act(dos, Act::Rows),
            _ => {}
        }
    }

    /// Left button went down at `p`.
    fn press(&mut self, dos: &mut Dos, p: Position) {
        let title = (0..MENUS.len()).find(|&i| {
            p.y == 0 && (menu_x(i) - 1..menu_x(i) + MENUS[i].0.len() as u16 + 1).contains(&p.x)
        });
        if let Some((m, _)) = self.menu {
            let r = dropdown(m);
            if r.contains(p) && p.y > r.y && p.y < r.bottom() - 1 {
                return self.act(dos, MENUS[m].1[(p.y - r.y - 1) as usize].1);
            }
            // Clicking the open title closes it, another title switches, elsewhere closes.
            self.menu = title.filter(|&t| t != m).map(|t| (t, 0));
            return;
        }
        if let Some(t) = title {
            self.menu = Some((t, 0));
            return;
        }
        // The topmost window under the pointer comes to the front; its title bar drags it,
        // a click on the map moves the hero there.
        let Some(&id) = self.z.iter().rev().find(|&&w| self.wins[w].contains(p)) else {
            return;
        };
        self.raise(id);
        let r = self.wins[id];
        if p.y == r.y {
            self.drag = Some((id, p.x - r.x));
        } else if id == 0 {
            let (mx, my) = (p.x.wrapping_sub(r.x + 2), p.y.wrapping_sub(r.y + 2));
            if mx < MAP_W && my < MAP_H {
                self.hero = (mx, my);
            }
        }
    }
}

impl DosGame for TextUi {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        dos.set_mode(0x03);
        let rows = Self::rows(dos);
        dos.mouse_mut().show(); // INT 33h function 01h: we draw the pointer, hide the OS one
        let mut ui = Self {
            term: Terminal::new(DosBackend::new(dos))?,
            blink: true,
            custom: true,
            galactic: false,
            tall: true,
            wins: [
                Rect::new(1, 2, 38, 12),
                Rect::new(42, 2, 36, 12),
                Rect::new(1, 15, 77, rows - 18),
            ],
            z: vec![2, 1, 0],
            drag: None,
            menu: None,
            hero: (7, 4),
            log: Vec::new(),
            t: 0,
            phase: 0,
        };
        ui.log("C:\\> DOSVIDEO.EXE".into());
        ui.log("VGA text mode 03h: 720x400, 9-dot characters, 70 Hz".into());
        ui.log("Font loaded into plane 2; characters 80h-85h redefined as tiles".into());
        ui.log("Windows drawn by ratatui; drag them by the title bar".into());
        ui.apply_font(dos);
        ui.draw(dos);
        Ok(ui)
    }

    fn step(&mut self, dos: &mut Dos) {
        dos.wait_vretrace();
        self.t += 1;
        while let Some(k) = dos.getch() {
            self.key(dos, k);
        }

        // Mouse: presses counted by the driver (none lost between frames), dragging while
        // the button is held, and the highlight follows the pointer in an open menu.
        let p = tui::mouse_cell(dos);
        if dos.mouse_mut().take_presses(0) > 0 {
            self.press(dos, p);
        }
        if !dos.mouse().left() {
            self.drag = None;
        }
        if let Some((id, grab)) = self.drag {
            let r = self.wins[id];
            self.wins[id] = Self::place(r, p.x as i32 - grab as i32, p.y as i32, Self::rows(dos));
        }
        if let Some((m, _)) = self.menu {
            let r = dropdown(m);
            if r.contains(p) && p.y > r.y && p.y < r.bottom() - 1 {
                self.menu = Some((m, (p.y - r.y - 1) as usize));
            }
        }

        // Animate the water: rewrite one glyph's bitmap, every water cell changes at once.
        if self.custom && self.t.is_multiple_of(8) {
            self.phase += 1;
            self.apply_font(dos);
        }
        if self.t.is_multiple_of(70) {
            let ticks = dos.bios_ticks();
            self.log(format!(
                "tick {:>5}  BIOS timer {ticks:>6}  hero at {:?}",
                self.t, self.hero
            ));
        }
        // Every frame; unchanged cells cost nothing.
        self.draw(dos);
    }

    fn overlay(&mut self, dos: &Dos, p: &mut Painter<'_>, screen: [f32; 4]) {
        let (c, r) = dos.text_size();
        let s = format!(
            "text {c}x{r}  {:.2} Hz   B blink:{}  F font:{}  H 25/50 rows",
            dos.refresh_hz(),
            if self.blink { "on" } else { "off" },
            if self.custom { "on" } else { "off" }
        );
        p.text(
            [screen[0] + 8.0, screen[1] + screen[3] + 8.0],
            &s,
            16.0,
            [0.7, 0.7, 0.7, 1.0],
        );
    }
}

const KEYS: &[(&str, &str)] = &[
    ("Arrows", "move the hero"),
    ("Mouse", "menus, drag windows by the title, click the map"),
    ("F10", "menus"),
    ("Tab", "bring the next window to the front"),
    ("B", "blink vs. bright backgrounds"),
    ("F", "custom tile font on/off"),
    ("G", "Standard Galactic Alphabet"),
    ("H", "80x25 / 80x50"),
    ("Esc", "quit"),
];

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<TextUi>(DosConfig::new("text mode", Adapter::Vga).with_keys(KEYS))
}
