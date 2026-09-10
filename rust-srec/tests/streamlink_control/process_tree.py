"""Owned native processes for the finite W26 fixture suite, never shell commands.

Windows launches suspended, confirms kill-on-close Job enrollment, then resumes.
POSIX observes the exclusively owned leader with WNOWAIT, kills its inherited
process group before reaping, and never probes/signals that group after reaping.
As with process_utils, deliberately escaping descendants are outside the contract.

PIPE output is drained concurrently and capped at 4 MiB per stream. Caller-owned
files/DEVNULL/inherited stdio keep their ordinary subprocess behavior and ownership.
wait/communicate timeouts terminate the tree but retain the owner for close().
run() adds a separate, bounded cleanup allowance (five seconds by default).
This is a bytes-only, single-caller fixture API, not a general Popen replacement.
"""

import ctypes
import math
import os
import shutil
import signal
import subprocess
import sys
import threading
import time
import warnings
import weakref


OUTPUT_LIMIT = 4 * 1024 * 1024
DEFAULT_TIMEOUT = 30.0
CLEANUP_TIMEOUT = 5.0
POLL_INTERVAL = 0.005
_RETAINED = set()  # Failed close() must not discard an unconfirmed process owner.
_RETAINED_LOCK = threading.Lock()


class ContainmentError(RuntimeError):
    pass


class OutputLimitExceeded(RuntimeError):
    pass


def _seconds(value, default):
    value = default if value is None else float(value)
    if not math.isfinite(value) or value < 0:
        raise ValueError("native fixture timeouts must be finite and nonnegative")
    return value


def _pause(deadline):
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        return False
    time.sleep(min(POLL_INTERVAL, remaining))
    return True


class _WindowsJob:
    def __init__(self):
        from ctypes import wintypes as w

        size = ctypes.c_size_t

        class Limits(ctypes.Structure):
            _fields_ = [("process_time", ctypes.c_longlong), ("job_time", ctypes.c_longlong),
                        ("flags", w.DWORD), ("min_working_set", size), ("max_working_set", size),
                        ("active_process_limit", w.DWORD), ("affinity", size),
                        ("priority", w.DWORD), ("scheduling", w.DWORD)]

        class Counters(ctypes.Structure):
            _fields_ = [(name, ctypes.c_ulonglong) for name in
                        ("read_ops", "write_ops", "other_ops", "read_bytes", "write_bytes", "other_bytes")]

        class Extended(ctypes.Structure):
            _fields_ = [("limits", Limits), ("io", Counters), ("process_memory", size),
                        ("job_memory", size), ("peak_process_memory", size), ("peak_job_memory", size)]

        class Accounting(ctypes.Structure):
            _fields_ = [(name, ctypes.c_longlong) for name in ("user", "kernel", "period_user", "period_kernel")] + [
                (name, w.DWORD) for name in ("page_faults", "processes", "active", "terminated")]

        class ThreadEntry(ctypes.Structure):
            _fields_ = [("size", w.DWORD), ("usage", w.DWORD), ("tid", w.DWORD),
                        ("pid", w.DWORD), ("priority", w.LONG), ("delta", w.LONG), ("flags", w.DWORD)]

        self.accounting = Accounting
        self.thread_entry = ThreadEntry
        self.handle = None
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        signatures = {
            "CreateJobObjectW": ([ctypes.c_void_p, w.LPCWSTR], w.HANDLE),
            "SetInformationJobObject": ([w.HANDLE, ctypes.c_int, ctypes.c_void_p, w.DWORD], w.BOOL),
            "QueryInformationJobObject": ([w.HANDLE, ctypes.c_int, ctypes.c_void_p, w.DWORD, ctypes.c_void_p], w.BOOL),
            "AssignProcessToJobObject": ([w.HANDLE, w.HANDLE], w.BOOL),
            "IsProcessInJob": ([w.HANDLE, w.HANDLE, ctypes.POINTER(w.BOOL)], w.BOOL),
            "TerminateJobObject": ([w.HANDLE, w.UINT], w.BOOL),
            "CloseHandle": ([w.HANDLE], w.BOOL),
            "CreateToolhelp32Snapshot": ([w.DWORD, w.DWORD], w.HANDLE),
            "Thread32First": ([w.HANDLE, ctypes.POINTER(ThreadEntry)], w.BOOL),
            "Thread32Next": ([w.HANDLE, ctypes.POINTER(ThreadEntry)], w.BOOL),
            "OpenThread": ([w.DWORD, w.BOOL, w.DWORD], w.HANDLE),
            "ResumeThread": ([w.HANDLE], w.DWORD),
        }
        for name, (arguments, result) in signatures.items():
            function = getattr(self.kernel, name)
            function.argtypes, function.restype = arguments, result
        self.handle = self.kernel.CreateJobObjectW(None, None)
        self._check(self.handle)
        limits = Extended()
        limits.limits.flags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        try:
            self._check(self.kernel.SetInformationJobObject(
                self.handle, 9, ctypes.byref(limits), ctypes.sizeof(limits)))
        except BaseException:
            self.close()
            raise

    @staticmethod
    def _check(success):
        if not success:
            raise ctypes.WinError(ctypes.get_last_error())

    def attach_and_resume(self, process, deadline):
        from ctypes import wintypes as w

        # CPython retains the process HANDLE, so thread lookup cannot target a
        # reused PID. The child has run no code and cannot have spawned children.
        handle = int(process._handle)
        self._check(self.kernel.AssignProcessToJobObject(self.handle, handle))
        enrolled = w.BOOL()
        self._check(self.kernel.IsProcessInJob(handle, self.handle, ctypes.byref(enrolled)))
        if not enrolled.value:
            raise ContainmentError("Windows did not confirm fixture Job enrollment")
        snapshot = self.kernel.CreateToolhelp32Snapshot(0x4, 0)  # TH32CS_SNAPTHREAD
        if snapshot == ctypes.c_void_p(-1).value:
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            entry = self.thread_entry()
            entry.size = ctypes.sizeof(entry)
            found = self.kernel.Thread32First(snapshot, ctypes.byref(entry))
            while found:
                if time.monotonic() >= deadline:
                    raise ContainmentError("suspended fixture enrollment deadline expired")
                if entry.pid == process.pid:
                    thread = self.kernel.OpenThread(0x2, False, entry.tid)  # THREAD_SUSPEND_RESUME
                    self._check(thread)
                    try:
                        previous = self.kernel.ResumeThread(thread)
                        if previous == 0xFFFFFFFF:
                            raise ctypes.WinError(ctypes.get_last_error())
                        if previous != 1:
                            raise ContainmentError("fixture primary thread did not have exactly one suspension")
                        return
                    finally:
                        self._check(self.kernel.CloseHandle(thread))
                found = self.kernel.Thread32Next(snapshot, ctypes.byref(entry))
            error = ctypes.get_last_error()
            if error not in (0, 18):  # ERROR_NO_MORE_FILES
                raise ctypes.WinError(error)
            raise ContainmentError("suspended fixture child has no primary thread")
        finally:
            self._check(self.kernel.CloseHandle(snapshot))

    def terminate(self):
        if self.handle is not None:
            self._check(self.kernel.TerminateJobObject(self.handle, 1))

    def wait_empty(self, deadline):
        while self.handle is not None:
            state = self.accounting()
            self._check(self.kernel.QueryInformationJobObject(
                self.handle, 1, ctypes.byref(state), ctypes.sizeof(state), None))
            if state.active == 0:
                return
            if not _pause(deadline):
                raise ContainmentError("fixture Job still owns running processes at the cleanup deadline")

    def close(self):
        if self.handle is not None:
            self._check(self.kernel.CloseHandle(self.handle))
            self.handle = None


class _Capture:
    def __init__(self, limit):
        self.limit = limit
        self.data = bytearray()
        self.lock = threading.Lock()

    def append(self, data):
        with self.lock:
            available = self.limit - len(self.data)
            self.data.extend(data[:available])
            return len(data) <= available

    def snapshot(self):
        with self.lock:
            return bytes(self.data)


def _report(owner, error):
    process = owner()
    if process is not None:
        process._record_failure(error)


def _read_pipe(stream, capture, name, owner):
    try:
        while True:
            data = stream.read(65536)
            if not data:
                break
            if not capture.append(data):
                _report(owner, OutputLimitExceeded(f"native fixture {name} exceeds {capture.limit} bytes"))
                # Continue draining/discarding until tree termination closes the
                # pipe; storage never grows after the limit was reached.
    except OSError as error:
        _report(owner, error)
    finally:
        stream.close()


def _write_pipe(stream, data, owner):
    try:
        view = memoryview(data)
        while view:
            count = stream.write(view[:65536])
            if count is None or count <= 0:
                raise OSError("native fixture stdin write made no progress")
            view = view[count:]
    except BrokenPipeError:
        pass  # As with communicate(), a child may intentionally stop reading.
    except OSError as error:
        _report(owner, error)
    finally:
        stream.close()


class NativeProcess:
    def __init__(self, command, *, stdin=None, stdout=None, stderr=None, env=None,
                 cwd=None, creationflags=0, startupinfo=None, start_new_session=True,
                 output_limit=OUTPUT_LIMIT):
        if isinstance(command, (str, bytes, os.PathLike)) or not command:
            raise TypeError("native fixture commands must be a nonempty argv sequence; no shell")
        if not isinstance(output_limit, int) or not 0 < output_limit <= OUTPUT_LIMIT:
            raise ValueError("capture limit must be between 1 byte and 4 MiB per stream")
        self.args = [os.fspath(argument) for argument in command]
        self._process = None
        self._job = None
        self._windows = os.name == "nt"
        self._lock = threading.RLock()
        self._failure = None
        self._observed = None
        self._signalled = False
        self._reaped = False
        self._identity_lost = False
        self._tree_finished = False
        self._closed = False
        self._resumed = False
        self._input_started = False
        self._captures = {}
        self._threads = []
        options = dict(stdin=stdin, stdout=stdout, stderr=stderr, env=env, cwd=cwd,
                       shell=False, close_fds=True, bufsize=0)
        if self._windows:
            if sys.implementation.name != "cpython":
                raise RuntimeError("the Windows fixture launcher requires CPython process handles")
            if creationflags & (0x8 | 0x01000000):  # DETACHED_PROCESS / CREATE_BREAKAWAY_FROM_JOB
                raise ValueError("detached/breakaway fixture processes are not supported")
            if creationflags & 0x10:  # Explicit hidden-console negative control.
                if startupinfo is None or not startupinfo.dwFlags & 1 or startupinfo.wShowWindow != 0:
                    raise ValueError("CREATE_NEW_CONSOLE requires explicit STARTF_USESHOWWINDOW/SW_HIDE")
                creationflags &= ~0x08000000
            else:
                creationflags |= 0x08000000  # CREATE_NO_WINDOW
            hidden = subprocess.STARTUPINFO() if startupinfo is None else startupinfo.copy()
            hidden.dwFlags |= 1
            hidden.wShowWindow = 0
            options.update(creationflags=creationflags | 0x4, startupinfo=hidden)  # CREATE_SUSPENDED
            search_path = None if env is None else next((value for key, value in env.items() if key.upper() == "PATH"), "")
            executable = shutil.which(self.args[0], path=search_path)
            if executable is None or os.path.splitext(executable)[1].lower() not in (".exe", ".com"):
                raise ValueError("Windows native fixtures require a resolvable .exe/.com executable")
            options["executable"] = executable
        else:
            if creationflags or startupinfo is not None or not start_new_session:
                raise ValueError("POSIX native fixtures require their own session/process group")
            if not all(hasattr(os, name) for name in ("waitid", "WNOWAIT", "WEXITED", "WNOHANG")):
                raise RuntimeError("POSIX fixture containment requires waitid/WNOWAIT")
            if signal.getsignal(signal.SIGCHLD) != signal.SIG_DFL:
                raise RuntimeError("native fixtures require exclusive child reaping and default SIGCHLD handling")
            options["start_new_session"] = True
        try:
            self._job = _WindowsJob() if self._windows else None
            deadline = time.monotonic() + CLEANUP_TIMEOUT
            self._process = subprocess.Popen(self.args, **options)
            if self._job is not None:
                self._job.attach_and_resume(self._process, deadline)
            self._resumed = True
            owner = weakref.ref(self)
            for name in ("stdout", "stderr"):
                stream = getattr(self._process, name)
                if stream is not None:
                    capture = self._captures[name] = _Capture(output_limit)
                    thread = threading.Thread(target=_read_pipe, args=(stream, capture, name, owner), daemon=True)
                    thread.start()
                    self._threads.append(thread)
        except BaseException as error:
            try:
                self.close(timeout=CLEANUP_TIMEOUT)
            except BaseException as cleanup:
                error.add_note(f"fixture startup cleanup was not confirmed: {cleanup}")
            raise

    @property
    def pid(self):
        return self._process.pid

    @property
    def returncode(self):
        return self._observed

    def _observe_locked(self):
        if self._observed is not None:
            return self._observed
        if self._windows:
            self._observed = self._process.poll()
            return self._observed
        if self._identity_lost:
            raise ContainmentError("fixture leader identity was lost; refusing to signal its old group")
        try:
            status = os.waitid(os.P_PID, self.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
        except InterruptedError:
            return None
        except ChildProcessError as error:
            self._identity_lost = True
            raise ContainmentError("another reaper or SIGCHLD policy released the fixture leader") from error
        if status is not None:
            self._observed = status.si_status if status.si_code == os.CLD_EXITED else -status.si_status
        return self._observed

    def _terminate_locked(self):
        if self._signalled or self._tree_finished or self._process is None:
            return
        if self._windows:
            try:
                self._job.terminate()
            finally:
                if not self._resumed:
                    # Enrollment may have failed. The suspended leader has no
                    # descendants, and its HANDLE still identifies it exactly.
                    self._process.kill()
        else:
            if self._reaped or self._identity_lost:
                raise ContainmentError("refusing to signal a fixture group after losing leader ownership")
            self._observe_locked()  # WNOWAIT confirms ownership without releasing the PID.
            try:
                os.killpg(self.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except PermissionError:
                # Darwin can report EPERM for an already-exited unsignalable
                # group. Do not apply that exception to a running leader.
                if sys.platform != "darwin" or self._observed is None:
                    raise
        self._signalled = True

    def _record_failure(self, error):
        with self._lock:
            if self._failure is not None:
                return
            self._failure = error
            try:
                self._terminate_locked()
            except Exception as cleanup:
                self._failure.add_note(f"fixture tree termination failed: {cleanup}")

    def poll(self):
        with self._lock:
            if self._process is None:
                return self._observed
            code = self._observe_locked()
            if code is not None:
                self._terminate_locked()
            return code  # POSIX leader remains unreaped until finish/close.

    def captured(self, name):
        capture = self._captures.get(name)
        return None if capture is None else capture.snapshot()

    def _timeout(self, timeout):
        return subprocess.TimeoutExpired(self.args, timeout, self.captured("stdout"), self.captured("stderr"))

    def _join_io(self, deadline):
        for thread in self._threads:
            thread.join(max(0, deadline - time.monotonic()))
            if thread.is_alive():
                raise ContainmentError("native fixture pipe thread did not settle before its deadline")

    def _finish_tree(self, deadline):
        if self._tree_finished:
            return
        if self._process is None:
            if self._job is not None:
                self._job.close()
            self._tree_finished = True
            return
        with self._lock:
            self._terminate_locked()
        while self.poll() is None:
            if not _pause(deadline):
                raise ContainmentError("native fixture leader did not exit before its cleanup deadline")
        if self._job is not None:
            self._job.wait_empty(deadline)
        # Captured inherited pipes reach EOF after descendants are terminated.
        # Keep the POSIX leader waitable through those joins, then retire PGID.
        self._join_io(deadline)
        with self._lock:
            if not self._reaped:
                if not self._signalled:
                    raise ContainmentError("cannot reap before requesting tree termination")
                code = self._process.wait(timeout=max(0, deadline - time.monotonic()))
                self._reaped = True  # _signalled already prevents any later group signal.
                if code != self._observed:
                    raise ContainmentError("fixture exit status changed outside its exclusive owner")
            if self._job is not None:
                self._job.close()
                self._process._handle.Close()
            self._tree_finished = True

    def wait(self, timeout=DEFAULT_TIMEOUT):
        timeout = _seconds(timeout, DEFAULT_TIMEOUT)
        deadline = time.monotonic() + timeout
        while self.poll() is None:
            if not _pause(deadline):
                with self._lock:
                    self._terminate_locked()
                raise self._timeout(timeout)
        try:
            self._finish_tree(deadline)
        except ContainmentError:
            if time.monotonic() >= deadline:
                raise self._timeout(timeout)
            raise
        if self._failure is not None:
            raise self._failure
        return self._observed

    def communicate(self, input=None, timeout=DEFAULT_TIMEOUT):
        if input is not None and not isinstance(input, bytes):
            raise TypeError("native fixture input must be bytes")
        if input is not None and len(input) > OUTPUT_LIMIT:
            raise ValueError("native fixture input exceeds 4 MiB")
        if self._input_started and input is not None:
            raise ValueError("native fixture input was already supplied")
        stream = self._process.stdin
        if input is not None and stream is None:
            raise ValueError("input requires stdin=PIPE")
        if not self._input_started:
            self._input_started = True
            if stream is not None:
                thread = threading.Thread(target=_write_pipe, args=(stream, input or b"", weakref.ref(self)), daemon=True)
                thread.start()
                self._threads.append(thread)
        self.wait(timeout)
        return self.captured("stdout"), self.captured("stderr")

    def close(self, timeout=CLEANUP_TIMEOUT):
        if self._closed:
            return
        deadline = time.monotonic() + _seconds(timeout, CLEANUP_TIMEOUT)
        try:
            if self._process is not None:
                with self._lock:
                    self._terminate_locked()
                if self._process.stdin is not None and not self._input_started:
                    self._process.stdin.close()
            self._finish_tree(deadline)
            # On a failed enrollment the pipe workers were never started.
            if self._process is not None:
                for name in ("stdin", "stdout", "stderr"):
                    stream = getattr(self._process, name)
                    if stream is not None:
                        stream.close()
            self._closed = True
            with _RETAINED_LOCK:
                _RETAINED.discard(self)
        except BaseException:
            with _RETAINED_LOCK:
                _RETAINED.add(self)
            raise

    contain = close

    def __enter__(self):
        return self

    def __exit__(self, exc_type, error, traceback):
        try:
            self.close()
        except BaseException as cleanup:
            if error is None:
                raise
            error.add_note(f"native fixture cleanup remains owned: {cleanup}")

    def __del__(self):
        if not getattr(self, "_closed", True):
            try:
                self.close(timeout=0)
            except Exception as error:
                warnings.warn(f"native fixture cleanup remains owned: {error}", ResourceWarning)


def run(command, *, input=None, timeout=DEFAULT_TIMEOUT, check=True,
        cleanup_timeout=CLEANUP_TIMEOUT, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        stdin=None, **options):
    """Run a finite fixture; timeout plus cleanup_timeout bound all owned waits.

    Captures are bytes; output_limit (default 4 MiB each) applies only to PIPE.
    Caller-supplied files are never replaced with temporary capture files.
    """
    if input is not None:
        if stdin not in (None, subprocess.PIPE):
            raise ValueError("input conflicts with configured stdin")
        stdin = subprocess.PIPE
    timeout = _seconds(timeout, DEFAULT_TIMEOUT)
    cleanup_timeout = _seconds(cleanup_timeout, CLEANUP_TIMEOUT)
    process = NativeProcess(command, stdin=stdin, stdout=stdout, stderr=stderr, **options)
    try:
        output, errors = process.communicate(input=input, timeout=timeout)
        result = subprocess.CompletedProcess(process.args, process.returncode, output, errors)
        if check:
            result.check_returncode()
    except BaseException as error:
        try:
            process.close(timeout=cleanup_timeout)
        except BaseException as cleanup:
            error.add_note(f"native fixture cleanup remains owned: {cleanup}")
        raise
    process.close(timeout=cleanup_timeout)
    return result
