//! Surveys the timestamps of a legacy assignment, to size up how they can be carried in the
//! portal assignment's columns. Legacy stores `last_block_timestamp` as absolute epoch
//! milliseconds, one `uint64` per chunk; the columnar portal format wants something narrower, and
//! this says what each candidate would cost.
//!
//! # Running it
//!
//! The input is an uncompressed legacy assignment, so gunzip first:
//!
//! ```text
//! gunzip -k mainnet.fb.1.gz
//! cargo run --release -p sqd-assignments --features reader --example timestamp_report -- \
//!     mainnet.fb.1 --limit 5 --out /tmp/wide_timestamps.txt
//! ```
//!
//! `--limit N` caps how many offending chunks of each kind reach stdout (default 20; 0 for the
//! summary alone). `--out FILE` writes every chunk whose absolute value overflows `uint32`, one
//! `s3://dataset/chunk_id` per line — on mainnet that is ~6.4M lines, so expect a few hundred MB.
//! Reading and verifying a 1.2 GB assignment takes a few seconds and about 1.5 GB of memory.
//!
//! # What it reports
//!
//! - **absolute**: the raw millisecond value exceeds `u32::MAX`. True of every timestamp past
//!   1970-02-19, so on real data this is ~100% and only confirms that milliseconds need 64 bits.
//! - **offset**: the value minus the dataset's base (its first chunk's timestamp, which is what a
//!   `base_timestamp` column would hold) exceeds `u32::MAX`, or falls below the base and would
//!   underflow. `u32` milliseconds span 49.7 days, so any dataset with a longer history overflows.
//! - **sub-second**: how many chunks have a non-zero millisecond remainder, i.e. how much would
//!   actually be lost by storing seconds instead. This is the number that decides whether
//!   millisecond precision is worth its width.
//! - **absolute seconds**: whether seconds would overflow `u32` (they do not until 2106).

use std::{
    fs::File,
    io::{BufWriter, Write},
};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    let mut limit = 20usize;
    let mut out = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--limit" => limit = args.next().expect("--limit needs a value").parse()?,
            "--out" => out = Some(args.next().expect("--out needs a path")),
            _ => path = Some(arg),
        }
    }
    let path = path.expect("usage: timestamp_report <assignment.fb> [--limit N] [--out FILE]");

    let buf = std::fs::read(&path)?;
    eprintln!("read {} ({} bytes), verifying...", path, buf.len());
    let assignment = sqd_assignments::Assignment::from_owned(buf)
        .map_err(|e| anyhow::anyhow!("not a valid legacy assignment: {e}"))?;

    let mut out = out
        .map(|path| Ok::<_, anyhow::Error>(BufWriter::new(File::create(path)?)))
        .transpose()?;

    let (mut chunks, mut timestamped) = (0usize, 0usize);
    let (mut absolute, mut offset, mut underflow) = (0usize, 0usize, 0usize);
    let (mut sub_second, mut seconds_overflow) = (0usize, 0usize);
    let mut sub_second_datasets: std::collections::BTreeMap<String, usize> = Default::default();
    let (mut shown_absolute, mut shown_offset, mut shown_underflow) = (0usize, 0usize, 0usize);
    let mut datasets_with_offset_overflow = 0usize;

    for dataset in assignment.datasets().iter() {
        let dataset_id = dataset.id();
        // The portal builder bases a dataset on its first timestamped chunk.
        let base = dataset.chunks().iter().find_map(|chunk| chunk.last_block_timestamp());
        let mut dataset_offended = false;

        for chunk in dataset.chunks().iter() {
            chunks += 1;
            let Some(timestamp) = chunk.last_block_timestamp() else {
                continue;
            };
            timestamped += 1;
            if timestamp % 1000 != 0 {
                sub_second += 1;
                *sub_second_datasets.entry(dataset_id.to_owned()).or_default() += 1;
            }
            if timestamp / 1000 > u32::MAX as u64 {
                seconds_overflow += 1;
            }
            let where_ = format!("{dataset_id}/{}", chunk.id());

            if timestamp > u32::MAX as u64 {
                absolute += 1;
                if shown_absolute < limit {
                    shown_absolute += 1;
                    println!("absolute  {where_}  ts={timestamp}");
                }
                if let Some(out) = out.as_mut() {
                    writeln!(out, "{where_}")?;
                }
            }

            let base = base.expect("a timestamped chunk means the dataset has a base");
            match timestamp.checked_sub(base) {
                None => {
                    underflow += 1;
                    dataset_offended = true;
                    if shown_underflow < limit {
                        shown_underflow += 1;
                        println!(
                            "underflow {where_}  ts={timestamp} base={base} (ts precedes base by {})",
                            base - timestamp
                        );
                    }
                }
                Some(delta) if delta > u32::MAX as u64 => {
                    offset += 1;
                    dataset_offended = true;
                    if shown_offset < limit {
                        shown_offset += 1;
                        println!("offset    {where_}  ts={timestamp} base={base} delta={delta}");
                    }
                }
                Some(_) => {}
            }
        }
        if dataset_offended {
            datasets_with_offset_overflow += 1;
        }
    }

    if let Some(out) = out.as_mut() {
        out.flush()?;
    }

    println!("\n--- summary ---");
    println!("chunks: {chunks} ({timestamped} carry a timestamp)");
    println!(
        "absolute value exceeds u32::MAX: {absolute} ({:.1}% of timestamped)",
        percent(absolute, timestamped)
    );
    println!(
        "offset from the dataset base exceeds u32::MAX: {offset} ({:.1}%)",
        percent(offset, timestamped)
    );
    println!(
        "carry sub-second precision (ts % 1000 != 0): {sub_second} ({:.3}%) across {} datasets",
        percent(sub_second, timestamped),
        sub_second_datasets.len()
    );
    println!("would overflow u32 if stored as absolute SECONDS: {seconds_overflow}");
    {
        let mut worst: Vec<_> = sub_second_datasets.iter().collect();
        worst.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        for (id, n) in worst.iter().take(8) {
            println!("    {id}: {n}");
        }
    }
    println!("offset would underflow (timestamp precedes the base): {underflow}");
    println!(
        "datasets with at least one unencodable offset: {datasets_with_offset_overflow} of {}",
        assignment.datasets().len()
    );
    Ok(())
}

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}
