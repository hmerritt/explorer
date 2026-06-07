use std::{
    env,
    fs::{self, File},
    hint::black_box,
    path::{Path, PathBuf},
    time::Duration,
};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use explorer::benchmark_support::{cached_search, filter, scan, uncached_search};

const FIXTURE_VERSION: &str = "recursive-search-benchmark-v3";
const GROUP_COUNT: usize = 5;
const SECTION_COUNT: usize = 5;
const LEAF_COUNT: usize = 10;
const FILES_PER_LEAF: usize = 100;
const FILE_COUNT: usize = GROUP_COUNT * SECTION_COUNT * LEAF_COUNT * FILES_PER_LEAF;
const DIRECTORY_COUNT: usize =
    GROUP_COUNT + GROUP_COUNT * SECTION_COUNT + GROUP_COUNT * SECTION_COUNT * LEAF_COUNT;
const SCANNED_PATH_COUNT: usize = FILE_COUNT + DIRECTORY_COUNT;
const SPARSE_MATCH_COUNT: usize = FILE_COUNT / 1_000;
const DENSE_MATCH_COUNT: usize = FILE_COUNT / 10;

const NO_MATCH_QUERY: &str = "absent-token";
const SPARSE_QUERY: &str = "qxzsparse";
const DENSE_QUERY: &str = "jvkdense";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn get() -> Self {
        let target_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        let fixture_dir = target_dir.join(FIXTURE_VERSION);
        let root = fixture_dir.join("root");
        let marker = fixture_dir.join(".complete");

        if !fixture_is_complete(&marker) {
            let staging_dir = target_dir.join(format!(".{FIXTURE_VERSION}-{}", std::process::id()));
            if staging_dir.exists() {
                fs::remove_dir_all(&staging_dir).expect("remove stale benchmark fixture staging");
            }

            create_fixture(&staging_dir.join("root"));
            fs::write(staging_dir.join(".complete"), FIXTURE_VERSION)
                .expect("write benchmark fixture completion marker");

            if fixture_dir.exists() && !fixture_is_complete(&marker) {
                fs::remove_dir_all(&fixture_dir).expect("remove incomplete benchmark fixture");
            }
            if let Err(error) = fs::rename(&staging_dir, &fixture_dir) {
                if fixture_is_complete(&marker) {
                    fs::remove_dir_all(&staging_dir)
                        .expect("remove redundant benchmark fixture staging");
                } else {
                    panic!("publish benchmark fixture: {error}");
                }
            }
        }

        Self { root }
    }
}

fn fixture_is_complete(marker: &Path) -> bool {
    fs::read_to_string(marker).is_ok_and(|version| version == FIXTURE_VERSION)
}

fn measurement_time_overridden() -> bool {
    env::args_os().any(|argument| {
        let argument = argument.to_string_lossy();
        argument == "--measurement-time" || argument.starts_with("--measurement-time=")
    })
}

fn create_fixture(root: &Path) {
    for group in 0..GROUP_COUNT {
        for section in 0..SECTION_COUNT {
            for leaf in 0..LEAF_COUNT {
                let directory = root
                    .join(format!("group-{group:02}"))
                    .join(format!("section-{section:02}"))
                    .join(format!("leaf-{leaf:02}"));
                fs::create_dir_all(&directory).expect("create benchmark fixture directory");

                for file in 0..FILES_PER_LEAF {
                    let index = (((group * SECTION_COUNT + section) * LEAF_COUNT + leaf)
                        * FILES_PER_LEAF)
                        + file;
                    let name = if index % 10 == 0 {
                        format!("{DENSE_QUERY}-{index:06}.dat")
                    } else if index % 1_000 == 1 {
                        format!("{SPARSE_QUERY}-{index:06}.dat")
                    } else {
                        format!("regular-file-{index:06}.dat")
                    };
                    File::create(directory.join(name)).expect("create benchmark fixture file");
                }
            }
        }
    }
}

fn verify_fixture(root: &Path) {
    let paths = scan(root, false);
    assert_eq!(paths.len(), SCANNED_PATH_COUNT);
    assert_eq!(filter(&paths, NO_MATCH_QUERY).len(), 0);
    assert_eq!(filter(&paths, SPARSE_QUERY).len(), SPARSE_MATCH_COUNT);
    assert_eq!(filter(&paths, DENSE_QUERY).len(), DENSE_MATCH_COUNT);

    let sparse_cached = cached_search(&paths, SPARSE_QUERY);
    assert_eq!(sparse_cached.scanned_path_count(), SCANNED_PATH_COUNT);
    assert_eq!(sparse_cached.entry_count(), SPARSE_MATCH_COUNT);
    assert_eq!(
        cached_search(&paths, DENSE_QUERY).entry_count(),
        DENSE_MATCH_COUNT
    );

    let sparse_uncached = uncached_search(root, SPARSE_QUERY, false);
    assert_eq!(sparse_uncached.scanned_path_count(), SCANNED_PATH_COUNT);
    assert_eq!(sparse_uncached.entry_count(), SPARSE_MATCH_COUNT);

    eprintln!(
        "recursive index: values={} unique_ngrams={} postings={} posting_bytes={}",
        paths.len(),
        paths.ngram_count(),
        paths.posting_count(),
        paths.posting_bytes()
    );
}

fn recursive_search_benchmarks(criterion: &mut Criterion) {
    let fixture = Fixture::get();
    verify_fixture(&fixture.root);
    let measurement_time_overridden = measurement_time_overridden();

    let mut scan_group = criterion.benchmark_group("recursive_search/initial_materializing_scan");
    scan_group.sample_size(10);
    if !measurement_time_overridden {
        scan_group.measurement_time(Duration::from_secs(10));
    }
    scan_group.throughput(Throughput::Elements(SCANNED_PATH_COUNT as u64));
    scan_group.bench_function("hidden_files_off", |bencher| {
        bencher.iter(|| black_box(scan(black_box(&fixture.root), false)));
    });
    scan_group.finish();

    let cached_paths = scan(&fixture.root, false);
    let mut filter_group = criterion.benchmark_group("recursive_search/cached_filter");
    filter_group.sample_size(20);
    if !measurement_time_overridden {
        filter_group.measurement_time(Duration::from_secs(5));
    }
    filter_group.throughput(Throughput::Elements(SCANNED_PATH_COUNT as u64));
    for (query_name, query) in [
        ("no_match", NO_MATCH_QUERY),
        ("sparse_match", SPARSE_QUERY),
        ("dense_match", DENSE_QUERY),
    ] {
        filter_group.bench_with_input(
            BenchmarkId::from_parameter(query_name),
            query,
            |bencher, query| {
                bencher.iter(|| black_box(filter(black_box(&cached_paths), black_box(query))));
            },
        );
    }
    filter_group.finish();

    let mut cached_search_group = criterion.benchmark_group("recursive_search/cached_full_search");
    cached_search_group.sample_size(10);
    if !measurement_time_overridden {
        cached_search_group.measurement_time(Duration::from_secs(10));
    }
    cached_search_group.throughput(Throughput::Elements(SCANNED_PATH_COUNT as u64));
    for (query_name, query) in [("sparse_match", SPARSE_QUERY), ("dense_match", DENSE_QUERY)] {
        cached_search_group.bench_with_input(
            BenchmarkId::from_parameter(query_name),
            query,
            |bencher, query| {
                bencher
                    .iter(|| black_box(cached_search(black_box(&cached_paths), black_box(query))));
            },
        );
    }
    cached_search_group.finish();

    let mut uncached_search_group =
        criterion.benchmark_group("recursive_search/uncached_full_search");
    uncached_search_group.sample_size(10);
    if !measurement_time_overridden {
        uncached_search_group.measurement_time(Duration::from_secs(10));
    }
    uncached_search_group.throughput(Throughput::Elements(SCANNED_PATH_COUNT as u64));
    uncached_search_group.bench_function("sparse_match", |bencher| {
        bencher.iter(|| {
            black_box(uncached_search(
                black_box(&fixture.root),
                black_box(SPARSE_QUERY),
                false,
            ))
        });
    });
    uncached_search_group.finish();
}

criterion_group!(benches, recursive_search_benchmarks);
criterion_main!(benches);
