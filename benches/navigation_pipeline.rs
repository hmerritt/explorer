use std::{
    fs::{self, File},
    hint::black_box,
    path::{Path, PathBuf},
};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use explorer::benchmark_support::load_entries;

const FIXTURE_VERSION: &str = "navigation-pipeline-benchmark-v1";
const VISIBLE_ENTRIES: &[&str] = &["business", "invoices", "notes.txt", "personal", "todo.md"];
const HIDDEN_ENTRIES: &[&str] = &[
    ".cache",
    ".DS_Store",
    ".gitkeep",
    ".localized",
    ".recent",
    ".settings",
    ".sync",
    ".thumbs",
    ".tmp",
    ".trash",
    "__MACOSX",
];

fn ensure_documents_fixture() -> PathBuf {
    let fixture_dir = PathBuf::from("target").join(FIXTURE_VERSION);
    let marker = fixture_dir.join(".complete");
    if marker.exists() {
        return fixture_dir;
    }

    if fixture_dir.exists() {
        fs::remove_dir_all(&fixture_dir).expect("remove incomplete benchmark fixture");
    }
    fs::create_dir_all(&fixture_dir).expect("create benchmark fixture");

    for name in VISIBLE_ENTRIES {
        create_entry(&fixture_dir, name);
    }
    for name in HIDDEN_ENTRIES {
        create_entry(&fixture_dir, name);
    }

    fs::write(marker, b"complete").expect("write benchmark marker");
    fixture_dir
}

fn create_entry(root: &Path, name: &str) {
    let path = root.join(name);
    if matches!(
        name,
        "business" | "invoices" | "personal" | ".cache" | "__MACOSX"
    ) {
        fs::create_dir(&path).expect("create benchmark directory");
    } else {
        File::create(&path).expect("create benchmark file");
    }
}

fn navigation_pipeline_benchmarks(criterion: &mut Criterion) {
    let fixture = ensure_documents_fixture();
    let mut group = criterion.benchmark_group("navigation_pipeline/load_entries");
    group.throughput(Throughput::Elements(
        (VISIBLE_ENTRIES.len() + HIDDEN_ENTRIES.len()) as u64,
    ));

    group.bench_function("documents_small_hidden_off", |bencher| {
        bencher.iter(|| black_box(load_entries(black_box(&fixture), false)));
    });

    group.finish();
}

criterion_group!(benches, navigation_pipeline_benchmarks);
criterion_main!(benches);
