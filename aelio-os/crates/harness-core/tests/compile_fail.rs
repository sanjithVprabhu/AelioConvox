#[test]
fn authority_types_are_structurally_closed() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
