#!/usr/bin/env python3
"""Run the benchmark and write bench/results.json.

Combines three measurements:
  * in-process parse+print throughput / latency (the `bench` binary)
  * CLI cold-start latency on a trivial document (subprocess spawn + parse)
  * peak resident set while the CLI parses a large document (/usr/bin/time -l)

Machine + toolchain are recorded too, so results.json is reproducible.
See bench/methodology.md for the full methodology.
"""

import json
import os
import platform
import re
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BENCH_BIN = os.path.join(ROOT, "target", "release", "bench")
CLI = os.path.join(ROOT, "target", "release", "cjson-rs")
RESULTS = os.path.join(ROOT, "bench", "results.json")


def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, cwd=ROOT, **kw)


def measure_in_process():
    proc = run([BENCH_BIN])
    if proc.returncode != 0:
        sys.exit(f"bench binary failed: {proc.stderr}")
    return json.loads(proc.stdout)


def measure_startup(samples=200):
    """Cold-start wall time (process spawn + parse + exit) for `cjson-rs print -` on `{}`."""
    times = []
    for _ in range(samples):
        t0 = time.perf_counter()
        proc = run([CLI, "print", "-"], input="{}")
        if proc.returncode != 0:
            sys.exit(f"cli startup run failed: {proc.stderr}")
        times.append((time.perf_counter() - t0) * 1e3)  # ms
    times.sort()
    return {
        "startup_p50_ms": round(times[samples // 2], 3),
        "startup_p99_ms": round(times[int(samples * 0.99) - 1], 3),
        "startup_mean_ms": round(sum(times) / len(times), 3),
        "startup_samples": samples,
    }


def measure_rss():
    """Peak resident set while parsing a ~8 MiB document, via /usr/bin/time -l."""
    doc = "{"
    for i in range(120_000):
        if i:
            doc += ","
        doc += f'"key{i}":"value number {i} with some text padding padding",'
        doc += f'"n{i}":{i}'
    doc += "}"
    if len(doc) < 7_000_000:
        # pad to ~8 MiB of fairly flat text
        doc = doc[:-1] + ',"pad":"' + "x" * (8_000_000 - len(doc)) + '"}'

    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as fh:
        fh.write(doc)
        path = fh.name
    try:
        proc = subprocess.run(
            ["/usr/bin/time", "-l", CLI, "print", "-"],
            stdin=open(path, "rb"),
            capture_output=True,
            text=True,
            cwd=ROOT,
        )
    finally:
        os.unlink(path)
    if proc.returncode != 0:
        sys.exit(f"rss measurement failed: {proc.stderr}")
    # macOS /usr/bin/time -l prints "NNNNNN  maximum resident set size" (bytes).
    match = re.search(r"([\d,]+)\s+maximum resident set size", proc.stderr)
    if not match:
        sys.exit(f"could not parse rss from /usr/bin/time output: {proc.stderr[:200]}")
    return {"peak_rss_mb": round(int(match.group(1).replace(",", "")) / (1024 * 1024), 1)}


def machine_info():
    chip = "unknown"
    try:
        chip = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"],
                              capture_output=True, text=True).stdout.strip()
    except OSError:
        pass
    return {
        "os": platform.platform(),
        "cpu": chip,
        "cores": os.cpu_count(),
        "rustc": subprocess.run(["rustc", "--version"], capture_output=True,
                                text=True).stdout.strip(),
        "cc": subprocess.run(["cc", "--version"], capture_output=True,
                             text=True).stdout.splitlines()[0] if sys.platform != "win32" else "n/a",
    }


def main():
    metrics = measure_in_process()
    metrics.update(measure_startup())
    metrics.update(measure_rss())
    payload = {
        "generated_at_utc": subprocess.run(
            ["date", "-u", "+%Y-%m-%dT%H:%M:%SZ"], capture_output=True,
            text=True).stdout.strip(),
        "machine": machine_info(),
        "profile": {"codegen_units": 1, "lto": True, "strip": True},
        "metrics": metrics,
    }
    with open(RESULTS, "w") as fh:
        json.dump(payload, fh, indent=2)
        fh.write("\n")
    print(json.dumps(payload, indent=2))


if __name__ == "__main__":
    main()
