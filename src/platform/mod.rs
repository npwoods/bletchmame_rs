mod other;

#[cfg(target_os = "windows")]
mod windows;

// declarations for Windows platform
#[cfg(target_os = "windows")]
#[rustfmt::skip]
pub use {
    windows::childwnd::WinChildWindow as ChildWindow,
    windows::win_platform_init as platform_init,
    windows::WinCommandExt as CommandExt,
    windows::WinWindowAttributesExt as WindowAttributesExt,
    windows::WinWindowExt as WindowExt
};

// declarations for non-Windows platforms
#[cfg(not(target_os = "windows"))]
#[rustfmt::skip]
pub use {
    other::OtherChildWindow as ChildWindow,
    other::other_platform_init as platform_init,
    other::OtherCommandExt as CommandExt,
    other::OtherWindowAttributesExt as WindowAttributesExt,
    other::OtherWindowExt as WindowExt
};
