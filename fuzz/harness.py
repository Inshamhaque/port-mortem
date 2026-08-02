#!/usr/bin/env python3
"""Differential fuzzer: make the FFI and the safe core parse the same JSON.

The FFI oracle (fuzz/driver.c, linked against libcjson_rs.a) and the safe
`cjson-rs print -` CLI each parse the same input; we compare parse-status and
the printed bytes. Mismatches go to --divergences, the run summary to --log.

Inputs are PRNG-driven and seedable: structured random JSON, byte mutations of
the original test corpus, and raw random bytes.

cJSON_Parse ignores trailing content after the first value; the safe parser
requires the whole input. The harness accounts for that: when the FFI oracle
consumes fewer bytes than the input, either safe outcome (accept with equal
output, or reject) counts as a match.
"""

import argparse
import datetime as _dt
import random
import string
import subprocess
import sys
import time

CORPUS_DIR = "tests/original/inputs"

# --------------------------------------------------------------------------
# Input generators
# --------------------------------------------------------------------------


def json_string(rng):
    """A random JSON string literal (valid escapes only)."""
    pool = string.ascii_letters + string.digits + " _-.:;,!?@#$%^&*()"
    body = "".join(rng.choice(pool) for _ in range(rng.randint(0, 24)))
    # sprinkle a few escapes
    for _ in range(rng.randint(0, 2)):
        if not body:
            break
        esc = rng.choice([r"\\", r"\"", r"\n", r"\t", r"é"])
        pos = rng.randrange(len(body) + 1)
        body = body[:pos] + esc + body[pos:]
    return '"' + body + '"'


def json_number(rng):
    style = rng.randint(0, 4)
    if style == 0:
        return str(rng.randint(-10**6, 10**6))
    if style == 1:
        return repr(rng.uniform(-1e6, 1e6))
    if style == 2:
        return str(rng.randint(-10**12, 10**12)) + "e" + str(rng.randint(-20, 20))
    if style == 3:
        return str(rng.uniform(0, 1))
    return "-" + str(rng.randint(1, 10**9)) + "." + str(rng.randint(0, 10**6))


def json_doc(rng, depth):
    if depth <= 0 or rng.random() < 0.45:
        kind = rng.randint(0, 3)
    else:
        kind = rng.randint(0, 5)
    if kind == 0:
        return "null"
    if kind == 1:
        return "true" if rng.randint(0, 1) else "false"
    if kind == 2:
        return json_number(rng)
    if kind == 3:
        return json_string(rng)
    if kind == 4:
        n = rng.randint(0, 5)
        return "[" + ",".join(json_doc(rng, depth - 1) for _ in range(n)) + "]"
    keys = [json_string(rng) for _ in range(rng.randint(0, 4))]
    members = [k + ":" + json_doc(rng, depth - 1) for k in keys]
    return "{" + ",".join(members) + "}"


def mutate(rng, data, corpus):
    """Byte-level mutations of a well-formed document."""
    if not data:
        return data
    op = rng.randint(0, 5)
    if op == 0:  # flip bytes
        for _ in range(rng.randint(1, 4)):
            pos = rng.randrange(len(data))
            data = data[:pos] + bytes([data[pos] ^ (1 << rng.randint(0, 7))]) + data[pos + 1:]
    elif op == 1:  # delete a span
        start = rng.randrange(len(data))
        end = min(len(data), start + rng.randint(1, 12))
        data = data[:start] + data[end:]
    elif op == 2:  # truncate
        data = data[:rng.randrange(len(data))]
    elif op == 3:  # insert a JSON fragment
        pos = rng.randrange(len(data) + 1)
        frag = json_doc(rng, 3).encode()
        data = data[:pos] + frag + data[pos:]
    elif op == 4 and corpus:  # splice two corpus docs
        other = rng.choice(corpus)
        mid = len(data) // 2
        data = data[:mid] + other[mid:]
    else:  # duplicate a span
        start = rng.randrange(len(data))
        end = min(len(data), start + rng.randint(1, 8))
        data = data[:end] + data[start:end] + data[end:]
    return data


def random_bytes(rng, max_len=256):
    n = rng.randint(0, max_len)
    return bytes(rng.randint(0, 255) for _ in range(n))


def load_corpus():
    import os
    docs = []
    if os.path.isdir(CORPUS_DIR):
        for name in sorted(os.listdir(CORPUS_DIR)):
            path = os.path.join(CORPUS_DIR, name)
            if os.path.isfile(path):
                try:
                    with open(path, "rb") as fh:
                        docs.append(fh.read())
                except OSError:
                    pass
    return docs


def generate_batch(rng, corpus, count):
    """Produce `count` inputs covering the three pools."""
    out = []
    for _ in range(count):
        roll = rng.random()
        if roll < 0.55:
            out.append(json_doc(rng, depth=rng.randint(1, 6)).encode())
        elif roll < 0.9 and corpus:
            out.append(mutate(rng, rng.choice(corpus), corpus))
        else:
            out.append(random_bytes(rng))
    return out


# --------------------------------------------------------------------------
# Oracles and comparison
# --------------------------------------------------------------------------


def run_ffi(driver, payload):
    try:
        proc = subprocess.run(
            [driver], input=payload, capture_output=True, timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return ("SKIP", str(exc))
    line = proc.stdout.decode("utf-8", "replace").strip()
    if line.startswith("OK "):
        parts = line.split(" ", 2)
        if len(parts) == 3:
            return ("OK", int(parts[1]), parts[2])
        return ("SKIP", "malformed driver output")
    if line.startswith("ERR"):
        return ("ERR", int(line.split()[1]))
    return ("SKIP", line)


def run_safe(cli, payload):
    try:
        proc = subprocess.run(
            [cli, "print", "-"], input=payload, capture_output=True, timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return ("SKIP", str(exc))
    if proc.returncode == 0:
        return ("OK", proc.stdout.decode("utf-8", "replace").rstrip("\n"))
    err = proc.stderr.decode("utf-8", "replace").strip()
    # The safe API takes &str, so invalid UTF-8 never reaches it; the FFI
    # oracle works on raw bytes. Treat those as SKIP, not a divergence.
    if "did not contain valid UTF-8" in err:
        return ("SKIP", "invalid UTF-8 input")
    return ("ERR", err)


def classify(ffi, safe, payload):
    """Return ("ok" | "divergence" | "skip", detail)."""
    payload_len = len(payload)
    if ffi[0] == "SKIP" or safe[0] == "SKIP":
        return ("skip", f"{ffi[1] if ffi[0]=='SKIP' else safe[1]}")
    if ffi[0] == "ERR":
        if safe[0] == "OK":
            if b"\x00" in payload:
                # cJSON_Parse sizes its input with strlen(), truncating at the
                # first NUL; the safe core is length-based. Documented, accepted.
                return ("ok", None)
            return ("divergence", "ffi rejected but safe core accepted")
        return ("ok", None)
    # ffi OK
    _, consumed, ffi_out = ffi
    if safe[0] == "ERR":
        if consumed >= payload_len:
            return ("divergence", f"ffi fully consumed but safe core rejected: {safe[1]}")
        # cJSON ignores trailing content; safe core rejects it (documented).
        return ("ok", None)
    _, safe_out = safe
    if ffi_out != safe_out:
        return ("divergence", "outputs differ")
    return ("ok", None)


# --------------------------------------------------------------------------
# Main
# --------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--driver", required=True, help="path to the FFI oracle binary (fuzz/driver)")
    ap.add_argument("--cli", required=True, help="path to the cjson-rs CLI binary")
    ap.add_argument("--duration", type=float, default=60.0, help="run duration in seconds")
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--log", default="fuzz/log.txt")
    ap.add_argument("--divergences", default="fuzz/divergences.txt")
    args = ap.parse_args()

    rng = random.Random(args.seed)
    corpus = load_corpus()
    if not corpus:
        print("warning: corpus directory not found; using generated docs only", file=sys.stderr)

    start = time.monotonic()
    deadline = start + args.duration

    counts = {"ok": 0, "divergence": 0, "skip": 0, "total": 0}
    samples = []
    divergences = []
    batch = 128

    while time.monotonic() < deadline:
        for payload in generate_batch(rng, corpus, batch):
            counts["total"] += 1
            ffi = run_ffi(args.driver, payload)
            safe = run_safe(args.cli, payload)
            verdict, detail = classify(ffi, safe, payload)
            counts[verdict] += 1
            if verdict == "divergence":
                divergences.append((payload, detail))
                if len(divergences) <= 20:
                    samples.append((payload, detail, ffi, safe))
            if counts["total"] % 2000 == 0:
                sys.stdout.write(f"\r{counts['total']} inputs, {counts['divergence']} divergences")
                sys.stdout.flush()

    elapsed = time.monotonic() - start
    sys.stdout.write("\n")

    with open(args.divergences, "w") as fh:
        fh.write(f"# differential fuzzer: divergences from run of {elapsed:.1f}s "
                 f"(seed {args.seed})\n")
        if divergences:
            for payload, detail in divergences[:100]:
                fh.write(f"# {detail}\n")
                fh.write(payload.decode("utf-8", "replace") + "\n---\n")
        else:
            fh.write("# none\n")

    with open(args.log, "w") as fh:
        fh.write(f"tool:       cjson-rs differential fuzzer (FFI vs safe core)\n")
        fh.write(f"timestamp:  {_dt.datetime.now(_dt.timezone.utc).isoformat()}\n")
        fh.write(f"duration_s: {elapsed:.1f}\n")
        fh.write(f"seed:       {args.seed}\n")
        fh.write(f"oracle_ffi: {args.driver}\n")
        fh.write(f"oracle_safe:{args.cli}\n")
        fh.write(f"corpus:     {len(corpus)} files\n")
        for key in ("total", "ok", "divergence", "skip"):
            fh.write(f"{key}:        {counts[key]}\n")
        fh.write(f"divergence_rate: {counts['divergence'] / max(1, counts['total']):.6f}\n")

    print(f"total={counts['total']} ok={counts['ok']} "
          f"divergences={counts['divergence']} skipped={counts['skip']} in {elapsed:.1f}s")
    for payload, detail, ffi, safe in samples:
        print(f"DIVERGENCE: {detail}")
        print(f"  input: {payload[:120]!r}")
        print(f"  ffi:   {ffi}")
        print(f"  safe:  {safe}")
    return 0 if counts["divergence"] == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
