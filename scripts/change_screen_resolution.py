#!/usr/bin/env python3
"""
Change the Windows display resolution using the Win32 API.
Usage:
  - List available modes: change_screen_resolution.py --list
  - Change resolution: change_screen_resolution.py --width 1024 --height 768
On non-Windows platforms this is a no-op that exits successfully.
"""
import sys
import os
import argparse


def enumerate_display_modes():
    """Yield DEVMODE structures for all display modes supported by the primary display."""
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


def main():
    parser = argparse.ArgumentParser(description="Change or list screen resolutions (Windows only)")
    parser.add_argument("--width", type=int, help="Width in pixels")
    parser.add_argument("--height", type=int, help="Height in pixels")
    parser.add_argument("--refresh", type=int, default=None, help="Display frequency (optional)")
    parser.add_argument("--list", action='store_true', help="List available display modes")
    args = parser.parse_args()

    if os.name != 'nt':
        if args.list:
            print("Not on Windows; no modes to list")
        else:
            print("Not running on Windows; skipping resolution change.")
        return 0

    if args.list:
        modes = enumerate_display_modes()
        if not modes:
            print("No modes enumerated or failed to query modes")
            return 1
        # Deduplicate and sort
        unique = sorted(set(modes), key=lambda m: (m[0], m[1], -m[2], -m[3]))
        print("Available display modes:")
        for w, h, freq, bpp in unique:
            print(f"  - {w}x{h} @ {freq}Hz, {bpp} bpp")
        return 0

    if args.width is None or args.height is None:
        print("Either --list or both --width and --height must be specified")
        return 2

    # Fallback to previous change behavior
    try:
        import ctypes
        from ctypes import wintypes
    except Exception as e:
        print(f"Failed to import ctypes: {e}")
        return 3

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

    # set requested values
    dm.dmPelsWidth = args.width
    dm.dmPelsHeight = args.height
    # Only set display frequency if explicitly requested; many hosts expose only specific frequencies
    flags = DM_PELSWIDTH | DM_PELSHEIGHT
    if args.refresh is not None:
        dm.dmDisplayFrequency = args.refresh
        flags |= DM_DISPLAYFREQUENCY
    dm.dmFields = flags

    # test the change first
    result = ChangeDisplaySettings(ctypes.byref(dm), 1)  # CDS_TEST=1
    if result == DISP_CHANGE_SUCCESSFUL:
        result = ChangeDisplaySettings(ctypes.byref(dm), 0)  # CDS_UPDATEREGISTRY=0 (apply temporarily)
        if result == DISP_CHANGE_SUCCESSFUL:
            print(f"Screen resolution changed to {args.width}x{args.height} (freq {args.refresh})")
            return 0
        elif result == DISP_CHANGE_RESTART:
            print("Change successful but system restart required")
            return 0
        else:
            print(f"Change failed with code: {result}")
            return 4
    else:
        print(f"Requested mode not supported (test result {result})")
        return 5

if __name__ == '__main__':
    sys.exit(main())
