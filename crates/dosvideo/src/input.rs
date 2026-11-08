//! Keyboard and mouse, the way a DOS program sees them.
//!
//! The keyboard comes in both flavors DOS games used: a table of which keys are held, indexed
//! by XT (set 1) scancode, like a custom `INT 9` handler builds it, and the BIOS keyboard buffer
//! of `(scancode, ASCII)` pairs for `kbhit()` / `getch()` style input. Port `0x60` returns the
//! last make/break code.
//!
//! The mouse is an `INT 33h` driver: positions are in the driver's *virtual screen*
//! coordinates, which depend on the BIOS video mode, not on what the CRTC really shows. In mode
//! 13h that is 640x200, so X has to be halved, and a Mode X program at 320x240 still gets Y in
//! `0..200` until it sets the range itself, both well-known DOS annoyances.

use std::collections::VecDeque;

/// XT scancodes (set 1 make codes).
pub mod sc {
    pub const ESC: u8 = 0x01;
    pub const N1: u8 = 0x02;
    pub const N2: u8 = 0x03;
    pub const N3: u8 = 0x04;
    pub const N4: u8 = 0x05;
    pub const N5: u8 = 0x06;
    pub const N6: u8 = 0x07;
    pub const N7: u8 = 0x08;
    pub const N8: u8 = 0x09;
    pub const N9: u8 = 0x0A;
    pub const N0: u8 = 0x0B;
    pub const MINUS: u8 = 0x0C;
    pub const EQUALS: u8 = 0x0D;
    pub const BACKSPACE: u8 = 0x0E;
    pub const TAB: u8 = 0x0F;
    pub const Q: u8 = 0x10;
    pub const W: u8 = 0x11;
    pub const E: u8 = 0x12;
    pub const R: u8 = 0x13;
    pub const T: u8 = 0x14;
    pub const Y: u8 = 0x15;
    pub const U: u8 = 0x16;
    pub const I: u8 = 0x17;
    pub const O: u8 = 0x18;
    pub const P: u8 = 0x19;
    pub const LBRACKET: u8 = 0x1A;
    pub const RBRACKET: u8 = 0x1B;
    pub const ENTER: u8 = 0x1C;
    pub const CTRL: u8 = 0x1D;
    pub const A: u8 = 0x1E;
    pub const S: u8 = 0x1F;
    pub const D: u8 = 0x20;
    pub const F: u8 = 0x21;
    pub const G: u8 = 0x22;
    pub const H: u8 = 0x23;
    pub const J: u8 = 0x24;
    pub const K: u8 = 0x25;
    pub const L: u8 = 0x26;
    pub const SEMICOLON: u8 = 0x27;
    pub const APOSTROPHE: u8 = 0x28;
    pub const BACKTICK: u8 = 0x29;
    pub const LSHIFT: u8 = 0x2A;
    pub const BACKSLASH: u8 = 0x2B;
    pub const Z: u8 = 0x2C;
    pub const X: u8 = 0x2D;
    pub const C: u8 = 0x2E;
    pub const V: u8 = 0x2F;
    pub const B: u8 = 0x30;
    pub const N: u8 = 0x31;
    pub const M: u8 = 0x32;
    pub const COMMA: u8 = 0x33;
    pub const PERIOD: u8 = 0x34;
    pub const SLASH: u8 = 0x35;
    pub const RSHIFT: u8 = 0x36;
    pub const KP_STAR: u8 = 0x37;
    pub const ALT: u8 = 0x38;
    pub const SPACE: u8 = 0x39;
    pub const CAPSLOCK: u8 = 0x3A;
    pub const F1: u8 = 0x3B;
    pub const F2: u8 = 0x3C;
    pub const F3: u8 = 0x3D;
    pub const F4: u8 = 0x3E;
    pub const F5: u8 = 0x3F;
    pub const F6: u8 = 0x40;
    pub const F7: u8 = 0x41;
    pub const F8: u8 = 0x42;
    pub const F9: u8 = 0x43;
    pub const F10: u8 = 0x44;
    pub const NUMLOCK: u8 = 0x45;
    pub const SCROLLLOCK: u8 = 0x46;
    pub const HOME: u8 = 0x47;
    pub const UP: u8 = 0x48;
    pub const PGUP: u8 = 0x49;
    pub const KP_MINUS: u8 = 0x4A;
    pub const LEFT: u8 = 0x4B;
    pub const KP5: u8 = 0x4C;
    pub const RIGHT: u8 = 0x4D;
    pub const KP_PLUS: u8 = 0x4E;
    pub const END: u8 = 0x4F;
    pub const DOWN: u8 = 0x50;
    pub const PGDN: u8 = 0x51;
    pub const INSERT: u8 = 0x52;
    pub const DELETE: u8 = 0x53;
    pub const F11: u8 = 0x57;
    pub const F12: u8 = 0x58;
}

/// One entry of the BIOS keyboard buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPress {
    /// XT scancode.
    pub scan: u8,
    /// ASCII (CP437) code, `0` for extended keys (arrows, F-keys, ...).
    pub ascii: u8,
}

/// Keyboard state.
#[derive(Clone, Debug)]
pub struct Keyboard {
    down: [bool; 128],
    buffer: VecDeque<KeyPress>,
    last_code: u8,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self {
            down: [false; 128],
            buffer: VecDeque::new(),
            last_code: 0,
        }
    }
}

/// The BIOS buffer holds 15 keys; further presses beep and are dropped.
const BIOS_BUFFER: usize = 15;

impl Keyboard {
    /// Is the key with this scancode held?
    pub fn down(&self, scan: u8) -> bool {
        self.down[(scan & 0x7F) as usize]
    }

    /// The whole held-key table, like the array a custom keyboard ISR fills.
    pub fn table(&self) -> &[bool; 128] {
        &self.down
    }

    /// Is there a key in the BIOS buffer? (`INT 16h, AH=01h`, `kbhit()`.)
    pub fn hit(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Take the next key from the BIOS buffer (`INT 16h, AH=00h`, `getch()`), without waiting.
    pub fn read(&mut self) -> Option<KeyPress> {
        self.buffer.pop_front()
    }

    /// Drop all buffered keys (what games did before "press any key").
    pub fn flush(&mut self) {
        self.buffer.clear();
    }

    /// Last code on port 0x60: the scancode, with bit 7 set for a release.
    pub fn port60(&self) -> u8 {
        self.last_code
    }

    /// Feed a key press (host side). `ascii` is 0 for keys without a character.
    pub fn press(&mut self, scan: u8, ascii: u8) {
        let s = (scan & 0x7F) as usize;
        self.down[s] = true;
        self.last_code = scan & 0x7F;
        if self.buffer.len() < BIOS_BUFFER {
            self.buffer.push_back(KeyPress {
                scan: scan & 0x7F,
                ascii,
            });
        }
    }

    /// Feed a key release (host side).
    pub fn release(&mut self, scan: u8) {
        self.down[(scan & 0x7F) as usize] = false;
        self.last_code = scan | 0x80;
    }

    pub fn shift(&self) -> bool {
        self.down(sc::LSHIFT) || self.down(sc::RSHIFT)
    }

    /// The ASCII code of a scancode on a US keyboard.
    pub fn ascii_for(scan: u8, shift: bool) -> u8 {
        const LOWER: &[u8; 0x3A] =
            b"\0\x1b1234567890-=\x08\tqwertyuiop[]\r\0asdfghjkl;'`\0\\zxcvbnm,./\0*\0 ";
        const UPPER: &[u8; 0x3A] =
            b"\0\x1b!@#$%^&*()_+\x08\tQWERTYUIOP{}\r\0ASDFGHJKL:\"~\0|ZXCVBNM<>?\0*\0 ";
        let t = if shift { UPPER } else { LOWER };
        t.get(scan as usize).copied().unwrap_or(0)
    }
}

/// `INT 33h` mouse driver state.
#[derive(Clone, Copy, Debug)]
pub struct Mouse {
    /// Position in driver coordinates.
    pub x: i32,
    pub y: i32,
    /// Bit 0 left, bit 1 right, bit 2 middle.
    pub buttons: u8,
    pub(crate) range: [i32; 4],
    pub(crate) custom_range: bool,
    pub(crate) visible: bool,
    presses: [u16; 3],
}

impl Default for Mouse {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            buttons: 0,
            range: [0, 639, 0, 199],
            custom_range: false,
            visible: false,
            presses: [0; 3],
        }
    }
}

impl Mouse {
    pub fn left(&self) -> bool {
        self.buttons & 1 != 0
    }

    pub fn right(&self) -> bool {
        self.buttons & 2 != 0
    }

    pub fn middle(&self) -> bool {
        self.buttons & 4 != 0
    }

    /// Function 01h: show the mouse cursor. The driver draws nothing itself (draw the pointer
    /// in your program, e.g. with `tui::mouse_cursor` in text mode); the host hides the OS
    /// pointer over the display while the cursor is shown. A mode set hides it again.
    pub fn show(&mut self) {
        self.visible = true;
    }

    /// Function 02h: hide the mouse cursor (the host shows the OS pointer again).
    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Whether the program has shown the mouse cursor.
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Function 05h: how often a button was pressed since the last call.
    pub fn take_presses(&mut self, button: usize) -> u16 {
        std::mem::take(&mut self.presses[button.min(2)])
    }

    /// Function 07h / 08h: limit the driver coordinates.
    pub fn set_range(&mut self, x_min: i32, x_max: i32, y_min: i32, y_max: i32) {
        self.range = [x_min, x_max.max(x_min), y_min, y_max.max(y_min)];
        self.custom_range = true;
        self.x = self.x.clamp(self.range[0], self.range[1]);
        self.y = self.y.clamp(self.range[2], self.range[3]);
    }

    /// Host side: put the pointer at a normalized position over the active display.
    pub fn set_normalized(&mut self, fx: f32, fy: f32) {
        let [x0, x1, y0, y1] = self.range;
        let fx = fx.clamp(0.0, 0.999_99);
        let fy = fy.clamp(0.0, 0.999_99);
        self.x = x0 + (fx * (x1 - x0 + 1) as f32) as i32;
        self.y = y0 + (fy * (y1 - y0 + 1) as f32) as i32;
    }

    /// Host side: update the buttons, counting presses.
    pub fn set_buttons(&mut self, buttons: u8) {
        for b in 0..3 {
            if buttons & (1 << b) != 0 && self.buttons & (1 << b) == 0 {
                self.presses[b] = self.presses[b].saturating_add(1);
            }
        }
        self.buttons = buttons;
    }

    /// Default driver range for a BIOS mode (what the driver resets to on a mode set).
    pub(crate) fn reset_for_mode(&mut self, mode: u8, hercules: bool) {
        self.range = match mode {
            0x0F | 0x10 => [0, 639, 0, 349],
            0x11 | 0x12 => [0, 639, 0, 479],
            0x07 if hercules => [0, 639, 0, 199],
            0xFF => self.range,
            _ => [0, 639, 0, 199],
        };
        self.custom_range = false;
        self.visible = false;
        self.x = (self.range[0] + self.range[1]) / 2;
        self.y = (self.range[2] + self.range[3]) / 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_table() {
        assert_eq!(Keyboard::ascii_for(sc::A, false), b'a');
        assert_eq!(Keyboard::ascii_for(sc::A, true), b'A');
        assert_eq!(Keyboard::ascii_for(sc::N1, true), b'!');
        assert_eq!(Keyboard::ascii_for(sc::SPACE, false), b' ');
        assert_eq!(Keyboard::ascii_for(sc::ENTER, false), b'\r');
        assert_eq!(Keyboard::ascii_for(sc::UP, false), 0);
    }

    #[test]
    fn mode13h_mouse_reports_640_wide() {
        let mut m = Mouse::default();
        m.reset_for_mode(0x13, false);
        m.set_normalized(0.5, 0.5);
        assert_eq!((m.x, m.y), (320, 100));
    }
}
