use aelio_sol::SolValue;
use harness_core::taint::{
    check_prohibited, declassify_count, join_taint, tainted_eq, PcTaint, ProhibitedPosition,
    TaintedList, TaintedMap, TaintedValue,
};

#[test]
fn join_taint_propagates_any_input() {
    assert!(!join_taint([false, false]));
    assert!(join_taint([false, true]));
    assert!(join_taint([true, false]));
}

#[test]
fn all_nine_prohibited_positions_reject_tainted_values() {
    for position in ProhibitedPosition::ALL {
        let value = TaintedValue::tainted("secret".to_owned());
        let err = check_prohibited(position, &value).unwrap_err();
        assert_eq!(
            err,
            harness_core::taint::TaintError::Prohibited { position }
        );
    }
}

#[test]
fn clean_values_pass_prohibited_checks() {
    for position in ProhibitedPosition::ALL {
        let value = TaintedValue::clean("safe".to_owned());
        check_prohibited(position, &value).unwrap();
    }
}

#[test]
fn map_keys_cannot_launder_taint() {
    let mut map = TaintedMap {
        keys_tainted: false,
        entries: Default::default(),
    };
    let tainted_key = TaintedValue::tainted("city".to_owned());
    let clean_value = TaintedValue::clean(SolValue::Int(42));
    map.insert(tainted_key, clean_value);
    assert!(map.keys_tainted);
    let extracted = map.key_as_tainted_string("city").unwrap();
    assert!(extracted.tainted);
    assert_eq!(extracted.value, "city");
}

#[test]
fn group_by_shaped_construction_still_works() {
    let mut map = TaintedMap {
        keys_tainted: false,
        entries: Default::default(),
    };
    map.insert(
        TaintedValue::clean("city".to_owned()),
        TaintedValue::clean(SolValue::Int(1)),
    );
    assert!(!map.keys_tainted);
    let extracted = map.key_as_tainted_string("city").unwrap();
    assert!(!extracted.tainted);
}

#[test]
fn wrapping_in_a_list_does_not_launder() {
    let list = TaintedList::from_items(vec![TaintedValue::tainted(SolValue::Str("secret".into()))]);

    assert!(list.container_tainted);
}

#[test]
fn nesting_two_deep_does_not_launder() {
    let inner = TaintedList::from_items(vec![TaintedValue::tainted(SolValue::Int(7))]);
    let outer = TaintedList::from_items(vec![TaintedValue::clean(inner)]);

    assert!(outer.container_tainted);
    assert!(outer.get(0).unwrap().tainted);
}

#[test]
fn indexing_out_of_a_tainted_container_stays_tainted() {
    let list = TaintedList::from_items(vec![TaintedValue::tainted(SolValue::Int(7))]);

    let item = list.get(0).unwrap();
    assert_eq!(item.value, SolValue::Int(7));
    assert!(item.tainted);
}

#[test]
fn a_clean_element_of_a_tainted_container_is_conservatively_tainted() {
    let list = TaintedList::from_items(vec![
        TaintedValue::tainted(SolValue::Str("secret".into())),
        TaintedValue::clean(SolValue::Int(7)),
    ]);

    assert!(list.get(1).unwrap().tainted);
}

#[test]
fn comparison_result_carries_taint() {
    let result = tainted_eq(
        &TaintedValue::tainted(SolValue::Int(7)),
        &TaintedValue::clean(SolValue::Int(7)),
    );

    assert!(result.value);
    assert!(result.tainted);
}

#[test]
fn length_carries_taint_until_explicitly_declassified() {
    let list = TaintedList::from_items(vec![TaintedValue::tainted(SolValue::Int(7))]);

    let length = list.len();
    assert_eq!(length.value, 1);
    assert!(length.tainted);

    let declassified = declassify_count(&list.items);
    assert_eq!(declassified.value, 1);
    assert!(!declassified.tainted);
}

#[test]
fn implicit_flow_through_branch_selection_is_caught() {
    let pc = PcTaint::inherit_from_predicate(true);
    let selected = pc.taint_assignment(TaintedValue::clean(SolValue::Str("allow".into())));

    assert!(selected.tainted);
}

#[test]
fn pc_taint_is_inherited_by_nested_branches() {
    let inherited = PcTaint::inherit_from_predicate(true);
    let nested = inherited.enter_branch(false);
    assert!(nested.tainted);
    let joined = inherited.join(PcTaint::default());
    assert!(joined.tainted);
}

#[test]
fn declassify_count_is_clean_cardinality() {
    let items = vec![
        TaintedValue::tainted(SolValue::Str("secret".into())),
        TaintedValue::tainted(SolValue::Int(2)),
    ];
    let count = declassify_count(&items);
    assert!(!count.tainted);
    assert_eq!(count.value, 2);
}
