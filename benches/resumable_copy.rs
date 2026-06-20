use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use explorer::benchmark_support::{
    copy_paths, copy_with_cancel_after_progress, set_copy_fast, set_copy_parallelism,
};
use filetime::FileTime;

const FIXTURE_VERSION: &str = "resumable-copy-benchmark-v1";
const LARGE_BYTES: usize = 32 * 1024 * 1024;
const MANY_SMALL_FILES: usize = 512;
const SMALL_FILE_BYTES: usize = 4096;
static OUTPUT_COUNTER: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
    large: PathBuf,
    many_small: PathBuf,
    same_size_source: PathBuf,
    same_size_basis: PathBuf,
    shifted_source: PathBuf,
    shifted_basis: PathBuf,
}

impl Fixture {
    fn get() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(FIXTURE_VERSION);
        let marker = root.join(".complete");
        if !marker.exists() {
            if root.exists() {
                fs::remove_dir_all(&root).expect("remove incomplete copy fixture");
            }
            fs::create_dir_all(&root).expect("create copy fixture");

            let large = deterministic_bytes(LARGE_BYTES, 17);
            fs::write(root.join("large.bin"), &large).expect("write large fixture");

            let many_small = root.join("many-small");
            fs::create_dir(&many_small).expect("create many-small fixture");
            for index in 0..MANY_SMALL_FILES {
                fs::write(
                    many_small.join(format!("file-{index:04}.bin")),
                    deterministic_bytes(SMALL_FILE_BYTES, index as u8),
                )
                .expect("write small fixture");
            }

            let mut same_source = deterministic_bytes(8 * 1024 * 1024, 23);
            let same_basis = same_source.clone();
            same_source[4 * 1024 * 1024..4 * 1024 * 1024 + 4096].fill(99);
            fs::write(root.join("same-size-source.bin"), same_source)
                .expect("write same-size source");
            fs::write(root.join("same-size-basis.bin"), same_basis).expect("write same-size basis");

            let shifted_basis = deterministic_bytes(8 * 1024 * 1024, 41);
            let mut shifted_source = Vec::with_capacity(shifted_basis.len() + 4096);
            shifted_source.extend_from_slice(b"inserted-block");
            shifted_source.resize(4096, b'i');
            shifted_source.extend_from_slice(&shifted_basis);
            fs::write(root.join("shifted-source.bin"), shifted_source)
                .expect("write shifted source");
            fs::write(root.join("shifted-basis.bin"), shifted_basis).expect("write shifted basis");

            fs::write(&marker, FIXTURE_VERSION).expect("write copy fixture marker");
        }

        Self {
            large: root.join("large.bin"),
            many_small: root.join("many-small"),
            same_size_source: root.join("same-size-source.bin"),
            same_size_basis: root.join("same-size-basis.bin"),
            shifted_source: root.join("shifted-source.bin"),
            shifted_basis: root.join("shifted-basis.bin"),
            root,
        }
    }
}

fn deterministic_bytes(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|index| seed.wrapping_add((index % 251) as u8))
        .collect()
}

fn fresh_output(root: &Path, name: &str) -> PathBuf {
    let counter = OUTPUT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let output = root.join(format!("{name}-{}-{counter}", std::process::id()));
    if output.exists() {
        fs::remove_dir_all(&output).expect("remove prior output");
    }
    fs::create_dir(&output).expect("create output");
    output
}

fn copy_mtime(source: &Path, destination: &Path) {
    let metadata = fs::metadata(source).expect("source metadata");
    filetime::set_file_mtime(
        destination,
        FileTime::from_last_modification_time(&metadata),
    )
    .expect("copy mtime");
}

fn copy_benchmarks(criterion: &mut Criterion) {
    let fixture = Fixture::get();

    let mut large = criterion.benchmark_group("resumable_copy/large_file");
    large.throughput(Throughput::Bytes(LARGE_BYTES as u64));
    for (name, fast) in [("safe", false), ("fast", true)] {
        large.bench_function(name, |bencher| {
            bencher.iter_batched(
                || {
                    set_copy_fast(fast);
                    fresh_output(&fixture.root, name)
                },
                |output| {
                    copy_paths(std::slice::from_ref(&fixture.large), &output);
                    fs::remove_dir_all(output).expect("remove output");
                    set_copy_fast(false);
                },
                BatchSize::SmallInput,
            );
        });
    }
    large.finish();

    let mut files = criterion.benchmark_group("resumable_copy/files");
    files.throughput(Throughput::Elements(MANY_SMALL_FILES as u64));
    for (name, parallelism) in [("many_small_1", Some(1)), ("many_small_default", None)] {
        files.bench_function(name, |bencher| {
            bencher.iter_batched(
                || {
                    set_copy_parallelism(parallelism);
                    fresh_output(&fixture.root, name)
                },
                |output| {
                    copy_paths(std::slice::from_ref(&fixture.many_small), &output);
                    fs::remove_dir_all(output).expect("remove output");
                    set_copy_parallelism(None);
                },
                BatchSize::SmallInput,
            );
        });
    }
    files.finish();

    let mut update = criterion.benchmark_group("resumable_copy/update");
    update.bench_function("unchanged_quick_skip", |bencher| {
        bencher.iter_batched(
            || {
                let output = fresh_output(&fixture.root, "unchanged-output");
                let destination = output.join("large.bin");
                fs::copy(&fixture.large, &destination).expect("seed unchanged destination");
                copy_mtime(&fixture.large, &destination);
                output
            },
            |output| {
                copy_paths(std::slice::from_ref(&fixture.large), &output);
                fs::remove_dir_all(output).expect("remove output");
            },
            BatchSize::SmallInput,
        );
    });
    update.bench_function("same_size_delta", |bencher| {
        bencher.iter_batched(
            || {
                let output = fresh_output(&fixture.root, "same-size-output");
                fs::copy(
                    &fixture.same_size_basis,
                    output.join("same-size-source.bin"),
                )
                .expect("seed same-size basis");
                output
            },
            |output| {
                copy_paths(std::slice::from_ref(&fixture.same_size_source), &output);
                fs::remove_dir_all(output).expect("remove output");
            },
            BatchSize::SmallInput,
        );
    });
    update.bench_function("shifted_insert_delta", |bencher| {
        bencher.iter_batched(
            || {
                let output = fresh_output(&fixture.root, "shifted-output");
                fs::copy(&fixture.shifted_basis, output.join("shifted-source.bin"))
                    .expect("seed shifted basis");
                output
            },
            |output| {
                copy_paths(std::slice::from_ref(&fixture.shifted_source), &output);
                fs::remove_dir_all(output).expect("remove output");
            },
            BatchSize::SmallInput,
        );
    });
    update.bench_function("cancel_resume", |bencher| {
        bencher.iter_batched(
            || fresh_output(&fixture.root, "cancel-output"),
            |output| {
                let cancelled =
                    copy_with_cancel_after_progress(std::slice::from_ref(&fixture.large), &output);
                assert!(cancelled);
                copy_paths(std::slice::from_ref(&fixture.large), &output);
                fs::remove_dir_all(output).expect("remove output");
            },
            BatchSize::SmallInput,
        );
    });
    update.finish();
}

criterion_group!(benches, copy_benchmarks);
criterion_main!(benches);
