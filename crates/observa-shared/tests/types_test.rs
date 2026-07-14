use observa_shared::Severity;

#[test]
fn severity_ordering_exists() {
    assert!(Severity::Critical > Severity::Error);
}
