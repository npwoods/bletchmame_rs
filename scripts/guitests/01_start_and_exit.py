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

# Disable fail-safe to prevent interruption on mouse edge
pyautogui.FAILSAFE = False

# Globals for screenshot handling
_screenshot_dir = None
_script_name = pathlib.Path(__file__).stem
_screenshot_counter = 0


def _ensure_dir(path):
    try:
        os.makedirs(path, exist_ok=True)
    except Exception:
        pass


def take_screenshot(label=None):
    """Save a screenshot to the configured screenshot dir, if any."""
    global _screenshot_counter
    if not _screenshot_dir:
        return None
    _ensure_dir(_screenshot_dir)
    ts = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    _screenshot_counter += 1
    fname = f"{_script_name}_{_screenshot_counter:03d}_{ts}"
    if label:
        # sanitize label for filesystem
        safe_label = ''.join(c if c.isalnum() or c in ('-', '_') else '_' for c in label)[:64]
        fname = f"{fname}_{safe_label}"
    path = os.path.join(_screenshot_dir, fname + ".png")
    try:
        img = pyautogui.screenshot()
        img.save(path)
        print(f"[INFO]: Saved screenshot: {path}")
        return path
    except Exception as e:
        print(f"[WARNING]: Failed to save screenshot: {e}")
        return None


def sleep_and_maybe_capture(seconds, label=None, force_capture=False):
    """Sleep for given seconds. If a screenshot dir is set, capture after sleep
    when the sleep is significant (>=1s) or force_capture is True."""
    try:
        time.sleep(seconds)
    except Exception:
        # If interrupted, still attempt a screenshot
        if _screenshot_dir:
            take_screenshot(label or "sleep_interrupted")
        raise

    # Capture after pauses that are likely meaningful
    if _screenshot_dir and (force_capture or seconds >= 1.0):
        take_screenshot(label)


def wait_for_window(titles, timeout=30):
    """Wait for a window by matching its title.

    `titles` may be a single string or an iterable of strings. The function returns
    the first window whose title exactly matches any of the provided titles.
    """
    if not titles:
        raise ValueError("titles is required for wait_for_window")

    # Normalize to a list of strings
    if isinstance(titles, str):
        titles = [titles]

    print(f"[INFO] Waiting for window matching any of: {titles}...")
    start_time = time.time()
    try:
        import pygetwindow
    except ImportError:
        print(f"[ERROR] pygetwindow is required for wait_for_window.")
        raise

    while time.time() - start_time < timeout:
        try:
            windows = pygetwindow.getAllWindows()
            for w in windows:
                wtitle = getattr(w, 'title', '')
                if any(t == wtitle for t in titles):
                    print(f"[INFO] Window found by title at ({getattr(w, 'left', None)}, {getattr(w, 'top', None)})")
                    return w
        except Exception as e:
            print(f"[ERROR] Error checking for window: {e}")
        time.sleep(0.5)
    raise TimeoutError(f"[ERROR] Window matching titles '{titles}' did not appear within {timeout}s")


def test_gui(exe_path, exe_log):
    """Test the GUI application"""
    # Start the application (build the command so we can log it)
    cmd = [str(exe_path), '--title', 'BletchMameAuto', '--prefix-title-with-mode']
    # Allow passing a --log value via CLI --log argument
    exe_log = (exe_log or '').strip()
    if exe_log:
        cmd += ['--log', exe_log]
    print(f"[INFO] Starting BletchMAME with command: {' '.join(cmd)}")
    
    if not os.path.exists(exe_path):
        print(f"[FAIL] Executable not found: {exe_path}")
        return False
    
    process = subprocess.Popen(
        cmd,
        creationflags=subprocess.CREATE_NEW_PROCESS_GROUP if sys.platform == 'win32' else 0,
    )
    
    print(f"[INFO] Process started with PID: {process.pid}")
    
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
        if _screenshot_dir:
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
        if _screenshot_dir:
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
        if _screenshot_dir:
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
    args = parser.parse_args()

    global _screenshot_dir
    _screenshot_dir = args.screenshot_dir

    if _screenshot_dir:
        # Accept either absolute or workspace-relative paths
        _screenshot_dir = os.path.abspath(_screenshot_dir)
        _ensure_dir(_screenshot_dir)

    success = test_gui(args.exe_path, args.log)
    sys.exit(0 if success else 1)

if __name__ == "__main__":
    main()
