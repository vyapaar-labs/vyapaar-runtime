//! Tests for RunId validation and generation (ADR 0017, Crockford Base32).

use vyapaar_runtime_api::{RunId, RunIdError, RunIdGenerator};

#[test]
fn parse_valid_run_id() {
    let valid = "01JBP8R3D5K87M4Q2N1W6E9TXV";
    let parsed = RunId::parse(valid).expect("should parse valid run id");
    assert_eq!(parsed.as_str(), valid);
    assert_eq!(parsed.to_string(), valid);
}

#[test]
fn parse_rejects_invalid_lengths() {
    assert_eq!(RunId::parse(""), Err(RunIdError::InvalidLength(0)));
    assert_eq!(RunId::parse("01JBP8"), Err(RunIdError::InvalidLength(6)));
    assert_eq!(
        RunId::parse("01JBP8R3D5K87M4Q2N1W6E9TX"),
        Err(RunIdError::InvalidLength(25))
    );
    assert_eq!(
        RunId::parse("01JBP8R3D5K87M4Q2N1W6E9TXVA"),
        Err(RunIdError::InvalidLength(27))
    );
}

#[test]
fn parse_rejects_path_traversal() {
    assert_eq!(RunId::parse("../"), Err(RunIdError::InvalidLength(3)));
    assert_eq!(
        RunId::parse("../0123456789ABCDEFGHIJKLM"),
        Err(RunIdError::InvalidCharacter('.'))
    );
    assert_eq!(
        RunId::parse("..%2F..%2Fetc0000000000000"),
        Err(RunIdError::InvalidCharacter('.'))
    );
    assert_eq!(
        RunId::parse("%2e%2e%2f00000000000000000"),
        Err(RunIdError::InvalidCharacter('%'))
    );
    assert_eq!(
        RunId::parse("..\\x0000000000000000000000"),
        Err(RunIdError::InvalidCharacter('.'))
    );
    assert_eq!(
        RunId::parse("/etc/passwd000000000000000"),
        Err(RunIdError::InvalidCharacter('/'))
    );
}

#[test]
fn parse_rejects_crockford_exclusions() {
    // Excluded letters: I, L, O, U
    assert_eq!(
        RunId::parse("01JBP8R3D5K87M4Q2N1W6E9TXI"),
        Err(RunIdError::InvalidCharacter('I'))
    );
    assert_eq!(
        RunId::parse("01JBP8R3D5K87M4Q2N1W6E9TXL"),
        Err(RunIdError::InvalidCharacter('L'))
    );
    assert_eq!(
        RunId::parse("01JBP8R3D5K87M4Q2N1W6E9TXO"),
        Err(RunIdError::InvalidCharacter('O'))
    );
    assert_eq!(
        RunId::parse("01JBP8R3D5K87M4Q2N1W6E9TXU"),
        Err(RunIdError::InvalidCharacter('U'))
    );
    // Lowercase rejected
    assert_eq!(
        RunId::parse("01jbp8r3d5k87m4q2n1w6e9txv"),
        Err(RunIdError::InvalidCharacter('j'))
    );
}

#[test]
fn generator_produces_valid_and_monotonic_ids() {
    let mut generator = RunIdGenerator::default();
    let id1 = generator.next(1000, [1; 10]).unwrap();
    let id2 = generator.next(1000, [1; 10]).unwrap();
    let id3 = generator.next(1001, [1; 10]).unwrap();

    assert_eq!(RunId::parse(id1.as_str()).unwrap(), id1);
    assert_eq!(RunId::parse(id2.as_str()).unwrap(), id2);
    assert_eq!(RunId::parse(id3.as_str()).unwrap(), id3);

    assert!(id1 < id2);
    assert!(id2 < id3);
}
