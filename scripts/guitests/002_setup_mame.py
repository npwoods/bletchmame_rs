#!/usr/bin/env python3
"""
GUI test: Launch app and setup MAME path via Settings -> Paths dialog.
"""
import sys
import argparse
import os
import time
import subprocess
import traceback

# helpers from common
from common import (
    wait_for_window, launch_app, activate_window, click_center,
    start_recording, stop_recording, wait_for_process_termination
)

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


def test_setup_mame(exe_path, exe_log, mame_dir):
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

        # Open Settings menu and choose Paths first (Alt+S, p)
        print("[INFO] Opening Settings menu and selecting Paths (Alt+S, p)...")
        pyautogui.hotkey('alt', 's')
    
        time.sleep(0.2)
        pyautogui.press('p')
        time.sleep(0.5)

        paths_dialog = wait_for_window("Paths", timeout=5)

        # In the Paths dialog, tab three times to reach the MAME executable selector, then Enter
        print("[INFO] Navigating Paths dialog: TAB x3, Enter to open file chooser")
        pyautogui.press('tab', presses=3, interval=0.12)
        time.sleep(0.15)
        pyautogui.press('enter')
        time.sleep(0.8)

        # Wait for the browse dialog
        browse_dialog = wait_for_window("Open", timeout=5)

        # Type the path to mame.exe (mame_dir + mame.exe) or fallback to mame.exe
        import pathlib
        chooser_path = str(pathlib.Path(mame_dir) / "mame.exe")

        # Remove control characters and trim
        chooser_path = re.sub(r'[\x00-\x1F\x7F]', '', chooser_path).strip()

        print(f"[INFO] Typing MAME exe path into file chooser: {chooser_path}")
        paste_text(chooser_path)
        time.sleep(0.5)

        # Confirm file chooser and then confirm Paths dialog
        pyautogui.press('enter')
        time.sleep(0.5)
        pyautogui.hotkey('shift', 'tab')
        time.sleep(0.1)
        pyautogui.hotkey('shift', 'tab')
        time.sleep(0.1)
        pyautogui.press('enter')
        time.sleep(3.0)

        # Since we configured MAME, the window should be ready
        window = wait_for_window(["[ready] BletchMameAuto"], timeout=30)

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
    parser = argparse.ArgumentParser()
    parser.add_argument('exe_path', help='Path to BletchMAME executable')
    parser.add_argument('--record', dest='record', help='Path to record video (mp4)', default=None)
    parser.add_argument('--log', '-l', dest='log', help='Value to pass to executable as --log', default='')
    parser.add_argument('--mame-dir', dest='mame_dir', help='Path to extracted MAME directory (optional)', default='')
    args = parser.parse_args()

    if args.record:
        try:
            os.makedirs(os.path.dirname(args.record), exist_ok=True)
            ok = start_recording(args.record)
            if not ok:
                print("[WARN] start_recording failed; recording will be disabled for this run")
        except Exception as e:
            print(f"[WARN] Failed to start recording: {e}")

    success = test_setup_mame(args.exe_path, args.log, args.mame_dir)

    try:
        stop_recording()
    except Exception:
        pass

    sys.exit(0 if success else 1)


if __name__ == '__main__':
    main()
