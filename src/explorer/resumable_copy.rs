use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use filetime::FileTime;
use rayon::prelude::*;
use xxhash_rust::xxh3::xxh3_64_with_seed;

use crate::explorer::filesystem::{
    FileOperationPhase, FileOperationProgress, finalize_copied_file, paths_are_on_same_volume,
    preserve_file_metadata,
};

pub(super) const RSYNC_MIN_BLOCK_SIZE: usize = 700;
const RSYNC_MAX_BLOCK_SIZE: usize = 128 * 1024;
const RSYNC_WRITE_SIZE: usize = 32 * 1024;
const RSYNC_CHUNK_SIZE: usize = 32 * 1024;
const RSYNC_MAX_MAP_SIZE: usize = 256 * 1024;
const RSYNC_IO_BUFFER_SIZE: usize = 32 * 1024;
const RSYNC_CHECKSUM_SEED: u64 = 0;
const RSYNC_CHAR_OFFSET: u32 = 0;
const TRADITIONAL_SIGNATURE_TABLE_SIZE: usize = 1 << 16;
const LITERAL_FLUSH_SIZE: usize = RSYNC_CHUNK_SIZE;
const PARALLEL_COPY_CHUNK_SIZE: usize = 4 * 1024 * 1024;
const PARALLEL_COPY_PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(25);
const PARALLEL_VERIFICATION_MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum CopyDurability {
    #[default]
    Safe,
    Fast,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum CopySyncMode {
    ExplorerReplace,
    #[default]
    RsyncUpdate,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct CopyOptions {
    pub(super) sync_mode: CopySyncMode,
    pub(super) durability: CopyDurability,
}

impl CopyOptions {
    pub(super) fn explorer_safe() -> Self {
        Self {
            sync_mode: CopySyncMode::ExplorerReplace,
            durability: CopyDurability::Safe,
        }
    }

    pub(super) fn rsync_update(durability: CopyDurability) -> Self {
        Self {
            sync_mode: CopySyncMode::RsyncUpdate,
            durability,
        }
    }

    pub(super) fn should_sync(self) -> bool {
        self.durability == CopyDurability::Safe
    }
}

#[derive(Clone, Debug)]
struct BlockSignature {
    offset: u64,
    len: usize,
    weak: u32,
    strong: u64,
    chain: isize,
}

#[derive(Clone, Debug, Default)]
struct SignatureTable {
    signatures: Vec<BlockSignature>,
    buckets: Vec<isize>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct DeltaCopyStats {
    reused_blocks: usize,
    literal_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VerificationStrategy {
    Sequential,
    Parallel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RollingChecksum {
    a: u32,
    b: u32,
    len: usize,
}

impl RollingChecksum {
    fn new(bytes: &[u8]) -> Self {
        let len = bytes.len();
        let mut a = 0u32;
        let mut b = 0u32;

        for (index, byte) in bytes.iter().enumerate() {
            let value = u32::from(*byte) + RSYNC_CHAR_OFFSET;
            a = (a + value) & 0xffff;
            b = (b + ((len - index) as u32 * value)) & 0xffff;
        }

        Self { a, b, len }
    }

    fn roll(&mut self, removed: u8, added: u8) {
        let removed = u32::from(removed) + RSYNC_CHAR_OFFSET;
        let added = u32::from(added) + RSYNC_CHAR_OFFSET;

        self.a = (self.a + 0x1_0000 - removed + added) & 0xffff;
        self.b = (self.b + 0x1_0000 - ((self.len as u32 * removed) & 0xffff) + self.a) & 0xffff;
    }

    fn value(self) -> u32 {
        (self.b << 16) | self.a
    }
}

impl SignatureTable {
    fn new(mut signatures: Vec<BlockSignature>) -> Self {
        if signatures.is_empty() {
            return Self::default();
        }

        let mut table_size = (signatures.len() / 8) * 10 + 11;
        table_size = table_size.max(TRADITIONAL_SIGNATURE_TABLE_SIZE);
        let mut buckets = vec![-1; table_size];

        for index in 0..signatures.len() {
            let bucket = signature_bucket_for_size(signatures[index].weak, table_size);
            signatures[index].chain = buckets[bucket];
            buckets[bucket] = index as isize;
        }

        Self {
            signatures,
            buckets,
        }
    }

    fn is_empty(&self) -> bool {
        self.signatures.is_empty()
    }

    fn has_candidate(&self, weak: u32, len: usize) -> bool {
        let mut index = self.first_candidate_index(weak);
        while index >= 0 {
            let signature = &self.signatures[index as usize];
            if signature.weak == weak && signature.len == len {
                return true;
            }
            index = signature.chain;
        }
        false
    }

    fn match_bytes(&self, bytes: &[u8], weak: u32) -> Option<BlockSignature> {
        let strong = strong_hash(bytes);
        let mut index = self.first_candidate_index(weak);
        while index >= 0 {
            let signature = &self.signatures[index as usize];
            if signature.weak == weak && signature.len == bytes.len() && signature.strong == strong
            {
                return Some(signature.clone());
            }
            index = signature.chain;
        }
        None
    }

    fn first_candidate_index(&self, weak: u32) -> isize {
        if self.buckets.is_empty() {
            return -1;
        }

        self.buckets[signature_bucket_for_size(weak, self.buckets.len())]
    }
}

fn signature_bucket_for_size(weak: u32, table_size: usize) -> usize {
    if table_size == TRADITIONAL_SIGNATURE_TABLE_SIZE {
        let s1 = weak & 0xffff;
        let s2 = weak >> 16;
        ((s1 + s2) & 0xffff) as usize
    } else {
        weak as usize % table_size
    }
}

fn rsync_block_size(file_len: u64) -> usize {
    let min = RSYNC_MIN_BLOCK_SIZE as u64;
    let max = RSYNC_MAX_BLOCK_SIZE as u64;
    if file_len <= min.saturating_mul(min) {
        return RSYNC_MIN_BLOCK_SIZE;
    }

    let mut c = 1u64;
    let mut len = file_len;
    loop {
        len >>= 2;
        if len == 0 {
            break;
        }
        c = match c.checked_shl(1) {
            Some(next) => next,
            None => return RSYNC_MAX_BLOCK_SIZE,
        };
        if c >= max {
            return RSYNC_MAX_BLOCK_SIZE;
        }
    }

    let mut block_size = 0u64;
    while c >= 8 {
        block_size |= c;
        if file_len < block_size.saturating_mul(block_size) {
            block_size &= !c;
        }
        c >>= 1;
    }

    block_size.clamp(min, max) as usize
}

struct BufferedByteReader {
    file: File,
    buffer: Vec<u8>,
    position: usize,
    filled: usize,
}

impl BufferedByteReader {
    fn new(file: File, block_size: usize) -> Self {
        let buffer_size = block_size.clamp(RSYNC_IO_BUFFER_SIZE, RSYNC_MAX_MAP_SIZE);
        Self {
            file,
            buffer: vec![0; buffer_size],
            position: 0,
            filled: 0,
        }
    }

    fn read_byte(&mut self) -> io::Result<Option<u8>> {
        if self.position == self.filled {
            self.filled = self.file.read(&mut self.buffer)?;
            self.position = 0;
            if self.filled == 0 {
                return Ok(None);
            }
        }

        let byte = self.buffer[self.position];
        self.position += 1;
        Ok(Some(byte))
    }
}

#[derive(Debug)]
struct RollingWindow {
    bytes: Vec<u8>,
    start: usize,
    len: usize,
}

impl RollingWindow {
    fn new(capacity: usize) -> Self {
        Self {
            bytes: vec![0; capacity],
            start: 0,
            len: 0,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn clear(&mut self) {
        self.start = 0;
        self.len = 0;
    }

    fn push_back(&mut self, byte: u8) {
        debug_assert!(self.len < self.bytes.len());
        let index = (self.start + self.len) % self.bytes.len();
        self.bytes[index] = byte;
        self.len += 1;
    }

    fn pop_front(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }

        let byte = self.bytes[self.start];
        self.start = (self.start + 1) % self.bytes.len();
        self.len -= 1;
        Some(byte)
    }

    fn to_vec(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(self.len);
        if self.len == 0 {
            return output;
        }

        let first_len = (self.bytes.len() - self.start).min(self.len);
        output.extend_from_slice(&self.bytes[self.start..self.start + first_len]);
        if first_len < self.len {
            output.extend_from_slice(&self.bytes[..self.len - first_len]);
        }
        output
    }
}

#[allow(dead_code)]
pub(super) fn copy_with_delta_progress(
    source: &Path,
    destination: &Path,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> io::Result<()> {
    copy_with_delta_progress_with_options(
        source,
        destination,
        cancel,
        progress,
        on_progress,
        CopyOptions::explorer_safe(),
    )
}

pub(super) fn copy_with_delta_progress_with_options(
    source: &Path,
    destination: &Path,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    options: CopyOptions,
) -> io::Result<()> {
    let source_len = fs::metadata(source)?.len();
    copy_with_delta_progress_impl(
        source,
        destination,
        rsync_block_size(source_len),
        cancel,
        progress,
        on_progress,
        options,
    )
}

pub(super) fn cleanup_resumable_copy_progress(_source: &Path, destination: &Path) {
    remove_partial_file(&partial_path_for(destination));
}

#[cfg(test)]
pub(super) fn copy_with_delta_progress_for_test(
    source: &Path,
    destination: &Path,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> io::Result<()> {
    copy_with_delta_progress_impl(
        source,
        destination,
        block_size,
        cancel,
        progress,
        on_progress,
        CopyOptions::explorer_safe(),
    )
}

fn copy_with_delta_progress_impl(
    source: &Path,
    destination: &Path,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    options: CopyOptions,
) -> io::Result<()> {
    if block_size == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "resumable copy block size must be non-zero",
        ));
    }

    let metadata = fs::metadata(source)?;
    let source_len = metadata.len();
    let partial = partial_path_for(destination);
    let resume_partial = partial_resume_state(&partial)?;
    let verified_before_existing_check = progress.verified_bytes;

    if destination.is_file() {
        let same_data = match options.sync_mode {
            CopySyncMode::RsyncUpdate => {
                destination_quick_matches_source(&metadata, destination).unwrap_or(false)
                    && destination_content_matches_source_with_progress(
                        source,
                        destination,
                        source_len,
                        block_size,
                        cancel,
                        progress,
                        on_progress,
                    )?
            }
            CopySyncMode::ExplorerReplace => destination_is_identical_to_source(
                source,
                destination,
                source_len,
                block_size,
                cancel,
                progress,
                on_progress,
            )?,
        };

        if same_data {
            preserve_file_metadata(&metadata, destination)?;
            if options.should_sync() {
                sync_file(destination);
                sync_parent_directory(destination);
            }
            remove_partial_file(&partial);
            return Ok(());
        }
    }
    let existing_check_verified = progress
        .verified_bytes
        .saturating_sub(verified_before_existing_check);
    if existing_check_verified > 0 {
        progress.reserve_work_bytes(existing_check_verified);
    }

    let basis = if resume_partial {
        None
    } else {
        destination.is_file().then(|| destination.to_path_buf())
    };

    let should_verify_after_write =
        options.durability == CopyDurability::Safe || basis.is_some() || resume_partial;
    if should_verify_after_write {
        progress.reserve_work_bytes(source_len);
    }

    let stats = if resume_partial {
        progress.phase = FileOperationPhase::Resuming;
        progress.current_item = Some(source.to_path_buf());
        on_progress(progress.clone());

        let valid_prefix = validate_partial_prefix(source, &partial, source_len, cancel)?;
        seed_resume_progress(valid_prefix, progress, on_progress);
        progress.phase = FileOperationPhase::Copying;
        on_progress(progress.clone());
        append_source_remainder_to_partial(
            source,
            &partial,
            source_len,
            valid_prefix,
            cancel,
            progress,
            on_progress,
            options,
        )?;
        DeltaCopyStats {
            reused_blocks: 0,
            literal_bytes: source_len.saturating_sub(valid_prefix),
        }
    } else {
        write_delta_scratch(
            source,
            basis.as_deref(),
            &partial,
            source_len,
            block_size,
            cancel,
            progress,
            on_progress,
            options,
        )?
    };

    if should_verify_after_write || stats.literal_bytes != source_len || stats.reused_blocks != 0 {
        let verify_result = verify_and_repair(
            source,
            &partial,
            source_len,
            block_size,
            cancel,
            progress,
            on_progress,
            options,
        );

        if let Err(error) = verify_result {
            return Err(error);
        }
    }

    preserve_file_metadata(&metadata, &partial)?;
    finalize_copied_file(
        source,
        &partial,
        destination,
        &metadata,
        cancel,
        progress,
        on_progress,
        options,
    )?;
    if options.should_sync() {
        sync_parent_directory(destination);
    }
    remove_partial_file(&partial);
    Ok(())
}

pub(super) fn destination_quick_matches_source(
    source_metadata: &fs::Metadata,
    destination: &Path,
) -> io::Result<bool> {
    let destination_metadata = fs::metadata(destination)?;
    Ok(source_metadata.len() == destination_metadata.len()
        && FileTime::from_last_modification_time(source_metadata)
            == FileTime::from_last_modification_time(&destination_metadata))
}

pub(super) fn destination_content_matches_source_with_progress(
    source: &Path,
    destination: &Path,
    source_len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> io::Result<bool> {
    if fs::metadata(destination)?.len() != source_len {
        return Ok(false);
    }

    progress.phase = FileOperationPhase::Verifying;
    progress.current_item = Some(source.to_path_buf());
    on_progress(progress.clone());

    files_equal_with_progress(
        source,
        destination,
        source_len,
        block_size,
        cancel,
        |bytes| {
            progress.add_verified_bytes(bytes);
            on_progress(progress.clone());
        },
    )
}

fn partial_path_for(destination: &Path) -> PathBuf {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let mut file_name = destination
        .file_name()
        .unwrap_or_else(|| OsStr::new("file"))
        .to_os_string();
    file_name.push(".partial");
    parent.join(file_name)
}

fn partial_resume_state(partial: &Path) -> io::Result<bool> {
    match fs::metadata(partial) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(io::Error::other(format!(
            "{} already exists and is not a file.",
            partial.display()
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn validate_partial_prefix(
    source: &Path,
    partial: &Path,
    source_len: u64,
    cancel: &Arc<AtomicBool>,
) -> io::Result<u64> {
    let mut source_file = File::open(source)?;
    let mut partial_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(partial)?;
    let partial_len = partial_file.metadata()?.len();
    let compare_len = partial_len.min(source_len);
    let mut source_buffer = vec![0; RSYNC_IO_BUFFER_SIZE];
    let mut partial_buffer = vec![0; RSYNC_IO_BUFFER_SIZE];
    let mut valid_prefix = 0u64;

    while valid_prefix < compare_len {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let read_len = (compare_len - valid_prefix).min(source_buffer.len() as u64) as usize;
        source_file.read_exact(&mut source_buffer[..read_len])?;
        partial_file.read_exact(&mut partial_buffer[..read_len])?;

        if source_buffer[..read_len] != partial_buffer[..read_len] {
            let matching = source_buffer[..read_len]
                .iter()
                .zip(&partial_buffer[..read_len])
                .take_while(|(source, partial)| source == partial)
                .count() as u64;
            valid_prefix = valid_prefix.saturating_add(matching);
            partial_file.set_len(valid_prefix)?;
            return Ok(valid_prefix);
        }

        valid_prefix = valid_prefix.saturating_add(read_len as u64);
    }

    if partial_len != valid_prefix {
        partial_file.set_len(valid_prefix)?;
    }
    Ok(valid_prefix)
}

fn seed_resume_progress(
    bytes: u64,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) {
    if bytes == 0 {
        return;
    }

    progress.add_copied_bytes(bytes);
    on_progress(progress.clone());
}

fn append_source_remainder_to_partial(
    source: &Path,
    partial: &Path,
    source_len: u64,
    start_offset: u64,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    options: CopyOptions,
) -> io::Result<()> {
    let mut source_file = File::open(source)?;
    source_file.seek(SeekFrom::Start(start_offset))?;
    let mut output = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(partial)?;
    output.set_len(start_offset)?;
    output.seek(SeekFrom::Start(start_offset))?;

    let mut offset = start_offset;
    let mut buffer = vec![0; RSYNC_IO_BUFFER_SIZE];
    while offset < source_len {
        if cancel.load(Ordering::Relaxed) {
            if options.should_sync() {
                let _ = output.sync_all();
            }
            return Err(cancelled_error());
        }

        let read_len = (source_len - offset).min(buffer.len() as u64) as usize;
        let read = source_file.read(&mut buffer[..read_len])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "source ended before resumable copy completed",
            ));
        }

        output.write_all(&buffer[..read])?;
        let written = read as u64;
        offset = offset.saturating_add(written);
        progress.add_copied_bytes(written);
        on_progress(progress.clone());
    }

    output.set_len(source_len)?;
    if options.should_sync() {
        output.sync_all()?;
    }
    Ok(())
}

fn write_delta_scratch(
    source: &Path,
    basis: Option<&Path>,
    scratch: &Path,
    source_len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    options: CopyOptions,
) -> io::Result<DeltaCopyStats> {
    if scratch.exists() {
        fs::remove_file(scratch)?;
    }

    let signatures = if let Some(basis) = basis {
        progress.phase = FileOperationPhase::Indexing;
        progress.current_item = Some(basis.to_path_buf());
        on_progress(progress.clone());
        build_basis_signatures(basis, block_size, cancel)?
    } else {
        SignatureTable::default()
    };

    progress.phase = FileOperationPhase::Copying;
    progress.current_item = Some(source.to_path_buf());
    on_progress(progress.clone());

    let mut output = File::create(scratch)?;
    let mut stats = DeltaCopyStats::default();
    let mut basis_file = if basis.is_some() {
        Some(File::open(basis.expect("basis path"))?)
    } else {
        None
    };

    let scan_result = scan_source_to_output(
        source,
        &signatures,
        basis_file.as_mut(),
        &mut output,
        block_size,
        cancel,
        progress,
        on_progress,
        &mut stats,
    );
    if let Err(error) = scan_result {
        if options.should_sync() {
            let _ = output.sync_all();
        }
        return Err(error);
    }

    output.set_len(source_len)?;
    if options.should_sync() {
        output.sync_all()?;
    }
    Ok(stats)
}

fn build_basis_signatures(
    basis: &Path,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
) -> io::Result<SignatureTable> {
    let file = Arc::new(File::open(basis)?);
    let len = file.metadata()?.len();
    let block_size_u64 = block_size as u64;
    let block_count = len.div_ceil(block_size_u64);
    let signatures = (0..block_count)
        .into_par_iter()
        .map(|block_index| {
            if cancel.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }

            let offset = block_index.saturating_mul(block_size_u64);
            let read_len = (len - offset).min(block_size_u64) as usize;
            let mut buffer = vec![0; read_len];
            read_exact_at(&file, offset, &mut buffer)?;
            let weak = RollingChecksum::new(&buffer).value();
            Ok(BlockSignature {
                offset,
                len: read_len,
                weak,
                strong: strong_hash(&buffer),
                chain: -1,
            })
        })
        .collect::<io::Result<Vec<_>>>()?;

    if cancel.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    Ok(SignatureTable::new(signatures))
}

fn scan_source_to_output(
    source: &Path,
    signatures: &SignatureTable,
    mut basis_file: Option<&mut File>,
    output: &mut File,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    stats: &mut DeltaCopyStats,
) -> io::Result<()> {
    if signatures.is_empty() {
        return copy_source_literals_buffered(source, output, cancel, progress, on_progress, stats);
    }

    let mut source = BufferedByteReader::new(File::open(source)?, block_size);
    let mut window = RollingWindow::new(block_size);
    let mut pending_literal = Vec::with_capacity(LITERAL_FLUSH_SIZE);
    fill_window(&mut source, &mut window, block_size)?;

    if window.len() == block_size {
        let mut weak = RollingChecksum::new(&window.to_vec());

        while window.len() == block_size {
            if cancel.load(Ordering::Relaxed) {
                flush_literal(output, &mut pending_literal, progress, on_progress, stats)?;
                return Err(cancelled_error());
            }

            if let Some(signature) = matching_signature(&window, signatures, weak.value()) {
                flush_literal(output, &mut pending_literal, progress, on_progress, stats)?;
                let basis_file = basis_file
                    .as_deref_mut()
                    .ok_or_else(|| io::Error::other("matched block without basis file"))?;
                copy_basis_block(basis_file, output, &signature, progress, on_progress)?;
                stats.reused_blocks += 1;
                window.clear();
                fill_window(&mut source, &mut window, block_size)?;
                if window.len() == block_size {
                    weak = RollingChecksum::new(&window.to_vec());
                }
            } else {
                let removed = window.pop_front().expect("full window");
                pending_literal.push(removed);
                if pending_literal.len() >= LITERAL_FLUSH_SIZE {
                    flush_literal(output, &mut pending_literal, progress, on_progress, stats)?;
                }

                if let Some(byte) = source.read_byte()? {
                    window.push_back(byte);
                    weak.roll(removed, byte);
                } else {
                    break;
                }
            }
        }
    }

    if !window.is_empty() {
        let tail = window.to_vec();
        let weak = RollingChecksum::new(&tail).value();
        if let Some(signature) = matching_signature_bytes(&tail, signatures, weak) {
            flush_literal(output, &mut pending_literal, progress, on_progress, stats)?;
            let basis_file = basis_file
                .as_deref_mut()
                .ok_or_else(|| io::Error::other("matched tail without basis file"))?;
            copy_basis_block(basis_file, output, &signature, progress, on_progress)?;
            stats.reused_blocks += 1;
        } else {
            pending_literal.extend_from_slice(&tail);
        }
    }

    flush_literal(output, &mut pending_literal, progress, on_progress, stats)
}

fn copy_source_literals_buffered(
    source: &Path,
    output: &mut File,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    stats: &mut DeltaCopyStats,
) -> io::Result<()> {
    let mut source = File::open(source)?;
    let mut buffer = vec![0; RSYNC_IO_BUFFER_SIZE];

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        output.write_all(&buffer[..read])?;
        let written = read as u64;
        stats.literal_bytes = stats.literal_bytes.saturating_add(written);
        progress.add_copied_bytes(written);
        on_progress(progress.clone());
    }

    Ok(())
}

fn fill_window(
    source: &mut BufferedByteReader,
    window: &mut RollingWindow,
    block_size: usize,
) -> io::Result<()> {
    while window.len() < block_size {
        match source.read_byte()? {
            Some(byte) => window.push_back(byte),
            None => break,
        }
    }
    Ok(())
}

fn matching_signature(
    window: &RollingWindow,
    signatures: &SignatureTable,
    weak: u32,
) -> Option<BlockSignature> {
    if !signatures.has_candidate(weak, window.len()) {
        return None;
    }
    let bytes = window.to_vec();
    matching_signature_bytes(&bytes, signatures, weak)
}

fn matching_signature_bytes(
    bytes: &[u8],
    signatures: &SignatureTable,
    weak: u32,
) -> Option<BlockSignature> {
    signatures.match_bytes(bytes, weak)
}

fn flush_literal(
    output: &mut File,
    pending_literal: &mut Vec<u8>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    stats: &mut DeltaCopyStats,
) -> io::Result<()> {
    if pending_literal.is_empty() {
        return Ok(());
    }

    output.write_all(pending_literal)?;
    let written = pending_literal.len() as u64;
    stats.literal_bytes = stats.literal_bytes.saturating_add(written);
    progress.add_copied_bytes(written);
    on_progress(progress.clone());
    pending_literal.clear();
    Ok(())
}

fn copy_basis_block(
    basis: &File,
    output: &mut File,
    signature: &BlockSignature,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> io::Result<()> {
    let mut remaining = signature.len;
    let mut buffer = vec![0; remaining.min(RSYNC_WRITE_SIZE)];
    let mut offset = signature.offset;
    while remaining > 0 {
        let read_len = remaining.min(buffer.len());
        read_exact_at(basis, offset, &mut buffer[..read_len])?;
        output.write_all(&buffer[..read_len])?;
        remaining -= read_len;
        offset = offset.saturating_add(read_len as u64);
    }

    progress.add_copied_bytes(signature.len as u64);
    on_progress(progress.clone());
    Ok(())
}

fn verify_and_repair(
    source: &Path,
    scratch: &Path,
    source_len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    options: CopyOptions,
) -> io::Result<()> {
    progress.phase = FileOperationPhase::Verifying;
    progress.current_item = Some(source.to_path_buf());
    on_progress(progress.clone());

    let repaired = verify_repair_pass(source, scratch, source_len, block_size, cancel, |bytes| {
        progress.add_verified_bytes(bytes);
        on_progress(progress.clone());
    })?;
    if options.should_sync() {
        sync_file(scratch);
    }
    let verified = verify_offsets(source, scratch, &repaired, block_size, cancel)?;
    if !verified {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "resumable copy verification failed",
        ));
    }

    Ok(())
}

fn destination_is_identical_to_source(
    source: &Path,
    destination: &Path,
    source_len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> io::Result<bool> {
    let destination_len = fs::metadata(destination)?.len();
    if destination_len != source_len {
        return Ok(false);
    }

    progress.phase = FileOperationPhase::Verifying;
    progress.current_item = Some(source.to_path_buf());
    on_progress(progress.clone());

    files_equal_with_progress(
        source,
        destination,
        source_len,
        block_size,
        cancel,
        |bytes| {
            progress.add_verified_bytes(bytes);
            on_progress(progress.clone());
        },
    )
}

fn verify_repair_pass(
    source: &Path,
    scratch: &Path,
    source_len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    mut on_block_verified: impl FnMut(u64),
) -> io::Result<Vec<(u64, usize)>> {
    match verification_strategy(source, scratch, source_len) {
        VerificationStrategy::Sequential => verify_repair_pass_sequential(
            source,
            scratch,
            source_len,
            block_size,
            cancel,
            &mut on_block_verified,
        ),
        VerificationStrategy::Parallel => verify_repair_pass_parallel(
            source,
            scratch,
            source_len,
            block_size,
            cancel,
            on_block_verified,
        ),
    }
}

fn verify_repair_pass_sequential(
    source: &Path,
    scratch: &Path,
    source_len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    on_chunk_verified: &mut impl FnMut(u64),
) -> io::Result<Vec<(u64, usize)>> {
    let mut source = File::open(source)?;
    let mut scratch_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(scratch)?;
    let block_size = verification_chunk_size(block_size);
    let batch_size = verification_batch_size(block_size);
    let mut source_block = vec![0; batch_size];
    let mut scratch_block = vec![0; batch_size];
    let mut repaired = Vec::new();
    let mut offset = 0u64;

    while offset < source_len {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let len = (source_len - offset).min(batch_size as u64) as usize;
        source.read_exact(&mut source_block[..len])?;
        scratch_file.seek(SeekFrom::Start(offset))?;
        let scratch_read = read_exact_or_short(&mut scratch_file, &mut scratch_block[..len])?;

        let mut batch_offset = 0usize;
        while batch_offset < len {
            let block_len = (len - batch_offset).min(block_size);
            let block_range = batch_offset..batch_offset + block_len;
            let scratch_has_block = scratch_read >= block_range.end;
            let matches = scratch_has_block
                && source_block[block_range.clone()] == scratch_block[block_range.clone()];
            if !matches {
                let repair_offset = offset.saturating_add(batch_offset as u64);
                write_all_at(&scratch_file, repair_offset, &source_block[block_range])?;
                repaired.push((repair_offset, block_len));
            }
            batch_offset += block_len;
        }

        on_chunk_verified(len as u64);
        offset = offset.saturating_add(len as u64);
    }

    scratch_file.set_len(source_len)?;
    Ok(repaired)
}

fn verify_repair_pass_parallel(
    source: &Path,
    scratch: &Path,
    source_len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    mut on_chunk_verified: impl FnMut(u64),
) -> io::Result<Vec<(u64, usize)>> {
    let (progress_tx, progress_rx) = mpsc::channel::<u64>();
    let (result_tx, result_rx) = mpsc::channel::<io::Result<Vec<(u64, usize)>>>();

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let result = verify_repair_pass_impl(
                source,
                scratch,
                source_len,
                block_size,
                cancel,
                progress_tx,
            );
            let _ = result_tx.send(result);
        });

        loop {
            match result_rx.recv_timeout(PARALLEL_COPY_PROGRESS_POLL_INTERVAL) {
                Ok(result) => {
                    drain_parallel_copy_progress(&progress_rx, &mut on_chunk_verified);
                    return result;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    drain_parallel_copy_progress(&progress_rx, &mut on_chunk_verified);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("parallel repair worker disconnected"));
                }
            }
        }
    })
}

fn verify_repair_pass_impl(
    source: &Path,
    scratch: &Path,
    source_len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    progress_tx: mpsc::Sender<u64>,
) -> io::Result<Vec<(u64, usize)>> {
    let source = Arc::new(File::open(source)?);
    let scratch_file = Arc::new(
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(scratch)?,
    );
    let block_size = verification_chunk_size(block_size);
    let batch_size = verification_batch_size(block_size);
    let batch_size_u64 = batch_size as u64;
    let batch_count = source_len.div_ceil(batch_size_u64);
    let repaired = Mutex::new(Vec::new());

    (0..batch_count).into_par_iter().try_for_each_init(
        || (vec![0; batch_size], vec![0; batch_size]),
        |(source_block, scratch_block), batch_index| {
            if cancel.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }

            let offset = batch_index.saturating_mul(batch_size_u64);
            let len = (source_len - offset).min(batch_size_u64) as usize;
            read_exact_at(&source, offset, &mut source_block[..len])?;
            let scratch_read = read_at_or_eof(&scratch_file, offset, &mut scratch_block[..len])?;

            let mut batch_repaired = Vec::new();
            let mut batch_offset = 0usize;
            while batch_offset < len {
                let block_len = (len - batch_offset).min(block_size);
                let block_range = batch_offset..batch_offset + block_len;
                let scratch_has_block = scratch_read >= block_range.end;
                let matches = scratch_has_block
                    && source_block[block_range.clone()] == scratch_block[block_range.clone()];
                if !matches {
                    let repair_offset = offset.saturating_add(batch_offset as u64);
                    write_all_at(&scratch_file, repair_offset, &source_block[block_range])?;
                    batch_repaired.push((repair_offset, block_len));
                }
                batch_offset += block_len;
            }
            if !batch_repaired.is_empty() {
                repaired
                    .lock()
                    .map_err(|_| io::Error::other("verification state poisoned"))?
                    .extend(batch_repaired);
            }
            let _ = progress_tx.send(len as u64);

            Ok(())
        },
    )?;

    scratch_file.set_len(source_len)?;
    repaired
        .into_inner()
        .map_err(|_| io::Error::other("verification state poisoned"))
}

fn verify_offsets(
    source: &Path,
    scratch: &Path,
    offsets: &[(u64, usize)],
    block_size: usize,
    cancel: &Arc<AtomicBool>,
) -> io::Result<bool> {
    if offsets.is_empty() {
        return Ok(true);
    }

    let source = Arc::new(File::open(source)?);
    let scratch = Arc::new(File::open(scratch)?);
    let verified = std::sync::atomic::AtomicBool::new(true);

    offsets.par_iter().try_for_each_init(
        || (Vec::new(), Vec::new()),
        |(source_block, scratch_block), (offset, len)| {
            if cancel.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }

            let len = (*len).min(block_size);
            source_block.resize(len, 0);
            scratch_block.resize(len, 0);
            read_exact_at(&source, *offset, source_block)?;
            read_exact_at(&scratch, *offset, scratch_block)?;
            if source_block != scratch_block {
                verified.store(false, Ordering::Relaxed);
            }
            Ok(())
        },
    )?;

    Ok(verified.load(Ordering::Relaxed))
}

pub(super) fn copy_file_contents_parallel_with_progress(
    source: &Path,
    output: &File,
    source_len: u64,
    cancel: &Arc<AtomicBool>,
    mut on_chunk_copied: impl FnMut(u64),
) -> io::Result<u64> {
    let (progress_tx, progress_rx) = mpsc::channel::<u64>();
    let (result_tx, result_rx) = mpsc::channel::<io::Result<u64>>();

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let result =
                copy_file_contents_parallel_impl(source, output, source_len, cancel, progress_tx);
            let _ = result_tx.send(result);
        });

        loop {
            match result_rx.recv_timeout(PARALLEL_COPY_PROGRESS_POLL_INTERVAL) {
                Ok(result) => {
                    drain_parallel_copy_progress(&progress_rx, &mut on_chunk_copied);
                    return result;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    drain_parallel_copy_progress(&progress_rx, &mut on_chunk_copied);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("parallel copy worker disconnected"));
                }
            }
        }
    })
}

fn copy_file_contents_parallel_impl(
    source: &Path,
    output: &File,
    source_len: u64,
    cancel: &Arc<AtomicBool>,
    progress_tx: mpsc::Sender<u64>,
) -> io::Result<u64> {
    output.set_len(source_len)?;
    if source_len == 0 {
        return Ok(0);
    }

    let source = Arc::new(File::open(source)?);
    let output = Arc::new(output.try_clone()?);
    let chunk_size = PARALLEL_COPY_CHUNK_SIZE as u64;
    let chunk_count = source_len.div_ceil(chunk_size);

    (0..chunk_count).into_par_iter().try_for_each_with(
        progress_tx,
        |progress_tx, chunk_index| {
            if cancel.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }

            let offset = chunk_index.saturating_mul(chunk_size);
            let len = (source_len - offset).min(chunk_size) as usize;
            let mut buffer = vec![0; len];
            read_exact_at(&source, offset, &mut buffer)?;
            write_all_at(&output, offset, &buffer)?;
            let _ = progress_tx.send(len as u64);
            Ok(())
        },
    )?;

    Ok(source_len)
}

fn drain_parallel_copy_progress(
    progress_rx: &mpsc::Receiver<u64>,
    on_chunk_copied: &mut impl FnMut(u64),
) {
    while let Ok(bytes) = progress_rx.try_recv() {
        on_chunk_copied(bytes);
    }
}

fn verification_strategy(source: &Path, destination: &Path, len: u64) -> VerificationStrategy {
    if len >= PARALLEL_VERIFICATION_MAX_BYTES || !paths_are_on_same_volume(source, destination) {
        VerificationStrategy::Sequential
    } else {
        VerificationStrategy::Parallel
    }
}

fn verification_chunk_size(block_size: usize) -> usize {
    block_size.clamp(1, RSYNC_MAX_BLOCK_SIZE)
}

fn verification_batch_size(block_size: usize) -> usize {
    let blocks_per_batch = (RSYNC_MAX_MAP_SIZE / block_size).max(1);
    block_size.saturating_mul(blocks_per_batch)
}

fn files_equal_with_progress(
    source: &Path,
    destination: &Path,
    len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    mut on_block_verified: impl FnMut(u64),
) -> io::Result<bool> {
    match verification_strategy(source, destination, len) {
        VerificationStrategy::Sequential => files_equal_sequential(
            source,
            destination,
            len,
            block_size,
            cancel,
            &mut on_block_verified,
        ),
        VerificationStrategy::Parallel => files_equal_parallel_with_progress(
            source,
            destination,
            len,
            block_size,
            cancel,
            on_block_verified,
        ),
    }
}

fn files_equal_sequential(
    source: &Path,
    destination: &Path,
    len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    on_chunk_verified: &mut impl FnMut(u64),
) -> io::Result<bool> {
    if len == 0 {
        return Ok(true);
    }

    let mut source = File::open(source)?;
    let mut destination = File::open(destination)?;
    let block_size = verification_chunk_size(block_size);
    let batch_size = verification_batch_size(block_size);
    let mut source_block = vec![0; batch_size];
    let mut destination_block = vec![0; batch_size];
    let mut offset = 0u64;

    while offset < len {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let read_len = (len - offset).min(batch_size as u64) as usize;
        source.read_exact(&mut source_block[..read_len])?;
        destination.read_exact(&mut destination_block[..read_len])?;
        if source_block[..read_len] != destination_block[..read_len] {
            return Ok(false);
        }
        on_chunk_verified(read_len as u64);
        offset = offset.saturating_add(read_len as u64);
    }

    Ok(true)
}

fn files_equal_parallel_with_progress(
    source: &Path,
    destination: &Path,
    len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    mut on_block_verified: impl FnMut(u64),
) -> io::Result<bool> {
    let (progress_tx, progress_rx) = mpsc::channel::<u64>();
    let (result_tx, result_rx) = mpsc::channel::<io::Result<bool>>();

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let result = files_equal_parallel_impl(
                source,
                destination,
                len,
                block_size,
                cancel,
                progress_tx,
            );
            let _ = result_tx.send(result);
        });

        loop {
            match result_rx.recv_timeout(PARALLEL_COPY_PROGRESS_POLL_INTERVAL) {
                Ok(result) => {
                    drain_parallel_copy_progress(&progress_rx, &mut on_block_verified);
                    return result;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    drain_parallel_copy_progress(&progress_rx, &mut on_block_verified);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other(
                        "parallel verification worker disconnected",
                    ));
                }
            }
        }
    })
}

fn files_equal_parallel_impl(
    source: &Path,
    destination: &Path,
    len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    progress_tx: mpsc::Sender<u64>,
) -> io::Result<bool> {
    if len == 0 {
        return Ok(true);
    }

    let source = Arc::new(File::open(source)?);
    let destination = Arc::new(File::open(destination)?);
    let block_size = verification_chunk_size(block_size);
    let batch_size = verification_batch_size(block_size);
    let batch_size_u64 = batch_size as u64;
    let batch_count = len.div_ceil(batch_size_u64);
    let equal = std::sync::atomic::AtomicBool::new(true);

    (0..batch_count).into_par_iter().try_for_each_init(
        || (vec![0; batch_size], vec![0; batch_size]),
        |(source_block, destination_block), batch_index| {
            if cancel.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }
            if !equal.load(Ordering::Relaxed) {
                return Ok(());
            }

            let offset = batch_index.saturating_mul(batch_size_u64);
            let read_len = (len - offset).min(batch_size_u64) as usize;
            read_exact_at(&source, offset, &mut source_block[..read_len])?;
            read_exact_at(&destination, offset, &mut destination_block[..read_len])?;
            if source_block[..read_len] != destination_block[..read_len] {
                equal.store(false, Ordering::Relaxed);
            } else {
                let _ = progress_tx.send(read_len as u64);
            }
            Ok(())
        },
    )?;

    Ok(equal.load(Ordering::Relaxed))
}

fn read_exact_at(file: &File, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
    let mut read_total = 0usize;
    while read_total < buffer.len() {
        let read = read_at(
            file,
            offset.saturating_add(read_total as u64),
            &mut buffer[read_total..],
        )?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "unexpected end of file",
            ));
        }
        read_total += read;
    }
    Ok(())
}

fn read_at_or_eof(file: &File, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
    let mut read_total = 0usize;
    while read_total < buffer.len() {
        let read = read_at(
            file,
            offset.saturating_add(read_total as u64),
            &mut buffer[read_total..],
        )?;
        if read == 0 {
            break;
        }
        read_total += read;
    }
    Ok(read_total)
}

fn read_exact_or_short(file: &mut File, buffer: &mut [u8]) -> io::Result<usize> {
    let mut read_total = 0usize;
    while read_total < buffer.len() {
        let read = file.read(&mut buffer[read_total..])?;
        if read == 0 {
            break;
        }
        read_total += read;
    }
    Ok(read_total)
}

fn write_all_at(file: &File, offset: u64, buffer: &[u8]) -> io::Result<()> {
    let mut written_total = 0usize;
    while written_total < buffer.len() {
        let written = write_at(
            file,
            offset.saturating_add(written_total as u64),
            &buffer[written_total..],
        )?;
        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "failed to write file chunk",
            ));
        }
        written_total += written;
    }
    Ok(())
}

#[cfg(unix)]
fn read_at(file: &File, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;

    file.read_at(buffer, offset)
}

#[cfg(windows)]
fn read_at(file: &File, offset: u64, buffer: &mut [u8]) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;

    file.seek_read(buffer, offset)
}

#[cfg(unix)]
fn write_at(file: &File, offset: u64, buffer: &[u8]) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;

    file.write_at(buffer, offset)
}

#[cfg(windows)]
fn write_at(file: &File, offset: u64, buffer: &[u8]) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;

    file.seek_write(buffer, offset)
}

fn strong_hash(bytes: &[u8]) -> u64 {
    xxh3_64_with_seed(bytes, RSYNC_CHECKSUM_SEED)
}

fn remove_partial_file(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

fn cancelled_error() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "file operation cancelled")
}

fn sync_file(path: &Path) {
    if let Ok(file) = File::open(path) {
        let _ = file.sync_all();
    }
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(directory) = File::open(parent)
    {
        let _ = directory.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_directory(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::{
        filesystem::{FileOperationKind, FileOperationProgress, set_test_path_volume_key},
        test_support::TempDir,
    };
    use filetime::FileTime;

    fn test_progress(total_bytes: u64) -> FileOperationProgress {
        FileOperationProgress {
            kind: FileOperationKind::Copy,
            phase: FileOperationPhase::Preparing,
            total_bytes,
            copied_bytes: 0,
            verified_bytes: 0,
            work_total_bytes: total_bytes,
            work_completed_bytes: 0,
            total_files: 1,
            completed_files: 0,
            current_item: None,
            cancellable: true,
        }
    }

    #[test]
    fn rolling_checksum_roll_matches_recomputed_window() {
        let bytes = b"abcdefg";
        let mut rolling = RollingChecksum::new(&bytes[..4]);
        rolling.roll(b'a', b'e');

        assert_eq!(rolling, RollingChecksum::new(&bytes[1..5]));
    }

    #[test]
    fn rolling_checksum_matches_rsync_weak_checksum_formula() {
        assert_eq!(RollingChecksum::new(b"abcd").value(), 0x03d4_018a);
    }

    #[test]
    fn strong_hash_uses_seeded_xxh3_64() {
        assert_eq!(strong_hash(b""), 0x2d06_8005_38d3_94c2);
        assert_eq!(strong_hash(b"same"), strong_hash(b"same"));
        assert_ne!(strong_hash(b"same"), strong_hash(b"different"));
    }

    #[test]
    fn rsync_block_size_matches_default_heuristic() {
        assert_eq!(rsync_block_size(0), RSYNC_MIN_BLOCK_SIZE);
        assert_eq!(
            rsync_block_size((RSYNC_MIN_BLOCK_SIZE * RSYNC_MIN_BLOCK_SIZE) as u64),
            RSYNC_MIN_BLOCK_SIZE
        );
        assert_eq!(rsync_block_size(500_000), 704);
        assert_eq!(rsync_block_size(1024 * 1024), 1024);
        assert_eq!(rsync_block_size(1024 * 1024 * 1024), 32 * 1024);
        assert_eq!(
            rsync_block_size((RSYNC_MAX_BLOCK_SIZE as u64).pow(2)),
            RSYNC_MAX_BLOCK_SIZE
        );
        assert_eq!(rsync_block_size(u64::MAX), RSYNC_MAX_BLOCK_SIZE);
    }

    #[test]
    fn signature_matching_reuses_shifted_basis_blocks() {
        let temp = TempDir::new();
        let basis = temp.path().join("basis.bin");
        fs::write(&basis, b"aaaabbbbccccdddd").expect("write basis");
        let signatures = build_basis_signatures(&basis, 4, &Arc::new(AtomicBool::new(false)))
            .expect("signatures");
        let mut window = RollingWindow::new(4);
        for byte in b"bbbb" {
            window.push_back(*byte);
        }

        let signature =
            matching_signature(&window, &signatures, RollingChecksum::new(b"bbbb").value())
                .expect("matching signature");

        assert_eq!(signature.offset, 4);
        assert_eq!(signature.len, 4);
    }

    #[test]
    fn delta_copy_literal_fallback_without_basis() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let scratch = temp.path().join("scratch.bin");
        fs::write(&source, b"literal only").expect("write source");
        let mut output = File::create(&scratch).expect("create scratch");
        let mut progress = test_progress(12);
        let mut stats = DeltaCopyStats::default();

        scan_source_to_output(
            &source,
            &SignatureTable::default(),
            None,
            &mut output,
            4,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
            &mut stats,
        )
        .expect("scan");

        drop(output);
        assert_eq!(fs::read(&scratch).unwrap(), b"literal only");
        assert_eq!(stats.reused_blocks, 0);
        assert_eq!(stats.literal_bytes, 12);
    }

    #[test]
    fn verification_repairs_corrupt_scratch_block() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let scratch = temp.path().join("scratch.bin");
        fs::write(&source, b"aaaabbbbccccdddd").expect("write source");
        fs::write(&scratch, b"aaaabbbbXXXXdddd").expect("write scratch");
        let mut progress = test_progress(16);
        let mut progress_events = Vec::new();

        verify_and_repair(
            &source,
            &scratch,
            16,
            4,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |progress| progress_events.push(progress),
            CopyOptions::explorer_safe(),
        )
        .expect("verify and repair");

        assert_eq!(fs::read(&scratch).unwrap(), fs::read(&source).unwrap());
        assert_eq!(progress.verified_bytes, 16);
        assert!(progress_events.iter().any(|progress| {
            progress.phase == FileOperationPhase::Verifying && progress.verified_bytes == 16
        }));
    }

    #[test]
    fn sequential_verification_repairs_sparse_corruption() {
        let temp = TempDir::new();
        let source_root = temp.path().join("source");
        let scratch_root = temp.path().join("scratch");
        fs::create_dir_all(&source_root).expect("create source root");
        fs::create_dir_all(&scratch_root).expect("create scratch root");
        let source = source_root.join("source.bin");
        let scratch = scratch_root.join("scratch.bin");
        let data = vec![17; RSYNC_MAX_BLOCK_SIZE * 2 + 123];
        let mut corrupt = data.clone();
        corrupt[RSYNC_MAX_BLOCK_SIZE + 17..RSYNC_MAX_BLOCK_SIZE + 21].fill(99);
        fs::write(&source, &data).expect("write source");
        fs::write(&scratch, corrupt).expect("write scratch");
        let _source_volume = set_test_path_volume_key(&source_root, Some("source-volume"));
        let _scratch_volume = set_test_path_volume_key(&scratch_root, Some("scratch-volume"));
        let mut progress = test_progress(data.len() as u64);

        verify_and_repair(
            &source,
            &scratch,
            data.len() as u64,
            1024,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
            CopyOptions::explorer_safe(),
        )
        .expect("verify and repair");

        assert_eq!(fs::read(&scratch).unwrap(), data);
        assert_eq!(progress.verified_bytes, data.len() as u64);
    }

    #[test]
    fn verification_repairs_only_the_selected_rsync_block() {
        let temp = TempDir::new();
        let source_root = temp.path().join("source");
        let scratch_root = temp.path().join("scratch");
        fs::create_dir_all(&source_root).expect("create source root");
        fs::create_dir_all(&scratch_root).expect("create scratch root");
        let source = source_root.join("source.bin");
        let scratch = scratch_root.join("scratch.bin");
        let block_size = RSYNC_MIN_BLOCK_SIZE;
        let data = (0..block_size * 3)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let mut corrupt = data.clone();
        corrupt[block_size + 17] ^= 0xff;
        fs::write(&source, &data).expect("write source");
        fs::write(&scratch, corrupt).expect("write scratch");
        let _source_volume = set_test_path_volume_key(&source_root, Some("source-volume"));
        let _scratch_volume = set_test_path_volume_key(&scratch_root, Some("scratch-volume"));
        let mut verified = 0;

        let repaired = verify_repair_pass(
            &source,
            &scratch,
            data.len() as u64,
            block_size,
            &Arc::new(AtomicBool::new(false)),
            |bytes| verified += bytes,
        )
        .expect("verify repair pass");

        assert_eq!(repaired, vec![(block_size as u64, block_size)]);
        assert_eq!(verified, data.len() as u64);
        assert_eq!(fs::read(&scratch).unwrap(), data);
    }

    #[test]
    fn verification_strategy_prefers_sequential_for_different_volumes_and_large_files() {
        let temp = TempDir::new();
        let source_root = temp.path().join("source");
        let destination_root = temp.path().join("destination");
        fs::create_dir_all(&source_root).expect("create source root");
        fs::create_dir_all(&destination_root).expect("create destination root");
        let source = source_root.join("source.bin");
        let destination = destination_root.join("destination.bin");
        let _source_volume = set_test_path_volume_key(&source_root, Some("source-volume"));
        let destination_volume =
            set_test_path_volume_key(&destination_root, Some("destination-volume"));

        assert_eq!(
            verification_strategy(&source, &destination, 1024),
            VerificationStrategy::Sequential
        );

        drop(destination_volume);
        let _destination_volume =
            set_test_path_volume_key(&destination_root, Some("source-volume"));
        assert_eq!(
            verification_strategy(&source, &destination, 1024),
            VerificationStrategy::Parallel
        );
        assert_eq!(
            verification_strategy(&source, &destination, PARALLEL_VERIFICATION_MAX_BYTES),
            VerificationStrategy::Sequential
        );
    }

    #[test]
    fn sequential_identical_verification_can_be_cancelled() {
        let temp = TempDir::new();
        let source_root = temp.path().join("source");
        let destination_root = temp.path().join("destination");
        fs::create_dir_all(&source_root).expect("create source root");
        fs::create_dir_all(&destination_root).expect("create destination root");
        let source = source_root.join("source.bin");
        let destination = destination_root.join("source.bin");
        let data = vec![23; RSYNC_MAX_BLOCK_SIZE * 2 + 123];
        fs::write(&source, &data).expect("write source");
        fs::write(&destination, &data).expect("write destination");
        let _source_volume = set_test_path_volume_key(&source_root, Some("source-volume"));
        let _destination_volume =
            set_test_path_volume_key(&destination_root, Some("destination-volume"));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut progress = test_progress(data.len() as u64);
        let mut requested_cancel = false;

        let result = copy_with_delta_progress_for_test(
            &source,
            &destination,
            1024,
            &cancel,
            &mut progress,
            &mut |progress| {
                if progress.verified_bytes > 0 && !requested_cancel {
                    requested_cancel = true;
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );

        assert!(matches!(
            result,
            Err(ref error) if error.kind() == io::ErrorKind::Interrupted
        ));
        assert!(progress.verified_bytes > 0);
        assert!(progress.verified_bytes < data.len() as u64);
    }

    #[test]
    fn identical_destination_is_no_op_and_cleans_partial() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        fs::write(&source, b"same content").expect("write source");
        fs::write(&destination, b"same content").expect("write destination");
        let partial = partial_path_for(&destination);
        fs::write(&partial, b"stale partial").expect("write partial");
        let mut progress = test_progress(12);
        let mut progress_events = Vec::new();

        copy_with_delta_progress_for_test(
            &source,
            &destination,
            4,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |progress| progress_events.push(progress),
        )
        .expect("identical no-op");

        assert_eq!(fs::read(&destination).unwrap(), b"same content");
        assert_eq!(progress.copied_bytes, 0);
        assert_eq!(progress.verified_bytes, 12);
        assert_eq!(progress.percent(), Some(1.0));
        assert!(progress_events.iter().any(|progress| {
            progress.phase == FileOperationPhase::Verifying
                && progress.copied_bytes == 0
                && progress.verified_bytes > 0
        }));
        assert!(!partial.exists());
    }

    #[test]
    fn identical_destination_reports_intermediate_verification_progress() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        let data = vec![31; RSYNC_MAX_BLOCK_SIZE * 2 + 123];
        fs::write(&source, &data).expect("write source");
        fs::write(&destination, &data).expect("write destination");
        let mut progress = test_progress(data.len() as u64);
        let mut progress_events = Vec::new();

        copy_with_delta_progress_for_test(
            &source,
            &destination,
            1024,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |progress| progress_events.push(progress),
        )
        .expect("identical no-op");

        let verifying_events = progress_events
            .iter()
            .filter(|progress| {
                progress.phase == FileOperationPhase::Verifying && progress.verified_bytes > 0
            })
            .collect::<Vec<_>>();
        assert!(
            verifying_events.len() >= 2,
            "expected multiple verification updates, got {verifying_events:?}"
        );
        assert!(verifying_events.iter().any(|progress| {
            progress.verified_bytes > 0 && progress.verified_bytes < data.len() as u64
        }));
        assert_eq!(progress.copied_bytes, 0);
        assert_eq!(progress.verified_bytes, data.len() as u64);
        assert_eq!(progress.percent(), Some(1.0));
    }

    #[test]
    fn copy_then_verify_progress_is_monotonic_and_finishes_after_verification() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        fs::write(&source, b"aaaabbbbccccdddd").expect("write source");
        let mut progress = test_progress(16);
        let mut progress_events = Vec::new();

        copy_with_delta_progress_for_test(
            &source,
            &destination,
            4,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |progress| progress_events.push(progress),
        )
        .expect("copy then verify");

        let percents = progress_events
            .iter()
            .filter_map(FileOperationProgress::percent)
            .collect::<Vec<_>>();
        for pair in percents.windows(2) {
            assert!(
                pair[1] >= pair[0],
                "progress regressed from {} to {} in {percents:?}",
                pair[0],
                pair[1]
            );
        }
        let copy_finished = progress_events
            .iter()
            .find(|progress| {
                progress.phase == FileOperationPhase::Copying && progress.copied_bytes == 16
            })
            .expect("copy progress event");
        assert!(
            copy_finished.percent().expect("copy percent") < 1.0,
            "copy phase must leave room for verification"
        );
        assert_eq!(progress.copied_bytes, 16);
        assert_eq!(progress.verified_bytes, 16);
        assert_eq!(progress.percent(), Some(1.0));
    }

    #[test]
    fn identical_destination_preserves_source_metadata_without_data_copy() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        fs::write(&source, b"same content").expect("write source");
        fs::write(&destination, b"same content").expect("write destination");
        let source_modified = FileTime::from_unix_time(1_700_000_000, 0);
        let destination_modified = FileTime::from_unix_time(1_600_000_000, 0);
        filetime::set_file_mtime(&source, source_modified).expect("set source mtime");
        filetime::set_file_mtime(&destination, destination_modified)
            .expect("set destination mtime");
        let mut progress = test_progress(12);

        copy_with_delta_progress_for_test(
            &source,
            &destination,
            4,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
        )
        .expect("identical metadata no-op");

        let destination_metadata = fs::metadata(&destination).expect("destination metadata");
        assert_eq!(
            FileTime::from_last_modification_time(&destination_metadata),
            source_modified
        );
        assert_eq!(fs::read(&destination).unwrap(), b"same content");
        assert_eq!(progress.copied_bytes, 0);
    }

    #[test]
    fn rsync_update_quick_match_skips_rewrite_after_content_confirmation() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        fs::write(&source, b"same content").expect("write source");
        fs::write(&destination, b"same content").expect("write destination");
        let modified = FileTime::from_unix_time(1_700_000_000, 0);
        filetime::set_file_mtime(&source, modified).expect("set source mtime");
        filetime::set_file_mtime(&destination, modified).expect("set destination mtime");
        let metadata = fs::metadata(&destination).expect("destination metadata before");
        let mut progress = test_progress(12);

        copy_with_delta_progress_impl(
            &source,
            &destination,
            4,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
            CopyOptions::rsync_update(CopyDurability::Safe),
        )
        .expect("quick skip");

        assert_eq!(fs::read(&destination).unwrap(), b"same content");
        assert_eq!(progress.copied_bytes, 0);
        assert_eq!(progress.verified_bytes, 12);
        assert_eq!(fs::metadata(&destination).unwrap().len(), metadata.len());
    }

    #[test]
    fn rsync_update_does_not_skip_same_size_same_mtime_changed_content() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        fs::write(&source, b"aaaabbbb").expect("write source");
        fs::write(&destination, b"aaaaXXXX").expect("write destination");
        let modified = FileTime::from_unix_time(1_700_000_000, 0);
        filetime::set_file_mtime(&source, modified).expect("set source mtime");
        filetime::set_file_mtime(&destination, modified).expect("set destination mtime");
        let mut progress = test_progress(8);

        copy_with_delta_progress_impl(
            &source,
            &destination,
            4,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
            CopyOptions::rsync_update(CopyDurability::Safe),
        )
        .expect("delta update");

        assert_eq!(fs::read(&destination).unwrap(), b"aaaabbbb");
        assert!(progress.copied_bytes > 0);
    }

    #[test]
    fn fast_durability_copies_literal_file_without_partial() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        fs::write(&source, vec![11; 128 * 1024]).expect("write source");
        let mut progress = test_progress(128 * 1024);

        copy_with_delta_progress_impl(
            &source,
            &destination,
            1024,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
            CopyOptions::rsync_update(CopyDurability::Fast),
        )
        .expect("fast copy");

        assert_eq!(fs::read(&destination).unwrap(), fs::read(&source).unwrap());
        assert!(!partial_path_for(&destination).exists());
    }

    #[test]
    fn parallel_copy_reports_each_completed_chunk() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        let data = vec![13; PARALLEL_COPY_CHUNK_SIZE * 2 + 123];
        fs::write(&source, &data).expect("write source");
        let output = File::create(&destination).expect("create destination");
        let mut chunks = Vec::new();

        let copied = copy_file_contents_parallel_with_progress(
            &source,
            &output,
            data.len() as u64,
            &Arc::new(AtomicBool::new(false)),
            |bytes| chunks.push(bytes),
        )
        .expect("parallel copy");

        drop(output);
        assert_eq!(copied, data.len() as u64);
        assert_eq!(chunks.iter().copied().sum::<u64>(), copied);
        assert!(
            chunks.len() >= 2,
            "expected multiple chunk progress events, got {chunks:?}"
        );
        assert!(chunks.iter().all(|bytes| *bytes > 0));
        assert_eq!(fs::read(&destination).unwrap(), data);
    }

    #[test]
    fn delta_copy_reuses_matching_blocks_and_literals_changed_block() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let basis = temp.path().join("basis.bin");
        let scratch = temp.path().join("scratch.bin");
        fs::write(&source, b"aaaabbbbccccdddd").expect("write source");
        fs::write(&basis, b"aaaaXXXXccccdddd").expect("write basis");
        let signatures =
            build_basis_signatures(&basis, 4, &Arc::new(AtomicBool::new(false))).expect("basis");
        let mut basis_file = File::open(&basis).expect("open basis");
        let mut output = File::create(&scratch).expect("create scratch");
        let mut progress = test_progress(16);
        let mut stats = DeltaCopyStats::default();

        scan_source_to_output(
            &source,
            &signatures,
            Some(&mut basis_file),
            &mut output,
            4,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
            &mut stats,
        )
        .expect("delta scan");

        drop(output);
        assert_eq!(fs::read(&scratch).unwrap(), b"aaaabbbbccccdddd");
        assert_eq!(stats.reused_blocks, 3);
        assert_eq!(stats.literal_bytes, 4);
    }

    #[test]
    fn cancelled_resumable_copy_preserves_partial_and_later_resumes() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        let data = vec![42; LITERAL_FLUSH_SIZE + 4096];
        fs::write(&source, &data).expect("write source");

        let cancel = Arc::new(AtomicBool::new(false));
        let mut progress = test_progress(data.len() as u64);
        let mut requested_cancel = false;
        let result = copy_with_delta_progress_for_test(
            &source,
            &destination,
            1024,
            &cancel,
            &mut progress,
            &mut |progress| {
                if progress.copied_bytes > 0 && !requested_cancel {
                    requested_cancel = true;
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );

        assert!(matches!(
            result,
            Err(ref error) if error.kind() == io::ErrorKind::Interrupted
        ));
        assert!(!destination.exists());
        let partial = partial_path_for(&destination);
        assert!(partial.exists());

        let mut progress = test_progress(data.len() as u64);
        copy_with_delta_progress_for_test(
            &source,
            &destination,
            1024,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
        )
        .expect("resume copy");

        assert_eq!(fs::read(&destination).unwrap(), data);
        assert!(!partial.exists());
    }

    #[test]
    fn cancelled_resume_appends_to_same_partial_and_later_resumes() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        let data = (0..RSYNC_IO_BUFFER_SIZE * 4 + 17)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(&source, &data).expect("write source");

        let cancel = Arc::new(AtomicBool::new(false));
        let mut progress = test_progress(data.len() as u64);
        let mut requested_cancel = false;
        let result = copy_with_delta_progress_for_test(
            &source,
            &destination,
            1024,
            &cancel,
            &mut progress,
            &mut |progress| {
                if progress.copied_bytes > 0 && !requested_cancel {
                    requested_cancel = true;
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        assert!(matches!(
            result,
            Err(ref error) if error.kind() == io::ErrorKind::Interrupted
        ));

        let partial = partial_path_for(&destination);
        let first_partial_len = fs::metadata(&partial)
            .expect("first partial metadata")
            .len();
        assert!(first_partial_len > 0);

        let cancel = Arc::new(AtomicBool::new(false));
        let mut progress = test_progress(data.len() as u64);
        let mut requested_cancel = false;
        let result = copy_with_delta_progress_for_test(
            &source,
            &destination,
            1024,
            &cancel,
            &mut progress,
            &mut |progress| {
                if progress.copied_bytes > first_partial_len && !requested_cancel {
                    requested_cancel = true;
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        assert!(matches!(
            result,
            Err(ref error) if error.kind() == io::ErrorKind::Interrupted
        ));

        let second_partial_len = fs::metadata(&partial)
            .expect("second partial metadata")
            .len();
        assert!(
            second_partial_len > first_partial_len,
            "partial should grow across resume cancel: {first_partial_len} -> {second_partial_len}"
        );

        let mut progress = test_progress(data.len() as u64);
        copy_with_delta_progress_for_test(
            &source,
            &destination,
            1024,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
        )
        .expect("final resume");

        assert_eq!(fs::read(&destination).unwrap(), data);
        assert!(!partial.exists());
    }

    #[test]
    fn resumed_copy_progress_is_seeded_from_valid_partial_prefix() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        let data = (0..RSYNC_IO_BUFFER_SIZE * 3 + 19)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(&source, &data).expect("write source");
        let prefix_len = (RSYNC_IO_BUFFER_SIZE + 7) as u64;
        let partial = partial_path_for(&destination);
        fs::write(&partial, &data[..prefix_len as usize]).expect("write partial prefix");

        let mut progress = test_progress(data.len() as u64);
        let mut progress_events = Vec::new();
        copy_with_delta_progress_for_test(
            &source,
            &destination,
            1024,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |progress| progress_events.push(progress),
        )
        .expect("resume copy");

        let resuming_index = progress_events
            .iter()
            .position(|progress| progress.phase == FileOperationPhase::Resuming)
            .expect("resuming progress event");
        let seeded_copying_index = progress_events
            .iter()
            .position(|progress| {
                progress.phase == FileOperationPhase::Copying && progress.copied_bytes >= prefix_len
            })
            .expect("seeded copying progress event");
        assert!(
            resuming_index < seeded_copying_index,
            "resume validation should be reported before resumed copying"
        );
        let first_copied = progress_events
            .iter()
            .find(|progress| {
                progress.phase == FileOperationPhase::Copying && progress.copied_bytes > 0
            })
            .expect("seeded copy progress");
        assert_eq!(first_copied.copied_bytes, prefix_len);
        assert!(first_copied.work_completed_bytes >= prefix_len);
        assert_eq!(fs::read(&destination).unwrap(), data);
        assert!(!partial.exists());
    }

    #[test]
    fn resumed_copy_truncates_divergent_partial_prefix() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        fs::write(&source, b"aaaabbbbcccc").expect("write source");
        let partial = partial_path_for(&destination);
        fs::write(&partial, b"aaaaXXXXstale").expect("write divergent partial");
        let mut progress = test_progress(12);

        copy_with_delta_progress_for_test(
            &source,
            &destination,
            4,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
        )
        .expect("resume from divergent partial");

        assert_eq!(fs::read(&destination).unwrap(), b"aaaabbbbcccc");
        assert!(!partial.exists());
    }

    #[test]
    fn resumed_copy_truncates_overlong_partial() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        fs::write(&source, b"source").expect("write source");
        let partial = partial_path_for(&destination);
        fs::write(&partial, b"sourcestale").expect("write overlong partial");
        let mut progress = test_progress(6);

        copy_with_delta_progress_for_test(
            &source,
            &destination,
            4,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
        )
        .expect("resume from overlong partial");

        assert_eq!(fs::read(&destination).unwrap(), b"source");
        assert!(!partial.exists());
    }

    #[test]
    fn copy_fails_when_partial_path_is_not_a_file() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        fs::write(&source, b"source").expect("write source");
        let partial = partial_path_for(&destination);
        fs::create_dir(&partial).expect("create partial directory");
        let mut progress = test_progress(6);

        let error = copy_with_delta_progress_for_test(
            &source,
            &destination,
            4,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
        )
        .expect_err("partial directory should fail");

        assert!(
            error
                .to_string()
                .contains("already exists and is not a file")
        );
    }

    #[test]
    fn replace_reuses_existing_destination_as_basis() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        fs::write(&source, b"aaaabbbbccccdddd").expect("write source");
        fs::write(&destination, b"aaaaXXXXccccdddd").expect("write destination");
        let mut progress = test_progress(16);

        copy_with_delta_progress_for_test(
            &source,
            &destination,
            4,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
        )
        .expect("delta replace");

        assert_eq!(fs::read(&destination).unwrap(), fs::read(&source).unwrap());
        assert!(progress.copied_bytes >= 16);
    }
}
