//! Context menu for right-click actions.

pub(crate) enum ContextMenuAction {
    Copy,
    Paste,
    SelectAll,
    Clear,
    SplitRight,
    SplitDown,
}

/// Show a native macOS right-click context menu and return the selected action.
///
/// Uses `popUpContextMenu:withEvent:forView:` which is synchronous — it blocks
/// until the user picks an item or dismisses the menu. Menu item callbacks
/// record the selected tag into a thread-local, which we read after the menu
/// closes.
#[cfg(target_os = "macos")]
pub(crate) fn show_context_menu(
    window: &winit::window::Window,
    has_selection: bool,
) -> Option<ContextMenuAction> {
    use objc2::rc::{Allocated, Id};
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, NSObject, Sel};
    use objc2::{class, msg_send, msg_send_id, sel};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::cell::Cell;

    let handle = match window.window_handle() {
        Ok(h) => h,
        Err(_) => return None,
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    let ns_view = appkit.ns_view.as_ptr() as *mut AnyObject;

    // Tag constants.
    const TAG_COPY: isize = 1;
    const TAG_PASTE: isize = 2;
    const TAG_SELECT_ALL: isize = 3;
    const TAG_CLEAR: isize = 4;
    const TAG_SPLIT_RIGHT: isize = 5;
    const TAG_SPLIT_DOWN: isize = 6;

    // Thread-local to communicate the selected tag from the ObjC callback.
    thread_local! {
        static SELECTED_TAG: Cell<isize> = const { Cell::new(0) };
    }

    // ObjC callback function for menu item selection.
    unsafe extern "C" fn menu_item_clicked(
        _this: *mut AnyObject,
        _sel: Sel,
        sender: *mut AnyObject,
    ) {
        let tag: isize = msg_send![&*sender, tag];
        SELECTED_TAG.with(|cell| cell.set(tag));
    }

    unsafe {
        // Register (or reuse) a small ObjC class with a `menuAction:` method.
        static REGISTERED: std::sync::Once = std::sync::Once::new();
        static mut MENU_TARGET_CLASS: *const AnyClass = std::ptr::null();

        REGISTERED.call_once(|| {
            let superclass = class!(NSObject);
            let mut builder = ClassBuilder::new("TermojinalMenuTarget", superclass)
                .expect("failed to create TermojinalMenuTarget class");
            builder.add_method(
                sel!(menuAction:),
                menu_item_clicked as unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            MENU_TARGET_CLASS = builder.register() as *const AnyClass;
        });

        // Create an instance of our target class.
        let target_class = &*MENU_TARGET_CLASS;
        let target: Id<NSObject> = msg_send_id![target_class, new];
        let action_sel = sel!(menuAction:);

        // Create the menu.
        let menu: Id<NSObject> = msg_send_id![class!(NSMenu), new];
        // Disable auto-enable so we can manually control enabled state.
        let () = msg_send![&*menu, setAutoenablesItems: false];

        // Helper: NSString from &str (null-terminated via CString).
        let make_nsstring = |s: &str| -> Id<NSObject> {
            let cstr = std::ffi::CString::new(s).unwrap();
            let ns: Id<NSObject> = msg_send_id![
                class!(NSString),
                stringWithUTF8String: cstr.as_ptr()
            ];
            ns
        };

        // Helper: create a menu item.
        let make_item = |title: &str, key_equiv: &str, tag: isize, enabled: bool| -> Id<NSObject> {
            let ns_title = make_nsstring(title);
            let ns_key = make_nsstring(key_equiv);
            let item: Allocated<NSObject> = msg_send_id![class!(NSMenuItem), alloc];
            let item: Id<NSObject> = msg_send_id![
                item,
                initWithTitle: &*ns_title,
                action: action_sel,
                keyEquivalent: &*ns_key
            ];
            let () = msg_send![&*item, setTarget: &*target];
            let () = msg_send![&*item, setTag: tag];
            let () = msg_send![&*item, setEnabled: enabled];
            item
        };

        let make_separator = || -> Id<NSObject> { msg_send_id![class!(NSMenuItem), separatorItem] };

        // --- Build menu ---
        let copy_item = make_item("Copy", "c", TAG_COPY, has_selection);
        let () = msg_send![&*menu, addItem: &*copy_item];

        let paste_item = make_item("Paste", "v", TAG_PASTE, true);
        let () = msg_send![&*menu, addItem: &*paste_item];

        let sep1 = make_separator();
        let () = msg_send![&*menu, addItem: &*sep1];

        let select_all_item = make_item("Select All", "a", TAG_SELECT_ALL, true);
        let () = msg_send![&*menu, addItem: &*select_all_item];

        let clear_item = make_item("Clear", "", TAG_CLEAR, true);
        let () = msg_send![&*menu, addItem: &*clear_item];

        let sep2 = make_separator();
        let () = msg_send![&*menu, addItem: &*sep2];

        let split_right_item = make_item("Split Right", "d", TAG_SPLIT_RIGHT, true);
        let () = msg_send![&*menu, addItem: &*split_right_item];

        let split_down_item = make_item("Split Down", "D", TAG_SPLIT_DOWN, true);
        let () = msg_send![&*menu, addItem: &*split_down_item];

        // Reset selected tag before showing menu.
        SELECTED_TAG.with(|cell| cell.set(0));

        // Get current event from NSApplication (needed for popUpContextMenu).
        let ns_app: Id<NSObject> = msg_send_id![class!(NSApplication), sharedApplication];
        let current_event: Option<Id<NSObject>> = msg_send_id![&*ns_app, currentEvent];

        let Some(event) = current_event else {
            return None;
        };

        // popUpContextMenu:withEvent:forView: is synchronous — blocks until
        // the user selects an item or dismisses the menu.
        let () = msg_send![
            class!(NSMenu),
            popUpContextMenu: &*menu,
            withEvent: &*event,
            forView: &*ns_view
        ];

        // Read back which tag was selected (0 = dismissed without selection).
        let selected = SELECTED_TAG.with(|cell| cell.get());

        match selected {
            TAG_COPY => Some(ContextMenuAction::Copy),
            TAG_PASTE => Some(ContextMenuAction::Paste),
            TAG_SELECT_ALL => Some(ContextMenuAction::SelectAll),
            TAG_CLEAR => Some(ContextMenuAction::Clear),
            TAG_SPLIT_RIGHT => Some(ContextMenuAction::SplitRight),
            TAG_SPLIT_DOWN => Some(ContextMenuAction::SplitDown),
            _ => None,
        }
    }
}
