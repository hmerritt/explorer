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

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use explorer::benchmark_support::load_image_thumbnail_for_benchmark;

const FIXTURE_VERSION: &str = "image-thumbnails-benchmark-v1";
const THUMBNAIL_SIZE: u32 = 128;
const LARGE_WIDTH: u32 = 1600;
const LARGE_HEIGHT: u32 = 1200;
const BATCH_COUNT: usize = 32;

struct Fixture {
    large_png: PathBuf,
    large_jpeg: PathBuf,
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
            create_jpeg(&root.join("large.jpg"), LARGE_WIDTH, LARGE_HEIGHT, 0);
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
            large_jpeg: root.join("large.jpg"),
            large_svg: root.join("large.svg"),
            batch_jpegs: (0..BATCH_COUNT)
                .map(|index| root.join(format!("batch-{index:02}.jpg")))
                .collect(),
        }
    }
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
    load_image_thumbnail_for_benchmark(path, THUMBNAIL_SIZE)
        .expect("load benchmark thumbnail")
        .len()
}

fn load_batch(paths: &[PathBuf], parallelism: usize) -> usize {
    if parallelism <= 1 {
        return paths.iter().map(|path| load_thumbnail(path)).sum();
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
                    total.fetch_add(load_thumbnail(path), Ordering::Relaxed);
                }
            });
        }
    });
    total.load(Ordering::Relaxed)
}

fn image_thumbnail_benchmarks(criterion: &mut Criterion) {
    let fixture = Fixture::get();
    let mut single = criterion.benchmark_group("image_thumbnails/single_file");
    single.sample_size(10);
    single.measurement_time(Duration::from_secs(5));
    for (name, path) in [
        ("png_large", &fixture.large_png),
        ("jpeg_large", &fixture.large_jpeg),
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
                    ))
                });
            },
        );
    }
    batch.finish();
}

criterion_group!(benches, image_thumbnail_benchmarks);
criterion_main!(benches);
