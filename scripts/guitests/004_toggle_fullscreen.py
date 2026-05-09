#!/usr/bin/env python3
"""
GUI test script for BletchMAME

Tests that:
1. The application window opens
2. Fullscreen can be toggled via F11
3. Clicking Exit properly shuts down the application
"""

import sys
import time
import pyautogui

from common import (
    wait_for_window, launch_app, activate_window, click_center,
    start_recording, stop_recording, wait_for_process_termination,
    create_arg_parser
)

# common.py disables pyautogui.FAILSAFE already


def test_gui(exe_path, exe_log):
    """Test the GUI application"""

    for i in range(2):
        try:
            process = launch_app(exe_path, exe_log)
        except Exception as e:
            print(f"[FAIL] Could not start app: {e}")
            return False

        try:
            print(f"[INFO] Waiting for window to appear...")
            window = wait_for_window(["[ready] BletchMameAuto", "[report] BletchMameAuto"], timeout=30)
            activate_window(window)
            time.sleep(0.5)
            click_center(window)
            time.sleep(0.5)

            print("[INFO] Toggling full screen with F11")
            pyautogui.hotkey('F11')
            time.sleep(2.0)

            print("[INFO] Opening File menu with Alt+F...")
            pyautogui.hotkey('alt', 'f')
            time.sleep(1.5)
            
            print("[INFO] Sending 'x' to click Exit...")
            pyautogui.press('x')
            time.sleep(2.0)
            
            # Wait for the process to terminate
            return_code = wait_for_process_termination(process)

            if return_code == 0:
                print("[OK] Application exited successfully with code 0")
            else:
                print(f"[FAIL] Application exited with code: {return_code}")
                return False
                
        except TimeoutError as e:
            print(f"[FAIL] Test failed: {e}")
            wait_for_process_termination(process, timeout=1)
            return False
        except Exception as e:
            print(f"[FAIL] Test failed with error: {e}")
            import traceback
            traceback.print_exc()
            wait_for_process_termination(process, timeout=1)
            return False
        except KeyboardInterrupt:
            print("\n[SCRIPT][FAIL] Test interrupted by user")
            wait_for_process_termination(process, timeout=1)
            return False
    return True


def main():
    # Setup
    parser = create_arg_parser()
    args = parser.parse_args()
    start_recording(args.record)

    # Run the test
    success = test_gui(args.exe_path, args.log)

    # Cleanup
    stop_recording()
    sys.exit(0 if success else 1)

if __name__ == "__main__":
    main()
