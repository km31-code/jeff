// phase 20: active window context observer.
// polls NSWorkspace for the frontmost application and AXUIElement for the
// focused window title. all macOS api calls are in the inner module gated on
// cfg(target_os = "macos"). on other platforms the module provides stubs that
// always return None / false so the rest of the codebase compiles unchanged.

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct ActiveWindowContext {
    pub app_name: String,
    pub document_title: String,
    pub captured_at: i64,
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---- macos implementation ---------------------------------------------------

#[cfg(target_os = "macos")]
mod inner {
    use super::{unix_now, ActiveWindowContext};
    use std::ffi::{c_char, c_void, CStr, CString};

    type AXError = i32;
    const AX_SUCCESS: AXError = 0;
    const CF_STRING_ENCODING_UTF8: u32 = 0x08000100;

    // ApplicationServices.framework — Accessibility API
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
        fn AXUIElementCreateApplication(pid: i32) -> *const c_void;
        fn AXUIElementCopyAttributeValue(
            element: *const c_void,
            attribute: *const c_void,
            value: *mut *const c_void,
        ) -> AXError;
    }

    // CoreFoundation.framework — string and collection helpers
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        // kCFBooleanTrue is a CFBooleanRef (pointer-sized opaque pointer value)
        static kCFBooleanTrue: *const c_void;
        // key/value callbacks are large structs; we only need their address.
        // declaring as u8 lets us take a pointer without reading the struct.
        static kCFTypeDictionaryKeyCallBacks: u8;
        static kCFTypeDictionaryValueCallBacks: u8;
        fn CFRelease(cf: *const c_void);
        fn CFStringGetCStringPtr(the_string: *const c_void, encoding: u32) -> *const c_char;
        fn CFStringGetCString(
            the_string: *const c_void,
            buffer: *mut c_char,
            buffer_size: i64,
            encoding: u32,
        ) -> bool;
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFDictionaryCreate(
            alloc: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: i64,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> *const c_void;
    }

    // create a CFString from a Rust str. caller must CFRelease the result.
    // returns null on CString construction failure (embedded nul).
    unsafe fn make_cf_string(s: &str) -> *const c_void {
        let Ok(c) = CString::new(s) else {
            return std::ptr::null();
        };
        CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), CF_STRING_ENCODING_UTF8)
    }

    // copy a CFString to a Rust String. tries the fast path (internal buffer)
    // then falls back to a stack buffer copy.
    unsafe fn cf_string_to_rust(cf: *const c_void) -> Option<String> {
        if cf.is_null() {
            return None;
        }
        let ptr = CFStringGetCStringPtr(cf, CF_STRING_ENCODING_UTF8);
        if !ptr.is_null() {
            return Some(CStr::from_ptr(ptr).to_string_lossy().into_owned());
        }
        let mut buf = vec![0i8; 1024];
        if CFStringGetCString(
            cf,
            buf.as_mut_ptr(),
            buf.len() as i64,
            CF_STRING_ENCODING_UTF8,
        ) {
            Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
        } else {
            None
        }
    }

    // returns (localized_app_name, pid) for the frontmost application.
    // creates and drains an NSAutoreleasePool for this thread so that any
    // autoreleased objects returned by NSWorkspace are properly cleaned up
    // when the polling task runs outside the main thread.
    fn get_frontmost_app() -> Option<(String, i32)> {
        use objc2::{msg_send, runtime::AnyClass, runtime::AnyObject};
        unsafe {
            // use a runtime-safe class lookup so this function returns None
            // rather than panicking when NSAutoreleasePool or NSWorkspace are
            // unavailable (e.g. in test contexts without an AppKit runtime).
            let pool_cls = AnyClass::get(c"NSAutoreleasePool")?;
            let pool: *mut AnyObject = msg_send![pool_cls, new];

            let workspace_cls = AnyClass::get(c"NSWorkspace")?;
            let workspace: *mut AnyObject = msg_send![workspace_cls, sharedWorkspace];

            let result = if workspace.is_null() {
                None
            } else {
                let app: *mut AnyObject = msg_send![workspace, frontmostApplication];
                if app.is_null() {
                    None
                } else {
                    let name_ns: *mut AnyObject = msg_send![app, localizedName];
                    if name_ns.is_null() {
                        None
                    } else {
                        let cstr: *const c_char = msg_send![name_ns, UTF8String];
                        if cstr.is_null() {
                            None
                        } else {
                            let name = CStr::from_ptr(cstr).to_string_lossy().into_owned();
                            let pid: i32 = msg_send![app, processIdentifier];
                            Some((name, pid))
                        }
                    }
                }
            };

            let _: () = msg_send![pool, drain];
            result
        }
    }

    pub fn is_accessibility_trusted() -> bool {
        unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) }
    }

    pub fn request_accessibility_permission() {
        // call AXIsProcessTrustedWithOptions with kAXTrustedCheckOptionPrompt=true.
        // the string literal "AXTrustedCheckOptionPrompt" is the value of the
        // kAXTrustedCheckOptionPrompt constant (a CFString extern symbol), so we
        // create it from the known string value to avoid an extra extern dependency.
        unsafe {
            let key = make_cf_string("AXTrustedCheckOptionPrompt");
            if key.is_null() {
                return;
            }
            let bool_val: *const c_void = kCFBooleanTrue;
            let keys: [*const c_void; 1] = [key];
            let vals: [*const c_void; 1] = [bool_val];
            let dict = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                vals.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks as *const u8 as *const c_void,
                &kCFTypeDictionaryValueCallBacks as *const u8 as *const c_void,
            );
            if !dict.is_null() {
                AXIsProcessTrustedWithOptions(dict);
                CFRelease(dict);
            }
            CFRelease(key);
        }
    }

    pub fn poll_active_window() -> Option<ActiveWindowContext> {
        if !is_accessibility_trusted() {
            return None;
        }

        let (app_name, pid) = get_frontmost_app()?;

        // skip jeff itself to avoid self-referential context.
        if matches!(app_name.as_str(), "Jeff" | "jeff-desktop" | "jeff") {
            return None;
        }

        let document_title = unsafe {
            let element = AXUIElementCreateApplication(pid);
            if element.is_null() {
                return Some(ActiveWindowContext {
                    app_name,
                    document_title: String::new(),
                    captured_at: unix_now(),
                });
            }

            let window_attr = make_cf_string("AXFocusedWindow");
            let mut window_ref: *const c_void = std::ptr::null();
            let win_err = if !window_attr.is_null() {
                AXUIElementCopyAttributeValue(element, window_attr, &mut window_ref)
            } else {
                -1
            };
            CFRelease(element);
            if !window_attr.is_null() {
                CFRelease(window_attr);
            }

            if win_err != AX_SUCCESS || window_ref.is_null() {
                return Some(ActiveWindowContext {
                    app_name,
                    document_title: String::new(),
                    captured_at: unix_now(),
                });
            }

            let title_attr = make_cf_string("AXTitle");
            let mut title_ref: *const c_void = std::ptr::null();
            let title_err = if !title_attr.is_null() {
                AXUIElementCopyAttributeValue(window_ref, title_attr, &mut title_ref)
            } else {
                -1
            };
            CFRelease(window_ref);
            if !title_attr.is_null() {
                CFRelease(title_attr);
            }

            if title_err == AX_SUCCESS && !title_ref.is_null() {
                let s = cf_string_to_rust(title_ref).unwrap_or_default();
                CFRelease(title_ref);
                s
            } else {
                String::new()
            }
        };

        // strip common app-name suffixes so the LLM and nudge logic see a clean
        // document name rather than "My Notes — TextEdit" or "Page - Chrome".
        let document_title_clean = super::strip_title_suffix(&document_title);

        Some(ActiveWindowContext {
            app_name,
            document_title: document_title_clean,
            captured_at: unix_now(),
        })
    }
}

// ---- non-macos stubs --------------------------------------------------------

#[cfg(not(target_os = "macos"))]
mod inner {
    use super::ActiveWindowContext;

    pub fn is_accessibility_trusted() -> bool {
        false
    }

    pub fn request_accessibility_permission() {}

    pub fn poll_active_window() -> Option<ActiveWindowContext> {
        None
    }
}

pub use inner::{is_accessibility_trusted, poll_active_window, request_accessibility_permission};

// strip common app-name suffixes from window titles so the document name
// presented to the LLM and the nudge logic is clean.
// browsers, editors, and OS apps commonly append " — App", " - App", or " | App"
// after the document or page title. rfind removes only the last separator
// occurrence so intermediate separators in the document name are preserved.
// e.g. "TypeScript - Wikipedia - Google Chrome" → "TypeScript - Wikipedia"
//      "Notes on taxes — TextEdit"              → "Notes on taxes"
//      "index.html | VSCode"                    → "index.html"
pub fn strip_title_suffix(title: &str) -> String {
    // em-dash first: highest precedence in macOS app naming conventions.
    for sep in &[" \u{2014} ", " - ", " | "] {
        if let Some(pos) = title.rfind(sep) {
            let before = title[..pos].trim();
            if !before.is_empty() {
                return before.to_string();
            }
        }
    }
    title.to_string()
}

// ---- unit tests -------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_accessibility_trusted_does_not_panic() {
        // just verify the call returns without panicking.
        let _ = is_accessibility_trusted();
    }

    #[test]
    fn poll_active_window_returns_none_gracefully() {
        // without accessibility permission, must return None, not panic.
        // if permission happens to be granted in the test environment the
        // function may return Some, which is also acceptable.
        let _ = poll_active_window();
    }

    #[test]
    fn strip_title_suffix_removes_app_name_after_em_dash() {
        assert_eq!(
            strip_title_suffix("Notes on taxes \u{2014} TextEdit"),
            "Notes on taxes"
        );
    }

    #[test]
    fn strip_title_suffix_removes_last_hyphen_separated_suffix() {
        // only the last separator is stripped, preserving intermediate ones.
        assert_eq!(
            strip_title_suffix("TypeScript - Wikipedia - Google Chrome"),
            "TypeScript - Wikipedia"
        );
    }

    #[test]
    fn strip_title_suffix_removes_pipe_suffix() {
        assert_eq!(strip_title_suffix("index.html | VSCode"), "index.html");
    }

    #[test]
    fn strip_title_suffix_prefers_em_dash_over_hyphen() {
        // em-dash is tried first; if present it wins even when a hyphen also exists.
        assert_eq!(
            strip_title_suffix("My Draft - v2 \u{2014} Pages"),
            "My Draft - v2"
        );
    }

    #[test]
    fn strip_title_suffix_is_no_op_when_no_separator_present() {
        assert_eq!(
            strip_title_suffix("plain document title"),
            "plain document title"
        );
    }

    #[test]
    fn strip_title_suffix_does_not_strip_when_nothing_remains() {
        // separator at position 0 would leave an empty prefix — keep original.
        assert_eq!(strip_title_suffix(" - only suffix"), " - only suffix");
    }
}
