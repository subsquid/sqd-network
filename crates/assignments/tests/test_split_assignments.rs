mod common;

#[cfg(all(feature = "builder", feature = "reader"))]
#[test]
fn test_worker_assignment_round_trip() {
    use rand::{rngs::StdRng, SeedableRng};
    use sqd_assignments::WorkerAssignmentBuilder;

    let mut builder =
        WorkerAssignmentBuilder::new_with_rng("test-secret", StdRng::seed_from_u64(0))
            .check_continuity(false);

    builder
        .new_chunk()
        .id("0221000000/0221000000-0221000649-BQJdx")
        .dataset_id("s3://solana-mainnet-2")
        .dataset_base_url("https://solana-mainnet-2.sqd-datasets.io")
        .block_range(221000000..=221000649)
        .size(1000000)
        .schema_id(7)
        .tables_present(&["blocks", "transactions"])
        .worker_indexes(&[0])
        .finish()
        .unwrap();
    builder
        .new_chunk()
        .id("0221000000/0221000650-0221001549-AuRE1")
        .dataset_id("s3://solana-mainnet-2")
        .dataset_base_url("https://solana-mainnet-2.sqd-datasets.io")
        .block_range(221000650..=221001549)
        .size(1000000)
        .schema_id(7)
        .worker_indexes(&[0])
        .finish()
        .unwrap();
    builder.finish_dataset();

    let keypair = common::get_test_keypair();
    let peer_id = keypair.public().to_peer_id();
    let timestamp = 1750000000;
    builder.add_worker_with_timestamp(peer_id, sqd_assignments::WorkerStatus::Ok, timestamp);

    let bytes = builder.finish();
    let assignment = sqd_assignments::WorkerAssignment::from_owned(bytes).unwrap();

    let dataset = assignment.get_dataset("s3://solana-mainnet-2").unwrap();
    assert_eq!(dataset.last_block(), 221001549);
    assert_eq!(dataset.chunks().len(), 2);

    let worker = assignment.get_worker(&peer_id).unwrap();
    assert_eq!(worker.status(), sqd_assignments::WorkerStatus::Ok);
    let headers = worker.decrypt_headers(&keypair).unwrap();
    assert_eq!(headers.get("worker-id"), Some(&peer_id.to_string()));

    let chunks = worker.iter_chunks().collect::<Vec<_>>();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].id(), "0221000000/0221000000-0221000649-BQJdx");
    assert_eq!(chunks[0].dataset_base_url(), "https://solana-mainnet-2.sqd-datasets.io");
    assert_eq!(chunks[0].schema_id(), 7);
    assert_eq!(
        chunks[0].tables_present().unwrap().iter().collect::<Vec<_>>(),
        vec!["blocks", "transactions"]
    );
    assert!(chunks[1].tables_present().is_none(), "unset tables_present means all present");
}

#[cfg(all(feature = "builder", feature = "reader"))]
#[test]
fn test_portal_assignment_round_trip() {
    use sqd_assignments::PortalAssignmentBuilder;

    let mut builder = PortalAssignmentBuilder::new().check_continuity(false);

    builder
        .new_chunk()
        .id("0221000000/0221000000-0221000649-BQJdx")
        .dataset_id("s3://solana-mainnet-2")
        .block_range(221000000..=221000649)
        .last_block_timestamp(1696192039)
        .worker_indexes(&[0])
        .finish()
        .unwrap();
    builder
        .new_chunk()
        .id("0221000000/0221000650-0221001549-AuRE1")
        .dataset_id("s3://solana-mainnet-2")
        .block_range(221000650..=221001549)
        .last_block_timestamp(1696193050)
        .worker_indexes(&[0])
        .finish()
        .unwrap();
    builder.finish_dataset(7, Some("BQJdx"));

    let keypair = common::get_test_keypair();
    let peer_id = keypair.public().to_peer_id();
    builder.add_worker(peer_id, sqd_assignments::WorkerStatus::Ok);

    let bytes = builder.finish();
    let assignment = sqd_assignments::PortalAssignment::from_owned(bytes).unwrap();

    let dataset = assignment.get_dataset("s3://solana-mainnet-2").unwrap();
    assert_eq!(dataset.last_block(), 221001549);
    assert_eq!(dataset.schema_id(), 7);
    assert_eq!(dataset.last_block_hash(), Some("BQJdx"));

    assert_eq!(assignment.get_worker_id(0).unwrap(), peer_id);
    let worker = assignment.get_worker_by_index(0);
    assert_eq!(worker.peer_id().unwrap(), peer_id);
    assert_eq!(worker.status(), sqd_assignments::WorkerStatus::Ok);

    let chunk1 = assignment.find_chunk("s3://solana-mainnet-2", 221000000).unwrap();
    assert_eq!(chunk1.id(), "0221000000/0221000000-0221000649-BQJdx");
    assert_eq!(chunk1.last_block_timestamp(), Some(1696192039));
    assert_eq!(chunk1.worker_indexes().iter().collect::<Vec<_>>(), vec![0]);

    assert_eq!(
        assignment.find_chunk("s3://dummy", 0),
        Err(sqd_assignments::ChunkNotFound::UnknownDataset)
    );
}
