"""Real subprocess checks for retained failures, deadlines and exclusive logs."""
import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import unittest

from integration.upscaledb.runner.process_execution import capture_command, run_logged


class ProcessExecutionTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)

    def test_capture_keeps_failed_output_with_requested_cwd_and_environment(self):
        (self.root / 'input').write_text('17')
        command = [sys.executable, '-c',
                   'import os,pathlib,sys; '
                   'print(int(pathlib.Path("input").read_text())+int(os.environ["OFFSET"])); '
                   'print("operation failed",file=sys.stderr); sys.exit(7)']
        record = capture_command(command, 10, cwd=self.root, env={**os.environ, 'OFFSET': '25'})
        self.assertEqual(record['returncode'], 7)
        self.assertEqual(record['stdout'], '42\n')
        self.assertEqual(record['stderr'], 'operation failed\n')
        self.assertFalse(record['timeout'])
        self.assertLessEqual(record['start_monotonic_ns'], record['end_monotonic_ns'])

    def test_capture_retains_launch_failure(self):
        missing = self.root / 'missing-program'
        record = capture_command([missing], 10, cwd=self.root)
        self.assertIsNone(record['returncode'])
        self.assertFalse(record['timeout'])
        self.assertIn(str(missing), record['stderr'])

    def test_capture_timeout_kills_and_reaps_the_child(self):
        record = capture_command([sys.executable, '-c', 'import time; time.sleep(60)'],
                                 0, cwd=self.root)
        self.assertTrue(record['timeout'])
        self.assertEqual(record['returncode'], -signal.SIGKILL)

    def test_logged_success_keeps_both_streams_and_event(self):
        log = self.root / 'success.log'
        run_logged([sys.executable, '-c',
                    'import sys; print("committed",flush=True); print("diagnostic",file=sys.stderr)'],
                   log, 10, cwd=self.root, terminate_grace=1)
        self.assertEqual(log.read_text(), 'committed\ndiagnostic\n')
        event = json.loads(log.with_suffix('.event.json').read_text())
        self.assertEqual(event['returncode'], 0)
        self.assertNotIn('interrupted_or_timeout', event)

    def test_logged_failure_preserves_evidence_before_raising(self):
        log = self.root / 'failure.log'
        with self.assertRaises(RuntimeError):
            run_logged([sys.executable, '-c', 'import sys; print("failure evidence"); sys.exit(9)'],
                       log, 10, cwd=self.root, terminate_grace=1)
        self.assertEqual(log.read_text(), 'failure evidence\n')
        self.assertEqual(json.loads(log.with_suffix('.event.json').read_text())['returncode'], 9)

    def test_logged_timeout_is_not_success_even_after_graceful_exit(self):
        log = self.root / 'timeout.log'
        code = ('import signal,sys,time\n'
                'def finish(signum, frame):\n'
                '    print("cleanup completed",flush=True)\n'
                '    sys.exit(0)\n'
                'signal.signal(signal.SIGTERM,finish)\n'
                'print("ready",flush=True)\n'
                'time.sleep(60)\n')
        with self.assertRaises(RuntimeError):
            run_logged([sys.executable, '-c', code], log, 2, cwd=self.root, terminate_grace=2)
        self.assertEqual(log.read_text(), 'ready\ncleanup completed\n')
        event = json.loads(log.with_suffix('.event.json').read_text())
        self.assertTrue(event['interrupted_or_timeout'])
        self.assertEqual(event['returncode'], 0)

    def test_existing_log_is_not_overwritten_or_executed_again(self):
        log = self.root / 'existing.log'
        log.write_text('original evidence\n')
        with self.assertRaises(FileExistsError):
            run_logged([sys.executable, '-c', 'from pathlib import Path; Path("rerun").touch()'],
                       log, 10, cwd=self.root, terminate_grace=1)
        self.assertEqual(log.read_text(), 'original evidence\n')
        self.assertFalse((self.root / 'rerun').exists())


if __name__ == '__main__':
    unittest.main()
