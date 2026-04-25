// phase 19: macos login-item registration through SMAppService.
// macos 13+ exposes mainAppService for registering the main app itself as a
// login item. non-macos builds return a clear unsupported error.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginItemStatus {
    NotRegistered,
    Enabled,
    RequiresApproval,
    NotFound,
    Unsupported,
}

impl LoginItemStatus {
    pub fn is_enabled_or_pending(self) -> bool {
        matches!(self, Self::Enabled | Self::RequiresApproval)
    }
}

#[cfg(target_os = "macos")]
mod inner {
    use super::LoginItemStatus;
    use objc2::{class, msg_send, runtime::AnyObject};
    use std::ffi::c_char;

    // force-link ServiceManagement.framework so SMAppService is available.
    #[link(name = "ServiceManagement", kind = "framework")]
    extern "C" {}

    fn map_status(raw: isize) -> LoginItemStatus {
        match raw {
            1 => LoginItemStatus::Enabled,
            2 => LoginItemStatus::RequiresApproval,
            3 => LoginItemStatus::NotFound,
            _ => LoginItemStatus::NotRegistered,
        }
    }

    unsafe fn service() -> Result<*mut AnyObject, String> {
        let cls = class!(SMAppService);
        let service: *mut AnyObject = msg_send![cls, mainAppService];
        if service.is_null() {
            Err("macOS Login Item service is unavailable".to_string())
        } else {
            Ok(service)
        }
    }

    unsafe fn ns_error_message(error: *mut AnyObject) -> String {
        if error.is_null() {
            return String::new();
        }
        let description: *mut AnyObject = msg_send![error, localizedDescription];
        if description.is_null() {
            return String::new();
        }
        let cstr: *const c_char = msg_send![description, UTF8String];
        if cstr.is_null() {
            return String::new();
        }
        format!(": {}", std::ffi::CStr::from_ptr(cstr).to_string_lossy())
    }

    pub fn status() -> Result<LoginItemStatus, String> {
        unsafe {
            let service = service()?;
            let raw: isize = msg_send![service, status];
            Ok(map_status(raw))
        }
    }

    pub fn set_enabled(enabled: bool) -> Result<LoginItemStatus, String> {
        unsafe {
            let service = service()?;
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let ok: bool = if enabled {
                msg_send![service, registerAndReturnError: &mut error]
            } else {
                msg_send![service, unregisterAndReturnError: &mut error]
            };

            let next_status = status().unwrap_or(LoginItemStatus::Unsupported);
            if ok {
                return Ok(next_status);
            }

            // SMAppService reports an error if a service is already in the
            // requested state. Treat the observed target state as success.
            if enabled && next_status.is_enabled_or_pending() {
                return Ok(next_status);
            }
            if !enabled
                && matches!(
                    next_status,
                    LoginItemStatus::NotRegistered | LoginItemStatus::NotFound
                )
            {
                return Ok(next_status);
            }

            let operation = if enabled { "enable" } else { "disable" };
            let detail = ns_error_message(error);
            Err(format!("failed to {operation} macOS Login Item{detail}"))
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod inner {
    use super::LoginItemStatus;

    pub fn status() -> Result<LoginItemStatus, String> {
        Ok(LoginItemStatus::Unsupported)
    }

    pub fn set_enabled(_enabled: bool) -> Result<LoginItemStatus, String> {
        Err("launch at login is only available on macOS".to_string())
    }
}

pub fn login_item_status() -> Result<LoginItemStatus, String> {
    inner::status()
}

pub fn set_login_item_enabled(enabled: bool) -> Result<LoginItemStatus, String> {
    inner::set_enabled(enabled)
}

pub fn login_item_enabled_or_pending() -> Result<bool, String> {
    Ok(login_item_status()?.is_enabled_or_pending())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_or_pending_statuses_are_truthy() {
        assert!(!LoginItemStatus::NotRegistered.is_enabled_or_pending());
        assert!(LoginItemStatus::Enabled.is_enabled_or_pending());
        assert!(LoginItemStatus::RequiresApproval.is_enabled_or_pending());
        assert!(!LoginItemStatus::NotFound.is_enabled_or_pending());
        assert!(!LoginItemStatus::Unsupported.is_enabled_or_pending());
    }
}
