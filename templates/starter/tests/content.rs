//! Runs the content tests from src/lib.rs as part of `cargo test`.

use autoengine::tools::scenario::run_all;

#[test]
fn content_tests_pass() {
    let failures: Vec<String> = run_all(&mygame::content(), &mygame::tests())
        .into_iter()
        .filter(|r| !r.passed())
        .map(|r| format!("{}: {}", r.name, r.failures.join("; ")))
        .collect();
    assert!(failures.is_empty(), "{failures:#?}");
}
