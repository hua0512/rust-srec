"""Mock-only launcher ownership tests. Real process and signal APIs are guarded."""

from contextlib import contextmanager, ExitStack
import io
import ntpath
import os
import signal
import subprocess
import types
import unittest
from unittest import mock

import process_tree as tree


@contextmanager
def sandbox(*, windows=False, output=None, exit_code=0, lose_identity=False,
            exit_on_signal=True, enrollment_error=False):
    events = []
    children = []
    state = {"exit": exit_code}
    retained = set()

    def waitid(kind, pid, flags):
        events.append(("observe", flags))
        if lose_identity:
            raise ChildProcessError("external reaper")
        code = state["exit"]
        return None if code is None else types.SimpleNamespace(
            si_code=1 if code >= 0 else 2, si_status=abs(code))

    def killpg(pid, action):
        events.append(("kill-group", pid, action))
        if exit_on_signal and state["exit"] is None:
            state["exit"] = -9

    class Child:
        def __init__(self, arguments, **options):
            self.pid = 12345
            self.returncode = None
            self.options = options
            self.stdin = None
            self.stdout = io.BytesIO(output or b"") if options["stdout"] == subprocess.PIPE else None
            self.stderr = io.BytesIO(b"") if options["stderr"] == subprocess.PIPE else None
            self._handle = types.SimpleNamespace(Close=lambda: events.append(("close-process",)))
            children.append(self)
            events.append(("create", options))

        def poll(self):
            if not windows:
                raise AssertionError("Popen.poll would reap the POSIX leader too early")
            self.returncode = state["exit"]
            return self.returncode

        def wait(self, timeout):
            events.append(("reap",))
            if not windows:
                assert any(event[0] == "kill-group" for event in events)
            if state["exit"] is None:
                raise subprocess.TimeoutExpired("mock child", timeout)
            self.returncode = state["exit"]
            return self.returncode

        def kill(self):
            events.append(("kill-suspended-leader",))
            state["exit"] = 1

    class Startup:
        def __init__(self):
            self.dwFlags = 0
            self.wShowWindow = 0

        def copy(self):
            result = Startup()
            result.dwFlags, result.wShowWindow = self.dwFlags, self.wShowWindow
            return result

    class Job:
        def attach_and_resume(self, process, deadline):
            assert process.options["creationflags"] & 4
            events.append(("enroll",))
            if enrollment_error:
                raise RuntimeError("mock enrollment failure")
            events.append(("resume",))

        def terminate(self):
            events.append(("kill-job",))
            if state["exit"] is None:
                state["exit"] = 1

        def wait_empty(self, deadline):
            events.append(("job-empty",))

        def close(self):
            events.append(("close-job",))

    fake_os = types.SimpleNamespace(
        name="nt" if windows else "posix", PathLike=os.PathLike, fspath=os.fspath,
        path=ntpath, waitid=waitid, killpg=killpg, P_PID=1, WEXITED=4,
        WNOHANG=1, WNOWAIT=16, CLD_EXITED=1,
    )
    fake_subprocess = types.SimpleNamespace(
        Popen=Child, STARTUPINFO=Startup, PIPE=subprocess.PIPE,
        TimeoutExpired=subprocess.TimeoutExpired, CompletedProcess=subprocess.CompletedProcess,
    )
    with ExitStack() as stack:
        # Fail closed if a test accidentally reaches an unmocked native seam.
        stack.enter_context(mock.patch("subprocess.Popen", side_effect=AssertionError("native launch prohibited")))
        stack.enter_context(mock.patch.object(os, "killpg", side_effect=AssertionError("native signal prohibited"), create=True))
        stack.enter_context(mock.patch.object(os, "waitid", side_effect=AssertionError("native wait prohibited"), create=True))
        stack.enter_context(mock.patch.object(tree.ctypes, "WinDLL", side_effect=AssertionError("native Windows API prohibited"), create=True))
        stack.enter_context(mock.patch.object(tree, "os", fake_os))
        stack.enter_context(mock.patch.object(tree, "subprocess", fake_subprocess))
        stack.enter_context(mock.patch.object(tree, "signal", types.SimpleNamespace(
            SIGCHLD=17, SIG_DFL=0, SIGKILL=9, getsignal=lambda number: 0)))
        stack.enter_context(mock.patch.object(tree, "shutil", types.SimpleNamespace(which=lambda *args, **kwargs: "C:\\fixture.exe")))
        stack.enter_context(mock.patch.object(tree, "_WindowsJob", Job))
        stack.enter_context(mock.patch.object(tree, "_RETAINED", retained))
        try:
            yield types.SimpleNamespace(events=events, children=children, state=state,
                                        retained=retained, startup=Startup)
        finally:
            # Fault-injection owners intentionally remain retained; disarm only
            # these mock owners before restoring the real module dependencies.
            for owner in list(retained):
                owner._closed = True
            retained.clear()


class ProcessTreeContracts(unittest.TestCase):
    def test_posix_poll_keeps_leader_until_group_cleanup_and_never_signals_after_reap(self):
        with sandbox() as mock_tree:
            process = tree.NativeProcess(["fixture"])
            self.assertEqual(process.poll(), 0)
            self.assertNotIn(("reap",), mock_tree.events)
            self.assertEqual(process.wait(timeout=1), 0)
            events = [event[0] for event in mock_tree.events]
            self.assertLess(events.index("kill-group"), events.index("reap"))
            self.assertTrue(all(event[1] & 16 for event in mock_tree.events if event[0] == "observe"))
            process.close()
            process.close()
            self.assertEqual(process.poll(), 0)
            self.assertEqual(sum(event[0] == "kill-group" for event in mock_tree.events), 1)

    def test_posix_lost_leader_identity_never_targets_the_old_group(self):
        with sandbox(lose_identity=True) as mock_tree:
            process = tree.NativeProcess(["fixture"])
            with self.assertRaises(tree.ContainmentError):
                process.poll()
            with self.assertRaises(tree.ContainmentError):
                process.close()
            self.assertIn(process, mock_tree.retained)
            self.assertFalse(any(event[0] in ("kill-group", "reap") for event in mock_tree.events))

    def test_timed_out_cleanup_keeps_ownership_and_retries_without_a_second_group_signal(self):
        with sandbox(exit_code=None, exit_on_signal=False) as mock_tree:
            process = tree.NativeProcess(["fixture"])
            with self.assertRaises(tree.ContainmentError):
                process.close(timeout=0)
            self.assertIn(process, mock_tree.retained)
            mock_tree.state["exit"] = -9
            process.close(timeout=1)
            self.assertNotIn(process, mock_tree.retained)
            self.assertEqual(sum(event[0] == "kill-group" for event in mock_tree.events), 1)
            self.assertEqual(sum(event[0] == "reap" for event in mock_tree.events), 1)

    def test_captured_output_is_bounded_and_overflow_requests_tree_termination(self):
        with sandbox(output=b"0123456789") as mock_tree:
            process = tree.NativeProcess(["fixture"], stdout=subprocess.PIPE, output_limit=4)
            try:
                with self.assertRaises(tree.OutputLimitExceeded):
                    process.communicate(timeout=1)
                self.assertEqual(process.captured("stdout"), b"0123")
                self.assertEqual(sum(event[0] == "kill-group" for event in mock_tree.events), 1)
            finally:
                process.close()

    def test_caller_stdio_and_environment_are_preserved(self):
        with sandbox() as mock_tree:
            output = io.BytesIO()
            environment = {"FIXTURE_OPTION": "retained"}
            process = tree.NativeProcess(["fixture"], stdout=output, stderr=subprocess.DEVNULL,
                                         env=environment, cwd="fixture-directory")
            options = mock_tree.children[0].options
            self.assertIs(options["stdout"], output)
            self.assertIs(options["env"], environment)
            self.assertEqual(options["cwd"], "fixture-directory")
            self.assertFalse(options["shell"])
            self.assertTrue(options["start_new_session"])
            process.close()
            self.assertFalse(output.closed)

    def test_windows_enrollment_precedes_resume_and_job_outlives_leader_poll(self):
        with sandbox(windows=True) as mock_tree:
            process = tree.NativeProcess(["fixture.exe"])
            options = mock_tree.children[0].options
            self.assertTrue(options["creationflags"] & 4)
            self.assertTrue(options["creationflags"] & 0x08000000)
            self.assertEqual([event[0] for event in mock_tree.events[:3]], ["create", "enroll", "resume"])
            self.assertEqual(process.poll(), 0)
            self.assertNotIn(("close-job",), mock_tree.events)
            process.close()
            self.assertIn(("job-empty",), mock_tree.events)
            self.assertIn(("close-job",), mock_tree.events)

    def test_windows_failed_enrollment_kills_suspended_leader_without_resume(self):
        with sandbox(windows=True, enrollment_error=True) as mock_tree:
            with self.assertRaisesRegex(RuntimeError, "enrollment"):
                tree.NativeProcess(["fixture.exe"])
            events = [event[0] for event in mock_tree.events]
            self.assertNotIn("resume", events)
            self.assertIn("kill-job", events)
            self.assertIn("kill-suspended-leader", events)
            self.assertIn("close-job", events)

    def test_windows_new_console_requires_explicit_hidden_startup(self):
        with sandbox(windows=True) as mock_tree:
            with self.assertRaises(ValueError):
                tree.NativeProcess(["fixture.exe"], creationflags=0x10)
            self.assertEqual(mock_tree.children, [])
            startup = mock_tree.startup()
            startup.dwFlags = 1
            process = tree.NativeProcess(["fixture.exe"], creationflags=0x10, startupinfo=startup)
            options = mock_tree.children[0].options
            self.assertTrue(options["creationflags"] & 0x10)
            self.assertFalse(options["creationflags"] & 0x08000000)
            self.assertEqual(options["startupinfo"].wShowWindow, 0)
            process.close()


if __name__ == "__main__":
    unittest.main()
