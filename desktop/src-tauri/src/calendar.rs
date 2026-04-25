// calendar.rs — macOS EventKit integration for upcoming event context
//
// all calendar data stays in memory (CalendarState). nothing is written to SQLite.
// polling is gated on the privacy_calendar_context_enabled setting and
// the user granting calendar permission. code is conditionally compiled
// on #[cfg(target_os = "macos")] so other platforms compile cleanly.

use anyhow::Result;

use crate::{models::CalendarEventDto, state::CalendarState};

// -------------------------------------------------------------------------
// public interface (all platforms)
// -------------------------------------------------------------------------

pub fn request_calendar_permission() -> Result<bool> {
    #[cfg(target_os = "macos")]
    {
        inner::request_permission()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(false)
    }
}

pub fn get_calendar_permission_status() -> String {
    #[cfg(target_os = "macos")]
    {
        inner::authorization_status()
    }
    #[cfg(not(target_os = "macos"))]
    {
        "not_determined".to_string()
    }
}

/// fetch the soonest upcoming event within the next `hours` hours.
pub fn fetch_next_event(hours: u8) -> Option<CalendarEventDto> {
    #[cfg(target_os = "macos")]
    {
        inner::next_event(hours)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = hours;
        None
    }
}

/// return the cached next event from CalendarState.
pub fn get_cached_next_event(state: &CalendarState) -> Result<Option<CalendarEventDto>> {
    Ok(state.current())
}

// -------------------------------------------------------------------------
// macOS implementation via EventKit C bridge
// -------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod inner {
    use std::ffi::{c_char, c_void, CStr};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::models::CalendarEventDto;

    // Objective-C runtime
    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend(obj: *mut c_void, sel: *mut c_void, ...) -> *mut c_void;
    }

    // Foundation
    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}

    // EventKit
    #[link(name = "EventKit", kind = "framework")]
    extern "C" {}

    fn cls(name: &[u8]) -> *mut c_void {
        unsafe { objc_getClass(name.as_ptr() as *const c_char) }
    }

    fn sel(name: &[u8]) -> *mut c_void {
        unsafe { sel_registerName(name.as_ptr() as *const c_char) }
    }

    /// send a message with no args → pointer return
    unsafe fn msg0(receiver: *mut c_void, selector: *mut c_void) -> *mut c_void {
        // objc_msgSend signature varies; for pointer returns this is safe.
        objc_msgSend(receiver, selector)
    }

    fn unix_now_f64() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

    // EKAuthorizationStatus: 0=notDetermined 1=restricted 2=denied 3=authorized
    pub fn authorization_status() -> String {
        unsafe {
            let ek_class = cls(b"EKEventStore\0");
            if ek_class.is_null() {
                return "not_determined".to_string();
            }
            // authorizationStatusForEntityType: — EKEntityTypeEvent = 0
            // method returns NSInteger (isize on 64-bit)
            type ObjcMsgSendInt = unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> isize;
            let imp: ObjcMsgSendInt = std::mem::transmute(
                objc_msgSend as unsafe extern "C" fn(*mut c_void, *mut c_void, ...) -> *mut c_void,
            );
            let s = sel(b"authorizationStatusForEntityType:\0");
            let status = imp(ek_class, s, 0 /* EKEntityTypeEvent */);
            match status {
                3 => "granted".to_string(),
                2 => "denied".to_string(),
                _ => "not_determined".to_string(),
            }
        }
    }

    pub fn request_permission() -> anyhow::Result<bool> {
        let status = authorization_status();
        if status == "granted" {
            return Ok(true);
        }
        // spawn the permission request — the OS shows the dialog asynchronously
        unsafe {
            let ek_class = cls(b"EKEventStore\0");
            if ek_class.is_null() {
                return Ok(false);
            }
            let store = msg0(ek_class, sel(b"new\0"));
            if !store.is_null() {
                // requestAccessToEntityType:completion: — fire and forget
                // we pass a nil completion block; the OS will still show the dialog
                type MsgSendEntityCompletion = unsafe extern "C" fn(
                    *mut c_void,
                    *mut c_void,
                    usize,
                    *mut c_void,
                ) -> *mut c_void;
                let imp: MsgSendEntityCompletion = std::mem::transmute(
                    objc_msgSend
                        as unsafe extern "C" fn(*mut c_void, *mut c_void, ...) -> *mut c_void,
                );
                let s = sel(b"requestAccessToEntityType:completion:\0");
                imp(
                    store,
                    s,
                    0, /* EKEntityTypeEvent */
                    std::ptr::null_mut(),
                );
            }
        }
        Ok(false)
    }

    pub fn next_event(hours: u8) -> Option<CalendarEventDto> {
        if authorization_status() != "granted" {
            return None;
        }
        unsafe {
            let ek_class = cls(b"EKEventStore\0");
            if ek_class.is_null() {
                return None;
            }
            let store = msg0(ek_class, sel(b"new\0"));
            if store.is_null() {
                return None;
            }

            // NSDate representing now and now + hours*3600
            let nsdate_class = cls(b"NSDate\0");
            if nsdate_class.is_null() {
                return None;
            }
            let now: *mut c_void = msg0(nsdate_class, sel(b"date\0"));
            if now.is_null() {
                return None;
            }

            let end_secs = (hours as f64) * 3600.0;
            type MsgSendTimeInterval =
                unsafe extern "C" fn(*mut c_void, *mut c_void, f64) -> *mut c_void;
            let add_imp: MsgSendTimeInterval = std::mem::transmute(
                objc_msgSend as unsafe extern "C" fn(*mut c_void, *mut c_void, ...) -> *mut c_void,
            );
            let end = add_imp(now, sel(b"dateByAddingTimeInterval:\0"), end_secs);
            if end.is_null() {
                return None;
            }

            // calendars for events
            type MsgSendEntityType =
                unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> *mut c_void;
            let calendars_imp: MsgSendEntityType = std::mem::transmute(
                objc_msgSend as unsafe extern "C" fn(*mut c_void, *mut c_void, ...) -> *mut c_void,
            );
            let calendars = calendars_imp(store, sel(b"calendarsForEntityType:\0"), 0);
            if calendars.is_null() {
                return None;
            }

            // build predicate
            type MsgSendPredicate = unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *mut c_void,
            ) -> *mut c_void;
            let pred_imp: MsgSendPredicate = std::mem::transmute(
                objc_msgSend as unsafe extern "C" fn(*mut c_void, *mut c_void, ...) -> *mut c_void,
            );
            let predicate = pred_imp(
                store,
                sel(b"predicateForEventsWithStartDate:endDate:calendars:\0"),
                now,
                end,
                calendars,
            );
            if predicate.is_null() {
                return None;
            }

            // fetch events
            type MsgSendEvents =
                unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void;
            let events_imp: MsgSendEvents = std::mem::transmute(
                objc_msgSend as unsafe extern "C" fn(*mut c_void, *mut c_void, ...) -> *mut c_void,
            );
            let events = events_imp(store, sel(b"eventsMatchingPredicate:\0"), predicate);
            if events.is_null() {
                return None;
            }

            // count
            type MsgSendCount = unsafe extern "C" fn(*mut c_void, *mut c_void) -> usize;
            let count_imp: MsgSendCount = std::mem::transmute(
                objc_msgSend as unsafe extern "C" fn(*mut c_void, *mut c_void, ...) -> *mut c_void,
            );
            let count = count_imp(events, sel(b"count\0"));
            if count == 0 {
                return None;
            }

            // get first event
            type MsgSendObjectAtIndex =
                unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> *mut c_void;
            let obj_imp: MsgSendObjectAtIndex = std::mem::transmute(
                objc_msgSend as unsafe extern "C" fn(*mut c_void, *mut c_void, ...) -> *mut c_void,
            );
            let first = obj_imp(events, sel(b"objectAtIndex:\0"), 0);
            if first.is_null() {
                return None;
            }

            // title
            let title_ns: *mut c_void = msg0(first, sel(b"title\0"));
            if title_ns.is_null() {
                return None;
            }
            type MsgSendUtf8 = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *const c_char;
            let utf8_imp: MsgSendUtf8 = std::mem::transmute(
                objc_msgSend as unsafe extern "C" fn(*mut c_void, *mut c_void, ...) -> *mut c_void,
            );
            let title_ptr = utf8_imp(title_ns, sel(b"UTF8String\0"));
            let title = if title_ptr.is_null() {
                return None;
            } else {
                CStr::from_ptr(title_ptr).to_string_lossy().into_owned()
            };

            // start date
            let start_date: *mut c_void = msg0(first, sel(b"startDate\0"));
            if start_date.is_null() {
                return None;
            }
            type MsgSendF64 = unsafe extern "C" fn(*mut c_void, *mut c_void) -> f64;
            let interval_imp: MsgSendF64 = std::mem::transmute(
                objc_msgSend as unsafe extern "C" fn(*mut c_void, *mut c_void, ...) -> *mut c_void,
            );
            let start_ts = interval_imp(start_date, sel(b"timeIntervalSince1970\0"));
            let now_ts = unix_now_f64();
            let minutes_until = ((start_ts - now_ts) / 60.0) as i64;

            // format start as simple ISO string using timestamp
            let starts_at = {
                use chrono::{TimeZone, Utc};
                Utc.timestamp_opt(start_ts as i64, 0)
                    .single()
                    .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                    .unwrap_or_default()
            };

            Some(CalendarEventDto {
                title,
                starts_at,
                minutes_until,
            })
        }
    }
}
