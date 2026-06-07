# Benchmarks

The recursive-search benchmark suite measures the initial materializing scan,
cached entry filtering, cached full search, and uncached end-to-end search
separately.

The ngram benchmark suite measures in-memory builder/finalization throughput,
cold searches, repeated identical searches, and incremental typing sequences.
Both suites report payload, unique trigram, posting, and compact posting-byte
statistics.

Run it with:

```sh
cargo bench --features benchmarks --bench recursive_search
cargo bench --features benchmarks --bench ngram
```

The first run creates a deterministic fixture containing 25,000 files under
`target/recursive-search-benchmark-v3/root`. Fixture creation and
validation happen before timed measurements. A versioned completion marker
beside the root allows later runs to reuse the fixture and causes incomplete or
outdated fixtures to be rebuilt. Fixture creation uses a process-specific
staging directory and atomically publishes it, so concurrent benchmark runs
cannot observe a partially created fixture.

A normal full-suite run should take about 90 seconds after the fixture and
release benchmark binary exist.

For reliable comparisons, run benchmarks on the same idle machine and
filesystem. Results from different operating systems, storage devices, power
profiles, or filesystem cache states are not directly comparable.

Save a baseline before making a performance change:

```sh
cargo bench --features benchmarks --bench recursive_search -- --save-baseline before
```

Compare the changed implementation against it:

```sh
cargo bench --features benchmarks --bench recursive_search -- --baseline before
```

For a higher-confidence run, override Criterion's measurement time:

```sh
cargo bench --features benchmarks --bench recursive_search -- --measurement-time 30
```

To rebuild the fixture, remove `target/recursive-search-benchmark-v3` before the
next run.
