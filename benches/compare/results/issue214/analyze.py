#!/usr/bin/env python3
"""Summarise an interleaved campaign.

usage: analyze.py <results-dir> <prefix> <baseline-label> <label>... [--csv out.csv]

Per scenario and side: per-round point estimate (Criterion's middle `time:`
value; for the latency harness the median of the round's runs), the median of
the rounds, per-round % change vs the baseline label, the median % change
(from the medians), the spread (max - min round % change) and the
BENCHMARKS.md noise flag (rounds disagree in sign beyond +/-2%, or spread >
10 pp).
"""
import glob
import os
import re
import statistics
import sys

UNIT = {"ps": 1e-3, "ns": 1.0, "µs": 1e3, "us": 1e3, "ms": 1e6, "s": 1e9}
CRIT_RE = re.compile(
    r"^(?P<name>[^\n]*?\S)\s*\n?\s*time:\s+\[(?P<lo>[\d.]+) (?P<lu>\S+) (?P<mid>[\d.]+) (?P<mu>\S+) (?P<hi>[\d.]+) (?P<hu>\S+)\]",
    re.M,
)
LAT_RE = re.compile(
    r"^\[contention\s*\]\s+(?P<name>contention_\w+).*?p50=(?P<p50>\d+)\s+p99=(?P<p99>\d+)\s+p99\.9=(?P<p999>\d+)",
    re.M,
)


def parse_crit(path):
    out = {}
    text = open(path, encoding="utf-8", errors="replace").read()
    for m in CRIT_RE.finditer(text):
        name = m.group("name").strip()
        if name.startswith("Benchmarking") or name.startswith("#"):
            continue
        name = re.sub(r"/(0\.9\.2|0\.10\.0)/", "/", name)
        out[name] = float(m.group("mid")) * UNIT[m.group("mu")]
    return out


def parse_lat(path):
    out = {}
    text = open(path, encoding="utf-8", errors="replace").read()
    for m in LAT_RE.finditer(text):
        for q in ("p50", "p99", "p999"):
            out[f"{m.group('name')} {q}"] = float(m.group(q))
    return out


def pct(a, b):
    return (b / a - 1.0) * 100.0


def main():
    args = sys.argv[1:]
    csv_path = None
    if "--csv" in args:
        i = args.index("--csv")
        csv_path = args[i + 1]
        del args[i : i + 2]
    res, prefix, base, *labels = args
    sides = [base] + labels
    # data[scenario][side][round] = value
    data = {}
    rounds = set()
    for path in glob.glob(os.path.join(res, f"*_{prefix}r*_*.txt")):
        fn = os.path.basename(path)[:-4]
        kind, rest = fn.split("_", 1)
        m = re.match(rf"{re.escape(prefix)}r(\d+)(?:k(\d+))?_(.+)$", rest)
        if not m:
            continue
        rnd, k, side = int(m.group(1)), m.group(2), m.group(3)
        rounds.add(rnd)
        vals = parse_lat(path) if kind == "lat" else parse_crit(path)
        for scen, v in vals.items():
            key = f"{kind}:{scen}"
            data.setdefault(key, {}).setdefault(side, {}).setdefault(rnd, []).append(v)
    rounds = sorted(rounds)
    rows = []
    for scen in sorted(data):
        d = data[scen]
        if base not in d:
            continue
        bvals = {r: statistics.median(d[base][r]) for r in rounds if r in d[base]}
        bmed = statistics.median(bvals.values())
        for side in sides:
            if side not in d:
                continue
            svals = {r: statistics.median(d[side][r]) for r in rounds if r in d[side]}
            smed = statistics.median(svals.values())
            rp = [pct(bvals[r], svals[r]) for r in rounds if r in bvals and r in svals]
            med = pct(bmed, smed)
            spread = max(rp) - min(rp) if rp else 0.0
            pos = any(x > 2 for x in rp)
            neg = any(x < -2 for x in rp)
            noisy = (pos and neg) or spread > 10
            nruns = sum(len(d[side][r]) for r in d[side])
            rows.append((scen, side, [svals.get(r) for r in rounds], smed, med, rp, spread, noisy, nruns))
    # print
    for scen, side, vals, smed, med, rp, spread, noisy, nruns in rows:
        rvals = " ".join(f"{v:10.1f}" if v is not None else "      None" for v in vals)
        rps = " ".join(f"{x:+6.2f}" for x in rp)
        print(f"{scen:70s} {side:5s} {rvals} | med {smed:10.1f} | {med:+6.2f}% | rounds {rps} | spread {spread:5.2f} {'NOISY' if noisy else ''}")
    if csv_path:
        with open(csv_path, "w", encoding="utf-8") as f:
            hdr = ["scenario", "side", "vs"] + [f"r{r}" for r in rounds] + ["median", "median_pct_change"] + [f"r{r}_pct_change" for r in rounds] + ["spread_pp", "noisy", "runs"]
            f.write(",".join(hdr) + "\n")
            for scen, side, vals, smed, med, rp, spread, noisy, nruns in rows:
                cells = [scen.replace(",", ";"), side, base] + [f"{v:.1f}" if v is not None else "" for v in vals] + [f"{smed:.1f}", f"{med:.2f}"] + [f"{x:.2f}" for x in rp] + [f"{spread:.2f}", str(noisy), str(nruns)]
                f.write(",".join(cells) + "\n")


if __name__ == "__main__":
    main()
