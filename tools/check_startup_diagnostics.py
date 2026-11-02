#!/usr/bin/env python3
"""Exercise media-free diagnostics on a real staged or unpacked executable.

Uses subprocess waiting on all platforms, including the Windows GUI subsystem.
On Windows it also crashes the executable on purpose (an access violation and a
stack overflow) and checks the native crash report. Elsewhere those modes must
fail with a message saying they are Windows only. This does not prove desktop
shortcut, Finder, dialogs, GPU or the Windows Event Viewer entry.
"""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile


# The deliberate native crashes: the Windows exit status and the name in the report.
NATIVE_CRASHES = {
    "native-crash": (0xC0000005, "EXCEPTION_ACCESS_VIOLATION"),
    "stack-overflow": (0xC00000FD, "EXCEPTION_STACK_OVERFLOW"),
}


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def check(binary, directory, windows=None):
    if windows is None:
        windows = os.name == "nt"
    directory.mkdir(parents=True, exist_ok=True)
    profile = directory / "isolated profile with spaces"
    logs = directory / "logs with spaces"
    profile.mkdir()
    logs.mkdir()
    env = os.environ.copy()
    env.update(TORE_DATA_DIR=str(profile), TORE_LOG_DIR=str(logs), TORE_NO_ERROR_DIALOG="1")
    env.pop("RUST_BACKTRACE", None)
    last_error = profile / "last-error.txt"

    def launch(mode, expected_status=None):
        arg = "--diagnostics-self-test" + (f"={mode}" if mode else "")
        result = subprocess.run([str(binary), arg], cwd=directory, env=env,
                                capture_output=True, text=True, errors="replace", timeout=90)
        require((result.returncode == 0) == (not mode),
                f"{arg}: unexpected exit {result.returncode}\n{result.stdout}\n{result.stderr}")
        if expected_status is not None:
            require(result.returncode & 0xFFFFFFFF == expected_status,
                    f"{arg}: exit {result.returncode:#x}, wanted {expected_status:#x}\n{result.stderr}")

    launch("")
    require(not last_error.exists(), "successful fresh launch wrote last-error.txt")
    for mode in ("error", "panic", "worker-panic"):
        before = {p for p in logs.iterdir() if p.is_file()}
        launch(mode)
        require(last_error.is_file(), f"{mode}: missing last-error.txt")
        report = last_error.read_text(encoding="utf-8")
        lowered = report.lower()
        require("deliberate" in lowered and ("panic" if "panic" in mode else "error") in lowered,
                f"{mode}: report does not identify the deliberate failure: {report}")
        require(str(logs) in report, f"{mode}: report missing actual log location")
        new_files = [p for p in logs.iterdir() if p.is_file() and p not in before]
        require(new_files, f"{mode}: no new session report")
        contents = "\n".join(p.read_text(encoding="utf-8") for p in new_files).lower()
        if "panic" in mode:
            require("backtrace" in contents and "thread" in contents,
                    f"{mode}: logs missing thread or backtrace")
        for field in ("version", "commit", "target"):
            require(field in contents, f"{mode}: logs missing {field} metadata")
        launch("")
        require(last_error.read_text(encoding="utf-8") == report,
                "successful launch replaced or cleared previous failure")
    for mode, (status, name) in NATIVE_CRASHES.items():
        before = {p for p in logs.iterdir() if p.is_file()}
        last_error.unlink(missing_ok=True)
        launch(mode, status if windows else None)
        require(last_error.is_file(), f"{mode}: missing last-error.txt")
        report = last_error.read_text(encoding="utf-8")
        if not windows:
            require("only on windows" in report.lower(), f"{mode}: no Windows-only message: {report}")
            continue
        for wanted in (name, "Native crash", "deliberate", "Faulting address", "Faulting module",
                       binary.name, "Thread: main", "Last GPU step", str(logs)):
            require(wanted.lower() in report.lower(), f"{mode}: report missing {wanted}: {report}")
        new_files = [p for p in logs.iterdir() if p.is_file() and p not in before]
        fatal = [p for p in new_files if p.name.endswith(".fatal.txt")]
        require(fatal, f"{mode}: no fatal report file")
        require(name in fatal[0].read_text(encoding="utf-8"), f"{mode}: fatal report missing {name}")
        launch("")
        require(last_error.read_text(encoding="utf-8") == report,
                "successful launch replaced or cleared previous crash report")
    print(f"Startup diagnostics passed: {binary}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="tore diagnostics check ") as directory:
        check(binary, Path(directory))


if __name__ == "__main__":
    main()
