//! Transfer keyboard focus before disabling its control; restore only in our foreground window.
use std::cell::Cell;
use windows_sys::Win32::{
    Foundation::HWND,
    UI::{
        Input::KeyboardAndMouse::{EnableWindow, GetFocus, SetFocus},
        WindowsAndMessaging::GetForegroundWindow,
    },
};

fn may_restore(pending: bool, enabled: bool, parent_focused: bool, foreground: bool) -> bool {
    pending && enabled && parent_focused && foreground
}

/// Caller owns both live HWNDs on the current UI thread. No RefCell borrows may
/// span this function: focus and enable operations synchronously invoke callbacks.
pub(super) unsafe fn update(parent: HWND, button: HWND, enabled: bool, pending: &Cell<bool>) {
    if button.is_null() {
        return;
    }
    // SAFETY: caller guarantees same-thread live parent/control handles. SetFocus
    // is local to our control hierarchy; it never activates another application's window.
    unsafe {
        if !enabled && GetFocus() == button {
            SetFocus(parent);
            pending.set(GetFocus() == parent);
        }
        EnableWindow(button, i32::from(enabled));
        if enabled {
            let restore = pending.replace(false);
            if may_restore(
                restore,
                enabled,
                GetFocus() == parent,
                GetForegroundWindow() == parent,
            ) {
                SetFocus(button);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*};
    #[test]
    fn restoration_requires_all_four_conditions() {
        for pending in [false, true] {
            for enabled in [false, true] {
                for focused in [false, true] {
                    for foreground in [false, true] {
                        assert_eq!(
                            may_restore(pending, enabled, focused, foreground),
                            pending && enabled && focused && foreground
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn native_disable_preserves_a_keyboard_target_and_does_not_steal_focus() {
        struct Owned(HWND);
        impl Drop for Owned {
            fn drop(&mut self) {
                // SAFETY: this test owns the parent and therefore its child controls.
                unsafe {
                    DestroyWindow(self.0);
                }
            }
        }
        // SAFETY: hidden windows are created on this test thread with stock classes.
        // No SendInput, foreground changes, desktop/Explorer access or global hotkeys.
        unsafe {
            let instance = GetModuleHandleW(null());
            let parent = CreateWindowExW(
                0,
                windows_sys::core::w!("STATIC"),
                null(),
                WS_OVERLAPPED,
                0,
                0,
                100,
                100,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            assert!(!parent.is_null());
            let _owner = Owned(parent);
            let button = CreateWindowExW(
                0,
                windows_sys::core::w!("BUTTON"),
                null(),
                WS_CHILD | WS_TABSTOP,
                0,
                0,
                50,
                20,
                parent,
                null_mut(),
                instance,
                null(),
            );
            assert!(!button.is_null());
            SetFocus(button);
            assert_eq!(GetFocus(), button);
            // Reproduce the pre-fix failure: disabling the focused button leaves
            // no target for queued keyboard input, even after re-enabling it.
            EnableWindow(button, 0);
            assert!(GetFocus().is_null());
            EnableWindow(button, 1);
            assert!(GetFocus().is_null());
            SetFocus(button);
            let pending = Cell::new(false);
            update(parent, button, false, &pending);
            assert_eq!(GetFocus(), parent);
            assert!(pending.get());
            // Simulate a user moving focus elsewhere before the background result.
            SetFocus(null_mut());
            update(parent, button, true, &pending);
            assert!(GetFocus().is_null());
            assert!(!pending.get());
        }
    }
}
