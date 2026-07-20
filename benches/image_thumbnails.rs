use std::{
    fs,
    hint::black_box,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use explorer::benchmark_support::{
    encode_cached_thumbnail_for_benchmark, load_image_thumbnail_ready_for_benchmark,
    prepare_cached_thumbnail_for_benchmark, queue_and_cancel_thumbnails_for_benchmark,
    resize_rgba_for_benchmark, write_cached_thumbnail_batch_for_benchmark,
};

const FIXTURE_VERSION: &str = "image-thumbnails-benchmark-v6";
const THUMBNAIL_SIZE: u32 = 128;
const LARGE_WIDTH: u32 = 1600;
const LARGE_HEIGHT: u32 = 1200;
const WIDE_TIFF_WIDTH: u32 = 400_000;
const WIDE_TIFF_HEIGHT: u32 = 2;
const PHOTO_TIFF_WIDTH: u32 = 4_000;
const PHOTO_TIFF_HEIGHT: u32 = 3_000;
const HUGE_TIFF_WIDTH: u32 = 8_000;
const HUGE_TIFF_HEIGHT: u32 = 6_000;
const BATCH_COUNT: usize = 32;

struct Fixture {
    large_png: PathBuf,
    large_transparent_png: PathBuf,
    large_jpeg: PathBuf,
    photo_jpeg: PathBuf,
    large_tiff: PathBuf,
    large_deflate_tiff: PathBuf,
    photo_lzw_tiff: PathBuf,
    huge_deflate_tiff: PathBuf,
    wide_tiff: PathBuf,
    large_webp: PathBuf,
    large_svg: PathBuf,
    batch_jpegs: Vec<PathBuf>,
}

impl Fixture {
    fn get() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(FIXTURE_VERSION);
        let marker = root.join(".complete");
        if !marker.exists() {
            if root.exists() {
                fs::remove_dir_all(&root).expect("remove incomplete thumbnail fixture");
            }
            fs::create_dir_all(&root).expect("create thumbnail fixture");
            create_png(&root.join("large.png"), LARGE_WIDTH, LARGE_HEIGHT);
            create_transparent_png(
                &root.join("large-transparent.png"),
                LARGE_WIDTH,
                LARGE_HEIGHT,
            );
            create_jpeg(&root.join("large.jpg"), LARGE_WIDTH, LARGE_HEIGHT, 0);
            create_jpeg(&root.join("photo-12mp.jpg"), 4000, 3000, 1);
            create_tiff(&root.join("large.tif"), LARGE_WIDTH, LARGE_HEIGHT);
            create_deflate_tiff(&root.join("large-deflate.tif"), LARGE_WIDTH, LARGE_HEIGHT);
            create_compressed_tiff(
                &root.join("photo-lzw.tif"),
                PHOTO_TIFF_WIDTH,
                PHOTO_TIFF_HEIGHT,
                tiff::encoder::Compression::Lzw,
            );
            create_compressed_tiff(
                &root.join("huge-deflate.tif"),
                HUGE_TIFF_WIDTH,
                HUGE_TIFF_HEIGHT,
                tiff::encoder::Compression::Deflate(tiff::encoder::DeflateLevel::Fast),
            );
            create_tiff(&root.join("wide.tif"), WIDE_TIFF_WIDTH, WIDE_TIFF_HEIGHT);
            create_webp(&root.join("large.webp"), LARGE_WIDTH, LARGE_HEIGHT);
            create_svg(&root.join("large.svg"));
            for index in 0..BATCH_COUNT {
                create_jpeg(
                    &root.join(format!("batch-{index:02}.jpg")),
                    LARGE_WIDTH,
                    LARGE_HEIGHT,
                    index as u8,
                );
            }
            fs::write(&marker, FIXTURE_VERSION).expect("write thumbnail fixture marker");
        }

        Self {
            large_png: root.join("large.png"),
            large_transparent_png: root.join("large-transparent.png"),
            large_jpeg: root.join("large.jpg"),
            photo_jpeg: root.join("photo-12mp.jpg"),
            large_tiff: root.join("large.tif"),
            large_deflate_tiff: root.join("large-deflate.tif"),
            photo_lzw_tiff: root.join("photo-lzw.tif"),
            huge_deflate_tiff: root.join("huge-deflate.tif"),
            wide_tiff: root.join("wide.tif"),
            large_webp: root.join("large.webp"),
            large_svg: root.join("large.svg"),
            batch_jpegs: (0..BATCH_COUNT)
                .map(|index| root.join(format!("batch-{index:02}.jpg")))
                .collect(),
        }
    }
}

fn create_transparent_png(path: &Path, width: u32, height: u32) {
    let image = image::DynamicImage::ImageRgba8(transparent_gradient_rgba(width, height));
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encode transparent benchmark png");
    fs::write(path, bytes).expect("write transparent benchmark png");
}

fn create_webp(path: &Path, width: u32, height: u32) {
    let image = image::DynamicImage::ImageRgb8(gradient_rgb(width, height, 0));
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::WebP,
        )
        .expect("encode benchmark webp");
    fs::write(path, bytes).expect("write benchmark webp");
}

fn create_png(path: &Path, width: u32, height: u32) {
    let image = image::DynamicImage::ImageRgba8(gradient_rgba(width, height, 0));
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encode benchmark png");
    fs::write(path, bytes).expect("write benchmark png");
}

fn create_jpeg(path: &Path, width: u32, height: u32, seed: u8) {
    let image = image::DynamicImage::ImageRgb8(gradient_rgb(width, height, seed));
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Jpeg,
        )
        .expect("encode benchmark jpeg");
    fs::write(path, bytes).expect("write benchmark jpeg");
}

fn create_tiff(path: &Path, width: u32, height: u32) {
    let image = gradient_rgb(width, height, 0);
    let mut bytes = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut bytes);
        let mut encoder = tiff::encoder::TiffEncoder::new(cursor).expect("create tiff encoder");
        let mut image_encoder = encoder
            .new_image::<tiff::encoder::colortype::RGB8>(width, height)
            .expect("create benchmark tiff image");
        image_encoder
            .rows_per_strip(16)
            .expect("set benchmark tiff strip size");
        image_encoder
            .write_data(image.as_raw())
            .expect("encode benchmark tiff");
    }
    fs::write(path, bytes).expect("write benchmark tiff");
}

fn create_deflate_tiff(path: &Path, width: u32, height: u32) {
    create_compressed_tiff(
        path,
        width,
        height,
        tiff::encoder::Compression::Deflate(tiff::encoder::DeflateLevel::Fast),
    );
}

fn create_compressed_tiff(
    path: &Path,
    width: u32,
    height: u32,
    compression: tiff::encoder::Compression,
) {
    let image = gradient_rgb(width, height, 2);
    let mut bytes = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut bytes);
        let mut encoder = tiff::encoder::TiffEncoder::new(cursor)
            .expect("create compressed tiff encoder")
            .with_compression(compression);
        let mut image_encoder = encoder
            .new_image::<tiff::encoder::colortype::RGB8>(width, height)
            .expect("create compressed benchmark tiff image");
        image_encoder
            .rows_per_strip(16)
            .expect("set compressed benchmark tiff strip size");
        image_encoder
            .write_data(image.as_raw())
            .expect("encode compressed benchmark tiff");
    }
    fs::write(path, bytes).expect("write compressed benchmark tiff");
}

fn create_svg(path: &Path) {
    fs::write(
        path,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="1200"><defs><linearGradient id="g" x1="0" x2="1" y1="0" y2="1"><stop offset="0" stop-color="#e23d3d"/><stop offset="0.5" stop-color="#2f8fdb"/><stop offset="1" stop-color="#f2d54b"/></linearGradient></defs><rect width="1600" height="1200" fill="url(#g)"/><circle cx="1100" cy="420" r="260" fill="#ffffff" fill-opacity="0.4"/><path d="M 120 940 C 420 680 640 1120 940 800 S 1380 780 1520 980" fill="none" stroke="#203040" stroke-width="72" stroke-linecap="round"/></svg>"##,
    )
    .expect("write benchmark svg");
}

fn gradient_rgba(width: u32, height: u32, seed: u8) -> image::RgbaImage {
    image::RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([
            ((x + u32::from(seed) * 17) % 256) as u8,
            ((y + u32::from(seed) * 29) % 256) as u8,
            (((x / 3 + y / 5) + u32::from(seed) * 11) % 256) as u8,
            255,
        ])
    })
}

fn transparent_gradient_rgba(width: u32, height: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([
            (x % 256) as u8,
            (y % 256) as u8,
            ((x / 3 + y / 5) % 256) as u8,
            ((x + y) % 256) as u8,
        ])
    })
}

fn gradient_rgb(width: u32, height: u32, seed: u8) -> image::RgbImage {
    image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([
            ((x + u32::from(seed) * 17) % 256) as u8,
            ((y + u32::from(seed) * 29) % 256) as u8,
            (((x / 3 + y / 5) + u32::from(seed) * 11) % 256) as u8,
        ])
    })
}

fn load_thumbnail(path: &Path) -> usize {
    let image = load_image_thumbnail_ready_for_benchmark(path, THUMBNAIL_SIZE)
        .expect("load benchmark thumbnail");
    encode_cached_thumbnail_for_benchmark(image)
        .expect("encode benchmark cache thumbnail")
        .len()
}

fn load_ready_thumbnail(path: &Path, size: u32) -> usize {
    load_image_thumbnail_ready_for_benchmark(path, size)
        .expect("prepare benchmark thumbnail")
        .len()
}

fn load_batch(paths: &[PathBuf], parallelism: usize, encode_cache: bool) -> usize {
    let load = |path: &Path| {
        if encode_cache {
            load_thumbnail(path)
        } else {
            load_ready_thumbnail(path, THUMBNAIL_SIZE)
        }
    };
    if parallelism <= 1 {
        return paths.iter().map(|path| load(path)).sum();
    }

    let paths = Arc::new(paths.to_vec());
    let next = AtomicUsize::new(0);
    let total = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..parallelism {
            let paths = paths.clone();
            let next = &next;
            let total = &total;
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index) else {
                        break;
                    };
                    total.fetch_add(load(path), Ordering::Relaxed);
                }
            });
        }
    });
    total.load(Ordering::Relaxed)
}

fn image_thumbnail_benchmarks(criterion: &mut Criterion) {
    let fixture = Fixture::get();
    let opaque_rgba = gradient_rgba(LARGE_WIDTH, LARGE_HEIGHT, 0);
    let transparent_rgba = transparent_gradient_rgba(LARGE_WIDTH, LARGE_HEIGHT);
    let mut resize = criterion.benchmark_group("image_thumbnails/rgba_resize");
    resize.sample_size(20);
    resize.measurement_time(Duration::from_secs(5));
    resize.throughput(Throughput::Elements(
        u64::from(LARGE_WIDTH) * u64::from(LARGE_HEIGHT),
    ));
    for (source_name, source) in [("opaque", &opaque_rgba), ("transparent", &transparent_rgba)] {
        for longest_side in [THUMBNAIL_SIZE, 400] {
            resize.bench_with_input(
                BenchmarkId::new(source_name, longest_side),
                &longest_side,
                |bencher, longest_side| {
                    bencher.iter_batched(
                        || source.clone(),
                        |image| {
                            let resized =
                                resize_rgba_for_benchmark(image, black_box(*longest_side))
                                    .expect("resize benchmark image");
                            black_box(resized.len())
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
        }
    }
    resize.finish();

    let mut ready = criterion.benchmark_group("image_thumbnails/ready_for_display");
    ready.sample_size(10);
    ready.measurement_time(Duration::from_secs(5));
    for (name, path) in [
        ("png_large", &fixture.large_png),
        ("png_transparent", &fixture.large_transparent_png),
        ("jpeg_large", &fixture.large_jpeg),
        ("jpeg_12mp", &fixture.photo_jpeg),
        ("tiff_large_uncompressed", &fixture.large_tiff),
        ("tiff_large_deflate", &fixture.large_deflate_tiff),
        ("tiff_photo_lzw", &fixture.photo_lzw_tiff),
        ("tiff_huge_deflate", &fixture.huge_deflate_tiff),
        ("tiff_wide_uncompressed", &fixture.wide_tiff),
        ("webp_large", &fixture.large_webp),
        ("svg_large", &fixture.large_svg),
    ] {
        for size in [THUMBNAIL_SIZE, 400] {
            ready.bench_with_input(BenchmarkId::new(name, size), &size, |bencher, size| {
                bencher.iter(|| black_box(load_ready_thumbnail(black_box(path), black_box(*size))));
            });
        }
    }
    ready.finish();

    let mut single = criterion.benchmark_group("image_thumbnails/single_file");
    single.sample_size(10);
    single.measurement_time(Duration::from_secs(5));
    for (name, path) in [
        ("png_large", &fixture.large_png),
        ("jpeg_large", &fixture.large_jpeg),
        ("tiff_large_uncompressed", &fixture.large_tiff),
        ("tiff_large_deflate", &fixture.large_deflate_tiff),
        ("tiff_photo_lzw", &fixture.photo_lzw_tiff),
        ("tiff_huge_deflate", &fixture.huge_deflate_tiff),
        ("tiff_wide_uncompressed", &fixture.wide_tiff),
        ("svg_large", &fixture.large_svg),
    ] {
        single.throughput(Throughput::Bytes(
            fs::metadata(path).expect("benchmark metadata").len(),
        ));
        single.bench_function(name, |bencher| {
            bencher.iter(|| black_box(load_thumbnail(black_box(path))));
        });
    }
    single.finish();

    let cached_qoi = encode_cached_thumbnail_for_benchmark(
        load_image_thumbnail_ready_for_benchmark(&fixture.large_png, THUMBNAIL_SIZE)
            .expect("create cache decode fixture"),
    )
    .expect("encode cache decode fixture");
    let mut cache = criterion.benchmark_group("image_thumbnails/disk_cache_decode");
    cache.sample_size(20);
    cache.measurement_time(Duration::from_secs(5));
    cache.bench_function("qoi_to_render_image", |bencher| {
        bencher.iter_batched(
            || cached_qoi.clone(),
            |bytes| black_box(prepare_cached_thumbnail_for_benchmark(bytes)),
            BatchSize::SmallInput,
        );
    });
    cache.finish();

    let cache_encode_image =
        load_image_thumbnail_ready_for_benchmark(&fixture.large_png, THUMBNAIL_SIZE)
            .expect("create cache encode fixture");
    let mut cache_encode = criterion.benchmark_group("image_thumbnails/cache_encode");
    cache_encode.sample_size(20);
    cache_encode.measurement_time(Duration::from_secs(5));
    cache_encode.bench_function("qoi_128_rgba", |bencher| {
        bencher.iter_batched(
            || cache_encode_image.clone(),
            |image| black_box(encode_cached_thumbnail_for_benchmark(image)),
            BatchSize::SmallInput,
        );
    });
    cache_encode.finish();

    let cache_write_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("image-thumbnail-cache-write-benchmark-v1");
    fs::create_dir_all(&cache_write_dir).expect("create cache write benchmark directory");
    let cache_write_image =
        load_image_thumbnail_ready_for_benchmark(&fixture.large_png, THUMBNAIL_SIZE)
            .expect("create cache write image");
    let cache_write_images = vec![cache_write_image; BATCH_COUNT];
    let mut cache_write = criterion.benchmark_group("image_thumbnails/disk_cache_write");
    cache_write.sample_size(10);
    cache_write.measurement_time(Duration::from_secs(5));
    cache_write.bench_function("qoi_batch_32_with_manifest", |bencher| {
        bencher.iter(|| {
            black_box(write_cached_thumbnail_batch_for_benchmark(
                black_box(&cache_write_dir),
                black_box(&cache_write_images),
            ))
        });
    });
    cache_write.finish();

    let mixed_paths = vec![
        fixture.large_png.clone(),
        fixture.large_transparent_png.clone(),
        fixture.large_jpeg.clone(),
        fixture.large_tiff.clone(),
        fixture.large_deflate_tiff.clone(),
        fixture.photo_lzw_tiff.clone(),
        fixture.large_webp.clone(),
        fixture.large_svg.clone(),
    ];
    let mut mixed = criterion.benchmark_group("image_thumbnails/cold_mixed_folder");
    mixed.sample_size(10);
    mixed.measurement_time(Duration::from_secs(5));
    for parallelism in [2, 4] {
        mixed.bench_with_input(
            BenchmarkId::from_parameter(parallelism),
            &parallelism,
            |b, p| {
                b.iter(|| black_box(load_batch(black_box(&mixed_paths), black_box(*p), false)));
            },
        );
    }
    mixed.finish();

    let mut queue = criterion.benchmark_group("image_thumbnails/queue_cancel");
    queue.sample_size(20);
    queue.measurement_time(Duration::from_secs(5));
    for count in [32, 256] {
        queue.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, count| {
            b.iter(|| black_box(queue_and_cancel_thumbnails_for_benchmark(*count)));
        });
    }
    queue.finish();

    let total_bytes = fixture
        .batch_jpegs
        .iter()
        .map(|path| fs::metadata(path).expect("benchmark metadata").len())
        .sum();
    let mut batch = criterion.benchmark_group("image_thumbnails/batch_jpeg");
    batch.sample_size(10);
    batch.measurement_time(Duration::from_secs(5));
    batch.throughput(Throughput::Bytes(total_bytes));
    for parallelism in [1, 2, 4, 8] {
        batch.bench_with_input(
            BenchmarkId::from_parameter(parallelism),
            &parallelism,
            |bencher, parallelism| {
                bencher.iter(|| {
                    black_box(load_batch(
                        black_box(&fixture.batch_jpegs),
                        black_box(*parallelism),
                        true,
                    ))
                });
            },
        );
    }
    batch.finish();
}

criterion_group!(benches, image_thumbnail_benchmarks);
criterion_main!(benches);
