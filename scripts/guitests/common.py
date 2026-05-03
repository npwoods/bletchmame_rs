import os
import time
import pathlib
import subprocess
import pyautogui
import inspect
import threading

# Disable failsafe by default for automated CI use
pyautogui.FAILSAFE = False

# Recording state
_record_thread = None
_record_stop_event = None
_record_path = None
_record_fps = 30


def _ensure_dir(path):
    try:
        os.makedirs(path, exist_ok=True)
    except Exception:
        pass


def take_screenshot(label=None):
    # Screenshots disabled — recordings are used instead.
    return None


def start_recording(path, fps=20):
    """Start recording the entire primary monitor to `path` at `fps` frames per second.

    Recording runs in a background thread and is stopped by calling stop_recording().
    Performs a quick pre-check to ensure backends (mss, imageio/ffmpeg) are available and can open a writer.
    Returns True if recording started, False on failure.
    """
    global _record_thread, _record_stop_event, _record_path, _record_fps
    if _record_thread is not None and _record_thread.is_alive():
        print("[WARN] Recording already in progress")
        return False
    _record_path = path
    _record_fps = fps

    # Pre-check imports and ability to open a writer so failures are surfaced early
    try:
        import mss as _mss
        import numpy as _np
        import imageio as _imageio
    except Exception as e:
        print(f"[ERROR] Recording backend import failed: {e}")
        return False
    try:
        # Try creating and closing a writer to ensure ffmpeg is available for mp4
        _w = _imageio.get_writer(_record_path, fps=_record_fps)
        _w.close()
    except Exception as e:
        print(f"[ERROR] Recording backend cannot open writer (ffmpeg may be missing): {e}")
        return False

    _record_stop_event = threading.Event()

    def _rec_worker(stop_event, out_path, fps):
        try:
            import mss
            import numpy as np
            import imageio
        except Exception as e:
            print(f"[ERROR] Recording backend import failed: {e}")
            return
        writer = None
        try:
            with mss.MSS() as s:
                # Select primary monitor if possible, else fall back to the first one or union
                monitor = s.monitors[0]
                for m in s.monitors:
                    if m.get("is_primary"):
                        monitor = m
                        break
                if monitor == s.monitors[0] and len(s.monitors) > 1:
                    monitor = s.monitors[1]

                try:
                    # Use a fast preset for ffmpeg to reduce capture overhead if available
                    writer = imageio.get_writer(out_path, fps=fps, quality=None, codec='libx264', pixelformat='yuv420p', ffmpeg_params=['-preset', 'ultrafast'])
                except Exception as e:
                    # Fallback if params are not supported by the installed imageio/ffmpeg version
                    try:
                        writer = imageio.get_writer(out_path, fps=fps)
                    except Exception as e2:
                        print(f"[ERROR] Could not open writer inside recorder: {e2}")
                        return

                print(f"[INFO] Recording started -> {out_path} @ {fps}fps on monitor {monitor.get('name', 'main')}")
                
                interval = 1.0 / float(fps)
                start_time = time.perf_counter()
                frames_written = 0

                while not stop_event.is_set():
                    # Capture frame
                    img = s.grab(monitor)
                    arr = np.asarray(img)
                    
                    # mss returns BGRA; convert to RGB
                    if arr.shape[2] == 4:
                        rgb = arr[..., :3][..., ::-1]
                    else:
                        rgb = arr[..., ::-1]

                    # Determine how many frames we SHOULD have written by now to maintain real-time speed
                    now = time.perf_counter()
                    expected_frames = int((now - start_time) * fps)
                    
                    # We must write at least one frame per capture, but if we are behind, 
                    # we write extra copies of this frame to fill the time.
                    num_to_write = max(1, expected_frames - frames_written)
                    
                    try:
                        for _ in range(num_to_write):
                            writer.append_data(rgb)
                            frames_written += 1
                    except Exception as e:
                        print(f"[ERROR] Failed to append frame: {e}")
                        break
                    
                    # Sleep until the next frame is due
                    next_frame_time = start_time + (frames_written * interval)
                    sleep_time = next_frame_time - time.perf_counter()
                    if sleep_time > 0:
                        time.sleep(sleep_time)

                print(f"[INFO] Stopping recording loop for {out_path}")
        except Exception as e:
            print(f"[ERROR] Recording failed during capture: {e}")
        finally:
            if writer is not None:
                try:
                    writer.close()
                    print(f"[INFO] Recording finished -> {out_path}")
                except Exception as e:
                    print(f"[WARN] Failed to close writer cleanly: {e}")

    _record_thread = threading.Thread(target=_rec_worker, args=(_record_stop_event, _record_path, _record_fps), daemon=True)
    _record_thread.start()
    return True


def stop_recording():
    """Stop ongoing recording (if any) and wait for thread to finish."""
    global _record_thread, _record_stop_event
    if _record_thread is None:
        return
    if _record_stop_event is None:
        return
    _record_stop_event.set()
    _record_thread.join(timeout=5)
    if _record_thread.is_alive():
        print("[WARN] Recording thread did not exit promptly")
    _record_thread = None
    _record_stop_event = None
    return



def sleep_and_maybe_capture(seconds, label=None, force_capture=False):
    # Simplified sleep; screenshots are disabled when recording is used.
    time.sleep(seconds)


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
