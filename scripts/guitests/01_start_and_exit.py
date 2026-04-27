#!/usr/bin/env python3
"""
GUI test script for BletchMAME

Tests that:
1. The application window opens
2. The File menu is accessible
3. Clicking Exit properly shuts down the application

This script accepts an optional --screenshot_dir argument. When provided,
screenshots will be saved after notable pauses and on errors so CI jobs can
upload them for inspection.
"""

import sys
import subprocess
import time
import pyautogui
import os
import argparse
import pathlib
import datetime

from common import set_screenshot_dir, take_screenshot, sleep_and_maybe_capture, wait_for_window, launch_app, activate_window, click_center

# common.py disables pyautogui.FAILSAFE already



def test_gui(exe_path, exe_log):
    """Test the GUI application"""
    try:
        process = launch_app(exe_path, exe_log)
    except Exception as e:
        print(f"[FAIL] Could not start app: {e}")
        return False

    # process started by launch_app
    
    try:
        print(f"[INFO] Waiting for window to appear...")
        window = wait_for_window(["[ready] BletchMameAuto", "[report] BletchMameAuto"], timeout=30)
        
        if window:
            print(f"[INFO] Window appeared successfully")
            
            # Activate and focus the window
            print("[INFO] Activating window...")
            try:
                window.activate()
                sleep_and_maybe_capture(0, "after_activate", force_capture=True)
                
                # Move cursor into window and click to ensure focus
                center_x = window.left + window.width // 2
                center_y = window.top + window.height // 2
                print(f"[INFO] Clicking window center at ({center_x}, {center_y})")
                pyautogui.click(center_x, center_y)
                time.sleep(0.5)
            except Exception as e:
                print(f"[WARN] Warning: Could not activate window: {e}")
        else:
            print("[WARN] Window not detected but process is running, attempting keyboard shortcuts anyway")
        
        print("[INFO] Opening File menu with Alt+F...")
        pyautogui.hotkey('alt', 'f')
        sleep_and_maybe_capture(1.5, "after_open_file_menu")
        
        print("[INFO] Sending 'x' to click Exit...")
        pyautogui.press('x')
        sleep_and_maybe_capture(2, "after_exit_press")
        
        # Wait for the process to terminate
        print("[INFO] Waiting for application to exit...")
        try:
            return_code = process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            print("[WARN] Application did not exit within 30s; attempting graceful shutdown")
            try:
                process.terminate()
                try:
                    return_code = process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    print("[WARN] terminate() did not stop process; killing")
                    try:
                        process.kill()
                        return_code = process.wait(timeout=5)
                    except Exception as e:
                        print(f"[ERROR] Failed to kill process: {e}")
                        return_code = None
            except Exception as e:
                print(f"[ERROR] Failed to terminate process: {e}")
                return_code = None

        if return_code == 0:
            print("[OK] Application exited successfully with code 0")
            return True
        else:
            print(f"[FAIL] Application exited with code: {return_code}")
            return False
            
    except TimeoutError as e:
        print(f"[FAIL] Test failed: {e}")
        take_screenshot("timeout_error")
        try:
            process.terminate()
            process.wait(timeout=5)
        except:
            try:
                process.kill()
            except:
                pass
        return False
    except Exception as e:
        print(f"[FAIL] Test failed with error: {e}")
        import traceback
        traceback.print_exc()
        take_screenshot("exception")
        try:
            process.terminate()
            process.wait(timeout=5)
        except:
            try:
                process.kill()
            except:
                pass
        return False
    except KeyboardInterrupt:
        print("\n[SCRIPT][FAIL] Test interrupted by user")
        take_screenshot("keyboard_interrupt")
        try:
            process.terminate()
        except:
            pass
        return False


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('exe_path', help='Path to BletchMAME executable')
    parser.add_argument('--screenshot_dir', '-s', dest='screenshot_dir', help='Directory to save screenshots', default=None)
    parser.add_argument('--log', '-l', dest='log', help='Value to pass to executable as --log', default='')
    # Accept --mame_dir for compatibility but ignore it
    parser.add_argument('--mame_dir', dest='mame_dir', help='Ignored compatibility arg', default='')
    args = parser.parse_args()

    # Configure screenshot directory using common helper
    if args.screenshot_dir:
        set_screenshot_dir(args.screenshot_dir)

    success = test_gui(args.exe_path, args.log)
    sys.exit(0 if success else 1)

if __name__ == "__main__":
    main()
