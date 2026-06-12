# Benchmarks

The recursive-search benchmark suite measures scanning, cached filtering,
metadata materialization, cached and uncached full searches, and cancellation.
The navigation-pipeline benchmark measures basic directory entry loading for a
small Documents-like folder with hidden entries.

Run it with:

```sh
cargo bench --features benchmarks --bench recursive_search
cargo bench --features benchmarks --bench navigation_pipeline
```

The first run creates a deterministic 25,000-file fixture under
`target/recursive-search-benchmark-v3`. Save a baseline before changing the
pipeline and compare against it:

```sh
cargo bench --features benchmarks --bench recursive_search -- --save-baseline before
cargo bench --features benchmarks --bench recursive_search -- --baseline before
```

The navigation benchmark creates its fixture under
`target/navigation-pipeline-benchmark-v1`.
