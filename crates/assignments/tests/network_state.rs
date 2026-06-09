use sqd_assignments::NetworkState;

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

    #[cfg(feature = "mvcc-chunks")]
    {
        assert!(state.worker_assignment.is_none());
        assert!(state.portal_assignment.is_none());
    }
}

#[cfg(feature = "mvcc-chunks")]
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
          }
        }"#,
    )
    .unwrap();

    assert_eq!(state.assignment.id, "legacy");
    assert_eq!(state.worker_assignment.unwrap().id, "worker");
    assert_eq!(state.portal_assignment.unwrap().id, "portal");
}

#[cfg(feature = "mvcc-chunks")]
#[test]
fn absent_split_assignments_are_skipped_when_serializing() {
    let state: NetworkState = serde_json::from_str(LEGACY_STATE).unwrap();

    let serialized = serde_json::to_value(state).unwrap();

    assert!(serialized.get("assignment").is_some());
    assert!(serialized.get("worker_assignment").is_none());
    assert!(serialized.get("portal_assignment").is_none());
}
