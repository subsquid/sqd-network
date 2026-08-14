mod common;

#[cfg(feature = "builder")]
use rand::{rngs::StdRng, SeedableRng};
#[cfg(feature = "builder")]
use sqd_assignments::{WorkerAssignmentBuilder, WorkerAssignmentChunkBuilder};

/// A deterministically seeded builder with no write schema registered yet.
#[cfg(feature = "builder")]
fn test_builder() -> WorkerAssignmentBuilder<StdRng> {
    WorkerAssignmentBuilder::new_with_rng("test-secret", StdRng::seed_from_u64(0))
}

/// A chunk builder with everything but the write-schema fields set.
#[cfg(feature = "builder")]
fn staged_chunk(
    builder: &mut WorkerAssignmentBuilder<StdRng>,
) -> WorkerAssignmentChunkBuilder<'_, StdRng> {
    builder
        .new_chunk()
        .id("0221000000/0221000000-0221000649-BQJdx")
        .dataset_id("s3://solana-mainnet-2")
        .dataset_base_url("https://solana-mainnet-2.sqd-datasets.io")
        .block_range(221000000..=221000649)
        .size(1000000)
        .worker_indexes(&[0])
}

#[cfg(all(feature = "builder", feature = "reader"))]
#[test]
fn test_worker_assignment_round_trip() {
    let mut builder = test_builder().check_continuity(false);
    builder.register_write_schema(7, &["blocks", "logs", "transactions"]).unwrap();
    // Registered after 7 but sorts before it: rosters must reach the blob id-sorted, which is
    // what `lookup_by_key` binary-searches on.
    builder.register_write_schema(3, &["blocks", "traces"]).unwrap();

    builder
        .new_chunk()
        .id("0221000000/0221000000-0221000649-BQJdx")
        .dataset_id("s3://solana-mainnet-2")
        .dataset_base_url("https://solana-mainnet-2.sqd-datasets.io")
        .block_range(221000000..=221000649)
        .size(1000000)
        .write_schema_id(7)
        .tables_present(&["blocks", "transactions"])
        .unwrap()
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
        .write_schema_id(7)
        .worker_indexes(&[0])
        .finish()
        .unwrap();
    // Same tables as the first chunk: the two must share one bitmap without confusing each other.
    builder
        .new_chunk()
        .id("0221000000/0221001550-0221001999-C7pQz")
        .dataset_id("s3://solana-mainnet-2")
        .dataset_base_url("https://solana-mainnet-2.sqd-datasets.io")
        .block_range(221001550..=221001999)
        .size(1000000)
        .write_schema_id(7)
        .tables_present(&["blocks", "transactions"])
        .unwrap()
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
    assert_eq!(dataset.last_block(), 221001999);
    assert_eq!(dataset.chunks().len(), 3);

    let worker = assignment.get_worker(&peer_id).unwrap();
    assert_eq!(worker.status(), sqd_assignments::WorkerStatus::Ok);
    let headers = worker.decrypt_headers(&keypair).unwrap();
    assert_eq!(headers.get("worker-id"), Some(&peer_id.to_string()));

    let chunks = worker.iter_chunks().collect::<Vec<_>>();
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].id(), "0221000000/0221000000-0221000649-BQJdx");
    assert_eq!(chunks[0].dataset_base_url(), "https://solana-mainnet-2.sqd-datasets.io");
    assert_eq!(chunks[0].write_schema_id(), 7);
    assert_eq!(
        assignment.chunk_tables(chunks[0]).unwrap().collect::<Vec<_>>(),
        vec!["blocks", "transactions"]
    );
    assert!(chunks[1].tables_present().is_none(), "unset tables_present means all present");
    assert_eq!(
        assignment.chunk_tables(chunks[1]).unwrap().collect::<Vec<_>>(),
        vec!["blocks", "logs", "transactions"],
        "an unset bitmap resolves to the write schema's whole roster"
    );
    assert_eq!(
        assignment.chunk_tables(chunks[2]).unwrap().collect::<Vec<_>>(),
        vec!["blocks", "transactions"],
        "a chunk sharing another's bitmap resolves to the same tables"
    );
    assert_eq!(
        assignment.get_write_schema(7).unwrap().tables().iter().collect::<Vec<_>>(),
        vec!["blocks", "logs", "transactions"]
    );
    assert_eq!(
        assignment.get_write_schema(3).unwrap().tables().iter().collect::<Vec<_>>(),
        vec!["blocks", "traces"],
        "every registered roster is looked up by id, whatever order it was registered in"
    );
    assert!(assignment.get_write_schema(8).is_none());

    assert_eq!(chunks[0].version(), 0);
    assert!(
        dataset.generations().is_none(),
        "a dataset of version-0 chunks stores no prefixes"
    );
    assert_eq!(
        assignment.chunk_url(chunks[0]).unwrap(),
        "https://solana-mainnet-2.sqd-datasets.io/0221000000/0221000000-0221000649-BQJdx"
    );
}

/// A batch job's rewrite of a chunk keeps the chunk id and swaps the prefix it hangs under.
#[cfg(all(feature = "builder", feature = "reader"))]
#[test]
fn test_chunk_generations_round_trip() {
    let mut builder = test_builder().check_continuity(false);
    builder.register_write_schema(7, &["blocks"]).unwrap();

    // An untouched dataset, staged first because `get_dataset` binary-searches on id.
    builder
        .new_chunk()
        .id("0000000000/0000000000-0000000999-274f02d8")
        .dataset_id("s3://ethereum-mainnet")
        .dataset_base_url("https://ethereum-mainnet.sqd-datasets.io")
        .block_range(0..=999)
        .size(1000)
        .write_schema_id(7)
        .worker_indexes(&[0])
        .finish()
        .unwrap();
    builder.finish_dataset();

    // Generations belong to the dataset being staged, so these apply to the one below only.
    builder.register_generation(4, "_bf/01HR2A9B4C6D8E0F2G4H6J8K0M").unwrap();
    // Registered after 4 but sorts before it: entries must reach the blob version-sorted, which is
    // what `lookup_by_key` binary-searches on.
    builder.register_generation(2, "_bf/01HQZK3M7X8P2NVWTC4RYFGDS9").unwrap();

    staged_chunk(&mut builder).write_schema_id(7).finish().unwrap();
    staged_chunk(&mut builder)
        .id("0221000000/0221000650-0221001549-AuRE1")
        .block_range(221000650..=221001549)
        .write_schema_id(7)
        .version(2)
        .finish()
        .unwrap();
    builder.finish_dataset();

    builder.add_worker_with_timestamp(
        common::get_test_keypair().public().to_peer_id(),
        sqd_assignments::WorkerStatus::Ok,
        1750000000,
    );
    let assignment = sqd_assignments::WorkerAssignment::from_owned(builder.finish()).unwrap();

    let dataset = assignment.get_dataset("s3://solana-mainnet-2").unwrap();
    assert_eq!(dataset.get_generation(2).unwrap().base_url(), "_bf/01HQZK3M7X8P2NVWTC4RYFGDS9");
    assert_eq!(
        dataset.get_generation(4).unwrap().base_url(),
        "_bf/01HR2A9B4C6D8E0F2G4H6J8K0M",
        "every registered generation is looked up by version, whatever order it was registered in"
    );
    assert!(dataset.get_generation(0).is_none(), "version 0 has no prefix");
    assert!(dataset.get_generation(3).is_none());

    // Version 0 keeps meaning the ingested copy in a dataset that does have generations: it is a
    // normal version whose defining property is having no entry.
    let chunks = dataset.chunks();
    assert_eq!(chunks.get(0).version(), 0);
    assert_eq!(
        assignment.chunk_url(chunks.get(0)).unwrap(),
        "https://solana-mainnet-2.sqd-datasets.io/0221000000/0221000000-0221000649-BQJdx",
        "version 0 hangs straight off the dataset base url"
    );
    assert_eq!(
        assignment.chunk_url(chunks.get(1)).unwrap(),
        "https://solana-mainnet-2.sqd-datasets.io/_bf/01HQZK3M7X8P2NVWTC4RYFGDS9\
         /0221000000/0221000650-0221001549-AuRE1",
        "a non-zero version puts its generation's prefix in between"
    );

    let other = assignment.get_dataset("s3://ethereum-mainnet").unwrap();
    assert!(
        other.generations().is_none(),
        "generations are staged per dataset, not carried over by the builder"
    );
}

#[cfg(feature = "builder")]
#[test]
fn test_register_generation_rejects_version_zero() {
    let error = test_builder()
        .register_generation(0, "_bf/01HQZK3M7X8P2NVWTC4RYFGDS9")
        .expect_err("giving version 0 a prefix would contradict what makes a chunk ingested");

    assert!(
        error.to_string().contains("needs no generation entry"),
        "unexpected error: {error}"
    );
}

#[cfg(feature = "builder")]
#[test]
fn test_register_generation_rejects_conflicting_base_url() {
    let mut builder = test_builder();
    builder.register_generation(2, "_bf/01HQZK3M7X8P2NVWTC4RYFGDS9").unwrap();

    let error = builder
        .register_generation(2, "_bf/01HR2A9B4C6D8E0F2G4H6J8K0M")
        .expect_err("one version cannot name two prefixes");

    assert!(error.to_string().contains("different base url"), "unexpected error: {error}");
}

/// Without a matching entry the worker has no prefix to download from, so the version is dangling.
#[cfg(feature = "builder")]
#[test]
fn test_finish_rejects_unregistered_generation() {
    let mut builder = test_builder();
    builder.register_write_schema(7, &["blocks"]).unwrap();

    let error = staged_chunk(&mut builder)
        .write_schema_id(7)
        .version(2)
        .finish()
        .expect_err("a chunk may not reference a generation with no entry in its dataset");

    assert!(
        error.to_string().contains("generation 2 is not registered"),
        "unexpected error: {error}"
    );
}

#[cfg(feature = "builder")]
#[test]
fn test_tables_present_rejects_table_outside_write_schema() {
    let mut builder = test_builder();
    builder.register_write_schema(7, &["blocks", "transactions"]).unwrap();

    let Err(error) = staged_chunk(&mut builder)
        .write_schema_id(7)
        .tables_present(&["blocks", "traces"])
    else {
        panic!("a table outside the write schema's roster must be rejected");
    };

    assert!(
        error.to_string().contains("'traces' is absent from write schema 7's roster"),
        "unexpected error: {error}"
    );
}

#[cfg(feature = "builder")]
#[test]
fn test_tables_present_requires_write_schema_id_first() {
    let mut builder = test_builder();
    builder.register_write_schema(7, &["blocks"]).unwrap();

    let Err(error) = staged_chunk(&mut builder).tables_present(&["blocks"]) else {
        panic!("tables_present encodes against the roster, so it needs the schema id");
    };

    assert!(
        error.to_string().contains("write_schema_id must be set"),
        "unexpected error: {error}"
    );
}

/// The roster defines the bit order, so unlike `tables_present` it is validated in release too.
#[cfg(feature = "builder")]
#[test]
fn test_unsorted_roster_is_rejected() {
    let mut builder = test_builder();

    let error = builder
        .register_write_schema(7, &["blocks", "transactions", "logs"])
        .expect_err("an unsorted roster must be rejected");
    assert!(error.to_string().contains("must be sorted"), "unexpected error: {error}");

    let error = builder
        .register_write_schema(7, &["blocks", "blocks"])
        .expect_err("a duplicate table would claim two bits for one table");
    assert!(error.to_string().contains("free of duplicates"), "unexpected error: {error}");
}

#[cfg(all(feature = "builder", debug_assertions))]
#[test]
#[should_panic(expected = "tables_present must be sorted")]
fn test_unsorted_tables_present_trips_debug_assertion() {
    let mut builder = test_builder();
    builder.register_write_schema(7, &["blocks", "logs", "transactions"]).unwrap();

    let _ = staged_chunk(&mut builder)
        .write_schema_id(7)
        .tables_present(&["transactions", "blocks"]);
}

/// Without the debug assertion the merge still rejects it: the roster cursor has already passed
/// the out-of-order name.
#[cfg(all(feature = "builder", not(debug_assertions)))]
#[test]
fn test_unsorted_tables_present_still_fails_without_debug_assertions() {
    let mut builder = test_builder();
    builder.register_write_schema(7, &["blocks", "logs", "transactions"]).unwrap();

    let Err(error) = staged_chunk(&mut builder)
        .write_schema_id(7)
        .tables_present(&["transactions", "blocks"])
    else {
        panic!("an unsorted tables_present must not yield a bitmap");
    };
    assert!(error.to_string().contains("'blocks' is absent"), "unexpected error: {error}");
}

/// A chunk holding every table never encodes a bitmap, so `finish` is the only place its schema
/// reference gets checked.
#[cfg(feature = "builder")]
#[test]
fn test_finish_rejects_unregistered_write_schema() {
    let mut builder = test_builder();

    let error = staged_chunk(&mut builder)
        .write_schema_id(7)
        .finish()
        .expect_err("a chunk may not reference a write schema with no roster in the blob");

    assert!(
        error.to_string().contains("write schema 7 is not registered"),
        "unexpected error: {error}"
    );
}

/// A bitmap only means anything against the roster it was encoded from, so a later
/// `write_schema_id` must not silently repoint it at another schema's tables.
#[cfg(feature = "builder")]
#[test]
fn test_finish_rejects_write_schema_changed_after_tables_present() {
    let mut builder = test_builder();
    builder.register_write_schema(7, &["blocks", "logs"]).unwrap();
    builder.register_write_schema(8, &["blocks", "traces"]).unwrap();

    let error = staged_chunk(&mut builder)
        .write_schema_id(7)
        .tables_present(&["logs"])
        .unwrap()
        .write_schema_id(8)
        .finish()
        .expect_err("the bitmap selects schema 7's 'logs', but schema 8 has 'traces' in that bit");

    assert!(
        error.to_string().contains("bitmap over write schema 7's roster"),
        "unexpected error: {error}"
    );
}

/// Taking the chunk by type, not just calling a method on it: a portal has to be able to pass one
/// around to decide what its version means for a query.
#[cfg(feature = "reader")]
fn version_of(chunk: sqd_assignments::fb::PortalAssignmentChunk<'_>) -> u32 {
    chunk.version()
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
        .version(2)
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
    assert_eq!(dataset.read_schema_id(), 7);
    assert_eq!(dataset.last_block_hash(), Some("BQJdx"));

    assert_eq!(assignment.get_worker_id(0).unwrap(), peer_id);
    let worker = assignment.get_worker_by_index(0);
    assert_eq!(worker.peer_id().unwrap(), peer_id);
    assert_eq!(worker.status(), sqd_assignments::WorkerStatus::Ok);

    let chunk1 = assignment.find_chunk("s3://solana-mainnet-2", 221000000).unwrap();
    assert_eq!(chunk1.id(), "0221000000/0221000000-0221000649-BQJdx");
    assert_eq!(chunk1.last_block_timestamp(), Some(1696192039));
    assert_eq!(chunk1.worker_indexes().iter().collect::<Vec<_>>(), vec![0]);
    assert_eq!(version_of(chunk1), 0, "an unset version means the ingested copy");

    let chunk2 = assignment.find_chunk("s3://solana-mainnet-2", 221000650).unwrap();
    assert_eq!(version_of(chunk2), 2, "the portal sees the version but not its storage prefix");

    assert_eq!(
        assignment.find_chunk("s3://dummy", 0),
        Err(sqd_assignments::ChunkNotFound::UnknownDataset)
    );
}
