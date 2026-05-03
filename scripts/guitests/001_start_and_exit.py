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

from common import wait_for_window, launch_app, activate_window, click_center, start_recording, stop_recording

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
        time.sleep(1.5)
        
        print("[INFO] Sending 'x' to click Exit...")
        pyautogui.press('x')
        time.sleep(2.0)
        
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
    parser.add_argument('--record', dest='record', help='Path to record video (mp4)', default=None)
    parser.add_argument('--log', '-l', dest='log', help='Value to pass to executable as --log', default='')
    # Accept --mame_dir for compatibility but ignore it
    parser.add_argument('--mame-dir', dest='mame_dir', help='Ignored compatibility arg', default='')
    args = parser.parse_args()

    # Start recording if requested
    if args.record:
        try:
            os.makedirs(os.path.dirname(args.record), exist_ok=True)
            ok = start_recording(args.record)
            if not ok:
                print(f"[WARN] start_recording failed; recording will be disabled for this run")
        except Exception as e:
            print(f"[WARN] Failed to start recording: {e}")

    success = test_gui(args.exe_path, args.log)

    # Stop recording if active
    try:
        stop_recording()
    except Exception:
        pass

    sys.exit(0 if success else 1)

if __name__ == "__main__":
    main()
