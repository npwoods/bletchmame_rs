#!/usr/bin/env python3
"""
GUI test: Launch app and import mame.ini via Settings -> Import MAME Ini dialog.
"""
import sys
import argparse
import os
import time
import subprocess
import traceback

# helpers from common
from common import set_screenshot_dir, take_screenshot, sleep_and_maybe_capture, wait_for_window, launch_app, activate_window, click_center

import pyautogui
import re


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
        take_screenshot("after_start")

        activate_window(window)
        sleep_and_maybe_capture(0.5, "after_activate", force_capture=True)
        click_center(window)
        sleep_and_maybe_capture(0.5, "after_click_center")

        # Open Settings menu and choose Paths first (Alt+S, p)
        print("[INFO] Opening Settings menu and selecting Paths (Alt+S, p)...")
        pyautogui.hotkey('alt', 's')
    
        sleep_and_maybe_capture(0.2, "after_open_settings")
        pyautogui.press('p')
        sleep_and_maybe_capture(0.5, "after_select_paths")
        take_screenshot("paths_dialog_before_wait")

        paths_dialog = wait_for_window("Paths", timeout=5)
        take_screenshot("paths_dialog")

        # In the Paths dialog, tab three times to reach the MAME executable selector, then Enter
        print("[INFO] Navigating Paths dialog: TAB x3, Enter to open file chooser")
        pyautogui.press('tab', presses=3, interval=0.12)
        sleep_and_maybe_capture(0.15, "after_tab_paths")
        pyautogui.press('enter')
        sleep_and_maybe_capture(0.8, "after_open_filechooser")

        # Type the path to mame.exe (mame_dir + mame.exe) or fallback to mame.exe
        import pathlib
        chooser_path = str(pathlib.Path(mame_dir) / "mame.exe")

        # Remove control characters and trim
        chooser_path = re.sub(r'[\x00-\x1F\x7F]', '', chooser_path).strip()

        print(f"[INFO] Typing MAME exe path into file chooser: {chooser_path}")
        paste_text(chooser_path)
        sleep_and_maybe_capture(0.5, "after_typing_mame_path")

        # Confirm file chooser and then confirm Paths dialog
        pyautogui.press('enter')
        sleep_and_maybe_capture(0.5, "after_confirm_filechooser")
        pyautogui.hotkey('shift', 'tab')
        time.sleep(0.1)
        pyautogui.hotkey('shift', 'tab')
        time.sleep(0.1)
        pyautogui.press('enter')
        sleep_and_maybe_capture(3.0, "after_confirm_paths")

        # Now open Settings -> Import MAME Ini
        print("[INFO] Opening Settings menu for Import MAME Ini (Alt+S)...")
        pyautogui.hotkey('alt', 's')
        sleep_and_maybe_capture(0.2, "after_open_settings_for_import")

        # Try pressing 'i' (common mnemonic for Import) otherwise navigate
        pyautogui.press('i')
        sleep_and_maybe_capture(1.0, "after_select_import")
        take_screenshot("import_dialog_before")

        # Wait for the file dialog titled "Import MAME INI"
        try:
            dialog = wait_for_window("Import MAME INI", timeout=10)
        except TimeoutError:
            # Try a bit longer or assume dialog is focused
            print("[WARN] Import dialog did not appear by title; attempting to type path anyway")
            dialog = None

        # Type the path into the filename field
        import pathlib
        mame_ini_path = str(pathlib.Path(mame_dir) / "mame.ini")
        print(f"[INFO] Typing path: {mame_ini_path}")
        paste_text(mame_ini_path)
        sleep_and_maybe_capture(0.5, "after_typing_path")

        # Confirm the dialog. Some dialogs require shifting focus; press Shift+Tab then Enter
        pyautogui.press('enter')
        time.sleep(0.5)
        pyautogui.hotkey('shift', 'tab')
        sleep_and_maybe_capture(0.5, "before_confirm_import")
        pyautogui.press('enter')
        sleep_and_maybe_capture(0.25, "after_confirm_import")

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

            sleep_and_maybe_capture(0.5, "waiting_import_close")
        else:
            print("[WARN] Import dialog did not close within expected time")

        sleep_and_maybe_capture(3.0, "after_import")

        # Attempt to close app via Alt+F then X (as in other tests)
        print("[INFO] Exiting application via Alt+F, x")
        pyautogui.hotkey('alt', 'f')
        sleep_and_maybe_capture(0.5, "after_open_file_menu")
        pyautogui.press('x')
        sleep_and_maybe_capture(2, "after_exit_press")

        try:
            return_code = process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            print("[WARN] Application did not exit within 30s; killing")
            try:
                process.kill()
                return_code = process.wait(timeout=5)
            except Exception as e:
                print(f"[ERROR] Failed to kill process: {e}")
                return_code = None

        if return_code == 0:
            print("[OK] Application exited successfully with code 0")
            return True
        else:
            print(f"[FAIL] Application exited with code: {return_code}")
            return False

    except Exception as e:
        print(f"[FAIL] Test failed: {e}")
        traceback.print_exc()
        try:
            process.kill()
        except:
            pass
        return False


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('exe_path', help='Path to BletchMAME executable')
    parser.add_argument('--screenshot_dir', '-s', dest='screenshot_dir', help='Directory to save screenshots', default=None)
    parser.add_argument('--log', '-l', dest='log', help='Value to pass to executable as --log', default='')
    parser.add_argument('--mame_dir', dest='mame_dir', help='Path to extracted MAME directory (optional)', default='')
    args = parser.parse_args()

    if args.screenshot_dir:
        set_screenshot_dir(args.screenshot_dir)

    success = test_import_mame_ini(args.exe_path, args.log, args.mame_dir)
    sys.exit(0 if success else 1)


if __name__ == '__main__':
    main()
