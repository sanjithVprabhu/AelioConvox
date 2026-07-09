"""Shared dataset IO, exact ground truth, and the evaluation loop for the benchmark.

Every competitor runner imports this so the dataset, predicate, ground truth, recall, and
results schema are identical across systems. See bench/BENCHMARKS.md.
"""
import json
import os
import time

import numpy as np

K = 10
SELS = [0.005, 0.02, 0.05, 0.1, 0.25, 0.5, 1.0]


def read_fvecs(path):
    a = np.fromfile(path, dtype="int32")
    if a.size == 0:
        return np.zeros((0, 0), dtype="float32")
    dim = int(a[0])
    a = a.reshape(-1, dim + 1)
    return a[:, 1:].copy().view("float32")


def read_attr(path):
    return np.fromfile(path, dtype="uint32")


def load(dirpath):
    base = read_fvecs(os.path.join(dirpath, "base.fvecs"))
    queries = read_fvecs(os.path.join(dirpath, "query.fvecs"))
    attr = read_attr(os.path.join(dirpath, "attr.u32"))
    return base, queries, attr


def evaluate(base, queries, attr, search_fn):
    """Run the selectivity sweep. `search_fn(query, thr, k) -> list[int]` (base row ids)."""
    rows = []
    for sel in SELS:
        thr = int(sel * 1000)
        mask = attr < thr
        matched = int(mask.sum())
        match_idx = np.where(mask)[0]
        sub = base[mask] if matched else base[:0]
        recall_sum = 0.0
        lat_sum = 0.0
        counted = 0
        for q in queries:
            if matched == 0:
                continue
            d = ((sub - q) ** 2).sum(axis=1)
            order = np.argsort(d, kind="stable")[:K]
            truth = set(match_idx[order].tolist())
            if not truth:
                continue
            counted += 1
            t0 = time.perf_counter()
            got = search_fn(q, thr, K)
            lat_sum += time.perf_counter() - t0
            recall_sum += len(truth & set(got[:K])) / len(truth)
        denom = max(counted, 1)
        rows.append({
            "selectivity": sel,
            "matched": matched,
            "strategy": "-",
            "recall": round(recall_sum / denom, 4),
            "latency_ms": round(lat_sum / denom * 1000.0, 4),
        })
    return rows


def save_results(system, dirpath, base, rows):
    out_dir = os.path.join(dirpath, "results")
    os.makedirs(out_dir, exist_ok=True)
    doc = {
        "system": system,
        "dataset": f"{base.shape[0]}x{base.shape[1]}",
        "k": K,
        "results": rows,
    }
    path = os.path.join(out_dir, f"{system}.json")
    with open(path, "w") as f:
        json.dump(doc, f, indent=2)
    print(f"wrote {path}")
    for r in rows:
        print(f"  sel={r['selectivity']:<6} recall={r['recall']:<7} {r['latency_ms']:.3f} ms")
