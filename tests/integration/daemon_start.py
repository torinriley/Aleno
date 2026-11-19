"""Exercise startup while the previous instance still owns its lock."""
import fcntl
import os
import pathlib
import subprocess
import tempfile
import time

root = pathlib.Path(__file__).resolve().parents[2]
daemon = root / 'target/release/aleno-daemon'
with tempfile.TemporaryDirectory(prefix='aleno-start-') as directory:
    env = dict(os.environ, ALENO_SOCKET=f'{directory}/daemon.sock')
    lock = open(f'{directory}/daemon.lock', 'w')
    fcntl.flock(lock, fcntl.LOCK_EX)
    start = time.monotonic()
    process = subprocess.Popen([daemon, 'start'], env=env)
    time.sleep(0.25)
    fcntl.flock(lock, fcntl.LOCK_UN)
    assert process.wait(timeout=3) == 0
    elapsed = time.monotonic() - start
    assert elapsed < 2, elapsed
    print(f'Lock-contention startup: {elapsed * 1000:.0f} ms (includes 250 ms held lock)', flush=True)
    try:
        for i in range(5):
            subprocess.run([daemon, 'stop'], env=env, check=True)
            start = time.monotonic()
            subprocess.run([daemon, 'start'], env=env, check=True, timeout=3)
            print(f'Immediate stop/start {i+1}: {(time.monotonic()-start)*1000:.0f} ms', flush=True)
    finally:
        subprocess.run([daemon, 'stop'], env=env)
        deadline = time.monotonic() + 3
        while pathlib.Path(env['ALENO_SOCKET']).exists() and time.monotonic() < deadline:
            time.sleep(.02)
