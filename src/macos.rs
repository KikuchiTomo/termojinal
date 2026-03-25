//! macOS-specific window and dock operations.

pub(crate) fn set_macos_window_transparent(window: &winit::window::Window) {
    use objc2::rc::Id;
    use objc2::runtime::NSObject;
    use objc2::{class, msg_send, msg_send_id};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = match window.window_handle() {
        Ok(h) => h,
        Err(_) => return,
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    let ns_view = appkit.ns_view.as_ptr() as *const NSObject;

    unsafe {
        // ns_view.window -> NSWindow
        let ns_window: Option<Id<NSObject>> = msg_send_id![ns_view, window];
        if let Some(ns_window) = ns_window {
            let clear_color: Id<NSObject> = msg_send_id![class!(NSColor), clearColor];
            let () = msg_send![&*ns_window, setBackgroundColor: &*clear_color];
            let () = msg_send![&*ns_window, setOpaque: false];
            log::info!("macOS window background set to clear for transparency");
        }
    }
}

// ---------------------------------------------------------------------------
// Window icon
// ---------------------------------------------------------------------------

/// Set the macOS Dock icon from an embedded PNG using raw objc messaging.
#[cfg(target_os = "macos")]
pub(crate) fn set_dock_icon() {
    use objc2::rc::Id;
    use objc2::runtime::NSObject;
    use objc2::{class, msg_send, msg_send_id};

    // Load icon PNG and add ~18% transparent padding (Apple HIG standard).
    let png_bytes = include_bytes!("../resources/Assets.xcassets/AppIcon.appiconset/256.png");
    let padded = match add_icon_padding(png_bytes) {
        Some(data) => data,
        None => png_bytes.to_vec(),
    };

    unsafe {
        let cls = class!(NSData);
        let ptr = padded.as_ptr() as *const std::ffi::c_void;
        let len = padded.len();
        let data: Id<NSObject> = msg_send_id![
            cls, dataWithBytes: ptr, length: len
        ];

        let cls = class!(NSImage);
        let image: Option<Id<NSObject>> = msg_send_id![
            msg_send_id![cls, alloc],
            initWithData: &*data
        ];

        if let Some(image) = image {
            let cls = class!(NSApplication);
            let app: Id<NSObject> = msg_send_id![cls, sharedApplication];
            let () = msg_send![&*app, setApplicationIconImage: &*image];
            log::info!("dock icon set");
        }
    }
}

/// Add transparent padding around an icon PNG (~18% on each side per Apple HIG).
#[cfg(target_os = "macos")]
pub(crate) fn add_icon_padding(png_bytes: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png).ok()?;
    let src = img.to_rgba8();

    // Target: 1024x1024 canvas with icon at ~824x824 (80% of canvas), centered.
    let canvas_size = 1024u32;
    let icon_size = (canvas_size as f32 * 0.80) as u32;
    let offset = (canvas_size - icon_size) / 2;

    let resized = image::imageops::resize(
        &src,
        icon_size,
        icon_size,
        image::imageops::FilterType::Lanczos3,
    );

    let mut canvas = image::RgbaImage::new(canvas_size, canvas_size);
    image::imageops::overlay(&mut canvas, &resized, offset as i64, offset as i64);

    let mut buf = std::io::Cursor::new(Vec::new());
    canvas.write_to(&mut buf, image::ImageFormat::Png).ok()?;
    Some(buf.into_inner())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn set_dock_icon() {}
