mod other;

#[cfg(target_os = "windows")]
mod windows;

// declarations for Windows platform
#[cfg(target_os = "windows")]
#[rustfmt::skip]
pub use {
    windows::win_platform_init as platform_init,
    windows::win_echo_console_main,
    windows::WinCommandExt as CommandExt,
    windows::WinWindowAttributesExt as WindowAttributesExt,
    windows::WinWindowExt as WindowExt,
    windows::WinNamedPipe
};

// declarations for non-Windows platforms
#[cfg(not(target_os = "windows"))]
#[rustfmt::skip]
pub use {
    other::other_platform_init as platform_init,
    other::OtherCommandExt as CommandExt,
    other::OtherWindowAttributesExt as WindowAttributesExt,
    other::OtherWindowExt as WindowExt
};
