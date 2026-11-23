//! Computed calls of own functions on ordinary objects: dynamic keys and
//! receivers, data/accessor transitions, live prototypes, proxies, symbols,
//! classes, in-place key construction and one-shot exception propagation.
//! Reference output is generated with Node on the exact JS fixture.

#[test]
fn computed_own_calls_match_node_through_mutation_and_exceptions() {
    let result = zipp_vm::run(include_str!("fixtures/computed_own_calls.mjs"))
        .expect("computed-call fixture compiles");
    assert!(result.error.is_none(), "runtime exception: {:?}", result.error);
    assert_eq!(
        result.output,
        [include_str!("fixtures/computed_own_calls.node.json").trim_end()],
    );
}
