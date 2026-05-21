#!/usr/bin/env python3
"""
GUI test: Import mame.ini via Settings -> Import MAME Ini dialog.
"""
import sys
import os
import time
import traceback
import pyautogui
import re
import pathlib

# helpers from common
from common import (
    wait_for_window, launch_app, activate_window, click_center,
    start_recording, stop_recording, wait_for_process_termination,
    create_arg_parser
)


def paste_text(text):
    # Remove control characters and trim
    text = re.sub(r'[\x00-\x1F\x7F]', '', text).strip()
    try:
        import pyperclip
        pyperclip.copy(text)
        if os.name == 'nt':
            pyautogui.hotkey('ctrl', 'v')
        else:
            pyautogui.hotkey('command', 'v')
        time.sleep(0.05)
    except Exception:
        # Fallback to typing if clipboard isn't available
        pyautogui.write(text, interval=0.02)


def test_import_mame_ini(exe_path, exe_log, mame_dir):
    try:
        process = launch_app(exe_path, exe_log)
    except Exception as e:
        print(f"[FAIL] Could not start app: {e}")
        return False

    try:
        window = wait_for_window(["[ready] BletchMameAuto", "[report] BletchMameAuto"], timeout=30)

        activate_window(window)
        time.sleep(0.5)
        click_center(window)
        time.sleep(0.5)

        # Now open Settings -> Import MAME Ini
        print("[INFO] Opening Settings menu for Import MAME Ini (Alt+S)...")
        pyautogui.hotkey('alt', 's')
        time.sleep(0.2)

        # Try pressing 'i' (common mnemonic for Import) otherwise navigate
        pyautogui.press('i')
        time.sleep(1.0)

        # Wait for the file dialog titled "Import MAME INI"
        try:
            dialog = wait_for_window("Import MAME INI", timeout=10)
        except TimeoutError:
            # Try a bit longer or assume dialog is focused
            print("[WARN] Import dialog did not appear by title; attempting to type path anyway")
            dialog = None

        # Type the path into the filename field
        mame_ini_path = str(pathlib.Path(mame_dir) / "mame.ini")
        print(f"[INFO] Typing path: {mame_ini_path}")
        paste_text(mame_ini_path)
        time.sleep(0.5)

        # Confirm the dialog. Some dialogs require shifting focus; press Shift+Tab then Enter
        pyautogui.press('enter')
        time.sleep(0.5)
        pyautogui.hotkey('shift', 'tab')
        time.sleep(0.5)
        pyautogui.press('enter')
        time.sleep(0.25)

        # Wait for the Import dialog to close and for the main window to be responsive
        print("[INFO] Waiting for Import dialog to close and main window to become ready")
        try:
            import pygetwindow
        except Exception:
            pygetwindow = None

        start_wait = time.time()
        wait_timeout = 15
        while time.time() - start_wait < wait_timeout:
            dialog_still_present = False
            if pygetwindow:
                try:
                    wins = pygetwindow.getAllWindows()
                    dialog_still_present = any(getattr(w, 'title', '') == 'Import MAME INI' for w in wins)
                except Exception:
                    dialog_still_present = False

            if not dialog_still_present:
                # ensure main window title is back (ready or report)
                try:
                    _ = wait_for_window(["[ready] BletchMameAuto", "[report] BletchMameAuto"], timeout=1)
                    print("[INFO] Import dialog closed and main window ready")
                    break
                except TimeoutError:
                    pass

            time.sleep(0.5)
        else:
            print("[WARN] Import dialog did not close within expected time")

        time.sleep(3.0)

        # Attempt to close app via Alt+F then X (as in other tests)
        print("[INFO] Exiting application via Alt+F, x")
        pyautogui.hotkey('alt', 'f')
        time.sleep(0.5)
        pyautogui.press('x')
        time.sleep(2.0)

        return_code = wait_for_process_termination(process)

        if return_code == 0:
            print("[OK] Application exited successfully with code 0")
            return True
        else:
            print(f"[FAIL] Application exited with code: {return_code}")
            return False

    except Exception as e:
        print(f"[FAIL] Test failed: {e}")
        traceback.print_exc()
        wait_for_process_termination(process, timeout=1)
        return False


def main():
    # Setup
    parser = create_arg_parser()
    args = parser.parse_args()
    start_recording(args.record)

    # Run the test
    success = test_import_mame_ini(args.exe_path, args.log, args.mame_dir)

    # Cleanup
    stop_recording()
    sys.exit(0 if success else 1)

if __name__ == '__main__':
    main()
