// phase 22: rate-only typing activity. the monitor records only event timing
// and a derived boolean. it never stores key values, key codes, or text.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

const TYPING_WINDOW: Duration = Duration::from_secs(2);
const TYPING_THRESHOLD_COUNT: usize = 2;

#[derive(Clone)]
pub struct TypingActivityState {
    inner: Arc<TypingActivityInner>,
}

struct TypingActivityInner {
    enabled: AtomicBool,
    monitor_available: AtomicBool,
    recent_keydowns: Mutex<VecDeque<Instant>>,
    last_error: Mutex<Option<String>>,
}

impl TypingActivityState {
    pub fn new(enabled: bool) -> Self {
        Self {
            inner: Arc::new(TypingActivityInner {
                enabled: AtomicBool::new(enabled),
                monitor_available: AtomicBool::new(false),
                recent_keydowns: Mutex::new(VecDeque::new()),
                last_error: Mutex::new(None),
            }),
        }
    }

    pub fn clone_state(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.inner.enabled.store(enabled, Ordering::SeqCst);
        if !enabled {
            self.clear();
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::SeqCst)
    }

    pub fn note_keydown(&self) {
        if !self.is_enabled() {
            return;
        }
        self.note_keydown_at(Instant::now());
    }

    pub fn is_typing(&self) -> bool {
        if !self.is_enabled() {
            return false;
        }
        self.prune_and_count(Instant::now()) >= TYPING_THRESHOLD_COUNT
    }

    pub fn clear(&self) {
        if let Ok(mut events) = self.inner.recent_keydowns.lock() {
            events.clear();
        }
    }

    pub fn set_monitor_available(&self, available: bool) {
        self.inner
            .monitor_available
            .store(available, Ordering::SeqCst);
    }

    pub fn set_monitor_error(&self, error: String) {
        self.set_monitor_available(false);
        if let Ok(mut guard) = self.inner.last_error.lock() {
            *guard = Some(error);
        }
    }

    pub fn monitor_available(&self) -> bool {
        self.inner.monitor_available.load(Ordering::SeqCst)
    }

    pub fn last_error(&self) -> Option<String> {
        self.inner.last_error.lock().ok().and_then(|guard| guard.clone())
    }

    fn note_keydown_at(&self, instant: Instant) {
        if let Ok(mut events) = self.inner.recent_keydowns.lock() {
            events.push_back(instant);
            prune_events(&mut events, instant);
        }
    }

    fn prune_and_count(&self, now: Instant) -> usize {
        self.inner
            .recent_keydowns
            .lock()
            .map(|mut events| {
                prune_events(&mut events, now);
                events.len()
            })
            .unwrap_or(0)
    }
}

fn prune_events(events: &mut VecDeque<Instant>, now: Instant) {
    while let Some(front) = events.front().copied() {
        if now.duration_since(front) <= TYPING_WINDOW {
            break;
        }
        events.pop_front();
    }
}

pub fn start_global_typing_monitor(state: TypingActivityState) {
    platform::start_global_typing_monitor(state);
}

#[cfg(target_os = "macos")]
mod platform {
    use super::TypingActivityState;
    use std::ffi::c_void;

    type CGEventTapProxy = *mut c_void;
    type CGEventType = u32;
    type CGEventRef = *mut c_void;
    type CGEventMask = u64;
    type CGEventTapCallBack =
        extern "C" fn(CGEventTapProxy, CGEventType, CGEventRef, *mut c_void) -> CGEventRef;

    const K_CG_SESSION_EVENT_TAP: u32 = 1;
    const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
    const K_CG_EVENT_TAP_OPTION_LISTEN_ONLY: u32 = 1;
    const K_CG_EVENT_KEY_DOWN: CGEventType = 10;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            events_of_interest: CGEventMask,
            callback: CGEventTapCallBack,
            user_info: *mut c_void,
        ) -> *const c_void;
        fn CGEventTapEnable(tap: *const c_void, enable: bool);
        fn CFMachPortCreateRunLoopSource(
            allocator: *const c_void,
            port: *const c_void,
            order: isize,
        ) -> *const c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFRunLoopCommonModes: *const c_void;
        fn CFRunLoopGetCurrent() -> *const c_void;
        fn CFRunLoopAddSource(rl: *const c_void, source: *const c_void, mode: *const c_void);
        fn CFRunLoopRun();
        fn CFRelease(cf: *const c_void);
    }

    extern "C" fn event_tap_callback(
        _proxy: CGEventTapProxy,
        event_type: CGEventType,
        event: CGEventRef,
        user_info: *mut c_void,
    ) -> CGEventRef {
        if event_type == K_CG_EVENT_KEY_DOWN && !user_info.is_null() {
            unsafe {
                let state = &*(user_info as *const TypingActivityState);
                state.note_keydown();
            }
        }
        event
    }

    pub fn start_global_typing_monitor(state: TypingActivityState) {
        let _ = std::thread::Builder::new()
            .name("jeff-typing-rate-monitor".to_string())
            .spawn(move || unsafe {
                let boxed_state = Box::new(state.clone());
                let user_info = Box::into_raw(boxed_state) as *mut c_void;
                let mask = 1u64 << K_CG_EVENT_KEY_DOWN;
                let tap = CGEventTapCreate(
                    K_CG_SESSION_EVENT_TAP,
                    K_CG_HEAD_INSERT_EVENT_TAP,
                    K_CG_EVENT_TAP_OPTION_LISTEN_ONLY,
                    mask,
                    event_tap_callback,
                    user_info,
                );
                if tap.is_null() {
                    state.set_monitor_error(
                        "macOS did not grant the key-rate monitor. Enable Input Monitoring or Accessibility if prompted.".to_string(),
                    );
                    return;
                }

                state.set_monitor_available(true);
                CGEventTapEnable(tap, true);
                let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
                if source.is_null() {
                    state.set_monitor_error("failed to create typing monitor run loop source".to_string());
                    CFRelease(tap);
                    return;
                }

                let run_loop = CFRunLoopGetCurrent();
                CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
                CFRelease(source);
                CFRunLoopRun();
                CFRelease(tap);
            });
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::TypingActivityState;

    pub fn start_global_typing_monitor(state: TypingActivityState) {
        state.set_monitor_error(
            "global key-rate monitor is only implemented on macOS right now".to_string(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_state_uses_rate_only_window() {
        let state = TypingActivityState::new(true);
        let now = Instant::now();
        state.note_keydown_at(now);
        assert!(!state.prune_and_count(now).ge(&TYPING_THRESHOLD_COUNT));
        state.note_keydown_at(now + Duration::from_millis(500));
        assert!(state.prune_and_count(now + Duration::from_millis(500)) >= TYPING_THRESHOLD_COUNT);
        assert_eq!(
            state.prune_and_count(now + Duration::from_secs(3)),
            0,
            "old keydown times should be pruned without storing key values"
        );
    }

    #[test]
    fn disabled_state_clears_activity() {
        let state = TypingActivityState::new(true);
        state.note_keydown();
        state.note_keydown();
        assert!(state.is_typing());
        state.set_enabled(false);
        assert!(!state.is_typing());
        state.note_keydown();
        assert!(!state.is_typing());
    }
}
