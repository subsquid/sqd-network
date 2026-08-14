//! Lists the chunks of a legacy assignment whose `last_block_timestamp` is *not* a millisecond
//! value — one small enough to fit a `uint32`, and so most likely written in seconds by mistake.
//! Prints the chunk's s3 path and its timestamp, one per line:
//!
//! ```text
//! s3://ethereum-mainnet/0000000000/0000000000-0000000999-274f02d8 1719290083
//! ```
//!
//! Milliseconds have exceeded `u32::MAX` since 1970-02-19, while seconds stay under it until
//! 2106, so the width of the value is what tells the two apart. A `0` matches too: it fits, and
//! it is what a missing timestamp looks like.
//!
//! # Running it
//!
//! The input is an uncompressed legacy assignment, so gunzip first:
//!
//! ```text
//! gunzip -k mainnet.fb.1.gz
//! cargo run --release -p sqd-assignments --features reader --example timestamp_report -- \
//!     mainnet.fb.1
//! ```
//!
//! Output goes to stdout and the count to stderr, so redirect or pipe through `head`. Reading and
//! verifying a 1.2 GB assignment takes a few seconds and about 1.5 GB of memory.

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
            if timestamp <= u32::MAX as u64 {
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
    eprintln!("{found} chunks whose timestamp is not in milliseconds");
    Ok(())
}
