# Benchmarks

The recursive-search benchmark suite measures scanning, cached filtering,
metadata materialization, cached and uncached full searches, and cancellation.

Run it with:

```sh
cargo bench --features benchmarks --bench recursive_search
```

The first run creates a deterministic 25,000-file fixture under
`target/recursive-search-benchmark-v3`. Save a baseline before changing the
pipeline and compare against it:

```sh
cargo bench --features benchmarks --bench recursive_search -- --save-baseline before
cargo bench --features benchmarks --bench recursive_search -- --baseline before
```
