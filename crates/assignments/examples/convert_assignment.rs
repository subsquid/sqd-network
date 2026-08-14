//! Converts a legacy assignment into the split worker and portal assignments, then verifies the
//! pair reproduces everything the source said.
//!
//! # Running it
//!
//! The input is an uncompressed legacy assignment, so gunzip first. Outputs are written to the
//! working directory, named after the input's first path component:
//!
//! ```text
//! gunzip -k mainnet.fb.1.gz
//! cargo run --release -p sqd-assignments --all-features --example convert_assignment -- \
//!     mainnet.fb.1
//! # writes mainnet.{worker,portal}.fb alongside .fb.gz and .fb.zst
//!
//! # pick which compressed copies to write: gzip, zstd, both (default) or none
//! cargo run --release -p sqd-assignments --all-features --example convert_assignment -- \
//!     mainnet.fb.1 --compress zstd
//!
//! # re-check outputs produced earlier, without rebuilding them
//! cargo run --release -p sqd-assignments --all-features --example convert_assignment -- \
//!     mainnet.fb.1 --verify-only
//! ```
//!
//! gzip runs at its default level. zstd defaults to 9, which on assignment blobs is both smaller
//! and faster than gzip — its own default of 3 is faster still but comes out slightly larger, and
//! 19 buys about 10% more at twenty times the cost. `--zstd-level` takes any of them.
//!
//! Mainnet needs roughly 4 GB of memory: the source and both outputs are held at once.
//!
//! # What the conversion does
//!
//! Most fields move across unchanged. The parts that don't:
//!
//! - **Schemas.** Legacy carries none, so they are derived from each chunk's file list: a table is
//!   a `*.parquet` name with the extension stripped, anything else ignored. A dataset's roster is
//!   the union of its chunks' tables, its `write_schema_id` is its 1-based ordinal, and a chunk
//!   missing tables gets a `tables_present` bitmap. `read_schema_id` takes the same ordinal, in
//!   its own id space.
//! - **Chunk ids.** The worker keeps them whole. The portal splits each into the `tops`,
//!   `first_blocks`, `block_deltas` and `hashes` columns and rebuilds it on read.
//! - **Sealed headers.** Copied byte for byte; the Cloudflare secret needed to mint fresh ones
//!   doesn't travel with an assignment, and the signature keeps its original timestamp.
//! - **Timestamps.** Copied as absolute milliseconds. Anomalies are reported, never repaired: a
//!   chunk whose timestamp was never recorded carries 0, and a few step backwards, both of which
//!   the legacy format has and its own reader comments on.
//!
//! # What the verification proves
//!
//! Every chunk's id, block range, timestamp, version, worker indexes and tables; every dataset's
//! id, order, head block and hash; every worker's identity, status and sealed header bytes. It
//! aborts on the first mismatch, since a conversion this size gets checked by nothing else.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::Context as _;
use sqd_assignments::{
    Assignment, PortalAssignment, PortalAssignmentBuilder, WorkerAssignment,
    WorkerAssignmentBuilder,
};

/// Which compressed copies to write beside the plain `.fb`.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Compress {
    gzip: bool,
    zstd: bool,
    /// A blob is compressed once and downloaded by every worker and portal, so this leans towards
    /// size: 9 lands under gzip on both axes, where zstd's own default of 3 does not.
    zstd_level: i32,
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut input = None;
    let mut verify_only = false;
    let mut compress = Compress {
        gzip: true,
        zstd: true,
        zstd_level: 9,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--verify-only" => verify_only = true,
            "--zstd-level" => {
                compress.zstd_level = args.next().context("--zstd-level needs a value")?.parse()?;
            }
            "--compress" => {
                let which = args.next().context("--compress needs gzip, zstd, both or none")?;
                let (gzip, zstd) = match which.as_str() {
                    "gzip" => (true, false),
                    "zstd" => (false, true),
                    "both" => (true, true),
                    "none" => (false, false),
                    other => {
                        anyhow::bail!("--compress takes gzip, zstd, both or none, got '{other}'")
                    }
                };
                compress = Compress {
                    gzip,
                    zstd,
                    ..compress
                };
            }
            _ => input = Some(PathBuf::from(arg)),
        }
    }
    let input = input.context("usage: convert_assignment <assignment.fb> [--verify-only]")?;

    // "mainnet.fb.1" -> "mainnet", so the outputs sit beside their source by name.
    let stem = input
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.split('.').next())
        .context("input has no usable file name")?
        .to_owned();
    let worker_path = PathBuf::from(format!("{stem}.worker.fb"));
    let portal_path = PathBuf::from(format!("{stem}.portal.fb"));

    let legacy = read_legacy(&input)?;

    if !verify_only {
        let started = Instant::now();
        let worker = build_worker(&legacy)?;
        let portal = build_portal(&legacy)?;
        eprintln!("built both assignments in {:.1}s", started.elapsed().as_secs_f64());
        write_out(&worker_path, &worker, compress)?;
        write_out(&portal_path, &portal, compress)?;
        report_sizes(&input, &worker_path, &portal_path, compress)?;
    }

    let worker = WorkerAssignment::from_owned(std::fs::read(&worker_path)?)
        .map_err(|e| anyhow::anyhow!("{} does not verify: {e}", worker_path.display()))?;
    let portal = PortalAssignment::from_owned(std::fs::read(&portal_path)?)
        .map_err(|e| anyhow::anyhow!("{} does not verify: {e}", portal_path.display()))?;
    verify(&legacy, &worker, &portal)?;
    eprintln!("verified: both assignments reproduce the source");
    Ok(())
}

fn read_legacy(path: &Path) -> anyhow::Result<Assignment> {
    let buf = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    eprintln!("read {} ({} bytes)", path.display(), buf.len());
    Assignment::from_owned(buf).map_err(|e| anyhow::anyhow!("not a valid legacy assignment: {e}"))
}

/// A table is a `*.parquet` file with the extension stripped; every other name is ignored.
fn tables_of(chunk: &sqd_assignments::fb::Chunk<'_>) -> Vec<String> {
    let mut tables: Vec<String> = chunk
        .files()
        .iter()
        .filter_map(|file| file.filename().strip_suffix(".parquet"))
        .map(str::to_owned)
        .collect();
    tables.sort();
    tables.dedup();
    tables
}

/// Each dataset's roster is the union of its chunks' tables, keyed by its 1-based ordinal.
fn rosters(legacy: &Assignment) -> Vec<Vec<String>> {
    legacy
        .datasets()
        .iter()
        .map(|dataset| {
            let mut union = BTreeSet::new();
            for chunk in dataset.chunks().iter() {
                union.extend(tables_of(&chunk));
            }
            union.into_iter().collect()
        })
        .collect()
}

fn build_worker(legacy: &Assignment) -> anyhow::Result<Vec<u8>> {
    let rosters = rosters(legacy);
    // Gaps are legal in the source, and the new format carries each chunk's own end, so a gap is
    // no longer something a reader can misread.
    let mut builder = WorkerAssignmentBuilder::new("").check_continuity(false);
    for (index, roster) in rosters.iter().enumerate() {
        builder.register_write_schema(schema_id(index), roster)?;
    }

    for (index, dataset) in legacy.datasets().iter().enumerate() {
        let write_schema_id = schema_id(index);
        let roster = &rosters[index];
        for chunk in dataset.chunks().iter() {
            let tables = tables_of(&chunk);
            let mut staged = builder
                .new_chunk()
                .id(chunk.id())
                .dataset_id(chunk.dataset_id())
                .dataset_base_url(chunk.dataset_base_url())
                .block_range(chunk.first_block()..=last_block_of(&chunk)?)
                .size(chunk.size())
                .write_schema_id(write_schema_id)
                .worker_indexes(&chunk.worker_indexes().iter().collect::<Vec<_>>());
            // A chunk holding the whole roster leaves the bitmap off entirely.
            if tables.len() != roster.len() {
                staged = staged.tables_present(&tables)?;
            }
            staged.finish()?;
        }
        builder.finish_dataset();
    }

    for index in 0..legacy.workers().len() {
        let entry = legacy.workers().get(index);
        let headers = entry.encrypted_headers().context("legacy worker without headers")?;
        let worker = legacy.get_worker_by_index(index as u16);
        builder.add_worker_with_sealed_headers(
            worker.peer_id()?,
            worker.status(),
            headers.identity().bytes(),
            headers.nonce().bytes(),
            headers.ciphertext().bytes(),
        );
    }
    Ok(builder.finish())
}

fn build_portal(legacy: &Assignment) -> anyhow::Result<Vec<u8>> {
    let mut builder = PortalAssignmentBuilder::new().check_continuity(false);
    let (mut zeros, mut descents) = (0usize, 0usize);
    let mut anomalous: BTreeMap<String, (usize, usize)> = BTreeMap::new();

    for (index, dataset) in legacy.datasets().iter().enumerate() {
        let mut previous = 0u64;
        let (mut dataset_zeros, mut dataset_descents) = (0usize, 0usize);
        for chunk in dataset.chunks().iter() {
            let mut staged = builder
                .new_chunk()
                .id(chunk.id())
                .dataset_id(chunk.dataset_id())
                .block_range(chunk.first_block()..=last_block_of(&chunk)?)
                .worker_indexes(&chunk.worker_indexes().iter().collect::<Vec<_>>());
            if let Some(timestamp) = chunk.last_block_timestamp() {
                if timestamp == 0 {
                    dataset_zeros += 1;
                }
                if timestamp < previous {
                    dataset_descents += 1;
                }
                previous = timestamp;
                staged = staged.last_block_timestamp(timestamp);
            }
            staged.finish()?;
        }
        if dataset_zeros > 0 || dataset_descents > 0 {
            zeros += dataset_zeros;
            descents += dataset_descents;
            anomalous.insert(dataset.id().to_owned(), (dataset_zeros, dataset_descents));
        }
        builder.finish_dataset(schema_id(index), dataset.last_block_hash())?;
    }

    for index in 0..legacy.workers().len() {
        let worker = legacy.get_worker_by_index(index as u16);
        builder.add_worker(worker.peer_id()?, worker.status());
    }

    if !anomalous.is_empty() {
        eprintln!(
            "timestamps copied as given: {zeros} unrecorded (0) and {descents} stepping backwards, \
             across {} datasets — a lookup near one of those lands on a neighbouring chunk:",
            anomalous.len()
        );
        for (dataset, (zeros, descents)) in &anomalous {
            eprintln!("    {dataset}: {zeros} zero, {descents} descending");
        }
    }
    Ok(builder.finish())
}

fn verify(
    legacy: &Assignment,
    worker: &WorkerAssignment,
    portal: &PortalAssignment,
) -> anyhow::Result<()> {
    let legacy_datasets = legacy.datasets();
    anyhow::ensure!(
        worker.datasets().len() == legacy_datasets.len()
            && portal.datasets().len() == legacy_datasets.len(),
        "dataset count differs"
    );

    for (index, source) in legacy_datasets.iter().enumerate() {
        let id = source.id();
        let w = worker.datasets().get(index);
        let p = portal.datasets().get(index);
        anyhow::ensure!(w.id() == id && p.id() == id, "dataset {index} is not {id}");
        anyhow::ensure!(
            w.last_block() == source.last_block() && p.last_block() == source.last_block(),
            "{id}: last_block differs"
        );
        anyhow::ensure!(p.last_block_hash() == source.last_block_hash(), "{id}: head hash differs");

        let chunks = source.chunks();
        anyhow::ensure!(w.chunks().len() == chunks.len(), "{id}: worker chunk count differs");
        anyhow::ensure!(p.chunk_count() == chunks.len(), "{id}: portal chunk count differs");
        anyhow::ensure!(w.base_url() == chunks.get(0).dataset_base_url(), "{id}: base url differs");

        for (i, source_chunk) in chunks.iter().enumerate() {
            let chunk_id = source_chunk.id();
            let last_block = last_block_of(&source_chunk)?;

            let wc = w.chunks().get(i);
            anyhow::ensure!(wc.id() == chunk_id, "{id}: worker chunk {i} id differs");
            anyhow::ensure!(
                wc.first_block() == source_chunk.first_block(),
                "{chunk_id}: worker first_block differs"
            );
            anyhow::ensure!(wc.size() == source_chunk.size(), "{chunk_id}: size differs");
            anyhow::ensure!(
                w.chunk_url(wc).is_none_or(|url| url.ends_with(chunk_id)),
                "{chunk_id}: download url does not end in the chunk id"
            );
            let resolved: Vec<&str> =
                worker.chunk_tables(wc).context("chunk names no roster")?.collect();
            anyhow::ensure!(
                resolved == tables_of(&source_chunk),
                "{chunk_id}: tables resolve to {resolved:?}"
            );

            let pc = p.chunk(i as u32).context("portal chunk missing")?;
            anyhow::ensure!(
                pc.id().as_deref() == Some(chunk_id),
                "{chunk_id}: portal id rebuilds as {:?}",
                pc.id()
            );
            anyhow::ensure!(
                pc.first_block() == source_chunk.first_block() && pc.last_block() == last_block,
                "{chunk_id}: portal block range differs"
            );
            anyhow::ensure!(
                pc.last_block_timestamp() == source_chunk.last_block_timestamp(),
                "{chunk_id}: timestamp differs"
            );
            anyhow::ensure!(pc.version() == 0 && wc.version() == 0, "{chunk_id}: version is not 0");
            let source_workers: Vec<u16> = source_chunk.worker_indexes().iter().collect();
            anyhow::ensure!(
                pc.worker_indexes().collect::<Vec<_>>() == source_workers
                    && wc.worker_indexes().iter().collect::<Vec<_>>() == source_workers,
                "{chunk_id}: worker indexes differ"
            );
        }
    }

    let legacy_workers = legacy.workers();
    anyhow::ensure!(
        worker.workers().len() == legacy_workers.len()
            && portal.workers().len() == legacy_workers.len(),
        "worker count differs"
    );
    for index in 0..legacy_workers.len() {
        let source = legacy.get_worker_by_index(index as u16);
        let peer_id = source.peer_id()?;
        let w = worker.get_worker_by_index(index as u16);
        let p = portal.get_worker_by_index(index as u16);
        anyhow::ensure!(
            w.peer_id()? == peer_id && p.peer_id()? == peer_id,
            "worker {index} identity differs"
        );
        let status = source.status();
        anyhow::ensure!(
            w.status() == status && p.status() == status,
            "worker {index} status differs"
        );
        let source_headers = legacy_workers
            .get(index)
            .encrypted_headers()
            .context("legacy worker without headers")?;
        let copied = worker.workers().get(index).encrypted_headers().context("headers lost")?;
        anyhow::ensure!(
            copied.identity().bytes() == source_headers.identity().bytes()
                && copied.nonce().bytes() == source_headers.nonce().bytes()
                && copied.ciphertext().bytes() == source_headers.ciphertext().bytes(),
            "worker {index} sealed headers differ"
        );
    }
    Ok(())
}

/// Legacy chunks carry no end block; the id does, and it agrees with `first_block` throughout.
fn last_block_of(chunk: &sqd_assignments::fb::Chunk<'_>) -> anyhow::Result<u64> {
    let id = chunk.id();
    let (_, rest) = id.split_once('/').with_context(|| format!("chunk id '{id}' has no top"))?;
    let mut parts = rest.splitn(3, '-');
    let (Some(first), Some(last), Some(_)) = (parts.next(), parts.next(), parts.next()) else {
        anyhow::bail!("chunk id '{id}' is not <top>/<first>-<last>-<hash>");
    };
    let first: u64 = first.parse().with_context(|| format!("chunk id '{id}'"))?;
    anyhow::ensure!(
        first == chunk.first_block(),
        "chunk id '{id}' disagrees with first_block {}",
        chunk.first_block()
    );
    last.parse().with_context(|| format!("chunk id '{id}'"))
}

/// Ordinals are 1-based, leaving 0 free to mean "unset".
fn schema_id(dataset_index: usize) -> u32 {
    dataset_index as u32 + 1
}

fn write_out(path: &Path, bytes: &[u8], compress: Compress) -> anyhow::Result<()> {
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))?;
    if compress.gzip {
        let started = Instant::now();
        let file = std::fs::File::create(path.with_extension("fb.gz"))?;
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        encoder.write_all(bytes)?;
        encoder.finish()?;
        eprintln!("  gzip {} in {:.1}s", path.display(), started.elapsed().as_secs_f64());
    }
    if compress.zstd {
        let started = Instant::now();
        let file = std::fs::File::create(path.with_extension("fb.zst"))?;
        let mut encoder =
            zstd::stream::write::Encoder::new(file, compress.zstd_level)?.auto_finish();
        encoder.write_all(bytes)?;
        drop(encoder);
        eprintln!(
            "  zstd -{} {} in {:.1}s",
            compress.zstd_level,
            path.display(),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

fn report_sizes(
    input: &Path,
    worker: &Path,
    portal: &Path,
    compress: Compress,
) -> anyhow::Result<()> {
    let size = |path: &Path| -> anyhow::Result<u64> { Ok(std::fs::metadata(path)?.len()) };
    let optional = |path: &Path, wanted: bool| -> anyhow::Result<String> {
        Ok(if wanted {
            size(path)?.to_string()
        } else {
            "-".to_owned()
        })
    };
    let legacy = size(input)?;
    eprintln!("{:<10} {:>14} {:>14} {:>14}", "", "plain", "gzip", "zstd");
    eprintln!("{:<10} {:>14} {:>14} {:>14}", "legacy", legacy, "-", "-");
    for (label, path) in [("worker", worker), ("portal", portal)] {
        let plain = size(path)?;
        eprintln!(
            "{:<10} {:>14} {:>14} {:>14}  ({:.1}% of legacy)",
            label,
            plain,
            optional(&path.with_extension("fb.gz"), compress.gzip)?,
            optional(&path.with_extension("fb.zst"), compress.zstd)?,
            plain as f64 * 100.0 / legacy as f64
        );
    }
    Ok(())
}
