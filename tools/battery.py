#!/usr/bin/env python3
"""Battery test runner for T.O.R.E-Fighters.

Runs scenarios (one `tore-app` invocation each, or for the `net` lane a Python
driver that owns several processes) in parallel, checks the output for problems
and writes one results folder per run. See docs/testing/README.md.

    python3 tools/battery.py --list
    python3 tools/battery.py --lane ai --jobs 8
    python3 tools/battery.py --scenario 'ai-fight-*' --keep-going
    python3 tools/battery.py --changed --budget 120      # only what the changes since the merge base can affect

Every scenario gets its own copy of an imported data folder (a copy-on-write
copy where the filesystem supports it), so runs cannot disturb each other or
John's real profile. Scenarios that open a window go through tools/agent-run.sh
and at most `--windows` run at once.
"""
from __future__ import annotations

import argparse
import concurrent.futures as futures
import dataclasses
import fnmatch
import importlib
import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
import traceback
from pathlib import Path
from typing import Callable, Optional

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(Path(__file__).resolve().parent))

import battery_selection as selection  # noqa: E402

# Run as a script, this file is `__main__`; the scenario modules `import battery`, which must be this same
# module, so that the classes they raise and build are the ones the runner catches and calls.
if __name__ == "__main__":
    sys.modules.setdefault("battery", sys.modules["__main__"])

LANES = ("menus", "flight", "ai", "replay", "net")

# Output that means something is wrong in any scenario, unless it opts out.
GENERIC_BAD = [
    (re.compile(r"panicked at"), "panic"),
    (re.compile(r"\bNaN\b"), "NaN in output"),
    (re.compile(r"(?<![A-Za-z])-?inf(?![A-Za-z])"), "infinity in output"),
    (re.compile(r"thread '.*' has overflowed its stack"), "stack overflow"),
    (re.compile(r"(?i)\bfatal\b.*error"), "fatal error"),
]


@dataclasses.dataclass
class Scenario:
    """One run of the app plus what a healthy run looks like."""

    name: str
    lane: str
    args: list[str]
    timeout: float = 300.0
    window: bool = False  # opens a window: wrap in agent-run.sh, limit concurrency
    expect: list[str] = dataclasses.field(default_factory=list)  # regexes that must appear
    forbid: list[str] = dataclasses.field(default_factory=list)  # regexes that must not
    allow_generic: list[str] = dataclasses.field(default_factory=list)  # generic labels to tolerate
    check: Optional[Callable[[str], list[str]]] = None  # extra problems from the output
    env: dict[str, str] = dataclasses.field(default_factory=dict)
    expect_exit: Optional[int] = 0
    notes: str = ""
    # Files (relative to the scenario's work folder) the run should produce.
    outputs: list[str] = dataclasses.field(default_factory=list)
    # Follow-up commands run after the main one, in the same environment (for
    # example reading a recording back). Their output is appended to the log
    # and to what `expect`, `forbid` and `check` see.
    then: list["Step"] = dataclasses.field(default_factory=list)
    # Looks at the work folder (and the joined output) once everything has run.
    check_work: Optional[Callable[[Path, str], list[str]]] = None
    # A defect found and left for someone else: the scenario still runs, its
    # failure is reported as known instead of failing the run, and it fails
    # once it starts passing so the marker gets removed.
    known_failure: str = ""
    # Several processes instead of one `tore-app` run: a function that gets a `Drive` and starts, feeds,
    # waits for and stops its own processes (a `tore-server` and `tore-bot`s, a hosting game and a bot).
    # The runner gives it the same timeout, the same per-scenario data folder and the same checks on the
    # combined output (every line is prefixed with its process's label, `[server] `), and kills whatever
    # it leaves running. `args` is unused. A driver that starts a windowed process sets `window=True`.
    driver: Optional[Callable[["Drive"], None]] = None
    # The binaries a driver starts besides the game: "server" and "bot". Checked before the run starts.
    uses: tuple[str, ...] = ()

    @property
    def windowed(self) -> bool:
        """Opens a game window anywhere: the main run (`window`) or any follow-up step (`Step.window`).

        Selection, listing and the `--with-windows no` filter read this. The runner's slot logic still reads
        `window` and `Step.window` separately: a scenario holds a window slot for its whole run only when
        `window` is set, and a window step takes one just for that step."""
        return self.window or any(step.window for step in self.then)


@dataclasses.dataclass
class Step:
    """A follow-up command. `app` steps run the game binary, others run as given."""

    args: list[str]
    app: bool = True
    expect_exit: Optional[int] = 0  # None accepts any exit code
    window: bool = False
    timeout: float = 120.0


@dataclasses.dataclass
class Result:
    name: str
    lane: str
    ok: bool
    seconds: float
    exit_code: Optional[int]
    problems: list[str]
    command: list[str]
    log: str


class DriveError(Exception):
    """A driver gives up: the message becomes the scenario's problem."""


class DriveTimeout(DriveError):
    """The scenario's time ran out while the driver was waiting."""


_PORT_LOCK = threading.Lock()
_PORTS_GIVEN: set[int] = set()


def free_port() -> int:
    """A UDP port nothing on this machine is using (IPv4 and IPv6), never handed out twice in this run."""
    with _PORT_LOCK:
        for _ in range(200):
            found = None
            for family, host in ((socket.AF_INET6, "::"), (socket.AF_INET, "0.0.0.0")):
                try:
                    probe = socket.socket(family, socket.SOCK_DGRAM)
                except OSError:
                    continue
                try:
                    probe.bind((host, found or 0))
                    found = probe.getsockname()[1]
                except OSError:
                    found = None
                    break
                finally:
                    probe.close()
            if found and found not in _PORTS_GIVEN:
                _PORTS_GIVEN.add(found)
                return found
    raise DriveError("no free UDP port")


def stop_group(popen: subprocess.Popen, hard: bool) -> None:
    """Asks a process started with `start_new_session` to end (`hard`: kills it), with its whole group.

    Windows has no process groups: there the process alone is ended (its children are not), which is
    enough for the tools' own tests; the battery itself runs on Linux and macOS.
    """
    try:
        if os.name != "nt":
            os.killpg(popen.pid, signal.SIGKILL if hard else signal.SIGTERM)
        elif hard:
            popen.kill()
        else:
            popen.terminate()
    except (ProcessLookupError, PermissionError):
        pass


class Proc:
    """One process a driver started: its output is captured (merged, line by line) as it runs."""

    def __init__(self, drive: "Drive", label: str, argv: list[str], window: bool, stdin: bool) -> None:
        self.drive = drive
        self.label = label
        self.argv = argv
        self.window = window
        self.stopped = False  # the driver stopped it on purpose
        self._lines: list[str] = []
        self._lock = threading.Lock()
        cmd = [str(ROOT / "tools" / "agent-run.sh"), *argv] if window else argv
        self.popen = subprocess.Popen(
            cmd, cwd=ROOT, env=drive.env, stdin=subprocess.PIPE if stdin else subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors="replace", bufsize=1,
            start_new_session=True,
        )
        self._reader = threading.Thread(target=self._read, daemon=True)
        self._reader.start()

    def _read(self) -> None:
        assert self.popen.stdout is not None
        for line in self.popen.stdout:
            line = line.rstrip("\n")
            with self._lock:
                self._lines.append(line)
            self.drive._record(f"[{self.label}] {line}")
        self.popen.stdout.close()

    def close(self) -> None:
        """Releases the pipes once the process is over."""
        for pipe in (self.popen.stdin,):
            try:
                if pipe:
                    pipe.close()
            except OSError:
                pass

    def text(self) -> str:
        """Everything the process has printed so far, without the label."""
        with self._lock:
            return "\n".join(self._lines)

    @property
    def code(self) -> Optional[int]:
        return self.popen.poll()

    def alive(self) -> bool:
        return self.popen.poll() is None

    def send(self, line: str) -> None:
        """Writes one line to the process's standard input (the server's console)."""
        self.drive._record(f"[driver] to {self.label}: {line}")
        try:
            assert self.popen.stdin is not None
            self.popen.stdin.write(line + "\n")
            self.popen.stdin.flush()
        except (BrokenPipeError, ValueError, OSError):
            pass

    def wait(self, timeout: float) -> Optional[int]:
        """The exit code, or None when it is still running after `timeout` seconds (scaled by --timeout-scale)."""
        limit = time.time() + timeout * self.drive.scale
        while self.alive():
            self.drive._check_time()
            if time.time() >= limit:
                return None
            time.sleep(0.05)
        self._reader.join(5)
        return self.code

    def wait_for(self, pattern: str, timeout: float) -> bool:
        """True once the output matches `pattern`; False when it exits or `timeout` seconds pass without it."""
        limit = time.time() + timeout * self.drive.scale
        while True:
            if re.search(pattern, self.text(), re.M):
                return True
            if not self.alive():
                self._reader.join(5)
                return bool(re.search(pattern, self.text(), re.M))
            self.drive._check_time()
            if time.time() >= limit:
                return False
            time.sleep(0.05)

    def expect(self, pattern: str, what: str = "") -> bool:
        """Records a problem unless the output so far matches `pattern`."""
        if re.search(pattern, self.text(), re.M):
            return True
        self.drive.problem(f"{self.label}: missing {what or 'output'} /{pattern}/")
        return False

    def forbid(self, pattern: str, what: str = "") -> bool:
        """Records a problem when the output so far matches `pattern`."""
        m = re.search(pattern, self.text(), re.M)
        if m:
            self.drive.problem(f"{self.label}: forbidden {what or 'output'} /{pattern}/: {m.group(0)[:200]}")
        return not m

    def finish(self, timeout: float, expect_exit: Optional[int] = 0) -> Optional[int]:
        """Waits for the process to end on its own; one that does not is stopped and reported. Checks the code."""
        code = self.wait(timeout)
        if code is None:
            self.drive.problem(f"{self.label} did not exit within {timeout:.0f}s")
            self.stop()
            return None
        self.stopped = True
        if expect_exit is not None and code != expect_exit:
            self.drive.problem(f"{self.label} exit code {code}, expected {expect_exit}")
        return code

    def stop(self, grace: float = 10.0) -> Optional[int]:
        """Asks the process group to end, and kills it after `grace` seconds."""
        self.stopped = True
        if self.alive():
            stop_group(self.popen, hard=False)
            try:
                self.popen.wait(grace)
            except subprocess.TimeoutExpired:
                stop_group(self.popen, hard=True)
                self.popen.wait()
        self._reader.join(5)
        return self.code


class Drive:
    """What a driver scenario works with: its folders, binaries and processes. See `Scenario.driver`."""

    def __init__(self, s: Scenario, opts: argparse.Namespace, work: Path, data: Path, env: dict, scale: float) -> None:
        self.scenario = s
        self.work = work
        self.data = data
        self.env = env
        self.scale = scale
        self.app = opts.bin
        self.server = opts.server_bin
        self.bot = opts.bot_bin
        self.problems: list[str] = []
        self.procs: list[Proc] = []
        self.timed_out = False
        # Set by a driver whose failure may only mean the machine was too busy to run it (a starved server); the
        # runner then runs the driver once more. See `Drive.inconclusive_because`.
        self.inconclusive: Optional[str] = None
        self._lines: list[str] = []
        self._lock = threading.Lock()

    def _record(self, line: str) -> None:
        with self._lock:
            self._lines.append(line)

    def output(self) -> str:
        with self._lock:
            return "\n".join(self._lines) + "\n"

    def log(self, text: str) -> None:
        """A line of the driver's own in the log."""
        self._record(f"[driver] {text}")

    def problem(self, text: str) -> None:
        self.problems.append(text)
        self.log(f"PROBLEM: {text}")

    def inconclusive_because(self, reason: str) -> None:
        """Says the run's problems may come from a starved machine, not from the game. The runner runs the driver
        once more; a second inconclusive run is reported as "inconclusive: <reason>" (still not a pass)."""
        self.inconclusive = reason
        self.log(f"INCONCLUSIVE: {reason}")

    def port(self) -> int:
        return free_port()

    def _check_time(self) -> None:
        if self.timed_out:
            raise DriveTimeout("the scenario's time ran out")

    def start(self, label: str, argv: list, window: bool = False, stdin: bool = False) -> Proc:
        """Starts a process (`argv[0]` is a path such as `d.server`), labelled in the log."""
        self._check_time()
        if window and not self.scenario.window:
            raise DriveError(f"{label}: a driver that opens a window must set window=True on its Scenario")
        argv = [str(a).replace("{work}", str(self.work)) for a in argv]
        self.log(f"start {label}: {' '.join(argv)}")
        proc = Proc(self, label, argv, window, stdin)
        self.procs.append(proc)
        return proc

    def run(self, label: str, argv: list, timeout: float = 60.0, expect_exit: Optional[int] = 0, window: bool = False) -> Proc:
        """Starts a process and waits for it to end (a probe such as `--find-games`)."""
        proc = self.start(label, argv, window=window)
        proc.finish(timeout, expect_exit)
        return proc

    def sleep(self, seconds: float) -> None:
        end = time.time() + seconds
        while time.time() < end:
            self._check_time()
            time.sleep(min(0.1, max(0.0, end - time.time())))

    def expire(self) -> None:
        """The scenario's time ran out: stop every process (from the timer's thread)."""
        self.timed_out = True
        for proc in list(self.procs):
            stop_group(proc.popen, hard=False)

        def reap() -> None:
            time.sleep(10)
            for proc in list(self.procs):
                if proc.alive():
                    stop_group(proc.popen, hard=True)

        threading.Thread(target=reap, daemon=True).start()

    def cleanup(self, finished_cleanly: bool) -> None:
        """Stops anything still running; after a clean driver that is a problem of its own."""
        for proc in self.procs:
            if proc.alive():
                if finished_cleanly and not proc.stopped:
                    self.problem(f"{proc.label} was still running when the driver finished")
                proc.stop()
            proc.close()


def run_driver(s: Scenario, opts: argparse.Namespace, work: Path, data: Path, env: dict) -> tuple[str, bool, list[str]]:
    """Runs a driver scenario. Returns (the combined output, timed out, the driver's problems).

    A driver that calls `Drive.inconclusive_because` (its problems may come from a starved machine) runs once more
    in a fresh copy of the data folder; the output holds both runs. When the second run is inconclusive too, its
    problems are led by "inconclusive: <reason>", so a report reads as "could not tell", not as a game fault."""
    output, timed_out, problems, why = run_driver_once(s, opts, work, data, env)
    if not why or timed_out:
        return output, timed_out, problems
    clone_profile(Path(opts.profile), data)
    again, timed_out, problems, why_again = run_driver_once(s, opts, work, data, env)
    output += f"\n[driver] RETRY: the first run was inconclusive ({why}); running the driver once more\n" + again
    if why_again and problems:
        problems = [f"inconclusive: {why_again} (this was the second run; the first was also inconclusive)", *problems]
    return output, timed_out, problems


def run_driver_once(
    s: Scenario, opts: argparse.Namespace, work: Path, data: Path, env: dict
) -> tuple[str, bool, list[str], Optional[str]]:
    """One run of a driver. Returns (the combined output, timed out, the driver's problems, why it is inconclusive)."""
    drive = Drive(s, opts, work, data, env, opts.timeout_scale)
    timer = threading.Timer(s.timeout * opts.timeout_scale, drive.expire)
    timer.daemon = True
    timer.start()
    clean = False
    try:
        s.driver(drive)
        clean = True
    except DriveTimeout:
        pass
    except DriveError as e:
        drive.problem(str(e))
    except Exception:  # a bug in the driver or a check it ran: report it with its trace
        drive.problem("the driver failed: " + traceback.format_exc().strip().splitlines()[-1])
        drive.log(traceback.format_exc())
    finally:
        timer.cancel()
        drive.cleanup(clean and not drive.timed_out)
    return drive.output(), drive.timed_out, drive.problems, (drive.inconclusive if drive.problems else None)


def load_scenarios() -> list[Scenario]:
    found: list[Scenario] = []
    pkg = ROOT / "tools" / "battery_scenarios"
    for path in sorted(pkg.glob("*.py")):
        if path.name.startswith("_"):
            continue
        module = importlib.import_module(f"battery_scenarios.{path.stem}")
        found.extend(module.scenarios())
    names = [s.name for s in found]
    dupes = {n for n in names if names.count(n) > 1}
    if dupes:
        raise SystemExit(f"duplicate scenario names: {sorted(dupes)}")
    for s in found:
        if s.lane not in LANES:
            raise SystemExit(f"{s.name}: unknown lane {s.lane!r}")
    return found


def clone_profile(source: Path, dest: Path) -> None:
    if dest.exists():
        shutil.rmtree(dest)
    dest.parent.mkdir(parents=True, exist_ok=True)
    # Scenarios own their recordings. Inherited finished/partial files can
    # satisfy readiness waits or contaminate exact counts and newest-file checks.
    entries = [path for path in source.iterdir() if path.name != "replays"]
    dest.mkdir()
    # A reflink copy is near free on Linux; elsewhere copy the same entries.
    try:
        copied = not entries or subprocess.run(
            ["cp", "-a", "--reflink=auto", *map(str, entries), str(dest)]
        ).returncode == 0
    except OSError:
        copied = False
    if not copied:
        shutil.rmtree(dest)
        shutil.copytree(source, dest, ignore=lambda folder, names: (
            ["replays"] if Path(folder) == source and "replays" in names else []
        ))


def judge(s: Scenario, output: str, code: Optional[int], timed_out: bool, work: Path) -> list[str]:
    problems: list[str] = []
    if timed_out:
        problems.append(f"timed out after {s.timeout:.0f}s")
    elif s.expect_exit is not None and code != s.expect_exit:
        problems.append(f"exit code {code}, expected {s.expect_exit}")
    for pattern, label in GENERIC_BAD:
        if label in s.allow_generic:
            continue
        m = pattern.search(output)
        if m:
            line = output[max(0, output.rfind("\n", 0, m.start()) + 1) : output.find("\n", m.end())]
            problems.append(f"{label}: {line.strip()[:200]}")
    for pattern in s.expect:
        if not re.search(pattern, output, re.M):
            problems.append(f"missing expected output /{pattern}/")
    for pattern in s.forbid:
        m = re.search(pattern, output, re.M)
        if m:
            problems.append(f"forbidden output /{pattern}/: {m.group(0)[:200]}")
    for rel in s.outputs:
        f = work / rel
        if not f.exists() or f.stat().st_size == 0:
            problems.append(f"expected file not written: {rel}")
    if s.check:
        problems.extend(s.check(output))
    if s.check_work:
        problems.extend(s.check_work(work, output))
    return problems


def run_steps(
    s: Scenario, opts: argparse.Namespace, env: dict, work: Path, output: str, step_problems: list[str],
    window_slots: Optional[threading.Semaphore] = None,
) -> str:
    """Runs a scenario's follow-up commands, appending their output; returns the joined output."""
    for i, step in enumerate(s.then):
        # `{bin}` is the tore-app under test (--bin), for steps that run it as an argument.
        step_cmd = [a.replace("{work}", str(work)).replace("{bin}", str(opts.bin)) for a in step.args]
        if step.app:
            step_cmd = [opts.bin, *step_cmd]
        if step.window:
            step_cmd = [str(ROOT / "tools" / "agent-run.sh"), *step_cmd]
        # A window step counts against the window limit unless the scenario already holds a slot.
        slot = window_slots if (step.window and not s.window and window_slots) else None
        if slot:
            slot.acquire()
        try:
            done = subprocess.run(
                step_cmd, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                errors="replace", timeout=step.timeout * opts.timeout_scale, start_new_session=True,
            )
            step_out, step_code = done.stdout, done.returncode
        except subprocess.TimeoutExpired as e:
            step_out = (e.stdout.decode(errors="replace") if isinstance(e.stdout, bytes) else (e.stdout or ""))
            step_code = None
            step_problems.append(f"step {i + 1} timed out")
        except OSError as e:
            step_out, step_code = f"could not run the step: {e}\n", None
            step_problems.append(f"step {i + 1} could not run: {e}")
        finally:
            if slot:
                slot.release()
        output += f"\n$ then {i + 1}: {' '.join(step_cmd)}\n{step_out}"
        if step_code is not None and step.expect_exit is not None and step_code != step.expect_exit:
            step_problems.append(f"step {i + 1} exit code {step_code}, expected {step.expect_exit}")
    return output


def run_one(s: Scenario, opts: argparse.Namespace, run_dir: Path, window_slots: threading.Semaphore) -> Result:
    work = run_dir / "work" / s.name
    work.mkdir(parents=True, exist_ok=True)
    data = work / "data"
    clone_profile(Path(opts.profile), data)
    resolved = [a.replace("{work}", str(work)) for a in s.args]
    cmd = [opts.bin, *resolved]
    if s.driver:
        cmd = ["driver", s.name]
    elif s.window:
        cmd = [str(ROOT / "tools" / "agent-run.sh"), *cmd]
    env = dict(os.environ)
    env.update({"TORE_DATA_DIR": str(data), "TORE_NO_ERROR_DIALOG": "1", "RUST_BACKTRACE": "1"})
    # A battery run never asks a real router to forward a port (a hosting game does by default).
    env["TORE_NO_PORT_MAPPING"] = "1"
    env.update({k: v.replace("{work}", str(work)) for k, v in s.env.items()})
    timeout = s.timeout * opts.timeout_scale
    started = time.time()
    if s.window:
        window_slots.acquire()
    try:
        step_problems: list[str] = []
        if s.driver:
            output, timed_out, driver_problems = run_driver(s, opts, work, data, env)
            step_problems += driver_problems
            returncode = 0  # the driver's own problems carry what went wrong
        else:
            proc = subprocess.Popen(
                cmd, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors="replace",
                start_new_session=True,
            )
            timed_out = False
            try:
                output, _ = proc.communicate(timeout=timeout)
            except subprocess.TimeoutExpired:
                timed_out = True
                stop_group(proc, hard=False)
                try:
                    output, _ = proc.communicate(timeout=10)
                except subprocess.TimeoutExpired:
                    stop_group(proc, hard=True)
                    output, _ = proc.communicate()
            returncode = proc.returncode
            output = run_steps(s, opts, env, work, output, step_problems, window_slots)
    finally:
        if s.window:
            window_slots.release()
    seconds = time.time() - started
    log = run_dir / "logs" / f"{s.name}.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    log.write_text(f"$ {' '.join(cmd)}\n\n{output}")
    problems = judge(s, output, None if timed_out else returncode, timed_out, work) + step_problems
    if s.known_failure:
        if problems:
            problems = [f"known failure ({s.known_failure}): {p}" for p in problems[:1]]
            passed = True
        else:
            problems = [f"known failure now passes, remove known_failure ({s.known_failure})"]
            passed = False
    else:
        passed = not problems
    if not opts.keep_data:
        shutil.rmtree(data, ignore_errors=True)
    return Result(s.name, s.lane, passed, seconds, None if timed_out else returncode, problems, cmd, str(log.relative_to(run_dir)))


def missing_binaries(scenarios: list[Scenario], opts: argparse.Namespace) -> list[str]:
    """The server and bot programs the chosen driver scenarios start that are not built."""
    wanted = {"server": opts.server_bin, "bot": opts.bot_bin}
    return [path for kind, path in wanted.items() if any(kind in s.uses for s in scenarios) and not Path(path).exists()]


def write_summary(run_dir: Path, results: list[Result], started: float) -> None:
    (run_dir / "results.json").write_text(json.dumps([dataclasses.asdict(r) for r in results], indent=1))
    failed = [r for r in results if not r.ok]
    lines = [
        f"# Battery run {run_dir.name}",
        "",
        f"{len(results)} scenarios, {len(results) - len(failed)} passed, {len(failed)} failed, "
        f"{time.time() - started:.0f}s wall clock.",
        "",
    ]
    if failed:
        lines += ["## Failures", ""]
        for r in failed:
            lines.append(f"- `{r.name}` ({r.lane}, {r.seconds:.1f}s): {'; '.join(r.problems)}  (log: `{r.log}`)")
        lines.append("")
    lines += ["## All scenarios", "", "| Scenario | Lane | Result | Seconds |", "| --- | --- | --- | --- |"]
    for r in sorted(results, key=lambda r: (r.lane, r.name)):
        lines.append(f"| `{r.name}` | {r.lane} | {'pass' if r.ok else 'FAIL'} | {r.seconds:.1f} |")
    (run_dir / "summary.md").write_text("\n".join(lines) + "\n")


def choose_changed(opts: argparse.Namespace, scenarios: list[Scenario]) -> tuple[list[Scenario], list[str]]:
    """Narrows `scenarios` to what the changed files can affect and prints the plan."""
    base = selection.default_base(ROOT) if opts.changed == "auto" else opts.changed
    changed = selection.changed_files(ROOT, base, opts.head)
    durations = selection.load_durations(Path(opts.out), [s.name for s in scenarios])
    budget = opts.budget if opts.budget is not None else selection.DEFAULT_BUDGET
    plan = selection.plan_for(
        changed, scenarios, durations, budget, opts.jobs, opts.windows, opts.with_windows,
        base=f"{base}{' to ' + opts.head if opts.head else ''}",
    )
    print(selection.format_plan(plan, None if opts.plan else 8), flush=True)
    return plan.scenarios, plan.unit_tests


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--list", action="store_true", help="list scenarios and exit")
    ap.add_argument("--lane", action="append", choices=LANES, help="run only this lane (repeatable)")
    ap.add_argument("--scenario", action="append", help="glob on scenario names (repeatable)")
    ap.add_argument("--jobs", type=int, default=None, help="scenarios at once (default 6; with --changed, half the cores, 4 to 12)")
    ap.add_argument("--windows", type=int, default=3, help="windowed scenarios at once (default 3)")
    ap.add_argument("--bin", default=str(ROOT / "target" / "debug" / "tore-app"))
    ap.add_argument("--server-bin", default=None, help="tore-server for the net lane (default: beside --bin)")
    ap.add_argument("--bot-bin", default=None, help="tore-bot for the net lane (default: beside --bin)")
    ap.add_argument("--profile", default=str(ROOT / ".local" / "bugbash-data"), help="imported data folder to clone per scenario")
    ap.add_argument("--out", default=str(ROOT / ".local" / "battery"), help="folder that receives one subfolder per run")
    ap.add_argument("--timeout-scale", type=float, default=1.0)
    ap.add_argument("--keep-data", action="store_true", help="keep each scenario's data folder (replays, logs)")
    ap.add_argument("--tag", default="", help="suffix for the run folder name")
    ap.add_argument(
        "--changed", nargs="?", const="auto", metavar="REF",
        help="run only the scenarios the files changed since REF can affect (default REF: the merge base with "
        "`multiplayer`, or HEAD~1 on it); prints what it chose and why. See docs/testing/README.md",
    )
    ap.add_argument("--head", metavar="REF", help="with --changed: compare REF to the base instead of the working tree")
    ap.add_argument("--budget", type=float, default=None, help="with --changed: wall-clock seconds to fit the choice into (default 120)")
    ap.add_argument("--with-windows", choices=("auto", "yes", "no"), default="auto", help="with --changed: windowed scenarios; auto means only when the change touches rendering or windowed input")
    ap.add_argument("--plan", action="store_true", help="with --changed: print the choice and stop")
    ap.add_argument("--no-unit-tests", action="store_true", help="with --changed: skip the Python unit tests it names for the battery's own files")
    opts = ap.parse_args(argv)
    sibling = lambda name: str(Path(opts.bin).with_name(name + (".exe" if sys.platform == "win32" else "")))  # noqa: E731
    opts.server_bin = opts.server_bin or sibling("tore-server")
    opts.bot_bin = opts.bot_bin or sibling("tore-bot")
    if opts.jobs is None:
        opts.jobs = selection.default_jobs() if opts.changed is not None else 6
    if opts.changed is None and (opts.budget is not None or opts.head or opts.plan):
        ap.error("--budget, --head and --plan need --changed")

    scenarios = load_scenarios()
    if opts.lane:
        scenarios = [s for s in scenarios if s.lane in opts.lane]
    if opts.scenario:
        scenarios = [s for s in scenarios if any(fnmatch.fnmatch(s.name, g) for g in opts.scenario)]
    if opts.changed is not None:
        try:
            scenarios, unit_tests = choose_changed(opts, scenarios)
        except RuntimeError as e:
            print(f"--changed: {e}", file=sys.stderr)
            return 2
        if opts.plan:
            return 0
        if unit_tests and not opts.no_unit_tests:
            code = subprocess.run(
                [sys.executable, "-m", "unittest", *unit_tests], cwd=ROOT / "tools"
            ).returncode
            if code:
                print("unit tests for the battery's own files failed", file=sys.stderr)
                return 1
    if opts.list:
        for s in scenarios:
            print(f"{s.lane:8} {'window' if s.windowed else '      '} {s.name}")
        print(f"{len(scenarios)} scenarios")
        return 0
    if not scenarios:
        if opts.changed is not None:
            print("Nothing to run: no scenario can be affected by these changes.")
            return 0
        print("no scenarios selected", file=sys.stderr)
        return 2
    if not Path(opts.bin).exists():
        print(f"binary not found: {opts.bin} (build it first)", file=sys.stderr)
        return 2
    # Checks that run the app themselves (e.g. `--recording-info`) use the same binary.
    os.environ["TORE_BATTERY_BIN"] = str(Path(opts.bin).resolve())
    for path in missing_binaries(scenarios, opts):
        print(f"binary not found: {path} (cargo build --locked -p tore-server -p tore-session)", file=sys.stderr)
        return 2
    if not (Path(opts.profile) / "media-source.txt").exists():
        print(f"{opts.profile} is not an imported data folder; import once with --import ... --import-only", file=sys.stderr)
        return 2

    stamp = time.strftime("%Y%m%d-%H%M%S") + (f"-{opts.tag}" if opts.tag else "")
    run_dir = Path(opts.out) / stamp
    run_dir.mkdir(parents=True, exist_ok=True)
    started = time.time()
    slots = threading.Semaphore(opts.windows)
    results: list[Result] = []
    with futures.ThreadPoolExecutor(max_workers=opts.jobs) as pool:
        pending = {pool.submit(run_one, s, opts, run_dir, slots): s for s in scenarios}
        for fut in futures.as_completed(pending):
            r = fut.result()
            results.append(r)
            print(f"{'pass' if r.ok else 'FAIL'} {r.name} ({r.seconds:.1f}s)", flush=True)
            for p in r.problems:
                print(f"     {p}", flush=True)
            write_summary(run_dir, results, started)
    write_summary(run_dir, results, started)
    failed = sum(1 for r in results if not r.ok)
    print(f"\n{len(results) - failed}/{len(results)} passed. Results: {run_dir}/summary.md")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
