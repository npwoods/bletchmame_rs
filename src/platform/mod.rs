mod other;

#[cfg(windows)]
mod windows;

#[cfg(unix)]
mod unix;

// declarations for Windows platform
#[cfg(target_os = "windows")]
#[rustfmt::skip]
pub use {
    windows::win_platform_init as platform_init,
    windows::win_interaction_monitor_init as interaction_monitor_init,
    windows::win_echo_interaction_monitor_main,
    windows::WinCommandExt as CommandExt,
    windows::WinWindowAttributesExt as WindowAttributesExt,
    windows::WinWindowExt as WindowExt
};

// declarations for Unix platforms
#[cfg(unix)]
#[rustfmt::skip]
pub use {
    other::other_platform_init as platform_init,
    unix::unix_interaction_monitor_init as interaction_monitor_init,
    other::OtherCommandExt as CommandExt,
    other::OtherWindowAttributesExt as WindowAttributesExt,
    other::OtherWindowExt as WindowExt,
};

// declarations for non-Windows/non-Unix platforms
#[cfg(all(not(windows), not(unix)))]
#[rustfmt::skip]
pub use {
    other::other_platform_init as platform_init,
    other::other_interaction_monitor_init as interaction_monitor_init,
    other::OtherCommandExt as CommandExt,
    other::OtherWindowAttributesExt as WindowAttributesExt,
    other::OtherWindowExt as WindowExt
};
