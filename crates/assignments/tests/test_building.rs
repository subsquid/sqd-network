mod common;

#[cfg(feature = "builder")]
#[test]
fn test_building() {
    use rand::{rngs::StdRng, SeedableRng};
    use sqd_assignments::AssignmentBuilder;

    let mut builder = AssignmentBuilder::new_with_rng("test-secret", StdRng::seed_from_u64(0))
        .check_continuity(false);

    builder
        .new_chunk()
        .id("0221000000/0221000000-0221000649-BQJdx")
        .dataset_id("s3://solana-mainnet-2")
        .dataset_base_url("https://solana-mainnet-2.sqd-datasets.io")
        .block_range(221000000..=221000649)
        .size(1000000)
        .worker_indexes(&[0])
        .last_block_hash("BQJdx")
        .last_block_timestamp(1696192039)
        .files(&[
            String::from("blocks.parquet"),
            String::from("transactions.parquet"),
            String::from("logs.parquet"),
        ])
        .finish()
        .unwrap();
    builder
        .new_chunk()
        .id("0221000000/0221000650-0221001549-AuRE1")
        .dataset_id("s3://solana-mainnet-2")
        .dataset_base_url("https://solana-mainnet-2.sqd-datasets.io")
        .block_range(221000650..=221001549)
        .size(1000000)
        .worker_indexes(&[0])
        .last_block_timestamp(1696193050)
        .files(&[
            String::from("blocks.parquet"),
            String::from("transactions.parquet"),
            String::from("logs.parquet"),
        ])
        .finish()
        .unwrap();
    builder.finish_dataset();

    let keypair = common::get_test_keypair();
    let peer_id = keypair.public().to_peer_id();
    let timestamp = 1750000000;
    builder.add_worker_with_timestamp(
        peer_id,
        sqd_assignments::WorkerStatus::Ok,
        &[0, 1],
        timestamp,
    );

    let bytes = builder.finish();
    assert_file_equals("assignment.fb", bytes);
}

#[cfg(feature = "builder")]
fn assert_file_equals(filename: &str, bytes: Vec<u8>) {
    use std::{fs, path::PathBuf};

    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push(filename);

    let matches = if let Ok(contents) = fs::read(&path) {
        contents == bytes
    } else {
        false
    };

    if !matches {
        path.set_file_name(format!("{}.actual", filename));
        fs::write(path, bytes).expect("Failed to write actual file");
        panic!("Binary files differ");
    }
}
