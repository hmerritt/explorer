use std::{
    fs::{self, File},
    hint::black_box,
    path::{Path, PathBuf},
};

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use explorer::benchmark_support::{extract_archives, set_archive_diagnostics};

const FIXTURE_VERSION: &str = "archive-extraction-benchmark-v1";

struct Fixture {
    root: PathBuf,
    large: PathBuf,
    small: PathBuf,
    deep: PathBuf,
}

impl Fixture {
    fn get() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(FIXTURE_VERSION);
        let marker = root.join(".complete");
        let large = root.join("large.ar");
        let small = root.join("small.ar");
        let deep = root.join("deep.ar");
        if !marker.exists() {
            if root.exists() {
                fs::remove_dir_all(&root).expect("remove incomplete archive fixture");
            }
            fs::create_dir_all(&root).expect("create archive fixture");
            create_ar(
                &large,
                (0..1).map(|_| ("large.bin".to_owned(), vec![b'x'; 16 * 1024 * 1024])),
            );
            create_ar(
                &small,
                (0..2_000).map(|index| (format!("f{index:04}.txt"), vec![b'x'; 128])),
            );
            create_ar(
                &deep,
                (0..500).map(|index| (format!("d{index:04}.txt"), vec![b'x'; 4096])),
            );
            fs::write(&marker, FIXTURE_VERSION).expect("write archive fixture marker");
        }
        Self {
            root,
            large,
            small,
            deep,
        }
    }
}

fn create_ar(path: &Path, entries: impl IntoIterator<Item = (String, Vec<u8>)>) {
    let mut builder = ar::Builder::new(File::create(path).expect("create archive"));
    for (name, data) in entries {
        let header = ar::Header::new(name.into_bytes(), data.len() as u64);
        builder
            .append(&header, data.as_slice())
            .expect("append archive entry");
    }
    builder.into_inner().expect("finish archive");
}

fn archive_extraction_benchmarks(criterion: &mut Criterion) {
    let fixture = Fixture::get();
    let mut group = criterion.benchmark_group("archive_extraction");
    group.sample_size(10);

    for (name, archive, bytes) in [
        ("one_large_file", &fixture.large, 16 * 1024 * 1024_u64),
        ("many_small_files", &fixture.small, 2_000 * 128_u64),
        ("many_medium_files", &fixture.deep, 500 * 4096_u64),
    ] {
        group.throughput(Throughput::Bytes(bytes));
        group.bench_function(name, |bencher| {
            bencher.iter_batched(
                || {
                    let output = fixture.root.join(format!("output-{}", std::process::id()));
                    if output.exists() {
                        fs::remove_dir_all(&output).expect("remove prior benchmark output");
                    }
                    fs::create_dir(&output).expect("create benchmark output");
                    output
                },
                |output| {
                    extract_archives(black_box(std::slice::from_ref(archive)), black_box(&output));
                    fs::remove_dir_all(output).expect("remove benchmark output");
                },
                BatchSize::PerIteration,
            );
        });
    }

    group.finish();

    let mut overhead = criterion.benchmark_group("archive_extraction/diagnostics_overhead");
    overhead.sample_size(10);
    for (name, enabled, verbose) in [
        ("disabled", false, false),
        ("summary", true, false),
        ("verbose", true, true),
    ] {
        set_archive_diagnostics(enabled, verbose);
        overhead.bench_function(name, |bencher| {
            bencher.iter_batched(
                || {
                    let output = fixture
                        .root
                        .join(format!("diagnostics-{}", std::process::id()));
                    if output.exists() {
                        fs::remove_dir_all(&output).expect("remove prior diagnostics output");
                    }
                    fs::create_dir(&output).expect("create diagnostics output");
                    output
                },
                |output| {
                    extract_archives(
                        black_box(std::slice::from_ref(&fixture.deep)),
                        black_box(&output),
                    );
                    fs::remove_dir_all(output).expect("remove diagnostics output");
                },
                BatchSize::PerIteration,
            );
        });
    }
    set_archive_diagnostics(false, false);
    overhead.finish();
}

criterion_group!(benches, archive_extraction_benchmarks);
criterion_main!(benches);
