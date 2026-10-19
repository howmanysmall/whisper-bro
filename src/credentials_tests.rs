#![allow(clippy::unwrap_used)]

use super::{Source, resolve};
use zeroize::Zeroizing;

fn key(value: &str) -> Option<Zeroizing<String>> {
    Some(Zeroizing::new(value.to_owned()))
}

#[test]
fn stored_key_beats_environment() {
    let (value, source) = resolve(key("stored"), key("environment")).unwrap();
    assert_eq!(value.as_str(), "stored");
    assert_eq!(source, Source::Stored);
}

#[test]
fn environment_is_used_when_nothing_is_stored() {
    let (value, source) = resolve(None, key("  environment\n")).unwrap();
    assert_eq!(value.as_str(), "environment");
    assert_eq!(source, Source::Environment);
}

#[test]
fn blank_environment_counts_as_absent() {
    assert!(resolve(None, key("")).is_none());
    assert!(resolve(None, key("  \t\n")).is_none());
}

#[test]
fn blank_environment_does_not_displace_stored_key() {
    let (value, source) = resolve(key("stored"), key("  ")).unwrap();
    assert_eq!(value.as_str(), "stored");
    assert_eq!(source, Source::Stored);
}

#[test]
fn nothing_anywhere_resolves_to_none() {
    assert!(resolve(None, None).is_none());
}
