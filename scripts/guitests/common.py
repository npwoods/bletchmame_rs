import os
import time
import pathlib
import subprocess
import pyautogui
import inspect

# Disable failsafe by default for automated CI use
pyautogui.FAILSAFE = False

_screenshot_dir = None
_script_name = None
_screenshot_counter = 0


def set_screenshot_dir(path):
    global _screenshot_dir, _script_name
    if path:
        _screenshot_dir = os.path.abspath(path)
        _ensure_dir(_screenshot_dir)
    else:
        _screenshot_dir = None
    # script name will be inferred from the caller when taking the first screenshot


def _ensure_dir(path):
    try:
        os.makedirs(path, exist_ok=True)
    except Exception:
        pass


def take_screenshot(label=None):
    """Save a screenshot to the configured screenshot dir, if any."""
    global _screenshot_counter, _script_name
    if not _screenshot_dir:
        return None
    _ensure_dir(_screenshot_dir)
    # infer caller script name if not set
    if not _script_name:
        try:
            for frame in inspect.stack()[1:]:
                fname = frame.filename
                if os.path.basename(fname) != os.path.basename(__file__):
                    _script_name = pathlib.Path(fname).stem
                    break
        except Exception:
            _script_name = pathlib.Path(__file__).stem
    _screenshot_counter += 1
    fname = f"{_script_name}_{_screenshot_counter:02d}"
    if label:
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
    try:
        time.sleep(seconds)
    except Exception:
        if _screenshot_dir:
            take_screenshot(label or "sleep_interrupted")
        raise
    if _screenshot_dir and (force_capture or seconds >= 1.0):
        take_screenshot(label)


def wait_for_window(titles, timeout=30):
    """Wait for a window by matching its title.

    `titles` may be a single string or an iterable of strings. Returns the
    first window whose title exactly matches any of the provided titles.
    """
    try:
        import pygetwindow
    except ImportError:
        raise RuntimeError("pygetwindow is required for wait_for_window")

    if not titles:
        raise ValueError("titles is required for wait_for_window")
    if isinstance(titles, str):
        titles = [titles]

    print(f"[INFO] Waiting for window matching any of: {titles}...")
    start_time = time.time()
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


def launch_app(exe_path, exe_log):
    cmd = [str(exe_path), '--title', 'BletchMameAuto', '--prefix-title-with-mode']
    exe_log = (exe_log or '').strip()
    if exe_log:
        cmd += ['--log', exe_log]
    print(f"[INFO] Starting app with command: {' '.join(cmd)}")
    if not os.path.exists(exe_path):
        raise FileNotFoundError(f"Executable not found: {exe_path}")
    process = subprocess.Popen(
        cmd,
        creationflags=subprocess.CREATE_NEW_PROCESS_GROUP if os.name == 'nt' else 0,
    )
    print(f"[INFO] Process started with PID: {process.pid}")
    return process


def activate_window(window):
    try:
        window.activate()
    except Exception as e:
        print(f"[WARN] Could not activate window: {e}")


def click_center(window):
    try:
        center_x = window.left + window.width // 2
        center_y = window.top + window.height // 2
        print(f"[INFO] Clicking window center at ({center_x}, {center_y})")
        pyautogui.click(center_x, center_y)
    except Exception as e:
        print(f"[WARN] Failed clicking center: {e}")
