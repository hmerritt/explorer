use std::{
    fs::{self, File},
    hint::black_box,
    path::{Path, PathBuf},
};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use explorer::benchmark_support::{collect_properties_fast, collect_properties_full};

const FIXTURE_VERSION: &str = "properties-benchmark-v1";
const GROUP_COUNT: usize = 6;
const SECTION_COUNT: usize = 6;
const LEAF_COUNT: usize = 20;
const FILES_PER_LEAF: usize = 20;
const FILE_COUNT: usize = GROUP_COUNT * SECTION_COUNT * LEAF_COUNT * FILES_PER_LEAF;
const DIRECTORY_COUNT: usize =
    GROUP_COUNT + GROUP_COUNT * SECTION_COUNT + GROUP_COUNT * SECTION_COUNT * LEAF_COUNT;

fn ensure_properties_fixture() -> PathBuf {
    let fixture_dir = PathBuf::from("target").join(FIXTURE_VERSION);
    let root = fixture_dir.join("root");
    let marker = fixture_dir.join(".complete");
    if fixture_is_complete(&marker) {
        return root;
    }

    if fixture_dir.exists() {
        fs::remove_dir_all(&fixture_dir).expect("remove incomplete benchmark fixture");
    }
    fs::create_dir_all(&root).expect("create benchmark root");
    create_properties_fixture(&root);
    fs::write(marker, FIXTURE_VERSION).expect("write benchmark marker");
    root
}

fn fixture_is_complete(marker: &Path) -> bool {
    fs::read_to_string(marker).is_ok_and(|version| version == FIXTURE_VERSION)
}

fn create_properties_fixture(root: &Path) {
    for group in 0..GROUP_COUNT {
        for section in 0..SECTION_COUNT {
            for leaf in 0..LEAF_COUNT {
                let directory = root
                    .join(format!("group-{group:02}"))
                    .join(format!("section-{section:02}"))
                    .join(format!("leaf-{leaf:02}"));
                fs::create_dir_all(&directory).expect("create benchmark directory");
                for file in 0..FILES_PER_LEAF {
                    let path = directory.join(format!("file-{file:02}.txt"));
                    File::create(path).expect("create benchmark file");
                }
            }
        }
    }
}

fn properties_benchmarks(criterion: &mut Criterion) {
    let fixture = ensure_properties_fixture();
    let item_count = (FILE_COUNT + DIRECTORY_COUNT) as u64;
    let mut group = criterion.benchmark_group("properties/directory_snapshot");
    group.throughput(Throughput::Elements(item_count));
    group.sample_size(10);

    group.bench_function("fast_root_metadata", |bencher| {
        bencher.iter(|| black_box(collect_properties_fast(black_box(&fixture))));
    });

    group.bench_function("full_recursive_totals", |bencher| {
        bencher.iter(|| black_box(collect_properties_full(black_box(&fixture))));
    });

    group.finish();
}

criterion_group!(benches, properties_benchmarks);
criterion_main!(benches);
