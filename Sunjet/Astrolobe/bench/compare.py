"""Aggregate every <dir>/results/*.json into a side-by-side comparison table.

  python bench/compare.py ./benchdata
"""
import glob
import json
import os
import sys


def main():
    dirpath = sys.argv[1] if len(sys.argv) > 1 else "./benchdata"
    files = sorted(glob.glob(os.path.join(dirpath, "results", "*.json")))
    if not files:
        print(f"no results in {dirpath}/results — run the per-system benchmarks first")
        return

    systems = []
    for f in files:
        with open(f) as fh:
            systems.append(json.load(fh))

    # selectivities from the first system (all share the dataset/sweep)
    sels = [r["selectivity"] for r in systems[0]["results"]]
    names = [s["system"] for s in systems]
    dataset = systems[0].get("dataset", "?")

    print(f"\nDataset: {dataset}   k={systems[0].get('k')}   (filtered vector search)\n")
    print("RECALL@k")
    header = f"{'sel':>7} " + "".join(f"{n:>12}" for n in names)
    print(header)
    print("-" * len(header))
    for i, sel in enumerate(sels):
        line = f"{sel:>7.3f} "
        for s in systems:
            line += f"{s['results'][i]['recall']:>12.3f}"
        print(line)

    print("\nLATENCY (ms/query)")
    print(header)
    print("-" * len(header))
    for i, sel in enumerate(sels):
        line = f"{sel:>7.3f} "
        for s in systems:
            line += f"{s['results'][i]['latency_ms']:>12.3f}"
        print(line)

    print("\nNote: recall vs selectivity is the headline — a system that ignores selectivity")
    print("when planning collapses on selective filters.")


if __name__ == "__main__":
    main()
