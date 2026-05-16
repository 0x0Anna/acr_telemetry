//! Global key-edge polling for pacenote start ambiguity (Windows).
//!
//! Ctrl+Up / Ctrl+Left: previous item, Ctrl+Down / Ctrl+Right: next, Ctrl+Enter: confirm.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacenotePickerNav {
    Prev,
    Next,
    Confirm,
}

#[cfg(windows)]
mod imp {
    use winapi::um::winuser::{
        GetAsyncKeyState, VK_CONTROL, VK_DOWN, VK_LEFT, VK_RETURN, VK_RIGHT, VK_UP,
    };

    #[inline]
    fn key_down(vk: i32) -> bool {
        unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
    }

    pub struct PacenotePickerKeyTracker {
        prev_up: bool,
        prev_down: bool,
        prev_left: bool,
        prev_right: bool,
        prev_return: bool,
    }

    impl PacenotePickerKeyTracker {
        /// Initialise edge detection from the **current** key state so a freshly opened picker
        /// does not treat still-held Ctrl+Enter (after confirming the previous overlay) as a new edge.
        pub fn new() -> Self {
            let up = key_down(VK_UP);
            let down = key_down(VK_DOWN);
            let left = key_down(VK_LEFT);
            let right = key_down(VK_RIGHT);
            let ret = key_down(VK_RETURN);
            Self {
                prev_up: up,
                prev_down: down,
                prev_left: left,
                prev_right: right,
                prev_return: ret,
            }
        }

        pub fn poll(&mut self) -> Option<super::PacenotePickerNav> {
            let ctrl = key_down(VK_CONTROL);
            let up = key_down(VK_UP);
            let down = key_down(VK_DOWN);
            let left = key_down(VK_LEFT);
            let right = key_down(VK_RIGHT);
            let ret = key_down(VK_RETURN);

            let nav = if ctrl {
                if (up && !self.prev_up) || (left && !self.prev_left) {
                    Some(super::PacenotePickerNav::Prev)
                } else if (down && !self.prev_down) || (right && !self.prev_right) {
                    Some(super::PacenotePickerNav::Next)
                } else if ret && !self.prev_return {
                    Some(super::PacenotePickerNav::Confirm)
                } else {
                    None
                }
            } else {
                None
            };

            self.prev_up = up;
            self.prev_down = down;
            self.prev_left = left;
            self.prev_right = right;
            self.prev_return = ret;
            nav
        }
    }
}

#[cfg(windows)]
pub use imp::PacenotePickerKeyTracker;

#[cfg(not(windows))]
#[derive(Default)]
pub struct PacenotePickerKeyTracker;

#[cfg(not(windows))]
impl PacenotePickerKeyTracker {
    pub fn new() -> Self {
        Self
    }

    pub fn poll(&mut self) -> Option<PacenotePickerNav> {
        None
    }
}
