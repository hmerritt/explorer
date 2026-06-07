use std::{hint::black_box, time::Duration};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use explorer::ngram_benchmark_support::{NgramIndex, NgramIndexBuilder, NgramSearchSession};

const FILE_COUNT: usize = 25_000;
const SPARSE_MATCH_COUNT: usize = FILE_COUNT / 1_000;
const DENSE_MATCH_COUNT: usize = FILE_COUNT / 10;

const NO_MATCH_QUERY: &str = "absent-token";
const SPARSE_QUERY: &str = "qxzsparse";
const DENSE_QUERY: &str = "jvkdense";

fn filenames() -> Vec<String> {
    (0..FILE_COUNT)
        .map(|index| {
            if index % 10 == 0 {
                format!("{DENSE_QUERY}-{index:06}.dat")
            } else if index % 1_000 == 1 {
                format!("{SPARSE_QUERY}-{index:06}.dat")
            } else {
                format!("regular-file-{index:06}.dat")
            }
        })
        .collect()
}

fn build_index(filenames: &[String]) -> NgramIndex<usize> {
    let mut builder = NgramIndexBuilder::new();
    for (id, filename) in filenames.iter().enumerate() {
        builder.add(filename, id);
    }
    builder.finish()
}

fn result_count(index: &NgramIndex<usize>, query: &str) -> usize {
    NgramSearchSession::new().search(index, query).len()
}

fn ngram_benchmarks(criterion: &mut Criterion) {
    let filenames = filenames();
    let index = build_index(&filenames);
    assert_eq!(index.len(), FILE_COUNT);
    assert_eq!(result_count(&index, NO_MATCH_QUERY), 0);
    assert_eq!(result_count(&index, SPARSE_QUERY), SPARSE_MATCH_COUNT);
    assert_eq!(result_count(&index, DENSE_QUERY), DENSE_MATCH_COUNT);
    eprintln!(
        "ngram index: values={} unique_ngrams={} postings={} posting_bytes={}",
        index.len(),
        index.ngram_count(),
        index.posting_count(),
        index.posting_bytes()
    );

    let mut build_group = criterion.benchmark_group("ngram/builder_and_finalization");
    build_group.sample_size(10);
    build_group.measurement_time(Duration::from_secs(5));
    build_group.throughput(Throughput::Elements(FILE_COUNT as u64));
    build_group.bench_function("25k_filenames", |bencher| {
        bencher.iter_batched(
            || filenames.clone(),
            |filenames| black_box(build_index(black_box(&filenames))),
            BatchSize::LargeInput,
        );
    });
    build_group.finish();

    let mut search_group = criterion.benchmark_group("ngram/cold_independent_search");
    search_group.sample_size(20);
    search_group.measurement_time(Duration::from_secs(5));
    search_group.throughput(Throughput::Elements(FILE_COUNT as u64));
    for (name, query) in [
        ("no_match", NO_MATCH_QUERY),
        ("sparse_match", SPARSE_QUERY),
        ("dense_match", DENSE_QUERY),
    ] {
        search_group.bench_with_input(
            BenchmarkId::from_parameter(name),
            query,
            |bencher, query| {
                bencher.iter_batched(
                    NgramSearchSession::new,
                    |mut session| black_box(session.search(&index, black_box(query)).len()),
                    BatchSize::SmallInput,
                );
            },
        );
    }
    search_group.finish();

    let mut repeated_group = criterion.benchmark_group("ngram/repeated_identical_search");
    repeated_group.sample_size(20);
    repeated_group.measurement_time(Duration::from_secs(5));
    repeated_group.throughput(Throughput::Elements(FILE_COUNT as u64));
    for (name, query) in [
        ("no_match", NO_MATCH_QUERY),
        ("sparse_match", SPARSE_QUERY),
        ("dense_match", DENSE_QUERY),
    ] {
        repeated_group.bench_with_input(
            BenchmarkId::from_parameter(name),
            query,
            |bencher, query| {
                let mut session = NgramSearchSession::new();
                session.search(&index, query);
                bencher.iter(|| black_box(session.search(&index, black_box(query)).len()));
            },
        );
    }
    repeated_group.finish();

    let append_queries = ["qxz", "qxzs", "qxzsp", "qxzspa", "qxzspar", "qxzsparse"];
    let append_backspace_queries = [
        "qxz",
        "qxzs",
        "qxzsp",
        "qxzspa",
        "qxzspar",
        "qxzsparse",
        "qxzspar",
        "qxzspa",
        "qxzsp",
        "qxzs",
        "qxz",
    ];
    let mut incremental_group = criterion.benchmark_group("ngram/incremental_typing");
    incremental_group.sample_size(20);
    incremental_group.measurement_time(Duration::from_secs(5));
    for (name, queries) in [
        ("append", append_queries.as_slice()),
        ("append_and_backspace", append_backspace_queries.as_slice()),
    ] {
        incremental_group.bench_with_input(
            BenchmarkId::from_parameter(name),
            queries,
            |bencher, queries| {
                let mut session = NgramSearchSession::new();
                bencher.iter(|| {
                    session.reset();
                    for query in queries.iter() {
                        black_box(session.search(&index, black_box(query)).len());
                    }
                });
            },
        );
    }
    incremental_group.finish();
}

criterion_group!(benches, ngram_benchmarks);
criterion_main!(benches);
