use std::{
    collections::HashMap,
    ffi::OsStr,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, UNIX_EPOCH},
};

use filetime::FileTime;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::explorer::filesystem::{
    FileOperationPhase, FileOperationProgress, preserve_file_metadata,
    replace_destination_with_temp,
};

pub(super) const RESUMABLE_COPY_BLOCK_SIZE: usize = 1024 * 1024;

const JOURNAL_VERSION: u32 = 1;
const LITERAL_FLUSH_SIZE: usize = 64 * 1024;
const PARALLEL_COPY_CHUNK_SIZE: usize = 4 * 1024 * 1024;
const PARALLEL_COPY_THRESHOLD: u64 = 8 * 1024 * 1024;
const PARALLEL_COPY_PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(25);

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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct CopyJournal {
    version: u32,
    operation_key: String,
    source: String,
    destination: String,
    source_len: u64,
    source_modified: String,
    block_size: usize,
}

#[derive(Clone, Debug)]
struct OperationIdentity {
    journal: CopyJournal,
}

#[derive(Clone, Debug)]
struct SidecarPaths {
    partial: PathBuf,
    next: PathBuf,
    journal: PathBuf,
}

#[derive(Clone, Debug)]
struct BlockSignature {
    offset: u64,
    len: usize,
    strong: [u8; 32],
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct DeltaCopyStats {
    reused_blocks: usize,
    literal_bytes: u64,
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
            let value = u32::from(*byte);
            a = (a + value) & 0xffff;
            b = (b + ((len - index) as u32 * value)) & 0xffff;
        }

        Self { a, b, len }
    }

    fn roll(&mut self, removed: u8, added: u8) {
        let removed = u32::from(removed);
        let added = u32::from(added);

        self.a = (self.a + 0x1_0000 - removed + added) & 0xffff;
        self.b = (self.b + 0x1_0000 - ((self.len as u32 * removed) & 0xffff) + self.a) & 0xffff;
    }

    fn value(self) -> u32 {
        (self.b << 16) | self.a
    }
}

struct BufferedByteReader {
    file: File,
    buffer: Vec<u8>,
    position: usize,
    filled: usize,
}

impl BufferedByteReader {
    fn new(file: File) -> Self {
        Self {
            file,
            buffer: vec![0; RESUMABLE_COPY_BLOCK_SIZE],
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
    copy_with_delta_progress_impl(
        source,
        destination,
        RESUMABLE_COPY_BLOCK_SIZE,
        cancel,
        progress,
        on_progress,
        options,
    )
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
    let identity = OperationIdentity::new(source, destination, &metadata, block_size)?;
    let sidecars = sidecar_paths(destination, &identity.journal.operation_key);

    if destination.is_file() {
        let same_data = match options.sync_mode {
            CopySyncMode::RsyncUpdate => {
                destination_quick_matches_source(&metadata, destination).unwrap_or(false)
                    && destination_content_matches_source(
                        source,
                        destination,
                        source_len,
                        block_size,
                        cancel,
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
            cleanup_sidecars(&sidecars);
            return Ok(());
        }
    }

    let journal_matches = read_journal(&sidecars.journal)
        .ok()
        .flatten()
        .is_some_and(|journal| journal == identity.journal);

    if !journal_matches {
        remove_sidecar_file(&sidecars.partial);
        remove_sidecar_file(&sidecars.next);
    }
    write_journal(&sidecars.journal, &identity.journal)?;

    let basis = if journal_matches {
        previous_partial_path(&sidecars)?
    } else {
        None
    }
    .or_else(|| destination.is_file().then(|| destination.to_path_buf()));

    let stats = match write_delta_scratch(
        source,
        basis.as_deref(),
        &sidecars.next,
        source_len,
        block_size,
        cancel,
        progress,
        on_progress,
        options,
    ) {
        Ok(stats) => stats,
        Err(error) => {
            preserve_scratch_as_partial(&sidecars);
            return Err(error);
        }
    };

    if options.durability == CopyDurability::Safe
        || basis.is_some()
        || stats.literal_bytes != source_len
        || stats.reused_blocks != 0
    {
        let verify_result = verify_and_repair(
            source,
            &sidecars.next,
            source_len,
            block_size,
            cancel,
            progress,
            on_progress,
            options,
        );

        if let Err(error) = verify_result {
            preserve_scratch_as_partial(&sidecars);
            return Err(error);
        }
    }

    preserve_file_metadata(&metadata, &sidecars.next)?;
    replace_destination_with_temp(&sidecars.next, destination)?;
    if options.should_sync() {
        sync_parent_directory(destination);
    }
    cleanup_sidecars(&sidecars);
    Ok(())
}

impl OperationIdentity {
    fn new(
        source: &Path,
        destination: &Path,
        metadata: &fs::Metadata,
        block_size: usize,
    ) -> io::Result<Self> {
        let source = stable_source_path(source);
        let destination = stable_destination_path(destination)?;
        let source_len = metadata.len();
        let source_modified = source_modified_fingerprint(metadata);
        let operation_key = operation_key(
            &source,
            &destination,
            source_len,
            &source_modified,
            block_size,
        );

        Ok(Self {
            journal: CopyJournal {
                version: JOURNAL_VERSION,
                operation_key,
                source,
                destination,
                source_len,
                source_modified,
                block_size,
            },
        })
    }
}

fn stable_source_path(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn stable_destination_path(path: &Path) -> io::Result<String> {
    if let Ok(canonical) = fs::canonicalize(path) {
        return Ok(canonical.to_string_lossy().into_owned());
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent)?;
    Ok(parent
        .join(path.file_name().unwrap_or_else(|| OsStr::new("file")))
        .to_string_lossy()
        .into_owned())
}

fn source_modified_fingerprint(metadata: &fs::Metadata) -> String {
    match metadata.modified() {
        Ok(modified) => match modified.duration_since(UNIX_EPOCH) {
            Ok(duration) => format!("+{}.{:09}", duration.as_secs(), duration.subsec_nanos()),
            Err(error) => {
                let duration = error.duration();
                format!("-{}.{:09}", duration.as_secs(), duration.subsec_nanos())
            }
        },
        Err(_) => "unknown".to_owned(),
    }
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

pub(super) fn destination_content_matches_source(
    source: &Path,
    destination: &Path,
    source_len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
) -> io::Result<bool> {
    if fs::metadata(destination)?.len() != source_len {
        return Ok(false);
    }

    files_equal_parallel(source, destination, source_len, block_size, cancel)
}

fn operation_key(
    source: &str,
    destination: &str,
    source_len: u64,
    source_modified: &str,
    block_size: usize,
) -> String {
    let mut hash = Sha256::new();
    hash.update(format!("explorer-resumable-copy-v{JOURNAL_VERSION}\0"));
    hash.update(source.as_bytes());
    hash.update(b"\0");
    hash.update(destination.as_bytes());
    hash.update(b"\0");
    hash.update(source_len.to_le_bytes());
    hash.update(b"\0");
    hash.update(source_modified.as_bytes());
    hash.update(b"\0");
    hash.update((block_size as u64).to_le_bytes());
    hex_hash(hash.finalize().as_slice())
}

fn sidecar_paths(destination: &Path, operation_key: &str) -> SidecarPaths {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .unwrap_or_else(|| OsStr::new("file"))
        .to_string_lossy();
    let base = format!(".explorer-copy-{operation_key}-{file_name}");

    SidecarPaths {
        partial: parent.join(format!("{base}.partial")),
        next: parent.join(format!("{base}.partial.next")),
        journal: parent.join(format!("{base}.json")),
    }
}

fn read_journal(path: &Path) -> io::Result<Option<CopyJournal>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_journal(path: &Path, journal: &CopyJournal) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(journal).map_err(io::Error::other)?;
    fs::write(&tmp, bytes)?;
    replace_sidecar_file(&tmp, path)
}

fn previous_partial_path(sidecars: &SidecarPaths) -> io::Result<Option<PathBuf>> {
    if sidecars.partial.is_file() {
        return Ok(Some(sidecars.partial.clone()));
    }

    if sidecars.next.is_file() {
        replace_sidecar_file(&sidecars.next, &sidecars.partial)?;
        return Ok(Some(sidecars.partial.clone()));
    }

    Ok(None)
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
        HashMap::new()
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
        source_len,
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
) -> io::Result<HashMap<u32, Vec<BlockSignature>>> {
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
            Ok((
                weak,
                BlockSignature {
                    offset,
                    len: read_len,
                    strong: strong_hash(&buffer),
                },
            ))
        })
        .collect::<io::Result<Vec<_>>>()?;

    let mut by_weak = HashMap::with_capacity(signatures.len());
    for (weak, signature) in signatures {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        by_weak.entry(weak).or_insert_with(Vec::new).push(signature);
    }

    Ok(by_weak)
}

fn scan_source_to_output(
    source: &Path,
    signatures: &HashMap<u32, Vec<BlockSignature>>,
    mut basis_file: Option<&mut File>,
    output: &mut File,
    source_len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    stats: &mut DeltaCopyStats,
) -> io::Result<()> {
    if signatures.is_empty() {
        return copy_source_literals_buffered(
            source,
            output,
            source_len,
            cancel,
            progress,
            on_progress,
            stats,
        );
    }

    let mut source = BufferedByteReader::new(File::open(source)?);
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
    source_len: u64,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
    stats: &mut DeltaCopyStats,
) -> io::Result<()> {
    if source_len >= PARALLEL_COPY_THRESHOLD {
        copy_file_contents_parallel_with_progress(source, output, source_len, cancel, |bytes| {
            stats.literal_bytes = stats.literal_bytes.saturating_add(bytes);
            progress.copied_bytes = progress.copied_bytes.saturating_add(bytes);
            on_progress(progress.clone());
        })?;
        return Ok(());
    }

    let mut source = File::open(source)?;
    let mut buffer = vec![0; RESUMABLE_COPY_BLOCK_SIZE];

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
        progress.copied_bytes = progress.copied_bytes.saturating_add(written);
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
    signatures: &HashMap<u32, Vec<BlockSignature>>,
    weak: u32,
) -> Option<BlockSignature> {
    signatures.get(&weak)?;
    let bytes = window.to_vec();
    matching_signature_bytes(&bytes, signatures, weak)
}

fn matching_signature_bytes(
    bytes: &[u8],
    signatures: &HashMap<u32, Vec<BlockSignature>>,
    weak: u32,
) -> Option<BlockSignature> {
    let candidates = signatures.get(&weak)?;
    let strong = strong_hash(bytes);
    candidates
        .iter()
        .find(|signature| signature.len == bytes.len() && signature.strong == strong)
        .cloned()
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
    progress.copied_bytes = progress.copied_bytes.saturating_add(written);
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
    let mut buffer = vec![0; remaining.min(64 * 1024)];
    let mut offset = signature.offset;
    while remaining > 0 {
        let read_len = remaining.min(buffer.len());
        read_exact_at(basis, offset, &mut buffer[..read_len])?;
        output.write_all(&buffer[..read_len])?;
        remaining -= read_len;
        offset = offset.saturating_add(read_len as u64);
    }

    progress.copied_bytes = progress.copied_bytes.saturating_add(signature.len as u64);
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

    let repaired = verify_repair_pass(source, scratch, source_len, block_size, cancel)?;
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

    files_equal_parallel(source, destination, source_len, block_size, cancel)
}

fn verify_repair_pass(
    source: &Path,
    scratch: &Path,
    source_len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
) -> io::Result<Vec<(u64, usize)>> {
    let source = Arc::new(File::open(source)?);
    let scratch_file = Arc::new(
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(scratch)?,
    );
    let block_size_u64 = block_size as u64;
    let block_count = source_len.div_ceil(block_size_u64);
    let repaired = Mutex::new(Vec::new());

    (0..block_count)
        .into_par_iter()
        .try_for_each(|block_index| {
            if cancel.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }

            let offset = block_index.saturating_mul(block_size_u64);
            let len = (source_len - offset).min(block_size_u64) as usize;
            let mut source_block = vec![0; len];
            let mut scratch_block = vec![0; len];
            read_exact_at(&source, offset, &mut source_block)?;
            let scratch_read = read_at(&scratch_file, offset, &mut scratch_block)?;
            scratch_block.truncate(scratch_read);

            let matches = source_block.len() == scratch_block.len()
                && strong_hash(&source_block) == strong_hash(&scratch_block);
            if !matches {
                write_all_at(&scratch_file, offset, &source_block)?;
                repaired
                    .lock()
                    .map_err(|_| io::Error::other("verification state poisoned"))?
                    .push((offset, len));
            }

            Ok(())
        })?;

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

    offsets.par_iter().try_for_each(|(offset, len)| {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let len = (*len).min(block_size);
        let mut source_block = vec![0; len];
        let mut scratch_block = vec![0; len];
        read_exact_at(&source, *offset, &mut source_block)?;
        read_exact_at(&scratch, *offset, &mut scratch_block)?;
        if strong_hash(&source_block) != strong_hash(&scratch_block) {
            verified.store(false, Ordering::Relaxed);
        }
        Ok(())
    })?;

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

fn files_equal_parallel(
    source: &Path,
    destination: &Path,
    len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
) -> io::Result<bool> {
    if len == 0 {
        return Ok(true);
    }

    let source = Arc::new(File::open(source)?);
    let destination = Arc::new(File::open(destination)?);
    let block_size = block_size as u64;
    let block_count = len.div_ceil(block_size);
    let equal = std::sync::atomic::AtomicBool::new(true);

    (0..block_count)
        .into_par_iter()
        .try_for_each(|block_index| {
            if cancel.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }
            if !equal.load(Ordering::Relaxed) {
                return Ok(());
            }

            let offset = block_index.saturating_mul(block_size);
            let read_len = (len - offset).min(block_size) as usize;
            let mut source_block = vec![0; read_len];
            let mut destination_block = vec![0; read_len];
            read_exact_at(&source, offset, &mut source_block)?;
            read_exact_at(&destination, offset, &mut destination_block)?;
            if source_block != destination_block {
                equal.store(false, Ordering::Relaxed);
            }
            Ok(())
        })?;

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

fn strong_hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

fn hex_hash(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push_str(&format!("{byte:02x}"));
    }
    encoded
}

fn preserve_scratch_as_partial(sidecars: &SidecarPaths) {
    if sidecars.next.is_file() {
        let _ = replace_sidecar_file(&sidecars.next, &sidecars.partial);
    }
}

fn cleanup_sidecars(sidecars: &SidecarPaths) {
    remove_sidecar_file(&sidecars.partial);
    remove_sidecar_file(&sidecars.next);
    remove_sidecar_file(&sidecars.journal);
}

fn remove_sidecar_file(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {}
    }
}

fn replace_sidecar_file(source: &Path, destination: &Path) -> io::Result<()> {
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(error) if destination.exists() => {
            fs::remove_file(destination)?;
            fs::rename(source, destination).map_err(|_| error)
        }
        Err(error) => Err(error),
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
        filesystem::{FileOperationKind, FileOperationProgress},
        test_support::TempDir,
    };
    use filetime::FileTime;

    fn test_progress(total_bytes: u64) -> FileOperationProgress {
        FileOperationProgress {
            kind: FileOperationKind::Copy,
            phase: FileOperationPhase::Preparing,
            total_bytes,
            copied_bytes: 0,
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
            &HashMap::new(),
            None,
            &mut output,
            12,
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

        verify_and_repair(
            &source,
            &scratch,
            16,
            4,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
            CopyOptions::explorer_safe(),
        )
        .expect("verify and repair");

        assert_eq!(fs::read(&scratch).unwrap(), fs::read(&source).unwrap());
    }

    #[test]
    fn identical_destination_is_no_op_and_cleans_matching_sidecars() {
        let temp = TempDir::new();
        let source = temp.path().join("source.bin");
        let destination = temp.path().join("destination.bin");
        fs::write(&source, b"same content").expect("write source");
        fs::write(&destination, b"same content").expect("write destination");
        let sidecars = resumable_sidecars_for_test(&source, &destination, 4);
        fs::write(&sidecars.partial, b"stale partial").expect("write partial sidecar");
        fs::write(&sidecars.next, b"stale next").expect("write next sidecar");
        write_matching_journal_for_test(&source, &destination, 4);
        let mut progress = test_progress(12);

        copy_with_delta_progress_for_test(
            &source,
            &destination,
            4,
            &Arc::new(AtomicBool::new(false)),
            &mut progress,
            &mut |_| {},
        )
        .expect("identical no-op");

        assert_eq!(fs::read(&destination).unwrap(), b"same content");
        assert_eq!(progress.copied_bytes, 0);
        assert!(!sidecars.partial.exists());
        assert!(!sidecars.next.exists());
        assert!(!sidecars.journal.exists());
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
    fn fast_durability_copies_literal_file_without_sidecars() {
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
        let sidecars = resumable_sidecars_for_test(&source, &destination, 1024);
        assert!(!sidecars.partial.exists());
        assert!(!sidecars.next.exists());
        assert!(!sidecars.journal.exists());
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
            16,
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
    fn cancelled_resumable_copy_preserves_sidecars_and_later_resumes() {
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
        let sidecars = resumable_sidecars_for_test(&source, &destination, 1024);
        assert!(sidecars.partial.exists());
        assert!(sidecars.journal.exists());

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
        assert!(!sidecars.partial.exists());
        assert!(!sidecars.next.exists());
        assert!(!sidecars.journal.exists());
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

    fn resumable_sidecars_for_test(
        source: &Path,
        destination: &Path,
        block_size: usize,
    ) -> SidecarPaths {
        let metadata = fs::metadata(source).expect("source metadata");
        let identity =
            OperationIdentity::new(source, destination, &metadata, block_size).expect("identity");
        sidecar_paths(destination, &identity.journal.operation_key)
    }

    fn write_matching_journal_for_test(source: &Path, destination: &Path, block_size: usize) {
        let metadata = fs::metadata(source).expect("source metadata");
        let identity =
            OperationIdentity::new(source, destination, &metadata, block_size).expect("identity");
        let sidecars = sidecar_paths(destination, &identity.journal.operation_key);
        write_journal(&sidecars.journal, &identity.journal).expect("write journal");
    }
}
