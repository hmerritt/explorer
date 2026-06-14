#[cfg(unix)]
pub fn normalize_mode(mode: u32) -> u32 {
    if mode == 0 {
        0o644
    } else {
        mode
    }
}
use std::{io::{self, Read, Write}, path::Path, time::Instant};

use crate::{ExtractOpts, ObserveEvent};

pub fn observed_copy(
    reader: &mut impl Read,
    writer: &mut impl Write,
    path: &Path,
    opts: &ExtractOpts,
) -> io::Result<u64> {
    opts.observer.observe(ObserveEvent::EntryStart {
        path,
        is_directory: false,
    });
    opts.observer.observe(ObserveEvent::FileCreate);
    let started = Instant::now();
    let bytes = io::copy(reader, writer)?;
    opts.observer.observe(ObserveEvent::OutputWrite {
        bytes,
        elapsed: started.elapsed(),
    });
    opts.observer.observe(ObserveEvent::EntryComplete {
        path,
        bytes,
        is_directory: false,
    });
    Ok(bytes)
}
