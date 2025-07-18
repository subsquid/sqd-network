#[cfg(feature = "builder")]
#[test]
fn test_building() {
    use sqd_assignments::AssignmentBuilder;
    use libp2p_identity::PeerId;

    let mut builder = AssignmentBuilder::new();
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
    let peer_id = PeerId::from_bytes(&[
        0, 32, 98, 135, 18, 113, 48, 224, 113, 242, 246, 177, 220, 29, 234, 70, 180, 119, 204, 168,
        179, 112, 41, 246, 97, 234, 58, 20, 91, 85, 137, 46, 52, 100,
    ])
    .unwrap();
    builder.add_worker(peer_id, sqd_assignments::assignment_fb::WorkerStatus::Ok, &[0]);
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
