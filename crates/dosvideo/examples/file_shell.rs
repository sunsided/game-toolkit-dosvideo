//! A DOS-style file shell built from ratatui widgets, running in VGA text mode.
//!
//! The usual early-90s layout: title and menu bars, a drive bar, a directory tree next to a
//! file list, a program list below, and a status line with the clock. It browses a small
//! built-in drive (so it runs the same everywhere, including the browser) and exercises the
//! `tui::DosBackend` the way a real program would: lists and tables with scroll state,
//! pull-down menus, modal dialogs with shadows, switching to 80x50, blink vs. bright
//! backgrounds, and an `INT 33h` mouse with the classic inverted-cell text cursor.
//!
//! Keys: `Tab` next pane, arrows / `PgUp` / `PgDn` / `Home` / `End` move, `Enter` opens (a
//! directory in the tree, a text file in the viewer, a program), `F10` the menus, `Esc` closes
//! menus and dialogs, `F3` / `Esc` on the desktop quits. The mouse clicks panes, rows and
//! menus.

use game_toolkit_dosvideo::KeyPress;
use game_toolkit_dosvideo::prelude::*;
use game_toolkit_dosvideo::tui::{self, DosBackend};
use ratatui::Terminal;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListState, Paragraph, Row, Table, TableState, Widget,
    Wrap,
};

// ------------------------------------------------------------------------------------------
// The drive

enum Entry {
    Dir(&'static str, &'static [Entry]),
    File(&'static str, u32, &'static str),
}

use Entry::{Dir, File};

const DATE: &str = "11-08-93";

const DRIVE: &[Entry] = &[
    Dir(
        "DOS",
        &[
            File("ANSI.SYS", 9_029, ""),
            File("COMMAND.COM", 54_619, ""),
            File("EDIT.COM", 413, ""),
            File("HIMEM.SYS", 29_136, ""),
            File("MEM.EXE", 32_502, ""),
            File("MODE.COM", 23_521, ""),
            File("MOUSE.COM", 56_408, ""),
            File("README.TXT", 812, DOS_README),
        ],
    ),
    Dir(
        "GAMES",
        &[
            Dir(
                "ROCKET",
                &[
                    File("ROCKET.EXE", 88_102, ""),
                    File("LEVELS.DAT", 41_216, ""),
                    File(
                        "SCORES.TXT",
                        120,
                        "1. MM   48200\r\n2. AJ   31950\r\n3. ZZ   10010",
                    ),
                ],
            ),
            Dir(
                "TILES",
                &[File("TILES.EXE", 61_733, ""), File("TILES.FNT", 4_096, "")],
            ),
            File("GAMES.TXT", 214, GAMES_TXT),
        ],
    ),
    Dir(
        "UTILS",
        &[
            Dir("PKZIP", &[File("PKZIP.EXE", 42_166, "")]),
            File("LIST.COM", 13_221, ""),
            File("SNOW.TXT", 401, SNOW_TXT),
        ],
    ),
    Dir(
        "WORK",
        &[
            File("LETTER.TXT", 690, LETTER),
            File("BUDGET.WK1", 7_204, ""),
            File(
                "TODO.TXT",
                88,
                "- back up C:\\WORK\r\n- buy a 14.4k modem\r\n- defrag",
            ),
        ],
    ),
    File("AUTOEXEC.BAT", 151, AUTOEXEC),
    File("CONFIG.SYS", 102, CONFIG),
    File("README.TXT", 640, README),
];

const AUTOEXEC: &str = "@ECHO OFF\r\nPROMPT $P$G\r\nPATH C:\\DOS;C:\\UTILS\r\nSET TEMP=C:\\TEMP\r\nLH C:\\DOS\\MOUSE.COM\r\nSHELL";
const CONFIG: &str = "DEVICE=C:\\DOS\\HIMEM.SYS\r\nDOS=HIGH,UMB\r\nFILES=30\r\nBUFFERS=20\r\nDEVICE=C:\\DOS\\ANSI.SYS";
const README: &str = "This drive lives inside the example. Every character on screen is a cell in text memory at B800:0000, written by ratatui through tui::DosBackend, scanned out by an emulated VGA and painted by the beam.\r\n\r\nTry View > 80x50 rows: the CRTC's max scan line drops to 7, the font becomes 8x8, and ratatui sees a taller terminal and redraws.";
const DOS_README: &str =
    "Nothing in this directory runs. It is all names and sizes, like a disk listing in a magazine.";
const GAMES_TXT: &str =
    "ROCKET  - land on the pad, mind the fuel\r\nTILES   - a puzzle played in redefined characters";
const SNOW_TXT: &str = "On a CGA card, writing to text memory while the card reads it shows white specks: 'snow'. Programs waited for retrace. A VGA card has no snow.";
const LETTER: &str = "Dear Sir or Madam,\r\n\r\nplease find enclosed my registration fee of $15 for the shareware version of your fine program. I would like the 3.5\" disks.\r\n\r\nRegards";

/// A directory in the tree view: its depth, name, whether it is the last child (for the
/// branch characters), and its entries.
struct TreeRow {
    depth: usize,
    name: &'static str,
    path: String,
    last: Vec<bool>,
    entries: &'static [Entry],
}

fn flatten(
    entries: &'static [Entry],
    depth: usize,
    path: &str,
    last: &[bool],
    out: &mut Vec<TreeRow>,
) {
    let dirs: Vec<_> = entries
        .iter()
        .filter_map(|e| match e {
            Dir(n, c) => Some((*n, *c)),
            File(..) => None,
        })
        .collect();
    for (i, (name, children)) in dirs.iter().enumerate() {
        let mut l = last.to_vec();
        l.push(i + 1 == dirs.len());
        let p = format!("{path}{name}\\");
        out.push(TreeRow {
            depth,
            name,
            path: p.clone(),
            last: l.clone(),
            entries: children,
        });
        flatten(children, depth + 1, &p, &l, out);
    }
}

fn tree() -> Vec<TreeRow> {
    let mut out = vec![TreeRow {
        depth: 0,
        name: "C:\\",
        path: "C:\\".into(),
        last: vec![],
        entries: DRIVE,
    }];
    flatten(DRIVE, 1, "C:\\", &[], &mut out);
    out
}

fn files(entries: &'static [Entry]) -> Vec<(&'static str, u32, &'static str)> {
    entries
        .iter()
        .filter_map(|e| match e {
            File(n, s, t) => Some((*n, *s, *t)),
            Dir(..) => None,
        })
        .collect()
}

// ------------------------------------------------------------------------------------------
// Look

#[derive(Clone, Copy)]
struct Scheme {
    desk: Color,
    text: Color,
    title: Color,
    accent: Color,
    select: Color,
}

const SCHEMES: [(&str, Scheme); 3] = [
    (
        "Classic",
        Scheme {
            desk: Color::Gray,
            text: Color::Black,
            title: Color::Blue,
            accent: Color::Cyan,
            select: Color::Blue,
        },
    ),
    (
        "Ocean",
        Scheme {
            desk: Color::Blue,
            text: Color::White,
            title: Color::Cyan,
            accent: Color::LightBlue,
            select: Color::Cyan,
        },
    ),
    (
        "Monochrome",
        Scheme {
            desk: Color::Black,
            text: Color::Gray,
            title: Color::Gray,
            accent: Color::DarkGray,
            select: Color::Gray,
        },
    ),
];

fn st(fg: Color, bg: Color) -> Style {
    Style::new().fg(fg).bg(bg)
}

fn shadow(buf: &mut Buffer, r: Rect) {
    let right = Rect::new(r.right(), r.y + 1, 2, r.height);
    let below = Rect::new(r.x + 2, r.bottom(), r.width, 1);
    for area in [right, below] {
        for p in area.intersection(buf.area).positions() {
            buf[p].set_style(st(Color::DarkGray, Color::Black));
        }
    }
}

// ------------------------------------------------------------------------------------------
// Menus and dialogs

const MENUS: [(&str, &[&str]); 4] = [
    ("File", &["Open      Enter", "View file", "Exit         F3"]),
    (
        "View",
        &["80x25 rows", "80x50 rows", "Blinking", "Bright backgrounds"],
    ),
    ("Options", &["Color scheme...", "About the cursor"]),
    ("Help", &["Keys", "About Shell..."]),
];

/// x position of each menu title on the menu bar.
fn menu_x(i: usize) -> u16 {
    2 + MENUS[..i]
        .iter()
        .map(|(n, _)| n.len() as u16 + 3)
        .sum::<u16>()
}

struct Dialog {
    title: String,
    text: String,
    scroll: u16,
}

#[derive(Clone, Copy, PartialEq)]
enum Pane {
    Drives,
    Tree,
    Files,
    Programs,
}

const PANES: [Pane; 4] = [Pane::Drives, Pane::Tree, Pane::Files, Pane::Programs];

const PROGRAMS: [&str; 5] = [
    "Command Prompt",
    "Editor",
    "Disk Utilities",
    "Games",
    "Text-mode Font Lab",
];

/// The screen areas, recomputed from the terminal size for drawing and for mouse hits.
struct Areas {
    title: Rect,
    menu: Rect,
    path: Rect,
    drives: Rect,
    tree: Rect,
    files: Rect,
    programs: Rect,
    status: Rect,
}

fn areas(size: Rect) -> Areas {
    let [title, menu, path, drives, mid, programs, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Fill(3),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(size);
    let [tree, files] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(mid);
    Areas {
        title,
        menu,
        path,
        drives,
        tree,
        files,
        programs,
        status,
    }
}

// ------------------------------------------------------------------------------------------
// The program

struct Shell {
    term: Terminal<DosBackend>,
    tree: Vec<TreeRow>,
    focus: Pane,
    tree_state: ListState,
    file_state: TableState,
    prog_state: ListState,
    drive: usize,
    /// Open menu and highlighted item.
    menu: Option<(usize, usize)>,
    dialog: Option<Dialog>,
    scheme: usize,
    tall: bool,
    blink: bool,
}

impl Shell {
    fn dir(&self) -> &TreeRow {
        &self.tree[self.tree_state.selected().unwrap_or(0)]
    }

    fn open_dialog(&mut self, title: &str, text: &str) {
        self.dialog = Some(Dialog {
            title: title.into(),
            text: text.into(),
            scroll: 0,
        });
    }

    fn run_menu(&mut self, dos: &mut Dos, menu: usize, item: usize) {
        self.menu = None;
        match (menu, item) {
            (0, 0) | (0, 1) => self.open(),
            (0, 2) => dos.exit(),
            (1, 0) | (1, 1) => {
                self.tall = item == 0;
                // Max scan line 15 or 7, the matching font; the backend sees the new size.
                dos.crtc(0x09, if self.tall { 0x4F } else { 0x47 });
                dos.load_font(if self.tall { Font::vga() } else { Font::cga() }, 0);
            }
            (1, 2) | (1, 3) => {
                self.blink = item == 2;
                dos.set_blink(self.blink);
            }
            (2, 0) => self.scheme = (self.scheme + 1) % SCHEMES.len(),
            (2, 1) => self.open_dialog(
                "Cursor",
                "The hardware cursor follows ratatui's Frame::set_cursor_position. The shell \
                 leaves it hidden, so the backend wrote CRTC register 0Ah with bit 5 set.",
            ),
            (3, 0) => self.open_dialog(
                "Keys",
                "Tab        next pane\nArrows     move\nPgUp/PgDn  page\nEnter      open\n\
                 F10        menus\nEsc        close\nF3         quit",
            ),
            _ => self.open_dialog(
                "About Shell",
                "DOSVIDEO File Shell 1.0\n\nratatui widgets on an emulated VGA in text mode \
                 03h. Every frame ratatui diffs its buffer and tui::DosBackend writes only the \
                 cells that changed to B800:0000.",
            ),
        }
    }

    /// Enter on the focused pane.
    fn open(&mut self) {
        match self.focus {
            Pane::Drives => {
                if self.drive != 2 {
                    self.open_dialog(
                        "Drive not ready",
                        "Not ready reading drive A\nAbort, Retry, Fail?",
                    );
                }
            }
            Pane::Tree => {
                self.focus = Pane::Files;
                self.file_state.select(Some(0));
            }
            Pane::Files => {
                let list = files(self.dir().entries);
                if let Some(&(name, size, text)) =
                    self.file_state.selected().and_then(|i| list.get(i))
                {
                    let body = if text.is_empty() {
                        format!("{name}: {size} bytes of binary data.")
                    } else {
                        text.replace("\r\n", "\n")
                    };
                    self.open_dialog(&format!("{}{name}", self.dir().path), &body);
                }
            }
            Pane::Programs => {
                let name = PROGRAMS[self.prog_state.selected().unwrap_or(0)];
                self.open_dialog(
                    name,
                    &format!("{name} would start here. Press Esc to return to the shell."),
                );
            }
        }
    }

    fn move_sel(&mut self, delta: i32) {
        let n_files = files(self.dir().entries).len();
        let (sel, n) = match self.focus {
            Pane::Drives => {
                self.drive = (self.drive as i32 + delta).clamp(0, 2) as usize;
                return;
            }
            Pane::Tree => (self.tree_state.selected(), self.tree.len()),
            Pane::Files => (self.file_state.selected(), n_files),
            Pane::Programs => (self.prog_state.selected(), PROGRAMS.len()),
        };
        if n == 0 {
            return;
        }
        let i = Some(sel.map_or(0, |s| (s as i32 + delta).clamp(0, n as i32 - 1) as usize));
        match self.focus {
            Pane::Tree => {
                self.tree_state.select(i);
                self.file_state = TableState::new();
            }
            Pane::Files => self.file_state.select(i),
            Pane::Programs => self.prog_state.select(i),
            Pane::Drives => {}
        }
    }

    fn key(&mut self, dos: &mut Dos, k: KeyPress) {
        if let Some(d) = &mut self.dialog {
            match k.scan {
                sc::UP => d.scroll = d.scroll.saturating_sub(1),
                sc::DOWN => d.scroll += 1,
                sc::ESC | sc::ENTER => self.dialog = None,
                _ => {}
            }
            return;
        }
        if let Some((m, i)) = self.menu {
            let n = MENUS[m].1.len();
            self.menu = match k.scan {
                sc::LEFT => Some(((m + MENUS.len() - 1) % MENUS.len(), 0)),
                sc::RIGHT => Some(((m + 1) % MENUS.len(), 0)),
                sc::UP => Some((m, (i + n - 1) % n)),
                sc::DOWN => Some((m, (i + 1) % n)),
                sc::ENTER => return self.run_menu(dos, m, i),
                sc::ESC | sc::F10 => None,
                _ => Some((m, i)),
            };
            return;
        }
        let page = 8;
        match k.scan {
            sc::ESC | sc::F3 => dos.exit(),
            sc::F10 => self.menu = Some((0, 0)),
            sc::TAB => {
                let i = PANES.iter().position(|&p| p == self.focus).unwrap_or(0);
                self.focus = PANES[(i + 1) % PANES.len()];
            }
            sc::UP => self.move_sel(-1),
            sc::DOWN => self.move_sel(1),
            sc::LEFT if self.focus == Pane::Drives => self.move_sel(-1),
            sc::RIGHT if self.focus == Pane::Drives => self.move_sel(1),
            sc::PGUP => self.move_sel(-page),
            sc::PGDN => self.move_sel(page),
            sc::HOME => self.move_sel(-1000),
            sc::END => self.move_sel(1000),
            sc::ENTER => self.open(),
            _ => {}
        }
    }

    fn click(&mut self, dos: &mut Dos, p: Position, size: Rect) {
        if self.dialog.is_some() {
            self.dialog = None;
            return;
        }
        let a = areas(size);
        // Menu bar and an open drop-down.
        if let Some((m, _)) = self.menu {
            let drop = dropdown_rect(m);
            if drop.contains(p) && p.y > drop.y && p.y < drop.bottom() - 1 {
                return self.run_menu(dos, m, (p.y - drop.y - 1) as usize);
            }
        }
        if a.menu.contains(p) {
            self.menu = (0..MENUS.len())
                .find(|&i| (menu_x(i)..menu_x(i) + MENUS[i].0.len() as u16).contains(&p.x))
                .map(|i| (i, 0));
            return;
        }
        self.menu = None;
        // Rows in a pane: a click on the selected row opens it.
        let row = |r: Rect, header: u16, offset: usize| {
            (p.y >= r.y + 1 + header && p.y < r.bottom() - 1)
                .then(|| (p.y - r.y - 1 - header) as usize + offset)
        };
        if a.drives.contains(p) {
            self.focus = Pane::Drives;
            let d = (p.x.saturating_sub(a.drives.x + 1) / 8) as usize;
            if d < 3 {
                self.drive = d;
            }
        } else if a.tree.contains(p) {
            self.focus = Pane::Tree;
            if let Some(i) = row(a.tree, 0, self.tree_state.offset())
                && i < self.tree.len()
            {
                if self.tree_state.selected() == Some(i) {
                    return self.open();
                }
                self.tree_state.select(Some(i));
                self.file_state = TableState::new();
            }
        } else if a.files.contains(p) {
            self.focus = Pane::Files;
            if let Some(i) = row(a.files, 1, self.file_state.offset())
                && i < files(self.dir().entries).len()
            {
                if self.file_state.selected() == Some(i) {
                    return self.open();
                }
                self.file_state.select(Some(i));
            }
        } else if a.programs.contains(p) {
            self.focus = Pane::Programs;
            if let Some(i) = row(a.programs, 0, self.prog_state.offset())
                && i < PROGRAMS.len()
            {
                if self.prog_state.selected() == Some(i) {
                    return self.open();
                }
                self.prog_state.select(Some(i));
            }
        }
    }

    fn draw(&mut self, dos: &mut Dos) {
        let (_, scheme) = SCHEMES[self.scheme];
        let ticks = dos.bios_ticks();
        let mouse = tui::mouse_cell(dos);
        let dir_files = files(self.dir().entries);
        let path = self.dir().path.clone();
        let (focus, drive, menu) = (self.focus, self.drive, self.menu);
        let tree = &self.tree;
        let dialog = &self.dialog;
        let (tree_state, file_state, prog_state) = (
            &mut self.tree_state,
            &mut self.file_state,
            &mut self.prog_state,
        );

        tui::draw(&mut self.term, dos, |f| {
            let size = f.area();
            let a = areas(size);
            let buf = f.buffer_mut();
            let body = st(scheme.text, scheme.desk);
            Clear.render(size, buf);
            buf.set_style(size, body);

            // Title bar, menu bar, current path.
            Paragraph::new("DOSVIDEO File Shell")
                .centered()
                .style(st(Color::White, scheme.title))
                .render(a.title, buf);
            let bar = st(Color::Black, Color::Gray);
            buf.set_style(a.menu, bar);
            for (i, (name, _)) in MENUS.iter().enumerate() {
                let open = menu.is_some_and(|(m, _)| m == i);
                let s = if open {
                    st(Color::White, Color::Black)
                } else {
                    bar
                };
                let r = Rect::new(menu_x(i) - 1, a.menu.y, name.len() as u16 + 2, 1);
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(
                        &name[..1],
                        s.fg(if open { Color::LightYellow } else { Color::Red }),
                    ),
                    Span::raw(&name[1..]),
                    Span::raw(" "),
                ])
                .style(s)
                .render(r, buf);
            }
            Line::raw(format!(" {path}")).render(a.path, buf);

            // Drive bar: a little disk icon per drive.
            let head = |pane: Pane| {
                if focus == pane {
                    st(Color::White, scheme.title)
                } else {
                    st(scheme.text, scheme.desk)
                }
            };
            for (i, name) in ["A:", "B:", "C:"].iter().enumerate() {
                let x = a.drives.x + 1 + i as u16 * 8;
                let sel = i == drive;
                let s = if sel { head(Pane::Drives) } else { body };
                let icon = if i < 2 { "▄▀" } else { "■■" };
                Line::from(vec![
                    Span::styled(
                        icon,
                        s.fg(if sel && focus == Pane::Drives {
                            Color::White
                        } else {
                            scheme.accent
                        }),
                    ),
                    Span::styled(format!("{name} "), s),
                ])
                .render(Rect::new(x, a.drives.y, 6, 1), buf);
            }

            // Panes: the title bar shows focus, the selection only gets the highlight color
            // in the focused pane.
            let pane_block = |title: &'static str, pane: Pane| {
                Block::new()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Plain)
                    .title(Line::from(title).centered().style(head(pane)))
                    .style(body)
            };
            let hl = |pane: Pane| {
                if focus == pane {
                    st(Color::White, scheme.select)
                } else {
                    Style::new().fg(scheme.select).add_modifier(Modifier::BOLD)
                }
            };

            // Directory tree with branch characters.
            let items: Vec<Line<'_>> = tree
                .iter()
                .map(|t| {
                    let mut s = String::new();
                    if t.depth > 0 {
                        for &last in &t.last[..t.depth - 1] {
                            s.push_str(if last { "   " } else { "│  " });
                        }
                        s.push_str(if t.last[t.depth - 1] {
                            "└──"
                        } else {
                            "├──"
                        });
                    }
                    Line::from(vec![
                        Span::styled(s, Style::new().fg(scheme.accent)),
                        Span::styled(
                            if t.depth > 0 { "■" } else { "" },
                            Style::new().fg(scheme.accent),
                        ),
                        Span::raw(t.name),
                    ])
                })
                .collect();
            let list = List::new(items)
                .block(pane_block(" Directory Tree ", Pane::Tree))
                .highlight_style(hl(Pane::Tree));
            ratatui::widgets::StatefulWidget::render(list, a.tree, buf, tree_state);

            // Files of the selected directory.
            let rows = dir_files.iter().map(|(name, size, _)| {
                let (stem, ext) = name.split_once('.').unwrap_or((name, ""));
                Row::new(vec![
                    format!("≡ {stem}"),
                    ext.to_string(),
                    format!("{size:>7}"),
                    DATE.to_string(),
                ])
            });
            let title = format!(" {path}*.* ");
            let table = Table::new(
                rows,
                [
                    Constraint::Length(11),
                    Constraint::Length(3),
                    Constraint::Length(8),
                    Constraint::Length(8),
                ],
            )
            .header(
                Row::new(["Name", "Ext", "   Size", "Date"]).style(Style::new().fg(scheme.accent)),
            )
            .block(
                Block::bordered()
                    .title(Line::from(title).centered().style(head(Pane::Files)))
                    .style(body),
            )
            .row_highlight_style(hl(Pane::Files));
            ratatui::widgets::StatefulWidget::render(table, a.files, buf, file_state);
            if dir_files.is_empty() {
                Line::raw("No files match.")
                    .centered()
                    .render(Rect::new(a.files.x, a.files.y + 3, a.files.width, 1), buf);
            }

            // Program list.
            let progs = List::new(PROGRAMS.map(|p| format!("▓ {p}")))
                .block(pane_block(" Main ", Pane::Programs))
                .highlight_style(hl(Pane::Programs));
            ratatui::widgets::StatefulWidget::render(progs, a.programs, buf, prog_state);

            // Status line with the BIOS clock (18.2 ticks per second).
            let secs = ticks * 10 / 182;
            let clock = format!(
                "{:02}:{:02}:{:02} ",
                9 + secs / 3600,
                secs / 60 % 60,
                secs % 60
            );
            let keys = " F10=Actions  Tab=Next pane  Enter=Open  F3=Exit";
            buf.set_style(a.status, st(Color::Black, Color::Cyan));
            Line::raw(keys).render(a.status, buf);
            Line::raw(clock).right_aligned().render(a.status, buf);

            // Drop-down menu.
            if let Some((m, sel)) = menu {
                let r = dropdown_rect(m);
                Clear.render(r, buf);
                Block::bordered().style(bar).render(r, buf);
                for (i, item) in MENUS[m].1.iter().enumerate() {
                    let s = if i == sel {
                        st(Color::White, Color::Black)
                    } else {
                        bar
                    };
                    let row = Rect::new(r.x + 1, r.y + 1 + i as u16, r.width - 2, 1);
                    Paragraph::new(format!(" {item}")).style(s).render(row, buf);
                }
                shadow(buf, r);
            }

            // Modal dialog.
            if let Some(d) = dialog {
                let w = 52.min(size.width - 4);
                let h = 12.min(size.height - 4);
                let r = Rect::new((size.width - w) / 2, (size.height - h) / 2, w, h);
                Clear.render(r, buf);
                let block = Block::bordered()
                    .border_type(BorderType::Double)
                    .title(Line::from(format!(" {} ", d.title)).centered())
                    .style(st(Color::White, Color::Blue));
                let inner = block.inner(r);
                block.render(r, buf);
                let [text, button] =
                    Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
                Paragraph::new(d.text.as_str())
                    .wrap(Wrap { trim: false })
                    .scroll((d.scroll, 0))
                    .render(text.inner(ratatui::layout::Margin::new(1, 0)), buf);
                Line::styled("<  OK  >", st(Color::Black, Color::Gray))
                    .centered()
                    .render(
                        Rect::new(button.x + button.width / 2 - 4, button.y, 8, 1),
                        buf,
                    );
                shadow(buf, r);
            }

            // The INT 33h text cursor.
            tui::mouse_cursor(buf, mouse);
        });
    }
}

fn dropdown_rect(m: usize) -> Rect {
    let items = MENUS[m].1;
    let w = items.iter().map(|s| s.len()).max().unwrap_or(0) as u16 + 4;
    Rect::new(menu_x(m) - 1, 2, w, items.len() as u16 + 2)
}

impl DosGame for Shell {
    fn init(dos: &mut Dos) -> anyhow::Result<Self> {
        dos.set_mode(0x03);
        dos.set_blink(false);
        dos.mouse_mut().show(); // INT 33h function 01h: we draw the pointer, hide the OS one
        let mut s = Self {
            term: Terminal::new(DosBackend::new(dos))?,
            tree: tree(),
            focus: Pane::Tree,
            tree_state: ListState::default().with_selected(Some(0)),
            file_state: TableState::new(),
            prog_state: ListState::default().with_selected(Some(0)),
            drive: 2,
            menu: None,
            dialog: None,
            scheme: 0,
            tall: true,
            blink: false,
        };
        s.draw(dos);
        Ok(s)
    }

    fn step(&mut self, dos: &mut Dos) {
        dos.wait_vretrace();
        while let Some(k) = dos.getch() {
            self.key(dos, k);
        }
        if dos.mouse_mut().take_presses(0) > 0 {
            let p = tui::mouse_cell(dos);
            let (c, r) = dos.text_size();
            self.click(dos, p, Rect::new(0, 0, c as u16, r as u16));
        }
        self.draw(dos);
    }
}

const KEYS: &[(&str, &str)] = &[
    ("Tab", "next pane"),
    ("Arrows", "move"),
    ("Enter", "open"),
    ("F10", "menus"),
    ("Mouse", "click panes, rows and menus"),
    ("F3", "quit"),
];

fn main() -> anyhow::Result<()> {
    env_logger::init();
    run::<Shell>(DosConfig::new("file shell", Adapter::Vga).with_keys(KEYS))
}
