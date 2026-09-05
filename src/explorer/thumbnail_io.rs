//! The CPU budget and source I/O shared by every image-thumbnail request.
use std::{
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    sync::{Arc, OnceLock, atomic::{AtomicBool, AtomicU64, Ordering}},
};

pub(super) fn concurrency() -> usize {
    std::thread::available_parallelism().map(usize::from).unwrap_or(4).clamp(2, 4)
}

pub(super) fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| rayon::ThreadPoolBuilder::new()
        .num_threads(concurrency())
        .thread_name(|index| format!("image-thumbnail-{index}"))
        .build().expect("create thumbnail decode pool"))
}

/// Unlike BufReader::seek(Start), nearby metadata seeks retain read-ahead.
/// TIFF tag access otherwise discards the same buffer repeatedly.
pub(super) struct BufferedSource<R> {
    inner: BufReader<R>,
    position: u64,
}

impl<R: Read> BufferedSource<R> {
    pub(super) fn new(reader: R) -> Self {
        Self { inner: BufReader::new(reader), position: 0 }
    }
}

impl<R: Read> Read for BufferedSource<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(output)?;
        self.position += count as u64;
        Ok(count)
    }
}

impl<R: Read> BufRead for BufferedSource<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> { self.inner.fill_buf() }
    fn consume(&mut self, count: usize) {
        self.inner.consume(count);
        self.position += count as u64;
    }
}

impl<R: Read + Seek> Seek for BufferedSource<R> {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        let position = match target {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
            SeekFrom::End(_) => None,
        };
        if let Some(position) = position {
            if let Ok(offset) = i64::try_from(i128::from(position) - i128::from(self.position)) {
                self.inner.seek_relative(offset)?;
                self.position = position;
                return Ok(position);
            }
        }
        self.position = self.inner.seek(target)?;
        Ok(self.position)
    }
}

/// Cancellation is checked at actual source reads, including generic codecs.
/// Counters are absent outside diagnostics; no atomic increments in normal use.
pub(super) struct SourceReader<'a, R> {
    pub(super) inner: R,
    pub(super) cancel: &'a AtomicBool,
    pub(super) bytes_read: Option<Arc<AtomicU64>>,
}

impl<R: Read> Read for SourceReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            // Interrupted is retried by read_exact; use an error that stops it.
            return Err(io::Error::other("Image thumbnail loading was cancelled."));
        }
        let count = self.inner.read(output)?;
        if let Some(bytes_read) = &self.bytes_read {
            bytes_read.fetch_add(count as u64, Ordering::Relaxed);
        }
        Ok(count)
    }
}

impl<R: Seek> Seek for SourceReader<'_, R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}
