"""pgvector runner for the shared filtered-vector benchmark.

  docker run -d --name pgv -e POSTGRES_PASSWORD=pw -p 5432:5432 pgvector/pgvector:pg16
  pip install psycopg2-binary numpy
  python bench/pgvector_bench.py ./benchdata "host=localhost user=postgres password=pw"

Measures pgvector's native filtered query: `WHERE cat < t ORDER BY emb <-> q LIMIT k`.
"""
import sys

import numpy as np
import psycopg2
from psycopg2.extras import execute_values

from common import evaluate, load, save_results


def vec_literal(v):
    return "[" + ",".join(repr(float(x)) for x in v) + "]"


def main():
    dirpath = sys.argv[1] if len(sys.argv) > 1 else "./benchdata"
    dsn = sys.argv[2] if len(sys.argv) > 2 else "host=localhost user=postgres password=pw"
    base, queries, attr = load(dirpath)
    n, dim = base.shape

    conn = psycopg2.connect(dsn)
    conn.autocommit = True
    cur = conn.cursor()
    cur.execute("CREATE EXTENSION IF NOT EXISTS vector")
    cur.execute("DROP TABLE IF EXISTS items")
    cur.execute(f"CREATE TABLE items (id int primary key, emb vector({dim}), cat int)")

    print(f"loading {n} rows into pgvector ...")
    batch = []
    for i in range(n):
        batch.append((i, vec_literal(base[i]), int(attr[i])))
        if len(batch) == 2000:
            execute_values(cur, "INSERT INTO items (id, emb, cat) VALUES %s", batch)
            batch = []
    if batch:
        execute_values(cur, "INSERT INTO items (id, emb, cat) VALUES %s", batch)

    print("building HNSW + btree indexes ...")
    cur.execute("CREATE INDEX ON items USING hnsw (emb vector_l2_ops)")
    cur.execute("CREATE INDEX ON items (cat)")
    cur.execute("ANALYZE items")

    def search(q, thr, k):
        cur.execute(
            "SELECT id FROM items WHERE cat < %s ORDER BY emb <-> %s LIMIT %s",
            (int(thr), vec_literal(q), int(k)),
        )
        return [r[0] for r in cur.fetchall()]

    rows = evaluate(base, queries, attr, search)
    save_results("pgvector", dirpath, base, rows)


if __name__ == "__main__":
    main()
