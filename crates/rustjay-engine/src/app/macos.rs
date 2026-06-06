//! macOS-specific app delegate hooks to prevent termination when the last
//! window is hidden, and to recreate (show) windows when the Dock icon is
//! clicked or the app is reactivated.
//!
//! # Safety Note
//!
//! This function uses `class_addMethod` + `std::mem::transmute` to modify
//! `WinitApplicationDelegate` at runtime. If winit's delegate class layout or
//! method signatures change in a future version, this invokes undefined
//! behavior. Monitor winit changelogs closely and consider upstreaming the
//! needed hooks.

// objc FFI: method-IMP transmutes (no clean named target type) and selector
// type-encoding strings that read clearest as raw nul-terminated literals.
#![allow(clippy::missing_transmute_annotations, clippy::manual_c_str_literals)]

use std::sync::OnceLock;
use winit::event_loop::EventLoopProxy;

use super::WindowAction;

static PROXY: OnceLock<EventLoopProxy<WindowAction>> = OnceLock::new();

pub fn set_proxy(proxy: EventLoopProxy<WindowAction>) {
    let _ = PROXY.set(proxy);
}

pub fn setup_macos_app_delegate() {
    use objc::runtime::{class_addMethod, Class, Object, Sel, BOOL, NO, YES};
    use objc::{sel, sel_impl};
    use std::mem;
    use std::os::raw::c_char;

    extern "C" fn should_terminate_after_last_window_closed(
        _self: &Object,
        _sel: Sel,
        _sender: *mut Object,
    ) -> BOOL {
        NO
    }

    extern "C" fn should_handle_reopen(
        _self: &Object,
        _sel: Sel,
        _sender: *mut Object,
        has_visible_windows: BOOL,
    ) -> BOOL {
        // Only show windows when none are currently visible; clicking the
        // Dock icon when windows are already on screen is a no-op.
        if has_visible_windows == NO {
            if let Some(proxy) = PROXY.get() {
                let _ = proxy.send_event(WindowAction::RecreateWindows);
            }
        }
        YES
    }

    unsafe {
        let Some(delegate_class) = Class::get("WinitApplicationDelegate") else {
            log::warn!(
                "WinitApplicationDelegate class not found — \
                 macOS window lifecycle hooks not installed. \
                 App will quit when the last window is closed."
            );
            return;
        };

        let cls = delegate_class as *const _ as *mut Class;

        let enc = "c@:@\0".as_ptr() as *const c_char;
        class_addMethod(
            cls,
            sel!(applicationShouldTerminateAfterLastWindowClosed:),
            mem::transmute::<extern "C" fn(&Object, Sel, *mut Object) -> BOOL, _>(
                should_terminate_after_last_window_closed,
            ),
            enc,
        );

        let enc2 = "c@:@c\0".as_ptr() as *const c_char;
        class_addMethod(
            cls,
            sel!(applicationShouldHandleReopen:hasVisibleWindows:),
            mem::transmute::<extern "C" fn(&Object, Sel, *mut Object, BOOL) -> BOOL, _>(
                should_handle_reopen,
            ),
            enc2,
        );
    }
}
