//! Lists the chunks of a legacy assignment whose `last_block_timestamp` is a millisecond value,
//! i.e. one too large for a `uint32`. Prints the chunk's s3 path and its timestamp, one per line:
//!
//! ```text
//! s3://ethereum-mainnet/0000000000/0000000000-0000000999-274f02d8 1719290083000
//! ```
//!
//! # Running it
//!
//! The input is an uncompressed legacy assignment, so gunzip first:
//!
//! ```text
//! gunzip -k mainnet.fb.1.gz
//! cargo run --release -p sqd-assignments --features reader --example timestamp_report -- \
//!     mainnet.fb.1 > wide_timestamps.txt
//! ```
//!
//! Output goes to stdout and the count to stderr, so redirect or pipe through `head` — on mainnet
//! this matches 6,355,464 of 6,358,494 chunks. Reading and verifying a 1.2 GB assignment takes a
//! few seconds and about 1.5 GB of memory.

use std::io::{ErrorKind, Write};

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: timestamp_report <assignment.fb>");

    let buf = std::fs::read(&path)?;
    let assignment = sqd_assignments::Assignment::from_owned(buf)
        .map_err(|e| anyhow::anyhow!("not a valid legacy assignment: {e}"))?;

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let mut found = 0usize;
    for dataset in assignment.datasets().iter() {
        let dataset_id = dataset.id();
        for chunk in dataset.chunks().iter() {
            let Some(timestamp) = chunk.last_block_timestamp() else {
                continue;
            };
            if timestamp > u32::MAX as u64 {
                found += 1;
                // `head` closing the pipe is a normal way to end, not a failure.
                match writeln!(out, "{dataset_id}/{} {timestamp}", chunk.id()) {
                    Err(e) if e.kind() == ErrorKind::BrokenPipe => return Ok(()),
                    result => result?,
                }
            }
        }
    }
    out.flush()?;
    eprintln!("{found} chunks with a millisecond timestamp");
    Ok(())
}
