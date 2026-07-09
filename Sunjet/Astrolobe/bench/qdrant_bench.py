"""Qdrant runner for the shared filtered-vector benchmark.

  docker run -d --name qdrant -p 6333:6333 qdrant/qdrant
  pip install qdrant-client numpy
  python bench/qdrant_bench.py ./benchdata http://localhost:6333

Measures Qdrant's native filtered search (filter on `cat`, L2 nearest, limit k).
"""
import sys

import numpy as np
from qdrant_client import QdrantClient, models

from common import evaluate, load, save_results

COLLECTION = "bench"


def main():
    dirpath = sys.argv[1] if len(sys.argv) > 1 else "./benchdata"
    url = sys.argv[2] if len(sys.argv) > 2 else "http://localhost:6333"
    base, queries, attr = load(dirpath)
    n, dim = base.shape

    client = QdrantClient(url=url)
    client.recreate_collection(
        COLLECTION,
        vectors_config=models.VectorParams(size=int(dim), distance=models.Distance.EUCLID),
    )

    print(f"uploading {n} points into Qdrant ...")
    batch = 2000
    for start in range(0, n, batch):
        end = min(start + batch, n)
        client.upsert(
            COLLECTION,
            points=models.Batch(
                ids=list(range(start, end)),
                vectors=[base[i].tolist() for i in range(start, end)],
                payloads=[{"cat": int(attr[i])} for i in range(start, end)],
            ),
        )
    # Index the payload field for efficient filtering.
    client.create_payload_index(COLLECTION, "cat", field_schema=models.PayloadSchemaType.INTEGER)

    def search(q, thr, k):
        res = client.query_points(
            collection_name=COLLECTION,
            query=q.tolist(),
            query_filter=models.Filter(
                must=[models.FieldCondition(key="cat", range=models.Range(lt=float(thr)))]
            ),
            limit=int(k),
        ).points
        return [p.id for p in res]

    rows = evaluate(base, queries, attr, search)
    save_results("qdrant", dirpath, base, rows)


if __name__ == "__main__":
    main()
