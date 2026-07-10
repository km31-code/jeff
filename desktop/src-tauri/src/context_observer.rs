// phase 20: active window context observer.
// polls NSWorkspace for the frontmost application and AXUIElement for the
// focused window title. all macOS api calls are in the inner module gated on
// cfg(target_os = "macos"). on other platforms the module provides stubs that
// always return None / false so the rest of the codebase compiles unchanged.
//
// phase 31: adds content observation polling — reads active document text
// via AXUIElement to compute a deterministic ContentObservation summary.
// Browser-extension observations enter through selection_capture.rs. In both
// paths raw text stays in perception modules/in-memory state; only summaries
// cross into prompts, storage, or model payloads.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const CONTENT_OBSERVATION_POLL_INTERVAL_SECONDS: u64 = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DraftState {
    Early,
    Mid,
    Late,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChangeMagnitude {
    None,
    Minor,
    Major,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentObservation {
    pub word_count: usize,
    pub draft_state: DraftState,
    pub content_changed: bool,
    pub change_magnitude: ChangeMagnitude,
    pub stable_for_ticks: u32,
    pub captured_at: i64,
}

#[derive(Debug, Default)]
pub struct ContentObservationState {
    pub raw_text: Option<String>,
    pub prior_text: Option<String>,
    pub observation: Option<ContentObservation>,
    pub last_captured_at: Option<i64>,
    pub capture_attempt_count: u32,
    pub capture_failed_count: u32,
    // apex b1: counts-only structural signals from the semantic document model.
    // no raw document text — safe to surface into the snapshot summary.
    pub document_paragraph_count: usize,
    pub document_structure_changed: bool,
    pub document_max_churn: u32,
    pub document_churn_hotspots: usize,
    // apex b6: browser-origin provenance for the latest observation. metadata
    // only; raw browser text follows the same in-memory boundary as AX text.
    pub source_origin: Option<String>,
    pub source_title: Option<String>,
}

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
        fn CFArrayGetCount(the_array: *const c_void) -> i64;
        fn CFArrayGetValueAtIndex(the_array: *const c_void, idx: i64) -> *const c_void;
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

    pub fn get_frontmost_pid() -> Option<i32> {
        let (app_name, pid) = get_frontmost_app()?;
        if matches!(app_name.as_str(), "Jeff" | "jeff-desktop" | "jeff") {
            return None;
        }
        Some(pid)
    }

    // read the text content of the frontmost application's focused text area
    // via the macOS Accessibility API. returns None silently on any failure.
    // does not call AXIsProcessTrustedWithOptions — the Phase 20 title-polling
    // path already asserts permission before this is called.
    // result is truncated to 50,000 characters as a memory guard.
    pub fn read_ax_document_text(pid: i32) -> Option<String> {
        unsafe {
            let app_el = AXUIElementCreateApplication(pid);
            if app_el.is_null() {
                return None;
            }

            // try the focused UI element first (fast path for most text editors).
            let focused_attr = make_cf_string("AXFocusedUIElement");
            let mut focused_el: *const c_void = std::ptr::null();
            let focus_err = if !focused_attr.is_null() {
                let err = AXUIElementCopyAttributeValue(app_el, focused_attr, &mut focused_el);
                CFRelease(focused_attr);
                err
            } else {
                -1
            };

            if focus_err == AX_SUCCESS && !focused_el.is_null() {
                let role = get_element_role(focused_el);
                if matches!(role.as_deref(), Some("AXTextArea") | Some("AXWebArea")) {
                    if let Some(text) = get_element_value(focused_el) {
                        CFRelease(focused_el);
                        CFRelease(app_el);
                        return Some(truncate_ax_text(text, 50_000));
                    }
                }
                CFRelease(focused_el);
            }

            // fall back: traverse children of the focused window (max depth 4).
            let window_attr = make_cf_string("AXFocusedWindow");
            let mut window_el: *const c_void = std::ptr::null();
            let win_err = if !window_attr.is_null() {
                let err = AXUIElementCopyAttributeValue(app_el, window_attr, &mut window_el);
                CFRelease(window_attr);
                err
            } else {
                -1
            };
            CFRelease(app_el);

            if win_err != AX_SUCCESS || window_el.is_null() {
                return None;
            }

            let result = find_text_in_children(window_el, "AXTextArea", 4)
                .or_else(|| find_text_in_children(window_el, "AXWebArea", 4));
            CFRelease(window_el);
            result.map(|t| truncate_ax_text(t, 50_000))
        }
    }

    // depth-first search for an element with the given role, returns its AXValue.
    // the CFArrayRef for each level's children is kept alive while its children
    // are examined, so element pointers remain valid throughout.
    unsafe fn find_text_in_children(
        element: *const c_void,
        target_role: &str,
        depth: u32,
    ) -> Option<String> {
        if depth == 0 {
            return None;
        }
        let children_attr = make_cf_string("AXChildren");
        if children_attr.is_null() {
            return None;
        }
        let mut children_ref: *const c_void = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(element, children_attr, &mut children_ref);
        CFRelease(children_attr);
        if err != AX_SUCCESS || children_ref.is_null() {
            return None;
        }
        let count = CFArrayGetCount(children_ref);
        let mut result = None;
        'search: for i in 0..count {
            let child = CFArrayGetValueAtIndex(children_ref, i);
            if child.is_null() {
                continue;
            }
            if get_element_role(child).as_deref() == Some(target_role) {
                if let Some(text) = get_element_value(child) {
                    result = Some(text);
                    break 'search;
                }
            }
            if let Some(text) = find_text_in_children(child, target_role, depth - 1) {
                result = Some(text);
                break 'search;
            }
        }
        CFRelease(children_ref);
        result
    }

    unsafe fn get_element_role(element: *const c_void) -> Option<String> {
        let attr = make_cf_string("AXRole");
        if attr.is_null() {
            return None;
        }
        let mut val_ref: *const c_void = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(element, attr, &mut val_ref);
        CFRelease(attr);
        if err != AX_SUCCESS || val_ref.is_null() {
            return None;
        }
        let role = cf_string_to_rust(val_ref);
        CFRelease(val_ref);
        role
    }

    unsafe fn get_element_value(element: *const c_void) -> Option<String> {
        let attr = make_cf_string("AXValue");
        if attr.is_null() {
            return None;
        }
        let mut val_ref: *const c_void = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(element, attr, &mut val_ref);
        CFRelease(attr);
        if err != AX_SUCCESS || val_ref.is_null() {
            return None;
        }
        let text = cf_string_to_rust(val_ref);
        CFRelease(val_ref);
        text
    }

    fn truncate_ax_text(s: String, max_chars: usize) -> String {
        if s.chars().count() <= max_chars {
            s
        } else {
            s.chars().take(max_chars).collect()
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

    pub fn get_frontmost_pid() -> Option<i32> {
        None
    }

    pub fn read_ax_document_text(_pid: i32) -> Option<String> {
        None
    }
}

pub use inner::{
    get_frontmost_pid, is_accessibility_trusted, poll_active_window,
    read_ax_document_text, request_accessibility_permission,
};

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

// ---- content observation summarizer ----------------------------------------

// pure, deterministic summary of captured document text. no i/o, no llm call.
// the raw text never travels beyond context_observer.rs; only this struct does.
pub fn summarize_content_observation(
    text: &str,
    prior: Option<&str>,
    prior_word_count: usize,
    stable_for_ticks: u32,
) -> ContentObservation {
    let word_count = text.split_whitespace().count();

    let draft_state = if word_count < 200 {
        DraftState::Early
    } else if word_count > 1000 {
        DraftState::Late
    } else {
        DraftState::Mid
    };

    // apex b1: the first-80-char prefix comparison is retired. change detection
    // is now full-text equality here (any edit anywhere counts), and the real
    // structural/churn signal comes from the semantic document model
    // (document_model.rs), surfaced separately in the snapshot.
    let content_changed = match prior {
        Some(p) => text.trim() != p.trim(),
        None => false,
    };

    let change_magnitude = if !content_changed {
        ChangeMagnitude::None
    } else {
        let diff = (word_count as i64 - prior_word_count as i64).abs();
        let threshold = (prior_word_count / 10) as i64;
        if diff < threshold {
            ChangeMagnitude::Minor
        } else {
            ChangeMagnitude::Major
        }
    };

    let new_stable = if content_changed { 0 } else { stable_for_ticks + 1 };

    ContentObservation {
        word_count,
        draft_state,
        content_changed,
        change_magnitude,
        stable_for_ticks: new_stable,
        captured_at: unix_now(),
    }
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

    #[test]
    fn summarize_empty_text_is_early_draft() {
        let obs = summarize_content_observation("", None, 0, 0);
        assert_eq!(obs.word_count, 0);
        assert_eq!(obs.draft_state, DraftState::Early);
        assert!(!obs.content_changed);
        assert_eq!(obs.change_magnitude, ChangeMagnitude::None);
    }

    #[test]
    fn summarize_detects_major_change() {
        // prior is 100 "aaa" words; current is 200 "bbb" words. the full text
        // differs, so content_changed = true (b1: full-text equality, no prefix).
        let prior = "aaa ".repeat(100);
        let current = "bbb ".repeat(200);
        let obs = summarize_content_observation(&current, Some(&prior), 100, 0);
        assert!(obs.content_changed, "content should be marked changed");
        assert_eq!(obs.change_magnitude, ChangeMagnitude::Major);
        assert_eq!(obs.stable_for_ticks, 0);
    }

    #[test]
    fn summarize_minor_change() {
        // prior 100 words, new 105 words — diff 5 < 10% of 100 = 10 → Minor
        let prior = "word ".repeat(100);
        // new text has different prefix to trigger content_changed
        let current = "changed ".repeat(1) + &"word ".repeat(104);
        let obs = summarize_content_observation(&current, Some(&prior), 100, 0);
        assert!(obs.content_changed);
        assert_eq!(obs.change_magnitude, ChangeMagnitude::Minor);
    }

    #[test]
    fn stable_for_ticks_resets_on_change() {
        let prior = "aaa ".repeat(100);
        let current = "bbb ".repeat(100);
        let obs = summarize_content_observation(&current, Some(&prior), 100, 5);
        assert!(obs.content_changed);
        assert_eq!(obs.stable_for_ticks, 0);
    }

    #[test]
    fn stable_for_ticks_increments_when_unchanged() {
        let text = "word ".repeat(50);
        let obs = summarize_content_observation(&text, Some(&text), 50, 3);
        assert!(!obs.content_changed);
        assert_eq!(obs.stable_for_ticks, 4);
    }

    #[test]
    fn draft_state_mid_range() {
        let text = "word ".repeat(500);
        let obs = summarize_content_observation(&text, None, 0, 0);
        assert_eq!(obs.draft_state, DraftState::Mid);
    }

    #[test]
    fn draft_state_late_over_1000_words() {
        let text = "word ".repeat(1001);
        let obs = summarize_content_observation(&text, None, 0, 0);
        assert_eq!(obs.draft_state, DraftState::Late);
    }
}
