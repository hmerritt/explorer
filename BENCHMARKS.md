# Benchmarks

The recursive-search benchmark suite measures scanning, cached filtering,
metadata materialization, cached and uncached full searches, and cancellation.
The navigation-pipeline benchmark measures basic directory entry loading for a
small Documents-like folder with hidden entries. The image-thumbnail benchmark
measures cold thumbnail extraction for large raster/SVG/TIFF files and parallel
JPEG batch extraction. The image-viewer benchmark measures native-resolution
opens, deferred ICC correction, and `RenderImage` construction. The properties
benchmark measures fast directory properties snapshots separately from exact
recursive totals.

Run it with:

```sh
cargo bench --features benchmarks --bench recursive_search
cargo bench --features benchmarks --bench navigation_pipeline
cargo bench --features benchmarks --bench image_thumbnails
cargo bench --features benchmarks --bench image_viewer
cargo bench --features benchmarks --bench properties
cargo bench --features benchmarks --bench resumable_copy
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

The image-thumbnail benchmark measures isolated Bilinear RGBA resizing,
ready-for-display extraction at 128px and 400px, PNG disk-cache decoding, full
PNG-encoded cache generation, and parallel JPEG batches. Fixtures cover opaque
and transparent PNG, JPEG (including 12MP), TIFF, WebP, and SVG under
`target/image-thumbnails-benchmark-v4`.

The image-viewer benchmark compares ICC-tagged native opens with synchronous
ICC, deferred first-ready opens, and ICC ignored; it also measures no-ICC native
opens, deferred ICC correction plus corrected `RenderImage` construction, and
`RenderImage` construction alone. Fixtures cover PNG, JPEG, TIFF, WebP, SVG,
and Display P3 ICC-tagged PNG/JPEG under `target/image-viewer-benchmark-v1`.

```sh
cargo bench --features benchmarks --bench image_viewer
```

Use the release profile when comparing shipped application performance:

```sh
cargo bench --profile release --features benchmarks --bench image_thumbnails
```

The properties benchmark creates a deterministic large directory fixture under
`target/properties-benchmark-v1`.

The resumable-copy benchmark creates deterministic large-file, many-small-file,
same-size edit, shifted insert, and cancel/resume fixtures under
`target/resumable-copy-benchmark-v1`.

## Archive extraction

The archive-extraction suite measures one large file, many small files, and
many medium files through the same planning and execution pipeline used by the
application. It also compares AR, ZIP, and compressed TAR extraction and
isolates listing, planning, and progress-publication stages. Fixtures live
under `target/archive-extraction-benchmark-v2`.

```sh
cargo bench --features benchmarks --bench archive_extraction
cargo bench --features benchmarks --bench archive_extraction -- --save-baseline before
cargo bench --features benchmarks --bench archive_extraction -- --baseline before
```

Runtime diagnostics are JSONL on stderr:

```sh
cargo run -- --debug=archive
cargo run -- --debug=archive-verbose
```

Summary mode redacts archive and entry paths. Verbose mode additionally emits
path-bearing `slow_entry` records.
