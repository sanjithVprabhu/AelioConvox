//! The serialized Edge section reproduces the in-memory index's adjacency and traversal.

use std::collections::BTreeSet;

use ll_graph::{serialize_edge_index, EdgeIndex, EdgeView};

fn sample() -> EdgeIndex {
    // 0->1 0->2 1->3 2->3 3->4 4->0 (a small cyclic graph), global ids == local.
    let edges = vec![(0u32, 1u64), (0, 2), (1, 3), (2, 3), (3, 4), (4, 0)];
    EdgeIndex::build(&edges, 5)
}

#[test]
fn view_matches_in_memory() {
    let g = sample();
    let bytes = serialize_edge_index(&g, 9);
    let view = EdgeView::parse(&bytes).expect("parse");

    assert_eq!(view.num_nodes(), g.num_nodes());
    for s in 0..g.num_nodes() as u32 {
        assert_eq!(view.out_neighbors(s), g.out_neighbors(s).to_vec(), "out {s}");
    }
    for t in 0..6u64 {
        assert_eq!(view.in_neighbors(t), g.in_neighbors(t).to_vec(), "in {t}");
        assert_eq!(view.might_target(t), g.might_target(t), "bloom {t}");
    }
}

#[test]
fn traversal_matches_across_formats() {
    let g = sample();
    let bytes = serialize_edge_index(&g, 0);
    let view = EdgeView::parse(&bytes).unwrap();
    let resolve = |t: u64| Some(t as u32);

    for depth in 1..=4 {
        assert_eq!(
            view.traverse_forward(&[0], depth, resolve),
            g.traverse_forward(&[0], depth, resolve),
            "depth {depth}"
        );
    }
    // 2 hops from node 0 reaches {1,2,3}
    assert_eq!(view.traverse_forward(&[0], 2, resolve), BTreeSet::from([1, 2, 3]));
}

#[test]
fn empty_graph_serializes() {
    let g = EdgeIndex::build(&[], 0);
    let bytes = serialize_edge_index(&g, 0);
    let view = EdgeView::parse(&bytes).unwrap();
    assert_eq!(view.num_nodes(), 0);
    assert!(view.out_neighbors(0).is_empty());
    assert!(view.in_neighbors(0).is_empty());
}
