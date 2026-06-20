use std::{
    collections::{HashMap, VecDeque},
    ffi::OsStr,
    fs::{self, File},
    io::{self, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::UNIX_EPOCH,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::explorer::filesystem::{
    FileOperationPhase, FileOperationProgress, preserve_file_metadata,
    replace_destination_with_temp,
};

pub(super) const RESUMABLE_COPY_BLOCK_SIZE: usize = 1024 * 1024;

const JOURNAL_VERSION: u32 = 1;
const LITERAL_FLUSH_SIZE: usize = 64 * 1024;

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

pub(super) fn copy_with_delta_progress(
    source: &Path,
    destination: &Path,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> io::Result<()> {
    copy_with_delta_progress_impl(
        source,
        destination,
        RESUMABLE_COPY_BLOCK_SIZE,
        cancel,
        progress,
        on_progress,
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
    )
}

fn copy_with_delta_progress_impl(
    source: &Path,
    destination: &Path,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
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

    if destination.is_file()
        && destination_is_identical_to_source(
            source,
            destination,
            source_len,
            block_size,
            cancel,
            progress,
            on_progress,
        )?
    {
        preserve_file_metadata(&metadata, destination)?;
        sync_file(destination);
        sync_parent_directory(destination);
        cleanup_sidecars(&sidecars);
        return Ok(());
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

    let copy_result = write_delta_scratch(
        source,
        basis.as_deref(),
        &sidecars.next,
        source_len,
        block_size,
        cancel,
        progress,
        on_progress,
    );

    if let Err(error) = copy_result {
        preserve_scratch_as_partial(&sidecars);
        return Err(error);
    }

    let verify_result = verify_and_repair(
        source,
        &sidecars.next,
        source_len,
        block_size,
        cancel,
        progress,
        on_progress,
    );

    if let Err(error) = verify_result {
        preserve_scratch_as_partial(&sidecars);
        return Err(error);
    }

    preserve_file_metadata(&metadata, &sidecars.next)?;
    replace_destination_with_temp(&sidecars.next, destination)?;
    sync_parent_directory(destination);
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
        block_size,
        cancel,
        progress,
        on_progress,
        &mut stats,
    );
    if let Err(error) = scan_result {
        let _ = output.sync_all();
        return Err(error);
    }

    output.set_len(source_len)?;
    output.sync_all()?;
    Ok(stats)
}

fn build_basis_signatures(
    basis: &Path,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
) -> io::Result<HashMap<u32, Vec<BlockSignature>>> {
    let mut file = File::open(basis)?;
    let mut signatures = HashMap::new();
    let mut offset = 0u64;
    let mut buffer = vec![0; block_size];

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        let bytes = &buffer[..read];
        let weak = RollingChecksum::new(bytes).value();
        signatures
            .entry(weak)
            .or_insert_with(Vec::new)
            .push(BlockSignature {
                offset,
                len: read,
                strong: strong_hash(bytes),
            });
        offset = offset.saturating_add(read as u64);
    }

    Ok(signatures)
}

fn scan_source_to_output(
    source: &Path,
    signatures: &HashMap<u32, Vec<BlockSignature>>,
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

    let mut source = BufReader::new(File::open(source)?);
    let mut window = VecDeque::with_capacity(block_size);
    let mut pending_literal = Vec::with_capacity(LITERAL_FLUSH_SIZE);
    fill_window(&mut source, &mut window, block_size)?;

    if window.len() == block_size {
        let mut weak = RollingChecksum::new(&window_bytes(&window));

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
                    weak = RollingChecksum::new(&window_bytes(&window));
                }
            } else {
                let removed = window.pop_front().expect("full window");
                pending_literal.push(removed);
                if pending_literal.len() >= LITERAL_FLUSH_SIZE {
                    flush_literal(output, &mut pending_literal, progress, on_progress, stats)?;
                }

                let mut byte = [0u8; 1];
                match source.read(&mut byte)? {
                    0 => break,
                    1 => {
                        window.push_back(byte[0]);
                        weak.roll(removed, byte[0]);
                    }
                    _ => unreachable!(),
                }
            }
        }
    }

    if !window.is_empty() {
        let tail = window_bytes(&window);
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
    source: &mut impl Read,
    window: &mut VecDeque<u8>,
    block_size: usize,
) -> io::Result<()> {
    while window.len() < block_size {
        let mut byte = [0u8; 1];
        match source.read(&mut byte)? {
            0 => break,
            1 => window.push_back(byte[0]),
            _ => unreachable!(),
        }
    }
    Ok(())
}

fn matching_signature(
    window: &VecDeque<u8>,
    signatures: &HashMap<u32, Vec<BlockSignature>>,
    weak: u32,
) -> Option<BlockSignature> {
    let bytes = window_bytes(window);
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

fn window_bytes(window: &VecDeque<u8>) -> Vec<u8> {
    window.iter().copied().collect()
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
    basis: &mut File,
    output: &mut File,
    signature: &BlockSignature,
    progress: &mut FileOperationProgress,
    on_progress: &mut impl FnMut(FileOperationProgress),
) -> io::Result<()> {
    basis.seek(SeekFrom::Start(signature.offset))?;
    let mut remaining = signature.len;
    let mut buffer = vec![0; remaining.min(64 * 1024)];
    while remaining > 0 {
        let read_len = remaining.min(buffer.len());
        basis.read_exact(&mut buffer[..read_len])?;
        output.write_all(&buffer[..read_len])?;
        remaining -= read_len;
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
) -> io::Result<()> {
    progress.phase = FileOperationPhase::Verifying;
    progress.current_item = Some(source.to_path_buf());
    on_progress(progress.clone());

    verify_pass(source, scratch, source_len, block_size, cancel, true)?;
    let verified = verify_pass(source, scratch, source_len, block_size, cancel, false)?;
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

    let mut source = File::open(source)?;
    let mut destination = File::open(destination)?;
    let mut source_buffer = vec![0; block_size];
    let mut destination_buffer = vec![0; block_size];

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let source_read = source.read(&mut source_buffer)?;
        let destination_read = destination.read(&mut destination_buffer)?;
        if source_read != destination_read {
            return Ok(false);
        }
        if source_read == 0 {
            return Ok(true);
        }

        if strong_hash(&source_buffer[..source_read])
            != strong_hash(&destination_buffer[..destination_read])
        {
            return Ok(false);
        }
    }
}

fn verify_pass(
    source: &Path,
    scratch: &Path,
    source_len: u64,
    block_size: usize,
    cancel: &Arc<AtomicBool>,
    repair: bool,
) -> io::Result<bool> {
    let mut source = File::open(source)?;
    let mut scratch_file = if repair {
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(scratch)?
    } else {
        fs::OpenOptions::new().read(true).open(scratch)?
    };

    let mut offset = 0u64;
    let mut all_verified = true;
    while offset < source_len {
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let len = (source_len - offset).min(block_size as u64) as usize;
        let source_block = read_exact_block(&mut source, offset, len)?;
        let scratch_block = read_available_block(&mut scratch_file, offset, len)?;
        let matches = source_block.len() == scratch_block.len()
            && strong_hash(&source_block) == strong_hash(&scratch_block);

        if !matches {
            all_verified = false;
            if repair {
                scratch_file.seek(SeekFrom::Start(offset))?;
                scratch_file.write_all(&source_block)?;
            }
        }

        offset = offset.saturating_add(len as u64);
    }

    if repair {
        scratch_file.set_len(source_len)?;
        scratch_file.sync_all()?;
    }

    Ok(all_verified || repair)
}

fn read_exact_block(file: &mut File, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut buffer = vec![0; len];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut buffer)?;
    Ok(buffer)
}

fn read_available_block(file: &mut File, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut buffer = vec![0; len];
    file.seek(SeekFrom::Start(offset))?;
    let read = file.read(&mut buffer)?;
    buffer.truncate(read);
    Ok(buffer)
}

fn strong_hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
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
        let window = VecDeque::from(Vec::from(&b"bbbb"[..]));

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
