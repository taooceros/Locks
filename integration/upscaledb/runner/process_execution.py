"""Shared process mechanics; experiment schedules and validators stay in controllers."""
import datetime as dt
import os
import signal
import subprocess
import time

from integration.upscaledb.runner.run_trials import atomic_new


def utc_now():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def capture_command(command, timeout, *, cwd, env=None, metadata=None):
    """Capture text and kill the process group on timeout, retaining failed outcomes."""
    record = {**(metadata or {}), 'command': list(map(str, command)),
              'start_utc': utc_now(), 'start_monotonic_ns': time.monotonic_ns(),
              'timeout_s': timeout}
    child = None
    try:
        child = subprocess.Popen(record['command'], cwd=cwd, env=env,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 text=True, start_new_session=True)
        try:
            stdout, stderr = child.communicate(timeout=timeout)
            record['timeout'] = False
        except subprocess.TimeoutExpired:
            record['timeout'] = True
            os.killpg(child.pid, signal.SIGKILL)
            stdout, stderr = child.communicate(timeout=5)
        record.update(returncode=child.returncode, stdout=stdout, stderr=stderr)
    except OSError as exc:
        record.update(returncode=None, timeout=False, stdout='', stderr=str(exc))
    finally:
        if child is not None and child.poll() is None:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate(timeout=5)
        record.update(end_utc=utc_now(), end_monotonic_ns=time.monotonic_ns())
    return record


def run_logged(command, log, timeout, *, cwd, terminate_grace, metadata=None):
    """Preserve a combined log/event; let nested runners clean up before hard kill."""
    event = {**(metadata or {}), 'command': command, 'started_utc': utc_now(),
             'timeout_seconds': timeout}
    with log.open('xb') as stream:
        child = subprocess.Popen(command, stdout=stream, stderr=subprocess.STDOUT,
                                 start_new_session=True, cwd=cwd)
        try:
            event['returncode'] = child.wait(timeout=timeout)
        except (subprocess.TimeoutExpired, KeyboardInterrupt):
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=terminate_grace)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
            event.update(returncode=child.returncode, interrupted_or_timeout=True)
        event['finished_utc'] = utc_now()
    atomic_new(log.with_suffix('.event.json'), event)
    if event['returncode'] != 0 or event.get('interrupted_or_timeout'):
        raise RuntimeError(f'command failed; log preserved: {log}')
