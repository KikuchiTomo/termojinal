//! macOS desktop notifications via UNUserNotificationCenter API.
//!
//! Uses `objc2` raw message sends to talk to `UNUserNotificationCenter`,
//! the modern replacement for the deprecated `NSUserNotificationCenter`
//! (removed in macOS 13+).

/// Initialize the notification system. Call once at startup.
///
/// With UNUserNotificationCenter the bundle identifier is read automatically
/// from the running .app bundle, so no explicit registration is needed.
/// This function is kept as a no-op for call-site compatibility.
pub fn init() {
    // UNUserNotificationCenter derives the app identity from the bundle,
    // so there is nothing to configure here.
    log::debug!("notification::init() — UNUserNotificationCenter uses bundle ID automatically");
}

/// Send a macOS desktop notification.
///
/// `title` — notification title (e.g. "termojinal").
/// `body`  — notification body text.
/// `sound` — if `true`, plays the default notification sound.
pub fn send_notification(title: &str, body: &str, sound: bool) {
    use std::ffi::CString;

    use objc2::rc::Id;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{msg_send, msg_send_id};

    // --- UNUserNotificationCenter ---
    let center_class = match AnyClass::get("UNUserNotificationCenter") {
        Some(cls) => cls,
        None => {
            log::warn!("UNUserNotificationCenter class not available");
            return;
        }
    };
    let center: Id<AnyObject> =
        unsafe { msg_send_id![center_class, currentNotificationCenter] };

    // --- UNMutableNotificationContent ---
    let content_class = match AnyClass::get("UNMutableNotificationContent") {
        Some(cls) => cls,
        None => {
            log::warn!("UNMutableNotificationContent class not available");
            return;
        }
    };
    let content: Id<AnyObject> = unsafe { msg_send_id![content_class, new] };

    // --- NSString helpers ---
    let ns_string_class = match AnyClass::get("NSString") {
        Some(cls) => cls,
        None => {
            log::warn!("NSString class not available");
            return;
        }
    };

    let title_c = CString::new(title).unwrap_or_default();
    let title_ns: Id<AnyObject> = unsafe {
        msg_send_id![ns_string_class, stringWithUTF8String: title_c.as_ptr()]
    };

    let body_c = CString::new(body).unwrap_or_default();
    let body_ns: Id<AnyObject> = unsafe {
        msg_send_id![ns_string_class, stringWithUTF8String: body_c.as_ptr()]
    };

    unsafe {
        let _: () = msg_send![&*content, setTitle: &*title_ns];
        let _: () = msg_send![&*content, setBody: &*body_ns];
    }

    // --- Sound ---
    if sound {
        if let Some(sound_class) = AnyClass::get("UNNotificationSound") {
            let default_sound: Id<AnyObject> =
                unsafe { msg_send_id![sound_class, defaultSound] };
            unsafe {
                let _: () = msg_send![&*content, setSound: &*default_sound];
            }
        }
    }

    // --- Unique request identifier ---
    let uuid_class = match AnyClass::get("NSUUID") {
        Some(cls) => cls,
        None => {
            log::warn!("NSUUID class not available");
            return;
        }
    };
    let uuid: Id<AnyObject> = unsafe { msg_send_id![uuid_class, new] };
    let uuid_string: Id<AnyObject> = unsafe { msg_send_id![&*uuid, UUIDString] };

    // --- UNNotificationRequest ---
    let request_class = match AnyClass::get("UNNotificationRequest") {
        Some(cls) => cls,
        None => {
            log::warn!("UNNotificationRequest class not available");
            return;
        }
    };
    let request: Id<AnyObject> = unsafe {
        msg_send_id![
            request_class,
            requestWithIdentifier: &*uuid_string
            content: &*content
            trigger: std::ptr::null::<AnyObject>()
        ]
    };

    // --- Deliver ---
    unsafe {
        let _: () = msg_send![
            &*center,
            addNotificationRequest: &*request
            withCompletionHandler: std::ptr::null::<AnyObject>()
        ];
    }

    log::debug!("notification sent: {title}");
}

/// Request notification permission if not already granted.
///
/// Uses `UNUserNotificationCenter` to check the current authorization status.
/// Only runs when the process is inside a `.app` bundle (has a valid bundle
/// identifier). Bare CLI binaries (e.g. from Homebrew) are skipped because
/// `UNUserNotificationCenter` does not work correctly without proper code
/// signing and bundle identity.
#[cfg(target_os = "macos")]
pub fn request_notification_permission_if_needed() {
    use std::sync::mpsc;

    use block2::RcBlock;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2::{msg_send, msg_send_id};

    // Only proceed if running inside a .app bundle.
    // UNUserNotificationCenter requires a valid bundle identifier and code
    // signature; without one, authorizationStatus always returns NotDetermined
    // and the dialog reappears on every launch.
    let is_bundled = std::env::current_exe()
        .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
        .unwrap_or(false);
    if !is_bundled {
        log::debug!("not running from .app bundle — skipping notification permission check");
        return;
    }

    let center_class = match AnyClass::get("UNUserNotificationCenter") {
        Some(cls) => cls,
        None => {
            log::warn!("UNUserNotificationCenter class not available");
            return;
        }
    };

    let center: objc2::rc::Id<AnyObject> =
        unsafe { msg_send_id![center_class, currentNotificationCenter] };

    // Check current notification authorization status.
    let (tx, rx) = mpsc::channel::<i64>();

    let check_block = RcBlock::new(move |settings: *mut AnyObject| {
        if settings.is_null() {
            let _ = tx.send(-1);
            return;
        }
        // UNAuthorizationStatus: 0 = NotDetermined, 1 = Denied, 2 = Authorized,
        // 3 = Provisional, 4 = Ephemeral
        let status: i64 = unsafe { msg_send![settings, authorizationStatus] };
        let _ = tx.send(status);
    });

    unsafe {
        let _: () = msg_send![&*center, getNotificationSettingsWithCompletionHandler: &*check_block];
    }

    // Wait for the result with a timeout.
    let status = match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(s) => s,
        Err(_) => {
            log::warn!("timeout checking notification permission status");
            return;
        }
    };

    // UNAuthorizationStatus values:
    //   0 = NotDetermined, 1 = Denied, 2 = Authorized, 3 = Provisional, 4 = Ephemeral
    match status {
        2 | 3 | 4 => {
            log::info!("notification permission already granted (status={status})");
        }
        0 => {
            // User has never been asked — request permission.
            log::info!("notification permission not determined, requesting...");

            // UNAuthorizationOptionBadge | UNAuthorizationOptionSound | UNAuthorizationOptionAlert
            // = (1<<0) | (1<<1) | (1<<2) = 7
            let options: u64 = 7;

            let request_block =
                RcBlock::new(move |granted: Bool, _error: *mut AnyObject| {
                    if granted.as_bool() {
                        log::info!("notification permission granted by user");
                    } else {
                        log::info!("notification permission denied by user");
                    }
                });

            unsafe {
                let _: () = msg_send![&*center, requestAuthorizationWithOptions: options completionHandler: &*request_block];
            }
        }
        1 => {
            log::info!("notification permission denied (user previously denied)");
        }
        _ => {
            log::info!("notification permission status: unknown ({status})");
        }
    }
}
