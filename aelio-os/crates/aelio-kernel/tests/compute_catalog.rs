//! §9 compute catalog — structure + list ops (F-008). Numeric/compare/string/hash covered by
//! conformance vectors; this exercises the newly-added pure structure and list families.

use aelio_kernel::compute::apply;
use aelio_kernel::error::ReasonCode;
use aelio_sol::SolValue as V;

fn s(x: &str) -> V {
    V::str(x)
}

#[test]
fn merge_honors_on_conflict() {
    let l = V::map([("a", V::Int(1)), ("b", V::Int(2))]);
    let r = V::map([("b", V::Int(9)), ("c", V::Int(3))]);
    // error (default) → Shape on the `b` clash.
    let e = apply("merge", &[l.clone(), r.clone(), s("error")]).unwrap_err();
    assert_eq!(e.code, ReasonCode::Shape);
    // left keeps b=2; right takes b=9. Non-clashing c=3 lands in both.
    let left = apply("merge", &[l.clone(), r.clone(), s("left")]).unwrap();
    assert_eq!(left.as_map().unwrap().get("b"), Some(&V::Int(2)));
    let right = apply("merge", &[l, r, s("right")]).unwrap();
    assert_eq!(right.as_map().unwrap().get("b"), Some(&V::Int(9)));
    assert_eq!(right.as_map().unwrap().get("c"), Some(&V::Int(3)));
}

#[test]
fn drop_and_keep() {
    let m = V::map([("a", V::Int(1)), ("b", V::Int(2)), ("c", V::Int(3))]);
    let dropped = apply("drop", &[m.clone(), V::list([s("b")])]).unwrap();
    assert_eq!(dropped.as_map().unwrap().len(), 2);
    assert!(dropped.as_map().unwrap().get("b").is_none());
    // single-key string form
    let kept = apply("keep", &[m, s("a")]).unwrap();
    assert_eq!(kept.as_map().unwrap().len(), 1);
    assert_eq!(kept.as_map().unwrap().get("a"), Some(&V::Int(1)));
}

#[test]
fn path_copy_copies_nested_value() {
    let container = V::map([("src", V::map([("token", s("xyz"))]))]);
    let out = apply("path_copy", &[container, s("src.token"), s("dst.token")]).unwrap();
    let dst = out
        .as_map()
        .unwrap()
        .get("dst")
        .and_then(|d| d.as_map())
        .and_then(|m| m.get("token"));
    assert_eq!(dst, Some(&s("xyz")));
    // missing source → Missing
    let empty = V::map::<_, &str>([]);
    let e = apply("path_copy", &[empty, s("nope"), s("x")]).unwrap_err();
    assert_eq!(e.code, ReasonCode::Missing);
}

#[test]
fn list_count_first_last_contains() {
    let l = V::list([V::Int(10), V::Int(20), V::Int(30)]);
    assert_eq!(apply("count", std::slice::from_ref(&l)).unwrap(), V::Int(3));
    assert_eq!(
        apply("first", std::slice::from_ref(&l)).unwrap(),
        V::Int(10)
    );
    assert_eq!(apply("last", std::slice::from_ref(&l)).unwrap(), V::Int(30));
    assert_eq!(
        apply("list_contains", &[l.clone(), V::Int(20)]).unwrap(),
        V::Bool(true)
    );
    assert_eq!(
        apply("list_contains", &[l, V::Int(99)]).unwrap(),
        V::Bool(false)
    );
    // first/last on empty → Missing
    assert_eq!(
        apply("first", &[V::list([])]).unwrap_err().code,
        ReasonCode::Missing
    );
}

#[test]
fn append_and_slice_enforce_max_items() {
    let l = V::list([V::Int(1), V::Int(2)]);
    // append within budget
    let a = apply("append", &[l.clone(), V::Int(3), V::Int(5)]).unwrap();
    assert_eq!(a.as_list().unwrap().len(), 3);
    // append over max_items → Budget.Size
    let e = apply("append", &[l.clone(), V::Int(3), V::Int(2)]).unwrap_err();
    assert_eq!(e.code, ReasonCode::BudgetSize);

    let big = V::list([V::Int(0), V::Int(1), V::Int(2), V::Int(3)]);
    let sl = apply("slice", &[big.clone(), V::Int(1), V::Int(3), V::Int(5)]).unwrap();
    assert_eq!(sl.as_list().unwrap(), &[V::Int(1), V::Int(2)]);
    // slice wider than max_items → Budget.Size
    assert_eq!(
        apply("slice", &[big.clone(), V::Int(0), V::Int(4), V::Int(2)])
            .unwrap_err()
            .code,
        ReasonCode::BudgetSize
    );
    // out-of-range bounds → Type
    assert_eq!(
        apply("slice", &[big, V::Int(0), V::Int(9), V::Int(9)])
            .unwrap_err()
            .code,
        ReasonCode::Type
    );
}
