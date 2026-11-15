//! Whether the platform asks for reduced motion.
//!
//! Read here, in the app, and kept on the theme, because the design system
//! stays free of platform code. The shell reads it when the window first
//! draws and again each time the window becomes key, which is when a reader
//! who just flipped the switch in System Settings comes back.

/// macOS's Reduce Motion accessibility setting.
#[cfg(target_os = "macos")]
pub(crate) fn system_prefers_reduced_motion() -> bool {
    objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

/// No platform setting to read: motion stays on.
#[cfg(not(target_os = "macos"))]
pub(crate) fn system_prefers_reduced_motion() -> bool {
    false
}
