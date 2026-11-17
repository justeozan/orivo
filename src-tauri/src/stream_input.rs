//! Keyboard and mouse for the stream window, on the Mac.
//!
//! The stream window has no webview, and Tauri tells a window like that nothing
//! about keys or the mouse. So the player listens to AppKit itself: a local
//! event monitor sees every event Orivo receives, keeps the ones meant for the
//! stream window, turns them into GameStream input packets and swallows them
//! (no beep, no menu shortcut firing behind the game). Everything for other
//! windows passes through untouched.
//!
//! Keys are sent by position, as Windows virtual-key codes of a US layout: the
//! host turns them into scancodes and its own layout gives the character, so an
//! AZERTY Mac on an AZERTY PC types what it says on the keys.
//!
//! ⌘Q and ⌘W stay Orivo's, so the stream window can always be closed.

use std::{cell::RefCell, ffi::c_void, ptr::NonNull, sync::Arc};

use block2::RcBlock;
use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSWindow};
use starfire_core::input::{self, MouseButton};

/// Where input packets go: the session's control channel.
pub type Send = Arc<dyn Fn(Vec<u8>) + std::marker::Send + Sync>;

thread_local! {
    /// The installed monitor, main thread only. One stream at a time.
    static MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

/// Start forwarding the stream window's keyboard and mouse. Main thread only.
///
/// # Safety
/// `ns_window` must be the live `NSWindow` of the stream window.
pub unsafe fn install(ns_window: *mut c_void, stream_size: (u32, u32), send: Send) {
    let Some(_mtm) = MainThreadMarker::new() else {
        return;
    };
    uninstall();
    // SAFETY: the caller hands the stream window's own NSWindow.
    let Some(window) = (unsafe { Retained::retain(ns_window.cast::<NSWindow>()) }) else {
        return;
    };
    window.setAcceptsMouseMovedEvents(true);
    let number = window.windowNumber();
    let state = RefCell::new(Translator::new(stream_size));

    let handler = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a live event for the duration of the call.
        let event_ref = unsafe { event.as_ref() };
        if event_ref.windowNumber() != number {
            return event.as_ptr();
        }
        let Ok(mut state) = state.try_borrow_mut() else {
            return event.as_ptr();
        };
        match state.translate(event_ref, &window) {
            Verdict::Send(packets) => {
                for packet in packets {
                    send(packet);
                }
                std::ptr::null_mut()
            }
            Verdict::Swallow => std::ptr::null_mut(),
            Verdict::PassThrough => event.as_ptr(),
        }
    });

    let mask = NSEventMask::KeyDown
        | NSEventMask::KeyUp
        | NSEventMask::FlagsChanged
        | NSEventMask::LeftMouseDown
        | NSEventMask::LeftMouseUp
        | NSEventMask::RightMouseDown
        | NSEventMask::RightMouseUp
        | NSEventMask::OtherMouseDown
        | NSEventMask::OtherMouseUp
        | NSEventMask::MouseMoved
        | NSEventMask::LeftMouseDragged
        | NSEventMask::RightMouseDragged
        | NSEventMask::OtherMouseDragged
        | NSEventMask::ScrollWheel;
    // SAFETY: the block lives as long as the monitor, which `uninstall` removes.
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &handler) };
    MONITOR.with(|slot| *slot.borrow_mut() = monitor);
}

/// Stop forwarding. Main thread only; harmless when nothing is installed.
pub fn uninstall() {
    MONITOR.with(|slot| {
        if let Some(monitor) = slot.borrow_mut().take() {
            // SAFETY: the monitor came from `addLocalMonitorForEventsMatchingMask_handler`.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
    });
}

enum Verdict {
    Send(Vec<Vec<u8>>),
    Swallow,
    PassThrough,
}

/// What AppKit says, turned into what the host understands. Keeps the state
/// the wire needs and AppKit does not repeat: which modifiers are down, and
/// the fraction of a trackpad scroll not sent yet.
struct Translator {
    stream_size: (u32, u32),
    modifiers: u8,
    scroll: Scroll,
}

impl Translator {
    fn new(stream_size: (u32, u32)) -> Self {
        Self {
            stream_size,
            modifiers: 0,
            scroll: Scroll::default(),
        }
    }

    fn translate(&mut self, event: &NSEvent, window: &NSWindow) -> Verdict {
        let kind = event.r#type();
        match kind {
            NSEventType::KeyDown | NSEventType::KeyUp => {
                let down = kind == NSEventType::KeyDown;
                if event
                    .modifierFlags()
                    .contains(NSEventModifierFlags::Command)
                    && is_orivo_shortcut(event)
                {
                    return Verdict::PassThrough;
                }
                if down && event.isARepeat() {
                    // The host repeats a held key itself.
                    return Verdict::Swallow;
                }
                match vk_from_mac_keycode(event.keyCode()) {
                    Some(vk) => Verdict::Send(vec![input::key(vk, self.modifiers, down)]),
                    None => Verdict::Swallow,
                }
            }
            NSEventType::FlagsChanged => {
                let flags = event.modifierFlags();
                match modifier_change(event.keyCode(), flags_bits(flags)) {
                    Some((vk, down)) => {
                        self.modifiers = modifier_mask(flags_bits(flags));
                        Verdict::Send(vec![input::key(vk, self.modifiers, down)])
                    }
                    None => Verdict::Swallow,
                }
            }
            NSEventType::LeftMouseDown
            | NSEventType::LeftMouseUp
            | NSEventType::RightMouseDown
            | NSEventType::RightMouseUp
            | NSEventType::OtherMouseDown
            | NSEventType::OtherMouseUp => {
                let down = matches!(
                    kind,
                    NSEventType::LeftMouseDown
                        | NSEventType::RightMouseDown
                        | NSEventType::OtherMouseDown
                );
                let Some(button) = mouse_button(event.buttonNumber()) else {
                    return Verdict::PassThrough;
                };
                // A click outside the picture (on the title bar) is the window's.
                let Some(position) = self.position(event, window) else {
                    return Verdict::PassThrough;
                };
                Verdict::Send(vec![position, input::mouse_button(button, down)])
            }
            NSEventType::MouseMoved
            | NSEventType::LeftMouseDragged
            | NSEventType::RightMouseDragged
            | NSEventType::OtherMouseDragged => match self.position(event, window) {
                Some(position) => Verdict::Send(vec![position]),
                None => Verdict::PassThrough,
            },
            NSEventType::ScrollWheel => {
                let packets = self.scroll.add(
                    event.scrollingDeltaX(),
                    event.scrollingDeltaY(),
                    event.hasPreciseScrollingDeltas(),
                );
                Verdict::Send(packets)
            }
            _ => Verdict::PassThrough,
        }
    }

    /// The pointer as an absolute position in the stream, or `None` when it is
    /// outside the picture. The picture fills the window's content view.
    fn position(&self, event: &NSEvent, window: &NSWindow) -> Option<Vec<u8>> {
        let view = window.contentView()?;
        let point = view.convertPoint_fromView(event.locationInWindow(), None);
        let size = view.frame().size;
        let flipped = view.isFlipped();
        let (x, y) = stream_point(
            (point.x, point.y),
            (size.width, size.height),
            flipped,
            self.stream_size,
        )?;
        let (width, height) = self.stream_size;
        Some(input::mouse_move_abs(
            x,
            y,
            clamp_i16(width),
            clamp_i16(height),
        ))
    }
}

fn is_orivo_shortcut(event: &NSEvent) -> bool {
    event
        .charactersIgnoringModifiers()
        .map(|characters| {
            let characters = characters.to_string().to_lowercase();
            characters == "q" || characters == "w"
        })
        .unwrap_or(false)
}

fn flags_bits(flags: NSEventModifierFlags) -> usize {
    flags.0
}

const FLAG_CAPS_LOCK: usize = 1 << 16;
const FLAG_SHIFT: usize = 1 << 17;
const FLAG_CONTROL: usize = 1 << 18;
const FLAG_OPTION: usize = 1 << 19;
const FLAG_COMMAND: usize = 1 << 20;

/// GameStream's modifier byte: shift, ctrl, alt, meta.
fn modifier_mask(flags: usize) -> u8 {
    let mut mask = 0;
    if flags & FLAG_SHIFT != 0 {
        mask |= 0x01;
    }
    if flags & FLAG_CONTROL != 0 {
        mask |= 0x02;
    }
    if flags & FLAG_OPTION != 0 {
        mask |= 0x04;
    }
    if flags & FLAG_COMMAND != 0 {
        mask |= 0x08;
    }
    mask
}

/// A modifier key went down or up: which key, and which way. AppKit reports
/// modifiers as a changed set, not a key event; the flag for that key says the
/// direction. ⌘ is the Windows key, ⌥ is Alt.
fn modifier_change(keycode: u16, flags: usize) -> Option<(u16, bool)> {
    let (vk, flag) = match keycode {
        0x38 | 0x3C => (0x10, FLAG_SHIFT),
        0x3B | 0x3E => (0x11, FLAG_CONTROL),
        0x3A | 0x3D => (0x12, FLAG_OPTION),
        0x37 => (0x5B, FLAG_COMMAND),
        0x36 => (0x5C, FLAG_COMMAND),
        0x39 => (0x14, FLAG_CAPS_LOCK),
        _ => return None,
    };
    Some((vk, flags & flag != 0))
}

fn mouse_button(number: isize) -> Option<MouseButton> {
    match number {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Right),
        2 => Some(MouseButton::Middle),
        3 => Some(MouseButton::Side1),
        4 => Some(MouseButton::Side2),
        _ => None,
    }
}

/// A point in the content view, as a point in the stream picture.
fn stream_point(
    point: (f64, f64),
    view: (f64, f64),
    flipped: bool,
    stream: (u32, u32),
) -> Option<(i16, i16)> {
    let (width, height) = view;
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let (x, y) = point;
    if !(0.0..=width).contains(&x) || !(0.0..=height).contains(&y) {
        return None;
    }
    let nx = x / width;
    // AppKit's origin is bottom-left unless the view is flipped; the stream's is top-left.
    let ny = if flipped {
        y / height
    } else {
        1.0 - y / height
    };
    let sx = (nx * f64::from(stream.0)).round();
    let sy = (ny * f64::from(stream.1)).round();
    Some((clamp_i16_f(sx), clamp_i16_f(sy)))
}

fn clamp_i16(value: u32) -> i16 {
    i16::try_from(value).unwrap_or(i16::MAX)
}

fn clamp_i16_f(value: f64) -> i16 {
    value.clamp(0.0, f64::from(i16::MAX)) as i16
}

/// One wheel notch is 120 on the wire. A mouse wheel reports notches; a
/// trackpad reports points, many per gesture, which add up until they make a
/// step worth sending.
#[derive(Default)]
struct Scroll {
    x: f64,
    y: f64,
}

const WHEEL_NOTCH: f64 = 120.0;
/// Wire units per trackpad point: a two-finger flick scrolls a page, not a line.
const UNITS_PER_POINT: f64 = 4.0;

impl Scroll {
    fn add(&mut self, dx: f64, dy: f64, precise: bool) -> Vec<Vec<u8>> {
        let scale = if precise {
            UNITS_PER_POINT
        } else {
            WHEEL_NOTCH
        };
        self.x += dx * scale;
        self.y += dy * scale;
        let mut packets = Vec::new();
        let (y, rest_y) = whole(self.y);
        self.y = rest_y;
        if y != 0 {
            packets.push(input::scroll_vertical(y));
        }
        // AppKit's positive x is a scroll to the left; the wire's is to the right.
        let (x, rest_x) = whole(self.x);
        self.x = rest_x;
        if x != 0 {
            packets.push(input::scroll_horizontal(x.saturating_neg()));
        }
        packets
    }
}

fn whole(value: f64) -> (i16, f64) {
    let whole = value
        .trunc()
        .clamp(f64::from(i16::MIN), f64::from(i16::MAX));
    (whole as i16, value - whole)
}

/// A Mac key, by position (`kVK_*`), as the Windows virtual key at the same
/// place on a US keyboard. Modifiers come through `modifier_change`.
fn vk_from_mac_keycode(keycode: u16) -> Option<u16> {
    Some(match keycode {
        0x00 => 0x41, // A
        0x01 => 0x53, // S
        0x02 => 0x44, // D
        0x03 => 0x46, // F
        0x04 => 0x48, // H
        0x05 => 0x47, // G
        0x06 => 0x5A, // Z
        0x07 => 0x58, // X
        0x08 => 0x43, // C
        0x09 => 0x56, // V
        0x0A => 0xE2, // ISO section key (OEM_102)
        0x0B => 0x42, // B
        0x0C => 0x51, // Q
        0x0D => 0x57, // W
        0x0E => 0x45, // E
        0x0F => 0x52, // R
        0x10 => 0x59, // Y
        0x11 => 0x54, // T
        0x12 => 0x31, // 1
        0x13 => 0x32, // 2
        0x14 => 0x33, // 3
        0x15 => 0x34, // 4
        0x16 => 0x36, // 6
        0x17 => 0x35, // 5
        0x18 => 0xBB, // =
        0x19 => 0x39, // 9
        0x1A => 0x37, // 7
        0x1B => 0xBD, // -
        0x1C => 0x38, // 8
        0x1D => 0x30, // 0
        0x1E => 0xDD, // ]
        0x1F => 0x4F, // O
        0x20 => 0x55, // U
        0x21 => 0xDB, // [
        0x22 => 0x49, // I
        0x23 => 0x50, // P
        0x24 => 0x0D, // Return
        0x25 => 0x4C, // L
        0x26 => 0x4A, // J
        0x27 => 0xDE, // '
        0x28 => 0x4B, // K
        0x29 => 0xBA, // ;
        0x2A => 0xDC, // backslash
        0x2B => 0xBC, // ,
        0x2C => 0xBF, // /
        0x2D => 0x4E, // N
        0x2E => 0x4D, // M
        0x2F => 0xBE, // .
        0x30 => 0x09, // Tab
        0x31 => 0x20, // Space
        0x32 => 0xC0, // `
        0x33 => 0x08, // Backspace
        0x35 => 0x1B, // Escape
        0x40 => 0x80, // F17
        0x41 => 0x6E, // keypad .
        0x43 => 0x6A, // keypad *
        0x45 => 0x6B, // keypad +
        0x47 => 0x90, // keypad clear, where a PC keyboard has Num Lock
        0x4B => 0x6F, // keypad /
        0x4C => 0x0D, // keypad Enter
        0x4E => 0x6D, // keypad -
        0x4F => 0x81, // F18
        0x50 => 0x82, // F19
        0x52 => 0x60, // keypad 0
        0x53 => 0x61, // keypad 1
        0x54 => 0x62, // keypad 2
        0x55 => 0x63, // keypad 3
        0x56 => 0x64, // keypad 4
        0x57 => 0x65, // keypad 5
        0x58 => 0x66, // keypad 6
        0x59 => 0x67, // keypad 7
        0x5A => 0x83, // F20
        0x5B => 0x68, // keypad 8
        0x5C => 0x69, // keypad 9
        0x60 => 0x74, // F5
        0x61 => 0x75, // F6
        0x62 => 0x76, // F7
        0x63 => 0x72, // F3
        0x64 => 0x77, // F8
        0x65 => 0x78, // F9
        0x67 => 0x7A, // F11
        0x69 => 0x7C, // F13
        0x6A => 0x7F, // F16
        0x6B => 0x7D, // F14
        0x6D => 0x79, // F10
        0x6F => 0x7B, // F12
        0x71 => 0x7E, // F15
        0x72 => 0x2D, // Help, where a PC keyboard has Insert
        0x73 => 0x24, // Home
        0x74 => 0x21, // Page Up
        0x75 => 0x2E, // Forward Delete
        0x76 => 0x73, // F4
        0x77 => 0x23, // End
        0x78 => 0x71, // F2
        0x79 => 0x22, // Page Down
        0x7A => 0x70, // F1
        0x7B => 0x25, // Left
        0x7C => 0x27, // Right
        0x7D => 0x28, // Down
        0x7E => 0x26, // Up
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_go_by_position_on_a_us_layout() {
        // The key left of S is A on QWERTY and Q on AZERTY: either way it is VK_A.
        assert_eq!(vk_from_mac_keycode(0x00), Some(0x41));
        assert_eq!(vk_from_mac_keycode(0x35), Some(0x1B));
        assert_eq!(vk_from_mac_keycode(0x7E), Some(0x26));
        assert_eq!(vk_from_mac_keycode(0x3F), None); // fn
    }

    #[test]
    fn modifiers_report_their_direction_from_the_flags() {
        assert_eq!(modifier_change(0x38, FLAG_SHIFT), Some((0x10, true)));
        assert_eq!(modifier_change(0x38, 0), Some((0x10, false)));
        assert_eq!(modifier_change(0x37, FLAG_COMMAND), Some((0x5B, true)));
        assert_eq!(
            modifier_change(0x3A, FLAG_OPTION | FLAG_SHIFT),
            Some((0x12, true))
        );
        assert_eq!(modifier_change(0x00, FLAG_SHIFT), None);
        assert_eq!(modifier_mask(FLAG_SHIFT | FLAG_COMMAND), 0x09);
    }

    #[test]
    fn the_pointer_lands_where_it_is_on_the_picture() {
        let stream = (1920, 1080);
        // Bottom-left origin: the top-left corner of the view is the stream's origin.
        assert_eq!(
            stream_point((0.0, 720.0), (1280.0, 720.0), false, stream),
            Some((0, 0))
        );
        assert_eq!(
            stream_point((640.0, 360.0), (1280.0, 720.0), false, stream),
            Some((960, 540))
        );
        assert_eq!(
            stream_point((1280.0, 0.0), (1280.0, 720.0), false, stream),
            Some((1920, 1080))
        );
        assert_eq!(
            stream_point((10.0, 10.0), (1280.0, 720.0), true, stream),
            Some((15, 15))
        );
        // Above the picture: the title bar, not the game.
        assert_eq!(
            stream_point((10.0, 730.0), (1280.0, 720.0), false, stream),
            None
        );
    }

    #[test]
    fn a_wheel_sends_notches_and_a_trackpad_adds_up_first() {
        let mut scroll = Scroll::default();
        assert_eq!(scroll.add(0.0, 1.0, false).len(), 1);
        assert_eq!(scroll.add(0.0, 0.1, true).len(), 0);
        assert_eq!(scroll.add(0.0, 0.2, true).len(), 1);
        assert_eq!(scroll.add(0.0, 0.0, false).len(), 0);
    }
}
