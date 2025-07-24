mod common;

#[cfg(feature = "reader")]
#[test]
fn test_get_worker() {
    use std::collections::BTreeMap;

    let keypair = common::get_test_keypair();
    let peer_id = keypair.public().to_peer_id();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("assignment.fb");
    let buf = std::fs::read(path).expect("Failed to read assignment.fb");
    let assignment = sqd_assignments::Assignment::from_owned(buf).unwrap();

    assert_eq!(assignment.get_worker_id(0).unwrap(), peer_id);

    let worker = assignment.get_worker(peer_id).unwrap();
    let headers = worker.decrypt_headers(&keypair).unwrap();
    assert_eq!(
        headers,
        BTreeMap::from([
            ("worker-id".to_owned(), peer_id.to_string()),
            (
                "worker-signature".to_owned(),
                "1750000000-hTro4YogGa0rcUMSZKpcQwdAh9O16hQnl1r05kNbtCc%3D".to_owned()
            )
        ])
    );
    assert_eq!(worker.status(), sqd_assignments::WorkerStatus::Ok);

    let chunks = worker.chunks();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks.get(0).id(), "0221000000/0221000000-0221000649-BQJdx");
    assert_eq!(chunks.get(1).id(), "0221000000/0221000650-0221001549-AuRE1");
}

#[cfg(feature = "reader")]
#[test]
fn test_get_chunks() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("assignment.fb");
    let buf = std::fs::read(path).expect("Failed to read assignment.fb");
    let assignment = sqd_assignments::Assignment::from_owned(buf).unwrap();

    let dataset = assignment.get_dataset("s3://solana-mainnet-2").unwrap();
    assert_eq!(dataset.last_block(), 221001549);

    assert_eq!(assignment.find_chunk("s3://dummy", 0), None);
    assert_eq!(assignment.find_chunk("s3://solana-mainnet-2", 220999999), None);
    assert_eq!(assignment.find_chunk("s3://solana-mainnet-2", 221001550), None);
    let chunk1 = assignment.find_chunk("s3://solana-mainnet-2", 221000000).unwrap();
    let chunk2 = assignment.find_chunk("s3://solana-mainnet-2", 221000650).unwrap();

    assert_eq!(chunk1.id(), "0221000000/0221000000-0221000649-BQJdx");
    assert_eq!(chunk1.base_url(), "0221000000/0221000000-0221000649-BQJdx");
    assert_eq!(chunk1.dataset_id(), "s3://solana-mainnet-2");
    assert_eq!(chunk1.dataset_base_url(), "https://solana-mainnet-2.sqd-datasets.io");
    assert_eq!(chunk1.first_block(), 221000000);
    assert_eq!(chunk1.size(), 1000000);
    assert_eq!(chunk1.files().len(), 3);
    chunk1
        .files()
        .iter()
        .zip(["blocks.parquet", "transactions.parquet", "logs.parquet"].into_iter())
        .for_each(|(file, expected)| {
            assert_eq!(file.filename(), expected);
            assert_eq!(file.url(), expected);
        });
    assert_eq!(chunk1.last_block_hash(), Some("BQJdx"));
    assert_eq!(chunk1.last_block_timestamp(), Some(1696192039));
    assert_eq!(chunk1.worker_indexes().iter().collect::<Vec<_>>(), vec![0]);

    assert_eq!(chunk2.first_block(), 221000650);
    assert_eq!(chunk2.last_block_hash(), None);
    assert_eq!(chunk2.last_block_timestamp(), None);
}
