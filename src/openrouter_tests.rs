use super::*;

const REASON: &str = r#"{"error":{"message":"Provider said no","code":1}}"#;

#[test]
fn statuses_map_to_distinct_actionable_errors() {
    assert!(matches!(classify(401, REASON), ApiError::Unauthorized));
    assert!(matches!(classify(403, REASON), ApiError::Unauthorized));
    assert!(matches!(classify(402, REASON), ApiError::NoCredits));
    assert!(matches!(classify(429, REASON), ApiError::RateLimited));
    match classify(503, REASON) {
        ApiError::Server {
            status: 503,
            message,
        } => assert_eq!(message, "Provider said no"),
        other => panic!("{other:?}"),
    }
    match classify(400, REASON) {
        ApiError::Rejected {
            status: 400,
            message,
        } => assert_eq!(message, "Provider said no"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_non_json_body_still_surfaces_its_text() {
    match classify(502, "Bad gateway from upstream") {
        ApiError::Server { message, .. } => assert_eq!(message, "Bad gateway from upstream"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn long_bodies_are_truncated() {
    match classify(400, &"x".repeat(1000)) {
        ApiError::Rejected { message, .. } => assert_eq!(message.len(), 200),
        other => panic!("{other:?}"),
    }
}

#[test]
fn only_transient_failures_are_retryable() {
    let cases = [
        (classify(401, ""), false),
        (classify(402, ""), false),
        (classify(429, ""), false),
        (classify(400, ""), false),
        (ApiError::Invalid("x".into()), false),
        (classify(503, ""), true),
        (ApiError::Connect("x".into()), true),
        (ApiError::Timeout, true),
    ];
    for (error, expected) in cases {
        assert_eq!(error.retryable(), expected, "{error:?}");
    }
}

#[test]
fn messages_tell_the_user_what_to_do() {
    assert!(ApiError::Unauthorized.to_string().contains("yap setup"));
    assert_eq!(
        ApiError::NoCredits.to_string(),
        "OpenRouter account has no credits"
    );
    assert_eq!(
        classify(400, REASON).to_string(),
        "OpenRouter rejected the request (400): Provider said no"
    );
}
