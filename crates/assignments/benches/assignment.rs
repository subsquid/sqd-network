//! What the split formats cost against the legacy `Assignment`, benchmarked along the access
//! patterns the portal and the worker actually use.
//!
//! ```text
//! cargo bench -p sqd-assignments --bench assignment
//! cargo bench -p sqd-assignments --bench assignment -- portal/stream_walk   # one group
//! ```
//!
//! # The two traversals
//!
//! Benchmarking either as a plain scan would flatter it:
//!
//! - **Portal — logically sequential, physically random.** A stream walks chunks in order but
//!   keeps no cursor: each step is `find_chunk(dataset, previous.last_block + 1)`, a fresh
//!   string-keyed dataset search plus a chunk search. It cannot hold a `ChunkRef`, since those are
//!   raw indices into a buffer swapped under it.
//! - **Worker — one sequential pass, then random O(1).** It scans its own chunks once per applied
//!   assignment, and every later access is a `ChunkRef` dereference with no search.
//!
//! Both formats are measured doing the same *job*, not the same calls: a stream step needs the
//! chunk's end, which legacy parses out of the id and the portal reads from `block_deltas`.
//!
//! # Fixture
//!
//! Synthetic by default, shaped like the mainnet assignment it stands in for: 200 datasets, ~2M
//! chunks, 7 replicas each, 2000 workers. Large enough that random access misses cache, which is
//! most of what these numbers are about.
//!
//! For a real one, point `SQD_BENCH_LEGACY` at it with the converted pair beside it — same stem
//! with `.worker.fb` and `.portal.fb`, which is what `convert_assignment` writes:
//!
//! ```text
//! cd /tmp && cargo run --release --manifest-path <repo>/Cargo.toml -p sqd-assignments \
//!     --all-features --example convert_assignment -- mainnet.fb.1.gz
//! SQD_BENCH_LEGACY=/tmp/mainnet.fb.1.gz cargo bench -p sqd-assignments --bench assignment
//! ```
//!
//! The legacy input may be plain, gzipped or zstd-compressed. Mainnet needs about 4 GB of memory:
//! all three blobs are held at once, and `load_verified` copies the largest per iteration.

use std::{io::Read, path::Path, sync::LazyLock};

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use libp2p_identity::{Keypair, PeerId};
use rand::{rngs::StdRng, Rng, SeedableRng};
use sqd_assignments::{
    Assignment, PortalAssignment, PortalAssignmentBuilder, WorkerAssignment,
    WorkerAssignmentBuilder, WorkerStatus,
};

const DATASETS: usize = 200;
const CHUNKS_PER_DATASET: usize = 10_000;
const BLOCKS_PER_CHUNK: u64 = 1_000;
/// Chunks per top-level directory, as subsquid/data writes them.
const CHUNKS_PER_TOP: usize = 1_000;
/// Mainnet averages 7.14.
const REPLICAS: usize = 7;
const WORKERS: usize = 2_000;
const TABLES: &[&str] = &["blocks", "logs", "statediffs", "traces", "transactions"];
/// How many steps a `stream_walk` takes, i.e. chunks consumed by one query.
const WALK: usize = 64;
/// Point lookups per random-access iteration, to keep timing above criterion's noise floor.
const LOOKUPS: usize = 64;

struct Fixture {
    /// The raw blobs, kept so the load benchmarks can re-parse them.
    legacy_bytes: Vec<u8>,
    worker_bytes: Vec<u8>,
    legacy: Assignment,
    worker: WorkerAssignment,
    portal: PortalAssignment,
    /// The worker every `iter_chunks`/`get_chunk` benchmark runs as.
    peer_id: PeerId,
    /// Shuffled (dataset, block) pairs, so random access really is random.
    probes: Vec<(String, u64)>,
    /// Shuffled timestamps covering the same span.
    timestamps: Vec<(String, u64)>,
}

fn dataset_id(index: usize) -> String {
    format!("s3://chain-{index:04}-mainnet")
}

fn chunk_id(chunk: usize, first: u64, last: u64) -> String {
    let top = (chunk / CHUNKS_PER_TOP) as u64 * CHUNKS_PER_TOP as u64 * BLOCKS_PER_CHUNK;
    format!("{top:010}/{first:010}-{last:010}-{:08x}", first ^ 0x274f02d8)
}

/// Milliseconds, as ingest records them.
fn timestamp(dataset: usize, chunk: usize) -> u64 {
    1_700_000_000_000 + (dataset as u64 * 97 + chunk as u64) * 12_000
}

fn workers() -> Vec<(PeerId, Keypair)> {
    let mut rng = StdRng::seed_from_u64(7);
    let mut workers: Vec<(PeerId, Keypair)> = (0..WORKERS)
        .map(|_| {
            let mut secret = [0u8; 32];
            rng.fill(&mut secret);
            let keypair = Keypair::ed25519_from_bytes(secret).expect("valid ed25519 secret");
            (keypair.public().to_peer_id(), keypair)
        })
        .collect();
    // Every builder asserts ascending PeerId order, and chunk indexes address this order.
    workers.sort_by_key(|(peer_id, _)| *peer_id);
    workers
}

/// Which workers hold a chunk. Deterministic, spread across the fleet.
fn replicas(dataset: usize, chunk: usize) -> Vec<u16> {
    let base = (dataset * 31 + chunk * 17) % WORKERS;
    let mut indexes: Vec<u16> = (0..REPLICAS).map(|r| ((base + r * 61) % WORKERS) as u16).collect();
    indexes.sort_unstable();
    indexes.dedup();
    indexes
}

fn build() -> Fixture {
    let workers = workers();
    let files: Vec<String> = TABLES.iter().map(|t| format!("{t}.parquet")).collect();

    let mut legacy =
        sqd_assignments::AssignmentBuilder::new_with_rng("secret", StdRng::seed_from_u64(1));
    let mut worker = WorkerAssignmentBuilder::new_with_rng("secret", StdRng::seed_from_u64(1));
    let mut portal = PortalAssignmentBuilder::new();

    for dataset in 0..DATASETS {
        let id = dataset_id(dataset);
        let base_url = format!("https://chain-{dataset:04}.sqd-datasets.io");
        worker.register_write_schema(dataset as u32 + 1, TABLES).expect("sorted roster");

        for chunk in 0..CHUNKS_PER_DATASET {
            let first = chunk as u64 * BLOCKS_PER_CHUNK;
            let last = first + BLOCKS_PER_CHUNK - 1;
            let cid = chunk_id(chunk, first, last);
            let indexes = replicas(dataset, chunk);
            let ts = timestamp(dataset, chunk);
            let head = chunk + 1 == CHUNKS_PER_DATASET;

            let mut staged = legacy
                .new_chunk()
                .id(&cid)
                .dataset_id(&id)
                .dataset_base_url(&base_url)
                .block_range(first..=last)
                .size(1_000_000)
                .last_block_timestamp(ts)
                .files(&files)
                .worker_indexes(&indexes);
            // Newer ingest carries the full hash on the dataset's head chunk only.
            if head {
                staged = staged.last_block_hash(&format!("0x{:064x}", first));
            }
            staged.finish().expect("contiguous");

            worker
                .new_chunk()
                .id(&cid)
                .dataset_id(&id)
                .dataset_base_url(&base_url)
                .block_range(first..=last)
                .size(1_000_000)
                .write_schema_id(dataset as u32 + 1)
                .worker_indexes(&indexes)
                .finish()
                .expect("contiguous");

            portal
                .new_chunk()
                .id(&cid)
                .dataset_id(&id)
                .block_range(first..=last)
                .last_block_timestamp(ts)
                .worker_indexes(&indexes)
                .finish()
                .expect("contiguous");
        }
        legacy.finish_dataset();
        worker.finish_dataset().expect("chunks staged");
        portal
            .finish_dataset(dataset as u32 + 1, Some(&format!("0x{:064x}", 0)))
            .expect("timestamped throughout");
    }

    for (peer_id, _) in &workers {
        legacy.add_worker(*peer_id, WorkerStatus::Ok, &[]);
        worker.add_worker(*peer_id, WorkerStatus::Ok);
        portal.add_worker(*peer_id, WorkerStatus::Ok);
    }

    let mut rng = StdRng::seed_from_u64(11);
    let probes = (0..LOOKUPS.max(WALK) * 4)
        .map(|_| {
            let dataset = rng.gen_range(0..DATASETS);
            let block = rng.gen_range(0..(CHUNKS_PER_DATASET as u64 * BLOCKS_PER_CHUNK));
            (dataset_id(dataset), block)
        })
        .collect();
    let timestamps = (0..LOOKUPS * 4)
        .map(|_| {
            let dataset = rng.gen_range(0..DATASETS);
            let chunk = rng.gen_range(0..CHUNKS_PER_DATASET);
            (dataset_id(dataset), timestamp(dataset, chunk))
        })
        .collect();

    let legacy_bytes = legacy.finish();
    let worker_bytes = worker.finish();
    Fixture {
        legacy: Assignment::from_owned(legacy_bytes.clone()).expect("verifies"),
        worker: WorkerAssignment::from_owned(worker_bytes.clone()).expect("verifies"),
        portal: PortalAssignment::from_owned(portal.finish()).expect("verifies"),
        legacy_bytes,
        worker_bytes,
        // The middle of the fleet, so its chunk count is typical.
        peer_id: workers[WORKERS / 2].0,
        probes,
        timestamps,
    }
}

/// Reads an assignment, decompressing it if it arrives that way.
fn read_blob(path: &Path) -> Vec<u8> {
    let raw = std::fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    match raw.first_chunk::<4>() {
        Some([0x1f, 0x8b, ..]) => {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(&raw[..]).read_to_end(&mut out).expect("gzip");
            out
        }
        Some([0x28, 0xb5, 0x2f, 0xfd]) => zstd::stream::decode_all(&raw[..]).expect("zstd"),
        _ => raw,
    }
}

/// Loads a real assignment and the split pair the converter wrote beside it.
fn load_real(legacy_path: &Path) -> Fixture {
    let stem = legacy_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.split('.').next())
        .expect("legacy path has a usable file name");
    let dir = legacy_path.parent().unwrap_or(Path::new("."));
    let sibling = |suffix: &str| {
        let path = dir.join(format!("{stem}.{suffix}.fb"));
        assert!(
            path.exists(),
            "{} is missing — run the convert_assignment example next to {}",
            path.display(),
            legacy_path.display()
        );
        read_blob(&path)
    };

    let legacy_bytes = read_blob(legacy_path);
    let worker_bytes = sibling("worker");
    let portal_bytes = sibling("portal");
    let legacy = Assignment::from_owned(legacy_bytes.clone()).expect("legacy verifies");
    let worker = WorkerAssignment::from_owned(worker_bytes.clone()).expect("worker verifies");
    let portal = PortalAssignment::from_owned(portal_bytes).expect("portal verifies");

    // Probe real datasets at real chunk boundaries, so every lookup resolves to something and the
    // benchmark measures a search rather than an early return.
    let mut rng = StdRng::seed_from_u64(11);
    let datasets = legacy.datasets();
    let mut probes = Vec::new();
    let mut timestamps = Vec::new();
    while probes.len() < LOOKUPS.max(WALK) * 4 {
        let dataset = datasets.get(rng.gen_range(0..datasets.len()));
        let chunks = dataset.chunks();
        let chunk = chunks.get(rng.gen_range(0..chunks.len()));
        let id = dataset.id().to_owned();
        // Only keep probes both formats resolve, so neither is timed doing less work.
        if legacy.find_chunk(&id, chunk.first_block()).is_ok()
            && portal.find_chunk(&id, chunk.first_block()).is_ok()
        {
            probes.push((id.clone(), chunk.first_block()));
        }
        if let Some(ts) = chunk.last_block_timestamp() {
            if timestamps.len() < LOOKUPS * 4
                && legacy.find_chunk_by_timestamp(&id, ts).is_ok()
                && portal.find_chunk_by_timestamp(&id, ts).is_ok()
            {
                timestamps.push((id, ts));
            }
        }
    }

    // A worker in the middle of the fleet, so its chunk count is typical.
    let workers = legacy.workers();
    let peer_id = legacy
        .get_worker_by_index((workers.len() / 2) as u16)
        .peer_id()
        .expect("valid peer id");

    eprintln!(
        "benchmarking {} ({} datasets, {} workers, {} chunks)",
        legacy_path.display(),
        datasets.len(),
        workers.len(),
        datasets.iter().map(|d| d.chunks().len()).sum::<usize>()
    );
    Fixture {
        legacy_bytes,
        worker_bytes,
        legacy,
        worker,
        portal,
        peer_id,
        probes,
        timestamps,
    }
}

static FIXTURE: LazyLock<Fixture> = LazyLock::new(|| match std::env::var("SQD_BENCH_LEGACY") {
    Ok(path) => load_real(Path::new(&path)),
    Err(_) => build(),
});

/// Legacy chunks carry no end block, so a stream step parses it out of the id.
fn legacy_last_block(chunk: &sqd_assignments::fb::Chunk<'_>) -> u64 {
    let (_, rest) = chunk.id().split_once('/').expect("top");
    rest.split('-').nth(1).expect("last block").parse().expect("numeric")
}

fn portal_lookups(c: &mut Criterion) {
    let f = &*FIXTURE;

    let mut group = c.benchmark_group("portal/get_dataset");
    group.bench_function("legacy", |b| {
        b.iter(|| {
            for (id, _) in f.probes.iter().take(LOOKUPS) {
                black_box(f.legacy.get_dataset(id).expect("present").last_block());
            }
        })
    });
    group.bench_function("portal", |b| {
        b.iter(|| {
            for (id, _) in f.probes.iter().take(LOOKUPS) {
                black_box(f.portal.get_dataset(id).expect("present").last_block());
            }
        })
    });
    group.finish();

    let mut group = c.benchmark_group("portal/find_chunk");
    group.bench_function("legacy", |b| {
        b.iter(|| {
            for (id, block) in f.probes.iter().take(LOOKUPS) {
                black_box(f.legacy.find_chunk(id, *block).expect("covered").first_block());
            }
        })
    });
    group.bench_function("portal", |b| {
        b.iter(|| {
            for (id, block) in f.probes.iter().take(LOOKUPS) {
                black_box(f.portal.find_chunk(id, *block).expect("covered").first_block());
            }
        })
    });
    group.finish();

    // The real traversal: no cursor is kept, so every step repeats both searches.
    let mut group = c.benchmark_group("portal/stream_walk");
    group.bench_function("legacy", |b| {
        b.iter(|| {
            let (id, start) = &f.probes[0];
            let mut block = *start;
            for _ in 0..WALK {
                let chunk = f.legacy.find_chunk(id, block).expect("covered");
                block = legacy_last_block(&chunk) + 1;
                black_box(chunk.first_block());
            }
        })
    });
    group.bench_function("portal", |b| {
        b.iter(|| {
            let (id, start) = &f.probes[0];
            let mut block = *start;
            for _ in 0..WALK {
                let chunk = f.portal.find_chunk(id, block).expect("covered");
                block = chunk.last_block() + 1;
                black_box(chunk.first_block());
            }
        })
    });
    group.finish();

    let mut group = c.benchmark_group("portal/find_chunk_by_timestamp");
    group.bench_function("legacy", |b| {
        b.iter(|| {
            for (id, ts) in f.timestamps.iter().take(LOOKUPS) {
                black_box(
                    f.legacy.find_chunk_by_timestamp(id, *ts).expect("covered").first_block(),
                );
            }
        })
    });
    group.bench_function("portal", |b| {
        b.iter(|| {
            for (id, ts) in f.timestamps.iter().take(LOOKUPS) {
                black_box(
                    f.portal.find_chunk_by_timestamp(id, *ts).expect("covered").first_block(),
                );
            }
        })
    });
    group.finish();

    // A portal copies the id out of the buffer to hand to a worker; the columnar format rebuilds
    // it from four columns instead.
    let mut group = c.benchmark_group("portal/chunk_id");
    group.bench_function("legacy", |b| {
        b.iter(|| {
            for (id, block) in f.probes.iter().take(LOOKUPS) {
                black_box(f.legacy.find_chunk(id, *block).expect("covered").id().to_owned());
            }
        })
    });
    group.bench_function("portal", |b| {
        b.iter(|| {
            for (id, block) in f.probes.iter().take(LOOKUPS) {
                black_box(f.portal.find_chunk(id, *block).expect("covered").id().expect("utf8"));
            }
        })
    });
    group.finish();

    // Per routing decision: the chunk's holders, each resolved to identity and status.
    let mut group = c.benchmark_group("portal/routing");
    group.bench_function("legacy", |b| {
        b.iter(|| {
            for (id, block) in f.probes.iter().take(LOOKUPS) {
                let chunk = f.legacy.find_chunk(id, *block).expect("covered");
                for index in chunk.worker_indexes().iter() {
                    let worker = f.legacy.get_worker_by_index(index);
                    black_box((worker.status(), worker.peer_id().expect("valid")));
                }
            }
        })
    });
    group.bench_function("portal", |b| {
        b.iter(|| {
            for (id, block) in f.probes.iter().take(LOOKUPS) {
                let chunk = f.portal.find_chunk(id, *block).expect("covered");
                for index in chunk.worker_indexes() {
                    let worker = f.portal.get_worker_by_index(index);
                    black_box((worker.status(), worker.peer_id().expect("valid")));
                }
            }
        })
    });
    group.finish();

    // Once per applied artifact, for per-dataset metrics.
    let mut group = c.benchmark_group("portal/scan_datasets");
    group.bench_function("legacy", |b| {
        b.iter(|| {
            for dataset in f.legacy.datasets().iter() {
                black_box((dataset.last_block(), dataset.chunks().len()));
            }
        })
    });
    group.bench_function("portal", |b| {
        b.iter(|| {
            for dataset in f.portal.datasets().iter() {
                black_box((dataset.last_block(), dataset.chunk_count()));
            }
        })
    });
    group.finish();
}

fn worker_access(c: &mut Criterion) {
    let f = &*FIXTURE;
    let (legacy_bytes, worker_bytes) = (&f.legacy_bytes, &f.worker_bytes);

    // The worker verifies what it applies; verification walks every table in the buffer.
    let mut group = c.benchmark_group("worker/load_verified");
    group.sample_size(20);
    group.bench_function("legacy", |b| {
        b.iter_batched(
            || legacy_bytes.to_vec(),
            |buf| black_box(Assignment::from_owned(buf).expect("verifies")),
            BatchSize::PerIteration,
        )
    });
    group.bench_function("worker", |b| {
        b.iter_batched(
            || worker_bytes.to_vec(),
            |buf| black_box(WorkerAssignment::from_owned(buf).expect("verifies")),
            BatchSize::PerIteration,
        )
    });
    group.finish();

    // One sequential pass over this worker's own chunks, once per applied assignment.
    let mut group = c.benchmark_group("worker/iter_chunks_with_ref");
    group.sample_size(20);
    group.bench_function("legacy", |b| {
        let worker = f.legacy.get_worker(&f.peer_id).expect("assigned");
        b.iter(|| {
            let mut count = 0usize;
            for (chunk_ref, chunk) in worker.iter_chunks_with_ref() {
                black_box((chunk_ref, chunk.first_block()));
                count += 1;
            }
            black_box(count)
        })
    });
    group.bench_function("worker", |b| {
        let worker = f.worker.get_worker(&f.peer_id).expect("assigned");
        b.iter(|| {
            let mut count = 0usize;
            for (chunk_ref, chunk) in worker.iter_chunks_with_ref() {
                black_box((chunk_ref, chunk.first_block()));
                count += 1;
            }
            black_box(count)
        })
    });
    group.finish();

    let legacy_refs: Vec<_> = f
        .legacy
        .get_worker(&f.peer_id)
        .expect("assigned")
        .iter_chunks_with_ref()
        .map(|(r, _)| r)
        .collect();
    let worker_refs: Vec<_> = f
        .worker
        .get_worker(&f.peer_id)
        .expect("assigned")
        .iter_chunks_with_ref()
        .map(|(r, _)| r)
        .collect();

    // Every download: a ChunkRef dereference, no search.
    let mut group = c.benchmark_group("worker/get_chunk");
    group.bench_function("legacy", |b| {
        b.iter(|| {
            for chunk_ref in legacy_refs.iter().take(LOOKUPS) {
                black_box(f.legacy.get_chunk(*chunk_ref).expect("in range").first_block());
            }
        })
    });
    group.bench_function("worker", |b| {
        b.iter(|| {
            for chunk_ref in worker_refs.iter().take(LOOKUPS) {
                black_box(f.worker.get_chunk(*chunk_ref).expect("in range").first_block());
            }
        })
    });
    group.finish();

    // What a download actually needs: the prefix its files hang under.
    let mut group = c.benchmark_group("worker/download_url");
    group.bench_function("legacy", |b| {
        b.iter(|| {
            for chunk_ref in legacy_refs.iter().take(LOOKUPS) {
                let chunk = f.legacy.get_chunk(*chunk_ref).expect("in range");
                black_box(format!("{}/{}", chunk.dataset_base_url(), chunk.id()));
            }
        })
    });
    group.bench_function("worker", |b| {
        b.iter(|| {
            for chunk_ref in worker_refs.iter().take(LOOKUPS) {
                black_box(f.worker.chunk_url(*chunk_ref).expect("resolvable"));
            }
        })
    });
    group.finish();

    // Once per applied assignment, by the worker's own peer id.
    let mut group = c.benchmark_group("worker/get_worker");
    group.bench_function("legacy", |b| {
        b.iter(|| black_box(f.legacy.get_worker(&f.peer_id).expect("assigned").status()))
    });
    group.bench_function("worker", |b| {
        b.iter(|| black_box(f.worker.get_worker(&f.peer_id).expect("assigned").status()))
    });
    group.finish();
}

criterion_group!(benches, portal_lookups, worker_access);
criterion_main!(benches);
