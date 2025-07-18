#[cfg(feature = "builder")]
#[test]
fn test_building() {
    use libp2p_identity::Keypair;
    use rand::{rngs::StdRng, SeedableRng};
    use sqd_assignments::AssignmentBuilder;

    let mut builder = AssignmentBuilder::new_with_rng("test-secret", StdRng::seed_from_u64(0));

    builder
        .new_chunk()
        .id("0221000000/0221000000-0221000649-9QgFD")
        .dataset_id("s3://solana-mainnet-2")
        .dataset_base_url("https://solana-mainnet-2.sqd-datasets.io")
        .first_block(221000000)
        .size(1000000)
        .worker_indexes(&[0])
        .files(&[
            String::from("blocks.parquet"),
            String::from("transactions.parquet"),
            String::from("logs.parquet"),
        ])
        .finish();
    builder.finish_dataset();

    let keypair = Keypair::ed25519_from_bytes([
        19, 199, 234, 213, 79, 151, 179, 242, 187, 43, 210, 20, 250, 252, 12, 246, 223, 244, 119,
        225, 81, 225, 146, 40, 65, 35, 81, 91, 121, 13, 204, 37,
    ])
    .unwrap();
    let peer_id = keypair.public().to_peer_id();
    let timestamp = 1750000000;
    builder.add_worker_with_timestamp(peer_id, sqd_assignments::WorkerStatus::Ok, &[0], timestamp);

    let bytes = builder.finish();
    assert_file_equals("assignment.fb", bytes);
}

fn assert_file_equals(filename: &str, bytes: Vec<u8>) {
    use std::fs;
    use std::path::PathBuf;

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
