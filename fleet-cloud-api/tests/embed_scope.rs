use fleet_cloud_api::embed::{EmbedClaims, EmbedTokenVerifier};
use uuid::Uuid;

#[test]
fn embed_token_enforces_signature_expiry_origin_and_task_scope() {
    let verifier = EmbedTokenVerifier::new(b"test-only-embed-secret");
    let task_id = Uuid::new_v4();
    let claims = EmbedClaims {
        organization_id: Uuid::new_v4(),
        project_id: Uuid::new_v4(),
        task_id: Some(task_id),
        origins: vec!["https://tickets.example.com".into()],
        expires_at: 2_000,
    };
    let token = verifier.issue(&claims).unwrap();

    let verified = verifier
        .verify(&token, "https://tickets.example.com", 1_999)
        .unwrap();
    assert_eq!(verified, claims);
    assert!(verified.allows_task(task_id));
    assert!(!verified.allows_task(Uuid::new_v4()));
    assert!(verifier
        .verify(&token, "https://evil.example.com", 1_999)
        .is_err());
    assert!(verifier
        .verify(&token, "https://tickets.example.com", 2_000)
        .is_err());

    let mut tampered = token.into_bytes();
    let last = tampered.last_mut().unwrap();
    *last = if *last == b'A' { b'B' } else { b'A' };
    assert!(verifier
        .verify(
            std::str::from_utf8(&tampered).unwrap(),
            "https://tickets.example.com",
            1_999,
        )
        .is_err());
}
