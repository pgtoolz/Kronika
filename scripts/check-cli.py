#!/usr/bin/env python3
"""Check shipped version/help output and argument errors without service startup."""

import argparse
import hashlib
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import tempfile


BINARIES = tuple(f"kronika-{name}" for name in ("collector", "web", "dump", "report"))
TIMEOUT_SECONDS = 5
SECRET = "cli-check-secret-must-not-appear"
# Required parameters and units, independent of help layout.
HELP_CONTENT = {
    "kronika-collector": (
        "KRONIKA_STORAGE_DIR", "KRONIKA_PG_DSN", "KRONIKA_COLLECTOR_MODE",
        "local", "postgresql", "KRONIKA_PG_SSL_ROOT_CERT", "sslmode=require",
        "KRONIKA_POSTGRES_EFFECTIVE_CPUS", "KRONIKA_INTERVAL_S", "5",
        "KRONIKA_SEGMENT_MAX_BYTES", "64MiB", "KRONIKA_SEGMENT_MAX_AGE_S", "900",
        "KRONIKA_RETENTION", "KRONIKA_LOG_LEVEL", "SIGTERM",
        "KRONIKA_JOURNAL_MAX_BYTES", "1GiB", "2GiB", "10GiB", "10GB",
        "automatic log discovery", "pg_current_logfile", "No --pg-log is needed",
    ),
    "kronika-web": (
        "KRONIKA_STORAGE_DIR", "KRONIKA_WEB_SOURCES", "127.0.0.1:8080",
        "KRONIKA_WEB_USER", "KRONIKA_WEB_PASSWORD", "0.0.0.0:8080",
        "http://SERVER_IP:8080/", "Both unset", "Both nonempty", "startup error", "catalog", "health", "/mcp", "TMPDIR",
        "kronika-collector", "KRONIKA_PG_DSN", "No default",
        "0", "1", "2", "3", "Health uses instance information saved by the collector",
        "All recorded data remains available for every value",
    ),
    "kronika-dump": (
        "--json", "--index", "--section", "--limit", "--from", "--to",
        "--out", "slice", "KRONIKA_STORAGE_DIR", "YYYY/MM/DD", "RFC3339",
        "inclusive",
    ),
    "kronika-report": (
        "--from", "--to-exclusive", "microseconds", "input", "output",
        ".zms", ".html", "standalone", "kronika-dump", "TMPDIR",
    ),
}
MALFORMED = {
    "kronika-collector": (["unexpected"],),
    "kronika-web": (["unexpected"],),
    "kronika-dump": (
        ["--limit"], ["--section", "not-a-number"], ["--from", "invalid-date"],
        ["slice", "--unknown-option"], ["slice", "--from"],
        ["slice", "--from", "invalid-date", "--to", "2024-02-29T00:00:00Z", "--out", "slice.zms"],
        ["slice", "--help", "--unknown-option"], ["slice", "--unknown-option", "-h"],
        ["--index", "--section", "1100001", "storage"], ["storage", "--limit", "1"],
    ),
    "kronika-report": (
        ["--from", "1", "input.zms", "output.html"],
        ["--from", "invalid", "--to-exclusive", "2", "input.zms", "output.html"],
        ["--from", "2", "--to-exclusive", "1", "input.zms", "output.html"],
    ),
}


def run(command, cwd, environment, trace=None):
    privileges = {}
    if os.geteuid() == 0:
        privileges = {"user": 65534, "group": 65534, "extra_groups": []}
    with subprocess.Popen(
        command,
        cwd=cwd,
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
        pass_fds=() if trace is None else (trace.fileno(),),
        **privileges,
    ) as child:
        try:
            stdout, stderr = child.communicate(timeout=TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.communicate()
            raise AssertionError(f"{command}: did not exit within {TIMEOUT_SECONDS} seconds") from None
        return child.returncode, stdout, stderr


def check_trace(trace, storage):
    calls = []
    for line in trace.splitlines():
        match = re.match(r"(?:\d+\s+)?([a-z0-9_]+)\(", line)
        if match:
            calls.append((match[1], line))
    assert sum(name == "execve" for name, _ in calls) == 1, trace
    forbidden = {
        "clone", "clone3", "fork", "vfork", "execveat",
        "socket", "socketpair", "bind", "connect", "listen", "accept", "accept4",
        "sendto", "sendmsg", "sendmmsg", "recvfrom", "recvmsg", "recvmmsg",
        "mkdir", "mkdirat", "unlink", "unlinkat", "rmdir", "rename", "renameat", "renameat2",
        "link", "linkat", "symlink", "symlinkat", "truncate", "chmod", "fchmodat",
        "chown", "lchown", "fchownat", "mknod", "mknodat", "creat",
    }
    for name, line in calls:
        assert name not in forbidden, f"CLI performed a startup operation: {line}"
        assert str(storage) not in line, f"CLI accessed configured storage: {line}"
        if name in {"open", "openat", "openat2"}:
            assert not re.search(r"O_(WRONLY|RDWR|CREAT|TRUNC|APPEND)", line), line


def invoke(binary, arguments, cwd, environment, root, strace, storage):
    command = [str(binary), *arguments]
    with tempfile.TemporaryFile(mode="w+b", dir=root) as trace:
        if strace:
            if os.geteuid() == 0:
                os.fchown(trace.fileno(), 65534, 65534)
            command = [
                strace, "-f", "-qq", "-s", "256", "-e", "trace=%process,%network,%file",
                "-o", f"/proc/self/fd/{trace.fileno()}", "--", *command,
            ]
        result = run(command, cwd, environment, trace if strace else None)
        if strace:
            trace.seek(0)
            check_trace(trace.read().decode(), storage)
    assert list(cwd.iterdir()) == [], f"{binary.name} wrote to its working directory"
    return result


def check_help(name, output, slice_help=False, brief=False):
    text = output.decode("utf-8")
    lowered = " ".join(text.lower().split())
    required = ("usage:", name, "--help", "-h")
    if slice_help:
        required += ("slice", "--from", "--to", "--out", "KRONIKA_STORAGE_DIR", "RFC3339", "inclusive")
    elif name == "kronika-collector" and brief:
        required += ("--version", "--storage-dir", "--mode", "--pg-dsn", "KRONIKA_STORAGE_DIR")
    else:
        required += ("--version", *HELP_CONTENT[name])
    for fragment in required:
        assert fragment.lower() in lowered, f"{name} help is missing launch information: {fragment}"
    assert SECRET not in text, f"{name} help printed an environment secret"
    if name == "kronika-web":
        for pattern in (r"^\s*0\s+(?:neither|none|no)\b", r"^\s*1\s+(?:linux|os)\b",
                        r"^\s*2\s+postgresql\b", r"^\s*3\s+.*(?:linux|os).*postgresql"):
            assert re.search(pattern, text, re.IGNORECASE | re.MULTILINE), (
                f"web help does not explain each source value: {pattern}"
            )
    if name == "kronika-report":
        assert re.search(r"\[[^\]\n]*,[^\]\n]*\)", text), "report help omits [closed, open) bounds"


def check(binary, version, root, strace):
    cwd = root / "read-only"
    storage = cwd / "storage-must-not-be-accessed"
    invalid = {
        "KRONIKA_STORAGE_DIR": str(storage),
        "KRONIKA_INTERVAL_S": "not-a-number",
        "KRONIKA_PG_DSN": f"invalid-postgresql-dsn-{SECRET}",
        "KRONIKA_WEB_LISTEN": "not-an-address",
        "KRONIKA_WEB_SOURCES": "not-a-number",
        "KRONIKA_WEB_USER": "cli-check-user",
        "KRONIKA_WEB_PASSWORD": SECRET,
        # Tokio rejects zero worker threads when its runtime is constructed.
        "TOKIO_WORKER_THREADS": "0",
        "TMPDIR": str(storage / "temporary-files"),
    }
    expected = (binary.name + " " + version + "\n").encode()
    commands = [("--version",), ("--help",), ("-h",)]
    if binary.name == "kronika-dump":
        commands += [("slice", "--help"), ("slice", "-h")]
    outputs = {}
    for label, environment in (("empty environment", {}), ("invalid configuration", invalid)):
        for arguments in commands:
            status, stdout, stderr = invoke(binary, arguments, cwd, environment, root, strace, storage)
            assert status == 0 and stdout and stderr == b"", (
                f"{binary.name} {arguments} ({label}): status={status}, stdout={stdout!r}, stderr={stderr!r}"
            )
            if arguments == ("--version",):
                assert stdout == expected, f"{binary.name}: unexpected version: {stdout!r}"
                description = f"stdout={stdout!r}"
            else:
                slice_help = arguments[0] == "slice"
                check_help(binary.name, stdout, slice_help, brief=arguments == ("-h",))
                kind = arguments[-1] if binary.name == "kronika-collector" else ("slice" if slice_help else "help")
                assert stdout == outputs.setdefault(kind, stdout), (
                    f"{binary.name}: {arguments} output differs by alias or environment"
                )
                description = (f"stdout_bytes={len(stdout)}, sha256={hashlib.sha256(stdout).hexdigest()}, "
                               "launch information passed")
            print(f"{binary.name} {' '.join(arguments)} [{label}]: status={status}, {description}, stderr={stderr!r}"
                  + (", no startup syscalls" if strace else ""), flush=True)
    if binary.name == "kronika-dump":
        assert outputs["slice"] != outputs["help"], "slice --help did not provide subcommand context"

    # Collector uses clap's help/version short circuit. Other binaries still
    # reject any extra argument, including one placed after --help/--version.
    malformed = [["--unknown-option"], ["-x"]]
    for option in ("--version", "--help", "-h"):
        malformed.append(["--unknown-option", option])
        if binary.name != "kronika-collector":
            malformed += [[option, "--unknown-option"], [option, "unexpected"]]
    malformed += list(MALFORMED[binary.name])
    for arguments in malformed:
        status, stdout, stderr = invoke(binary, arguments, cwd, {}, root, strace, storage)
        assert status != 0 and stdout == b"" and stderr, (
            f"{binary.name} {arguments}: unexpected argument handling: {(status, stdout, stderr)!r}"
        )
        assert re.search(rb"(?i)(usage:|unexpected argument|unknown (?:argument|option))", stderr), (
            f"{binary.name} {arguments}: did not report an argument error: {stderr!r}"
        )
        assert expected not in stdout + stderr, f"{binary.name} ignored an extra argument"
    print(f"{binary.name}: {len(malformed)} unknown/malformed invocations rejected before startup", flush=True)

    if binary.name == "kronika-collector":
        check_collector_arguments(binary, cwd, storage, root, strace)

    # Normal invocation still reaches its usual required-config/input errors.
    normal = []
    if binary.name in ("kronika-collector", "kronika-web"):
        normal.append(("empty environment", {}, b"--storage-dir" if binary.name == "kronika-collector" else b"KRONIKA_STORAGE_DIR"))
    config_errors = {"kronika-collector": b"--interval-s",
                     "kronika-web": b"KRONIKA_WEB_LISTEN"}
    if binary.name in config_errors:
        normal.append(("invalid configuration", dict(invalid, TOKIO_WORKER_THREADS="1"),
                       config_errors[binary.name]))
    else:
        normal.append(("missing input", {}, b"usage:"))
    if binary.name == "kronika-web":
        web_base = {"KRONIKA_STORAGE_DIR": str(storage), "KRONIKA_WEB_SOURCES": "1",
                    "KRONIKA_WEB_LISTEN": "127.0.0.1:0", "TOKIO_WORKER_THREADS": "1"}
        for credentials, message in (
            ({"KRONIKA_WEB_USER": "cli-check-user"}, b"KRONIKA_WEB_PASSWORD"),
            ({"KRONIKA_WEB_PASSWORD": SECRET}, b"KRONIKA_WEB_USER"),
            ({"KRONIKA_WEB_USER": "", "KRONIKA_WEB_PASSWORD": SECRET}, b"KRONIKA_WEB_USER is empty"),
            ({"KRONIKA_WEB_USER": "cli-check-user", "KRONIKA_WEB_PASSWORD": ""},
             b"KRONIKA_WEB_PASSWORD is empty"),
            ({"KRONIKA_WEB_USER": "", "KRONIKA_WEB_PASSWORD": ""}, b"KRONIKA_WEB_USER is empty"),
        ):
            normal.append(("invalid credentials", dict(web_base, **credentials), message))
        if os.name == "posix":
            for variable in ("KRONIKA_WEB_USER", "KRONIKA_WEB_PASSWORD"):
                normal.append(("non-Unicode credential", dict(web_base, **{variable: os.fsdecode(b"\xff")}),
                               (variable + " is not valid Unicode").encode()))
    for label, environment, message in normal:
        status, stdout, stderr = run([str(binary)], cwd, environment)
        assert status != 0 and stdout == b"" and message in stderr, (
            f"{binary.name} [{label}]: normal error changed: {(status, stdout, stderr)!r}"
        )
        assert SECRET.encode() not in stdout + stderr, f"{binary.name} printed a credential"
        assert list(cwd.iterdir()) == [], f"{binary.name} wrote before rejecting {label}"
        print(f"{binary.name} [no arguments, {label}]: status={status}, expected {message.decode()} error", flush=True)


def check_collector_arguments(binary, cwd, storage, root, strace):
    base = ["--storage-dir", str(storage)]
    cases = [
        ("mode requires DSN", base + ["--mode", "postgresql"], {}, b"requires --pg-dsn"),
        ("CLI mode overrides env", base + ["--mode", "postgresql"],
         {"KRONIKA_COLLECTOR_MODE": "invalid"}, b"requires --pg-dsn"),
        ("CLI interval overrides invalid env", base + ["--interval-s", "0", "--pg-statements-interval-s", "299"],
         {"KRONIKA_INTERVAL_S": "invalid"}, b"must be at least 300"),
        ("invalid DSN is redacted", base + ["--pg-dsn", f"host='unterminated {SECRET}"], {}, b"not a valid connection string"),
        ("legacy DSN overridden", base + ["--pg-dsn", "host=localhost", "--pg-statements-interval-s", "299"],
         {"KRONIKA_PG_DSN": f"invalid-{SECRET}", "KRONIKA_PG_DSNS": f"invalid-{SECRET}"}, b"must be at least 300"),
    ]
    for label, arguments, environment, message in cases:
        status, stdout, stderr = invoke(binary, arguments, cwd, environment, root, strace, storage)
        assert status != 0 and not stdout and message in stderr, (label, status, stdout, stderr)
        assert SECRET.encode() not in stdout + stderr, f"{label}: leaked DSN"
    for option in ("--help", "-h", "--version"):
        status, stdout, stderr = invoke(binary, [option, "--unknown-option"], cwd, {}, root, strace, storage)
        assert status == 0 and stdout and not stderr, (option, status, stdout, stderr)
    print("collector: CLI/env precedence, mode/interval bounds, DSN redaction and help short circuit passed", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path, help="extracted package or build output directory")
    parser.add_argument("--version", help="expected version; defaults to the package BUILDINFO value")
    parser.add_argument("--strace", action="store_true", help="also reject startup syscalls (Linux strace required)")
    parser.add_argument("--binary", choices=BINARIES, action="append", help="check only this binary; repeat to select several")
    arguments = parser.parse_args()
    version = arguments.version
    if version is None:
        metadata = dict(line.split("=", 1) for line in (arguments.directory / "BUILDINFO").read_text().splitlines())
        version = metadata["version"]
    assert version and not any(character.isspace() for character in version), "invalid expected version"
    strace = shutil.which("strace") if arguments.strace else None
    if arguments.strace and strace is None:
        parser.error("--strace requires strace on PATH")
    source = arguments.directory / "bin" if (arguments.directory / "bin").is_dir() else arguments.directory
    with tempfile.TemporaryDirectory(prefix="kronika-cli-") as scratch:
        root = Path(scratch)
        # Root-owned extraction directories can be inaccessible to the test uid.
        root.chmod(0o755)
        cwd = root / "read-only"
        cwd.mkdir(mode=0o555)
        try:
            for name in arguments.binary or BINARIES:
                binary = root / name
                shutil.copyfile(source / name, binary)
                binary.chmod(0o555)
                check(binary, version, root, strace)
                binary.unlink()
        finally:
            cwd.chmod(0o755)
    print("Selected CLI version, help and argument contracts passed as an unprivileged user in a read-only directory.")


if __name__ == "__main__":
    main()
