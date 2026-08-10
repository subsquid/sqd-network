use sqd_assignments::{NetworkState, SchemaBundle};

const LEGACY_STATE: &str = r#"{
  "network": "testnet",
  "assignment": {
    "url": "",
    "fb_url": "https://example.test/legacy.fb.0.gz",
    "fb_url_v1": "https://example.test/legacy.fb.1.gz",
    "id": "2026-06-09T12:00:00_LEGACY",
    "effective_from": 1781000000
  }
}"#;

#[test]
fn legacy_network_state_deserializes() {
    let state: NetworkState = serde_json::from_str(LEGACY_STATE).unwrap();

    assert_eq!(state.network, "testnet");
    assert_eq!(state.assignment.id, "2026-06-09T12:00:00_LEGACY");

    assert!(state.worker_assignment.is_none());
    assert!(state.portal_assignment.is_none());
    assert!(state.schema_bundle.is_none());
}

#[allow(deprecated)]
#[test]
fn deprecated_assignment_urls_default_and_skip_when_absent() {
    let state: NetworkState = serde_json::from_str(
        r#"{
          "network": "testnet",
          "assignment": {
            "fb_url_v1": "https://example.test/legacy.fb.1.gz",
            "id": "legacy",
            "effective_from": 1781000000
          }
        }"#,
    )
    .unwrap();

    assert!(state.assignment.url.is_none());
    assert!(state.assignment.fb_url.is_none());

    let serialized = serde_json::to_value(state).unwrap();
    assert!(serialized["assignment"].get("url").is_none());
    assert!(serialized["assignment"].get("fb_url").is_none());
}

#[test]
fn split_network_state_deserializes() {
    let state: NetworkState = serde_json::from_str(
        r#"{
          "network": "testnet",
          "assignment": {
            "url": "",
            "fb_url": "https://example.test/legacy.fb.0.gz",
            "fb_url_v1": "https://example.test/legacy.fb.1.gz",
            "id": "legacy",
            "effective_from": 1781000000
          },
          "worker_assignment": {
            "url": "",
            "fb_url": "https://example.test/worker.fb.0.gz",
            "fb_url_v1": "https://example.test/worker.fb.1.gz",
            "id": "worker",
            "effective_from": 1781000000
          },
          "portal_assignment": {
            "url": "",
            "fb_url": "https://example.test/portal.fb.0.gz",
            "fb_url_v1": "https://example.test/portal.fb.1.gz",
            "id": "portal",
            "effective_from": 1781000000
          },
          "schema_bundle": {
            "hash": "a1b2c3",
            "url": "https://example.test/schema.bundle.gz"
          }
        }"#,
    )
    .unwrap();

    assert_eq!(state.assignment.id, "legacy");
    assert_eq!(state.worker_assignment.unwrap().id, "worker");
    assert_eq!(state.portal_assignment.unwrap().id, "portal");

    assert_eq!(
        state.schema_bundle,
        Some(SchemaBundle {
            hash: "a1b2c3".to_owned(),
            url: "https://example.test/schema.bundle.gz".to_owned(),
        })
    );
}

#[test]
fn schema_bundle_requires_url() {
    for bundle in [r#"{"hash": "a1b2c3"}"#, r#"{"hash": "a1b2c3", "url": null}"#] {
        let json = format!(
            r#"{{
              "network": "testnet",
              "assignment": {{
                "fb_url_v1": "https://example.test/legacy.fb.1.gz",
                "id": "legacy",
                "effective_from": 1781000000
              }},
              "schema_bundle": {bundle}
            }}"#
        );

        serde_json::from_str::<NetworkState>(&json)
            .expect_err(&format!("schema bundle without a url must be rejected: {bundle}"));
    }
}

#[test]
fn schema_bundle_round_trips_through_json() {
    let bundle = SchemaBundle {
        hash: "a1b2c3".to_owned(),
        url: "https://example.test/schema.bundle.gz".to_owned(),
    };

    let serialized = serde_json::to_value(&bundle).unwrap();
    assert_eq!(serialized["hash"], "a1b2c3");
    assert_eq!(serialized["url"], "https://example.test/schema.bundle.gz");

    let round_tripped: SchemaBundle = serde_json::from_value(serialized).unwrap();
    assert_eq!(round_tripped, bundle);
}

#[test]
fn absent_split_assignments_stay_omitted_after_json_round_trip() {
    let state: NetworkState = serde_json::from_str(LEGACY_STATE).unwrap();

    let serialized = serde_json::to_string(&state).unwrap();
    let round_tripped: NetworkState = serde_json::from_str(&serialized).unwrap();
    let value = serde_json::to_value(round_tripped).unwrap();

    assert!(value.get("assignment").is_some());
    assert!(value.get("worker_assignment").is_none());
    assert!(value.get("portal_assignment").is_none());
    assert!(value.get("schema_bundle").is_none());
}
