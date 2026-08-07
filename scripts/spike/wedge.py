#!/usr/bin/env python3
"""Loop the shutdown-implicated suites under load until a server wedges.

`docs/09-m3-plan.md` §2: the drain hang has never been observed alive. Every
sighting so far has been a corpse — a leftover process noticed after a suite
had already moved on — so the first job is a loop that catches one *while it is
still stuck* and photographs it before anything kills it.

What makes a wedge detectable here rather than in the suites themselves: every
harness in the tree signals its server and waits (`Env::drop` runs `amx session
stop`, which SIGTERMs the peer and polls the socket for 10 s). A server still
alive after its own suite exited has therefore already had SIGTERM and ten
seconds, and needs no further judgement call — it is the wedge. Attribution is
by `XDG_RUNTIME_DIR`, read out of `/proc/<pid>/environ`: each iteration gets a
private temp root, so a survivor names the iteration that leaked it and nothing
in the developer's own session can be mistaken for one.

    scripts/spike/wedge.py --out /var/tmp/wedge --minutes 90
    scripts/spike/wedge.py --out /var/tmp/wedge --iterations 300 --keep
    scripts/spike/wedge.py --list

The temp root defaults to a disk-backed directory rather than `$TMPDIR`,
because `/tmp` is tmpfs on most Linux workstations and a session whose state
directory never touches a disk cannot reproduce a timing race that involves
one (`docs/notes/` — the persistence flake wanted the same treatment).

A capture is a directory per wedged process under `<out>/captures/`:

  status, threads   /proc/<pid>/status and per-thread comm/state/wchan
  fd                the descriptor table with link targets, ptmx masters counted
  children          the process's children, zombies marked
  stack             `eu-stack -p` (fast, no ptrace stop) and `gdb` backtraces
  cmdline, environ  what it was started as

Nothing is killed until it has been captured. With `--keep` the run stops at
the first wedge and leaves the process alive for a debugger to attach to; the
default captures, kills, and keeps looping, which is what a rate estimate needs.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]

# The suites the wedge has been seen behind, in the order they are most worth
# running: T17 drives whole processes through daemonize/attach/detach/stop, the
# shutdown seam is the tripwire written for this bug, and the rest reach the
# same drain with panes and agents alive in it.
SUITES = {
    "session_cli": "T17: daemonize, bind race, detach, session stop",
    "seams": "the persist/core shutdown seam and its repetition tripwire",
    "firstrun": "first attach seeds a workspace and a shell behind it",
    "agents": "the fifth actor: hub, trackers, attention queue",
    "adversarial": "floods, kills and other unkind shutdowns",
    "persistence": "sidecar writes racing the final capture",
}

# How long after a suite exits its leaked processes are looked for. The suites
# themselves already waited out `session stop`'s 10 s, so this is only slack
# for a process on its way out of `exit_group`.
SETTLE = 2.0

# How long one suite may run before the harness treats it as hung and captures
# everything it can see. Generously above the slowest suite under full load.
SUITE_TIMEOUT = 3600.0

# The storm's test path inside the `seams` binary. Filtered to on its own so a
# round can ask for thousands of stops without re-running the suite around it.
STORM_TEST = "shutdown::a_shutdown_storm_with_a_client_attached_never_wedges"


@dataclass
class Sighting:
    """One wedged process, as it was found."""

    pid: int
    iteration: int
    suite: str
    directory: Path


@dataclass
class Run:
    """What the loop has seen so far."""

    iterations: int = 0
    failures: int = 0
    sightings: list[Sighting] = field(default_factory=list)


# ------------------------------------------------------------------ the load


class Load:
    """`n` busy loops and, optionally, a writer that keeps the disk fsyncing.

    The CPU load is the condition every sighting has been under: the wedge is a
    scheduling race, and a machine with idle cores runs the drain's tasks in
    the order that happens to be right. The fsync load is here because the same
    trick is what made the persistence flake reproducible off CI — a state
    directory whose writes actually reach a disk.
    """

    def __init__(self, workers: int, fsync_dir: Path | None) -> None:
        self.workers = workers
        self.fsync_dir = fsync_dir
        self.procs: list[subprocess.Popen[bytes]] = []
        self.stop = threading.Event()
        self.thread: threading.Thread | None = None

    def start(self) -> None:
        for _ in range(self.workers):
            self.procs.append(
                subprocess.Popen(
                    ["sh", "-c", "while :; do :; done"],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
            )
        if self.fsync_dir is not None:
            self.fsync_dir.mkdir(parents=True, exist_ok=True)
            self.thread = threading.Thread(target=self._fsync_loop, daemon=True)
            self.thread.start()

    def _fsync_loop(self) -> None:
        assert self.fsync_dir is not None
        path = self.fsync_dir / "churn"
        payload = os.urandom(256 * 1024)
        while not self.stop.is_set():
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            try:
                os.write(fd, payload)
                os.fsync(fd)
            finally:
                os.close(fd)

    def close(self) -> None:
        self.stop.set()
        for proc in self.procs:
            proc.kill()
        for proc in self.procs:
            proc.wait()
        if self.thread is not None:
            self.thread.join(timeout=5)


# --------------------------------------------------------------- the process


def amx_processes() -> list[int]:
    """Every live pid whose executable is an `amx` built from this repo."""
    found = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            exe = os.readlink(entry / "exe")
        except OSError:
            continue
        if "/amx" in exe and str(REPO) in exe:
            found.append(int(entry.name))
    return found


def environ_of(pid: int) -> dict[str, str]:
    try:
        raw = Path(f"/proc/{pid}/environ").read_bytes()
    except OSError:
        return {}
    out = {}
    for item in raw.split(b"\0"):
        if b"=" in item:
            key, _, value = item.partition(b"=")
            out[key.decode("utf-8", "replace")] = value.decode("utf-8", "replace")
    return out


def read(pid: int, name: str) -> str:
    try:
        return Path(f"/proc/{pid}/{name}").read_text(errors="replace")
    except OSError:
        return ""


def threads_of(pid: int) -> str:
    """Per-thread name, run state and kernel wait channel.

    The same three fields the rig's own `thread_states` prints, so a capture
    and a test failure describe a wedge in the same words.
    """
    lines = []
    try:
        tasks = sorted(Path(f"/proc/{pid}/task").iterdir(), key=lambda p: int(p.name))
    except OSError:
        return f"pid {pid} has no /proc entry — it is already gone\n"
    for task in tasks:
        def field_of(name: str) -> str:
            try:
                return (task / name).read_text(errors="replace").strip()
            except OSError:
                return ""

        stat = field_of("stat")
        state = "?"
        if ") " in stat:
            state = stat.rsplit(") ", 1)[1].split()[0]
        lines.append(
            f"  {task.name} [{field_of('comm') or '?'}] "
            f"state={state} wchan={field_of('wchan') or '0'}"
        )
    return f"threads of pid {pid}:\n" + "\n".join(lines) + "\n"


def fds_of(pid: int) -> str:
    """The descriptor table with link targets, and the ptmx count called out.

    The count is the whole reason this is captured: a session that attached
    once holds one pty master, and the sighting that opened this spike held
    five (`docs/09-m3-plan.md` §2).
    """
    lines = []
    masters = 0
    try:
        entries = sorted(Path(f"/proc/{pid}/fd").iterdir(), key=lambda p: int(p.name))
    except OSError:
        return f"pid {pid} has no descriptor table — it is already gone\n"
    for entry in entries:
        try:
            target = os.readlink(entry)
        except OSError:
            target = "<unreadable>"
        if "ptmx" in target or target.startswith("/dev/pts"):
            masters += 1
        lines.append(f"  {entry.name} -> {target}")
    head = f"{len(lines)} descriptors, {masters} of them pty masters or slaves\n"
    return head + "\n".join(lines) + "\n"


def children_of(pid: int) -> str:
    """Direct children, with zombies marked."""
    out = subprocess.run(
        ["ps", "-o", "pid=,ppid=,stat=,etime=,comm=,args=", "--ppid", str(pid)],
        capture_output=True,
        text=True,
        check=False,
    )
    body = out.stdout.strip()
    return (body if body else "no children") + "\n"


def stacks_of(pid: int) -> str:
    """Native backtraces of every thread, from whichever tool is installed.

    `eu-stack` first: it is a lot faster than gdb and needs no ptrace stop long
    enough to change what it is looking at. gdb second, because it resolves
    tokio's frames far better and this is the only reader of them.
    """
    chunks = []
    for tool, argv in (
        ("eu-stack", ["eu-stack", "-p", str(pid)]),
        (
            "gdb",
            [
                "gdb",
                "-p",
                str(pid),
                "-batch",
                "-nx",
                "-ex",
                "set pagination off",
                "-ex",
                "thread apply all bt",
            ],
        ),
    ):
        if shutil.which(tool) is None:
            chunks.append(f"=== {tool}: not installed\n")
            continue
        try:
            out = subprocess.run(argv, capture_output=True, text=True, timeout=120, check=False)
            chunks.append(f"=== {tool}\n{out.stdout}\n{out.stderr}\n")
        except subprocess.TimeoutExpired:
            chunks.append(f"=== {tool}: timed out\n")
    return "".join(chunks)


def capture(pid: int, into: Path) -> None:
    """Photograph a wedged process. Reads only; the process stays as it was."""
    into.mkdir(parents=True, exist_ok=True)
    (into / "status").write_text(read(pid, "status"))
    (into / "threads").write_text(threads_of(pid))
    (into / "fd").write_text(fds_of(pid))
    (into / "children").write_text(children_of(pid))
    (into / "cmdline").write_text(read(pid, "cmdline").replace("\0", " "))
    (into / "environ").write_text(
        "\n".join(f"{k}={v}" for k, v in sorted(environ_of(pid).items()))
    )
    (into / "wchan").write_text(read(pid, "wchan"))
    # Last, because it is the one that stops the process to look at it.
    (into / "stack").write_text(stacks_of(pid))


# ----------------------------------------------------------------- the suites


def vendored_zig() -> Path | None:
    """The pinned Zig under `vendor/toolchain/`, if it has been fetched.

    `$AMX_ZIG` still wins. This only spares a long unattended run the failure
    mode of building nothing because a shell forgot one variable.
    """
    candidates = sorted((REPO / "vendor" / "toolchain").glob("zig-*/zig"))
    return candidates[-1] if candidates else None


def discover(names: list[str]) -> dict[str, Path]:
    """Ask cargo where the test binaries are, building them if need be."""
    env = dict(os.environ)
    if "AMX_ZIG" not in env:
        zig = vendored_zig()
        if zig is not None:
            env["AMX_ZIG"] = str(zig)
    argv = [
        "cargo",
        "test",
        "--workspace",
        "--all-features",
        "--no-run",
        "--message-format=json",
    ]
    proc = subprocess.run(argv, cwd=REPO, capture_output=True, text=True, env=env, check=False)
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise SystemExit("the test binaries could not be built")
    found: dict[str, Path] = {}
    for line in proc.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") != "compiler-artifact":
            continue
        executable = message.get("executable")
        if not executable or not message.get("profile", {}).get("test"):
            continue
        name = message["target"]["name"]
        if name in names:
            found[name] = Path(executable)
    missing = [name for name in names if name not in found]
    if missing:
        raise SystemExit(f"no test binary named {', '.join(missing)}")
    return found


def run_suite(
    binary: Path,
    tmp: Path,
    log: Path,
    filter_to: str | None = None,
    extra_env: dict[str, str] | None = None,
) -> int:
    """Run one suite with its own temp root; return its exit status."""
    tmp.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    env["TMPDIR"] = str(tmp)
    env.update(extra_env or {})
    # A process group, deliberately not a session: the suites open pty slaves
    # without `O_NOCTTY`, and a *session leader* that does that acquires the
    # terminal as its controlling one — so the first pty the harness closed
    # would `SIGHUP` the test binary out from under itself. (Observed: a
    # `start_new_session=True` version of this line killed the suite four tests
    # in and reported the servers it orphaned as wedges. They were not.) A new
    # process group gives the timeout path something to signal and touches no
    # terminal.
    argv = [str(binary), "--test-threads", os.environ.get("AMX_SPIKE_THREADS", "4")]
    if filter_to is not None:
        argv += [filter_to, "--exact", "--nocapture"]
    with log.open("wb") as handle:
        proc = subprocess.Popen(
            argv,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            process_group=0,
        )
        try:
            return proc.wait(timeout=SUITE_TIMEOUT)
        except subprocess.TimeoutExpired:
            # A suite that never returns is a sighting of its own: capture
            # every amx process it owns before taking the group down.
            return -1
        finally:
            if proc.poll() is None:
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait()


def survivors(tmp: Path) -> list[int]:
    """Live `amx` processes whose session lives under `tmp`."""
    root = str(tmp)
    found = []
    for pid in amx_processes():
        env = environ_of(pid)
        if any(str(value).startswith(root) for value in env.values()):
            found.append(pid)
    return found


# -------------------------------------------------------------------- the loop


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", type=Path, help="where captures and logs go")
    parser.add_argument("--iterations", type=int, default=0, help="stop after this many rounds")
    parser.add_argument("--minutes", type=float, default=0.0, help="stop after this long")
    parser.add_argument(
        "--load",
        type=int,
        default=os.cpu_count() or 8,
        help="busy loops to run beside the suites (default: one per core)",
    )
    parser.add_argument("--no-fsync-load", action="store_true", help="skip the fsyncing writer")
    parser.add_argument(
        "--tmp-root",
        type=Path,
        default=Path("/var/tmp"),
        help="disk-backed root for the suites' temp directories (default: /var/tmp)",
    )
    parser.add_argument(
        "--suites",
        default="session_cli,seams",
        help=f"comma-separated suite names; one of {', '.join(SUITES)}",
    )
    parser.add_argument(
        "--storm-cycles",
        type=int,
        default=0,
        help="also run the attached-client shutdown storm this many cycles per round",
    )
    parser.add_argument("--keep", action="store_true", help="stop and leave the first wedge alive")
    parser.add_argument("--list", action="store_true", help="print the suites and exit")
    args = parser.parse_args()

    if args.list:
        for name, what in SUITES.items():
            print(f"{name:14s} {what}")
        return 0
    if args.out is None:
        parser.error("--out is required")
    if args.iterations == 0 and args.minutes == 0.0:
        parser.error("give --iterations or --minutes")

    names = [name.strip() for name in args.suites.split(",") if name.strip()]
    unknown = [name for name in names if name not in SUITES]
    if unknown:
        parser.error(f"unknown suite(s): {', '.join(unknown)}")

    out: Path = args.out
    out.mkdir(parents=True, exist_ok=True)
    (out / "logs").mkdir(exist_ok=True)
    (out / "captures").mkdir(exist_ok=True)

    wanted = list(names)
    if args.storm_cycles and "seams" not in wanted:
        wanted.append("seams")
    print(f"building the suites: {', '.join(wanted)}", flush=True)
    binaries = discover(wanted)

    # A job is one binary invocation per round. The storm is the same binary
    # as the `seams` suite, filtered to the one test and told how many stops to
    # perform — the cheapest shutdown-per-second the tree can produce, and the
    # only one that leaves a wedged server alive to be photographed.
    jobs: list[tuple[str, Path, str | None, dict[str, str]]] = [
        (suite, binaries[suite], None, {}) for suite in names
    ]
    if args.storm_cycles:
        jobs.append(
            (
                "storm",
                binaries["seams"],
                STORM_TEST,
                {
                    "AMX_STORM_CYCLES": str(args.storm_cycles),
                    "AMX_SPIKE_PRESERVE": "1",
                },
            )
        )

    tmp_root: Path = args.tmp_root / f"amx-wedge-{os.getpid()}"
    tmp_root.mkdir(parents=True, exist_ok=True)

    load = Load(args.load, None if args.no_fsync_load else tmp_root / "fsync")
    load.start()
    print(
        f"load: {args.load} busy loops"
        + ("" if args.no_fsync_load else " + one fsyncing writer")
        + f"; temp root {tmp_root}",
        flush=True,
    )

    run = Run()
    started = time.monotonic()
    deadline = started + args.minutes * 60 if args.minutes else None
    try:
        while True:
            if args.iterations and run.iterations >= args.iterations:
                break
            if deadline is not None and time.monotonic() >= deadline:
                break
            run.iterations += 1
            for suite, binary, filter_to, extra_env in jobs:
                tmp = tmp_root / f"i{run.iterations}-{suite}"
                log = out / "logs" / f"i{run.iterations}-{suite}.log"
                status = run_suite(binary, tmp, log, filter_to, extra_env)
                if status != 0:
                    run.failures += 1
                time.sleep(SETTLE)
                stuck = survivors(tmp)
                for pid in stuck:
                    where = out / "captures" / f"i{run.iterations}-{suite}-{pid}"
                    print(f"WEDGE: pid {pid} survived {suite} — capturing into {where}", flush=True)
                    capture(pid, where)
                    run.sightings.append(Sighting(pid, run.iterations, suite, where))
                    if not args.keep:
                        try:
                            os.kill(pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                shutil.rmtree(tmp, ignore_errors=True)
                if stuck and args.keep:
                    print("--keep: leaving it alive and stopping here", flush=True)
                    raise KeyboardInterrupt
            elapsed = time.monotonic() - started
            print(
                f"round {run.iterations}: {run.failures} failed suite runs, "
                f"{len(run.sightings)} wedges, {elapsed / 60:.1f} min",
                flush=True,
            )
    except KeyboardInterrupt:
        print("stopping", flush=True)
    finally:
        load.close()
        summary = {
            "rounds": run.iterations,
            "suites": names,
            "failed_suite_runs": run.failures,
            "wedges": [
                {
                    "pid": s.pid,
                    "round": s.iteration,
                    "suite": s.suite,
                    "capture": str(s.directory),
                }
                for s in run.sightings
            ],
            "minutes": (time.monotonic() - started) / 60,
            "load": args.load,
            "fsync_load": not args.no_fsync_load,
        }
        (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
        if not args.keep:
            shutil.rmtree(tmp_root, ignore_errors=True)
        print(json.dumps(summary, indent=2))
    return 1 if run.sightings else 0


if __name__ == "__main__":
    sys.exit(main())
