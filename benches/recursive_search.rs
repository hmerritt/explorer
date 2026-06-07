use std::{
    env,
    fs::{self, File},
    hint::black_box,
    path::{Path, PathBuf},
    time::Duration,
};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use explorer::benchmark_support::{
    cached_search, cancelled_materialize, filter, materialize, scan, uncached_search,
};

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
                fs::remove_dir_all(&staging_dir).expect("remove stale benchmark staging");
            }

            create_fixture(&staging_dir.join("root"));
            fs::write(staging_dir.join(".complete"), FIXTURE_VERSION)
                .expect("write benchmark completion marker");

            if fixture_dir.exists() && !fixture_is_complete(&marker) {
                fs::remove_dir_all(&fixture_dir).expect("remove incomplete benchmark fixture");
            }
            if let Err(error) = fs::rename(&staging_dir, &fixture_dir) {
                if fixture_is_complete(&marker) {
                    fs::remove_dir_all(&staging_dir).expect("remove redundant benchmark staging");
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

fn create_fixture(root: &Path) {
    for group in 0..GROUP_COUNT {
        for section in 0..SECTION_COUNT {
            for leaf in 0..LEAF_COUNT {
                let directory = root
                    .join(format!("group-{group:02}"))
                    .join(format!("section-{section:02}"))
                    .join(format!("leaf-{leaf:02}"));
                fs::create_dir_all(&directory).expect("create benchmark directory");

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
                    File::create(directory.join(name)).expect("create benchmark file");
                }
            }
        }
    }
}

fn recursive_search_benchmarks(criterion: &mut Criterion) {
    let fixture = Fixture::get();
    let cached_paths = scan(&fixture.root, false);
    assert_eq!(cached_paths.len(), SCANNED_PATH_COUNT);

    let sparse = filter(&cached_paths, SPARSE_QUERY);
    let dense = filter(&cached_paths, DENSE_QUERY);
    assert_eq!(filter(&cached_paths, NO_MATCH_QUERY).len(), 0);
    assert_eq!(sparse.len(), SPARSE_MATCH_COUNT);
    assert_eq!(dense.len(), DENSE_MATCH_COUNT);
    assert_eq!(
        materialize(&cached_paths, &sparse).len(),
        SPARSE_MATCH_COUNT
    );
    assert_eq!(materialize(&cached_paths, &dense).len(), DENSE_MATCH_COUNT);

    let mut scan_group = criterion.benchmark_group("recursive_search/scan");
    scan_group.sample_size(10);
    scan_group.measurement_time(Duration::from_secs(10));
    scan_group.throughput(Throughput::Elements(SCANNED_PATH_COUNT as u64));
    scan_group.bench_function("hidden_files_off", |bencher| {
        bencher.iter(|| black_box(scan(black_box(&fixture.root), false)));
    });
    scan_group.finish();

    let mut filter_group = criterion.benchmark_group("recursive_search/cached_filter");
    filter_group.measurement_time(Duration::from_secs(5));
    filter_group.throughput(Throughput::Elements(SCANNED_PATH_COUNT as u64));
    for (name, query) in [
        ("no_match", NO_MATCH_QUERY),
        ("sparse_match", SPARSE_QUERY),
        ("dense_match", DENSE_QUERY),
    ] {
        filter_group.bench_with_input(
            BenchmarkId::from_parameter(name),
            query,
            |bencher, query| {
                bencher.iter(|| black_box(filter(black_box(&cached_paths), black_box(query))));
            },
        );
    }
    filter_group.finish();

    let mut materialize_group = criterion.benchmark_group("recursive_search/materialize");
    materialize_group.measurement_time(Duration::from_secs(5));
    for (name, filtered) in [("sparse_match", &sparse), ("dense_match", &dense)] {
        materialize_group.bench_with_input(
            BenchmarkId::from_parameter(name),
            filtered,
            |bencher, filtered| {
                bencher.iter(|| black_box(materialize(black_box(&cached_paths), filtered)));
            },
        );
    }
    materialize_group.finish();

    let mut cached_group = criterion.benchmark_group("recursive_search/cached_full_search");
    cached_group.sample_size(10);
    cached_group.measurement_time(Duration::from_secs(10));
    for (name, query) in [("sparse_match", SPARSE_QUERY), ("dense_match", DENSE_QUERY)] {
        cached_group.bench_with_input(
            BenchmarkId::from_parameter(name),
            query,
            |bencher, query| {
                bencher
                    .iter(|| black_box(cached_search(black_box(&cached_paths), black_box(query))));
            },
        );
    }
    cached_group.finish();

    let mut uncached_group = criterion.benchmark_group("recursive_search/uncached_full_search");
    uncached_group.sample_size(10);
    uncached_group.measurement_time(Duration::from_secs(10));
    uncached_group.bench_function("sparse_match", |bencher| {
        bencher.iter(|| {
            black_box(uncached_search(
                black_box(&fixture.root),
                black_box(SPARSE_QUERY),
                false,
            ))
        });
    });
    uncached_group.finish();

    let mut cancellation_group = criterion.benchmark_group("recursive_search/cancellation");
    cancellation_group.bench_function("dense_materialization_pre_cancelled", |bencher| {
        bencher.iter(|| {
            black_box(cancelled_materialize(
                black_box(&cached_paths),
                black_box(&dense),
            ))
        });
    });
    cancellation_group.finish();

    assert_eq!(
        cached_search(&cached_paths, SPARSE_QUERY).entry_count(),
        SPARSE_MATCH_COUNT
    );
    assert_eq!(
        uncached_search(&fixture.root, SPARSE_QUERY, false).scanned_path_count(),
        SCANNED_PATH_COUNT
    );
}

criterion_group!(benches, recursive_search_benchmarks);
criterion_main!(benches);
