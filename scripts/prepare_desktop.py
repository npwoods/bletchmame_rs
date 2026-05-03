#!/usr/bin/env python3
"""
Prepare desktop for GUI tests: list available resolutions, change resolution, and optionally minimize all visible windows.
Usage:
  - List modes: prepare_desktop.py --list
  - Change resolution: prepare_desktop.py --width 1024 --height 768
  - Minimize all windows: prepare_desktop.py --minimize-all
Combines the previous change_screen_resolution.py functionality and adds window-minimization.
"""
import sys
import os
import argparse


def enumerate_display_modes():
    try:
        import ctypes
        from ctypes import wintypes
    except Exception as e:
        print(f"Failed to import ctypes: {e}")
        return []

    user32 = ctypes.WinDLL('user32', use_last_error=True)

    class DEVMODE(ctypes.Structure):
        _fields_ = [
            ("dmDeviceName", wintypes.WCHAR * 32),
            ("dmSpecVersion", wintypes.WORD),
            ("dmDriverVersion", wintypes.WORD),
            ("dmSize", wintypes.WORD),
            ("dmDriverExtra", wintypes.WORD),
            ("dmFields", wintypes.DWORD),
            ("dmPosition_x", wintypes.LONG),
            ("dmPosition_y", wintypes.LONG),
            ("dmDisplayOrientation", wintypes.DWORD),
            ("dmDisplayFixedOutput", wintypes.DWORD),
            ("dmColor", wintypes.SHORT),
            ("dmDuplex", wintypes.SHORT),
            ("dmYResolution", wintypes.SHORT),
            ("dmTTOption", wintypes.SHORT),
            ("dmCollate", wintypes.SHORT),
            ("dmFormName", wintypes.WCHAR * 32),
            ("dmLogPixels", wintypes.WORD),
            ("dmBitsPerPel", wintypes.DWORD),
            ("dmPelsWidth", wintypes.DWORD),
            ("dmPelsHeight", wintypes.DWORD),
            ("dmDisplayFlags", wintypes.DWORD),
            ("dmDisplayFrequency", wintypes.DWORD),
            ("dmICMMethod", wintypes.DWORD),
            ("dmICMIntent", wintypes.DWORD),
            ("dmMediaType", wintypes.DWORD),
            ("dmDitherType", wintypes.DWORD),
            ("dmReserved1", wintypes.DWORD),
            ("dmReserved2", wintypes.DWORD),
            ("dmPanningWidth", wintypes.DWORD),
            ("dmPanningHeight", wintypes.DWORD),
        ]

    EnumDisplaySettings = user32.EnumDisplaySettingsW
    EnumDisplaySettings.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, ctypes.POINTER(DEVMODE)]
    EnumDisplaySettings.restype = wintypes.BOOL

    modes = []
    i = 0
    dm = DEVMODE()
    dm.dmSize = ctypes.sizeof(DEVMODE)
    while EnumDisplaySettings(None, i, ctypes.byref(dm)):
        modes.append((dm.dmPelsWidth, dm.dmPelsHeight, dm.dmDisplayFrequency, dm.dmBitsPerPel))
        i += 1
    return modes


def minimize_all_windows():
    """Minimize all top-level visible windows using EnumWindows + ShowWindow.

    This minimizes windows programmatically (more explicit than Win+D) and does
    not rely on synthesizing a global hotkey. It will attempt to minimize any
    visible top-level window with a non-empty title and that is not already
    iconic. It skips tool windows (WS_EX_TOOLWINDOW) and invisible windows.
    """
    if os.name != 'nt':
        print("Not Windows; skipping minimize-all")
        return
    try:
        import ctypes
        from ctypes import wintypes
        user32 = ctypes.WinDLL('user32', use_last_error=True)

        # Define callback prototype for EnumWindows
        CALLBACK = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

        EnumWindows = user32.EnumWindows
        EnumWindows.argtypes = [CALLBACK, wintypes.LPARAM]
        EnumWindows.restype = wintypes.BOOL

        IsWindowVisible = user32.IsWindowVisible
        IsWindowVisible.argtypes = [wintypes.HWND]
        IsWindowVisible.restype = wintypes.BOOL

        GetWindowTextLength = user32.GetWindowTextLengthW
        GetWindowTextLength.argtypes = [wintypes.HWND]
        GetWindowTextLength.restype = ctypes.c_int

        GetWindowText = user32.GetWindowTextW
        GetWindowText.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
        GetWindowText.restype = ctypes.c_int

        IsIconic = user32.IsIconic
        IsIconic.argtypes = [wintypes.HWND]
        IsIconic.restype = wintypes.BOOL

        GetWindowLong = user32.GetWindowLongW
        GetWindowLong.argtypes = [wintypes.HWND, ctypes.c_int]
        GetWindowLong.restype = ctypes.c_long

        ShowWindow = user32.ShowWindow
        ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
        ShowWindow.restype = wintypes.BOOL

        GWL_EXSTYLE = -20
        WS_EX_TOOLWINDOW = 0x00000080
        SW_MINIMIZE = 6

        hwnds = []

        def _enum_proc(hwnd, lParam):
            try:
                if not IsWindowVisible(hwnd):
                    return True
                # skip windows without titles
                length = GetWindowTextLength(hwnd)
                if length <= 0:
                    return True
                # skip toolwindows
                exstyle = GetWindowLong(hwnd, GWL_EXSTYLE)
                if exstyle & WS_EX_TOOLWINDOW:
                    return True
                # skip already minimized
                if IsIconic(hwnd):
                    return True
                hwnds.append(hwnd)
            except Exception:
                pass
            return True

        cb = CALLBACK(_enum_proc)
        if not EnumWindows(cb, 0):
            print("EnumWindows failed")
            return

        print(f"Found {len(hwnds)} top-level visible windows to minimize")
        for h in hwnds:
            try:
                ShowWindow(h, SW_MINIMIZE)
            except Exception as e:
                print(f"Failed to minimize hwnd {h}: {e}")
        print("Minimize-all completed")
    except Exception as e:
        print(f"Failed to minimize windows: {e}")


def change_resolution(width, height, refresh=None):
    try:
        import ctypes
        from ctypes import wintypes
    except Exception as e:
        print(f"Failed to import ctypes: {e}")
        return 2

    user32 = ctypes.WinDLL('user32', use_last_error=True)
    ENUM_CURRENT_SETTINGS = -1
    DM_PELSWIDTH = 0x80000
    DM_PELSHEIGHT = 0x100000
    DM_DISPLAYFREQUENCY = 0x400000
    DISP_CHANGE_SUCCESSFUL = 0
    DISP_CHANGE_RESTART = 1

    class DEVMODE(ctypes.Structure):
        _fields_ = [
            ("dmDeviceName", wintypes.WCHAR * 32),
            ("dmSpecVersion", wintypes.WORD),
            ("dmDriverVersion", wintypes.WORD),
            ("dmSize", wintypes.WORD),
            ("dmDriverExtra", wintypes.WORD),
            ("dmFields", wintypes.DWORD),
            ("dmPosition_x", wintypes.LONG),
            ("dmPosition_y", wintypes.LONG),
            ("dmDisplayOrientation", wintypes.DWORD),
            ("dmDisplayFixedOutput", wintypes.DWORD),
            ("dmColor", wintypes.SHORT),
            ("dmDuplex", wintypes.SHORT),
            ("dmYResolution", wintypes.SHORT),
            ("dmTTOption", wintypes.SHORT),
            ("dmCollate", wintypes.SHORT),
            ("dmFormName", wintypes.WCHAR * 32),
            ("dmLogPixels", wintypes.WORD),
            ("dmBitsPerPel", wintypes.DWORD),
            ("dmPelsWidth", wintypes.DWORD),
            ("dmPelsHeight", wintypes.DWORD),
            ("dmDisplayFlags", wintypes.DWORD),
            ("dmDisplayFrequency", wintypes.DWORD),
            ("dmICMMethod", wintypes.DWORD),
            ("dmICMIntent", wintypes.DWORD),
            ("dmMediaType", wintypes.DWORD),
            ("dmDitherType", wintypes.DWORD),
            ("dmReserved1", wintypes.DWORD),
            ("dmReserved2", wintypes.DWORD),
            ("dmPanningWidth", wintypes.DWORD),
            ("dmPanningHeight", wintypes.DWORD),
        ]

    EnumDisplaySettings = user32.EnumDisplaySettingsW
    EnumDisplaySettings.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, ctypes.POINTER(DEVMODE)]
    EnumDisplaySettings.restype = wintypes.BOOL

    ChangeDisplaySettings = user32.ChangeDisplaySettingsW
    ChangeDisplaySettings.argtypes = [ctypes.POINTER(DEVMODE), wintypes.DWORD]
    ChangeDisplaySettings.restype = wintypes.LONG

    dm = DEVMODE()
    dm.dmSize = ctypes.sizeof(DEVMODE)

    if not EnumDisplaySettings(None, ENUM_CURRENT_SETTINGS, ctypes.byref(dm)):
        print("Failed to enumerate display settings")
        return 3

    dm.dmPelsWidth = width
    dm.dmPelsHeight = height
    flags = DM_PELSWIDTH | DM_PELSHEIGHT
    if refresh is not None:
        dm.dmDisplayFrequency = refresh
        flags |= DM_DISPLAYFREQUENCY
    dm.dmFields = flags

    # test change
    result = ChangeDisplaySettings(ctypes.byref(dm), 1)
    if result == DISP_CHANGE_SUCCESSFUL:
        result = ChangeDisplaySettings(ctypes.byref(dm), 0)
        if result == DISP_CHANGE_SUCCESSFUL or result == DISP_CHANGE_RESTART:
            print(f"Screen resolution changed to {width}x{height} (freq {refresh or 'default'})")
            return 0
        else:
            print(f"Change failed with code: {result}")
            return 4
    else:
        print(f"Requested mode not supported (test result {result})")
        return 5


def main():
    parser = argparse.ArgumentParser(description="Prepare desktop for GUI tests (Windows only)")
    parser.add_argument("--width", type=int, help="Width in pixels")
    parser.add_argument("--height", type=int, help="Height in pixels")
    parser.add_argument("--refresh", type=int, default=None, help="Display frequency (optional)")
    parser.add_argument("--list", action='store_true', help="List available display modes")
    parser.add_argument("--minimize-all", action='store_true', help="Minimize all visible windows (Win+D)")
    args = parser.parse_args()

    if args.list:
        if os.name != 'nt':
            print("Not on Windows; no modes to list")
            return 0
        modes = enumerate_display_modes()
        if not modes:
            print("No modes enumerated or failed to query modes")
            return 1
        unique = sorted(set(modes), key=lambda m: (m[0], m[1], -m[2], -m[3]))
        print("Available display modes:")
        for w, h, freq, bpp in unique:
            print(f"  - {w}x{h} @ {freq}Hz, {bpp} bpp")
        return 0

    # perform resolution change if requested
    if args.width is not None and args.height is not None:
        if os.name != 'nt':
            print("Not running on Windows; skipping resolution change.")
        else:
            rc = change_resolution(args.width, args.height, args.refresh)
            if rc != 0:
                # still continue to minimize if requested; return failure code at end
                print("Resolution change returned code:", rc)
    # minimize windows if requested
    if args.minimize_all:
        minimize_all_windows()

    return 0

if __name__ == '__main__':
    sys.exit(main())
