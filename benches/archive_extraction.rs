use std::{
    fs::{self, File},
    hint::black_box,
    io::Write,
    path::{Path, PathBuf},
};

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use explorer::benchmark_support::{
    execute_prepared_archive_extraction, extract_archives, extract_archives_with_progress,
    list_archive, plan_archives, prepare_archive_extraction, set_archive_diagnostics,
};

const FIXTURE_VERSION: &str = "archive-extraction-benchmark-v2";

struct Fixture {
    root: PathBuf,
    large: PathBuf,
    small: PathBuf,
    deep: PathBuf,
    zip: PathBuf,
    tar_gz: PathBuf,
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
        let zip = root.join("medium.zip");
        let tar_gz = root.join("medium.tar.gz");
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
            create_zip(
                &zip,
                (0..500).map(|index| (format!("z{index:04}.txt"), vec![b'x'; 4096])),
            );
            create_tar_gz(
                &tar_gz,
                (0..500).map(|index| (format!("t{index:04}.txt"), vec![b'x'; 4096])),
            );
            fs::write(&marker, FIXTURE_VERSION).expect("write archive fixture marker");
        }
        Self {
            root,
            large,
            small,
            deep,
            zip,
            tar_gz,
        }
    }
}

fn create_zip(path: &Path, entries: impl IntoIterator<Item = (String, Vec<u8>)>) {
    let mut writer = zip::ZipWriter::new(File::create(path).expect("create zip archive"));
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in entries {
        writer.start_file(name, options).expect("start zip entry");
        writer.write_all(&data).expect("write zip entry");
    }
    writer.finish().expect("finish zip archive");
}

fn create_tar_gz(path: &Path, entries: impl IntoIterator<Item = (String, Vec<u8>)>) {
    let encoder = flate2::write::GzEncoder::new(
        File::create(path).expect("create tar.gz archive"),
        flate2::Compression::fast(),
    );
    let mut builder = tar::Builder::new(encoder);
    for (name, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, name, data.as_slice())
            .expect("append tar entry");
    }
    builder
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gzip");
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

    let mut formats = criterion.benchmark_group("archive_extraction/formats");
    formats.sample_size(10);
    for (name, archive) in [
        ("ar_500", &fixture.deep),
        ("zip_500", &fixture.zip),
        ("tar_gz_500", &fixture.tar_gz),
    ] {
        formats.bench_function(name, |bencher| {
            bencher.iter_batched(
                || {
                    let output = fixture.root.join(format!("format-{}", std::process::id()));
                    if output.exists() {
                        fs::remove_dir_all(&output).expect("remove prior format output");
                    }
                    fs::create_dir(&output).expect("create format output");
                    output
                },
                |output| {
                    extract_archives(black_box(std::slice::from_ref(archive)), black_box(&output));
                    fs::remove_dir_all(output).expect("remove format output");
                },
                BatchSize::PerIteration,
            );
        });
    }
    formats.finish();

    let mut stages = criterion.benchmark_group("archive_extraction/stages");
    stages.sample_size(10);
    stages.bench_function("listing_ar_2000", |bencher| {
        bencher.iter(|| black_box(list_archive(black_box(&fixture.small))));
    });
    stages.bench_function("planning_ar_2000", |bencher| {
        bencher.iter_batched(
            || {
                let output = fixture.root.join(format!("plan-{}", std::process::id()));
                if output.exists() {
                    fs::remove_dir_all(&output).expect("remove prior planning output");
                }
                fs::create_dir(&output).expect("create planning output");
                output
            },
            |output| {
                black_box(plan_archives(
                    black_box(std::slice::from_ref(&fixture.small)),
                    black_box(&output),
                ));
                fs::remove_dir_all(output).expect("remove planning output");
            },
            BatchSize::PerIteration,
        );
    });
    stages.bench_function("execution_ar_500", |bencher| {
        bencher.iter_batched(
            || {
                let output = fixture.root.join(format!("execute-{}", std::process::id()));
                if output.exists() {
                    fs::remove_dir_all(&output).expect("remove prior execution output");
                }
                fs::create_dir(&output).expect("create execution output");
                let prepared =
                    prepare_archive_extraction(std::slice::from_ref(&fixture.deep), &output);
                (output, prepared)
            },
            |(output, prepared)| {
                execute_prepared_archive_extraction(prepared);
                fs::remove_dir_all(output).expect("remove execution output");
            },
            BatchSize::PerIteration,
        );
    });
    stages.bench_function("progress_ar_500", |bencher| {
        bencher.iter_batched(
            || {
                let output = fixture
                    .root
                    .join(format!("progress-{}", std::process::id()));
                if output.exists() {
                    fs::remove_dir_all(&output).expect("remove prior progress output");
                }
                fs::create_dir(&output).expect("create progress output");
                output
            },
            |output| {
                black_box(extract_archives_with_progress(
                    black_box(std::slice::from_ref(&fixture.deep)),
                    black_box(&output),
                ));
                fs::remove_dir_all(output).expect("remove progress output");
            },
            BatchSize::PerIteration,
        );
    });
    stages.finish();

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
