use std::{
    fs,
    hint::black_box,
    path::{Path, PathBuf},
    time::Duration,
};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use explorer::benchmark_support::{
    ImageViewerBenchmarkIccMode, ImageViewerDeferredIccBenchmarkInput,
    apply_deferred_icc_input_for_benchmark, deferred_icc_input_for_benchmark,
    image_file_len_for_benchmark, open_image_viewer_for_benchmark,
    render_image_from_rgba_for_benchmark,
};
use image::ImageEncoder;

const FIXTURE_VERSION: &str = "image-viewer-benchmark-v1";
const SMALL_WIDTH: u32 = 256;
const SMALL_HEIGHT: u32 = 256;
const LARGE_WIDTH: u32 = 1600;
const LARGE_HEIGHT: u32 = 1200;
const ICC_WIDTH: u32 = 1600;
const ICC_HEIGHT: u32 = 1200;

struct Fixture {
    small_png: PathBuf,
    large_png: PathBuf,
    large_jpeg: PathBuf,
    photo_jpeg: PathBuf,
    large_tiff: PathBuf,
    large_webp: PathBuf,
    large_svg: PathBuf,
    icc_png: PathBuf,
    icc_jpeg: PathBuf,
}

impl Fixture {
    fn get() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(FIXTURE_VERSION);
        let marker = root.join(".complete");
        if !marker.exists() {
            if root.exists() {
                fs::remove_dir_all(&root).expect("remove incomplete image viewer fixture");
            }
            fs::create_dir_all(&root).expect("create image viewer fixture");
            create_png(&root.join("small.png"), SMALL_WIDTH, SMALL_HEIGHT, 0);
            create_png(&root.join("large.png"), LARGE_WIDTH, LARGE_HEIGHT, 1);
            create_jpeg(&root.join("large.jpg"), LARGE_WIDTH, LARGE_HEIGHT, 2);
            create_jpeg(&root.join("photo-12mp.jpg"), 4000, 3000, 3);
            create_tiff(&root.join("large.tif"), LARGE_WIDTH, LARGE_HEIGHT);
            create_webp(&root.join("large.webp"), LARGE_WIDTH, LARGE_HEIGHT);
            create_svg(&root.join("large.svg"));
            create_icc_png(&root.join("display-p3.png"), ICC_WIDTH, ICC_HEIGHT);
            create_icc_jpeg(&root.join("display-p3.jpg"), ICC_WIDTH, ICC_HEIGHT);
            fs::write(&marker, FIXTURE_VERSION).expect("write image viewer fixture marker");
        }

        Self {
            small_png: root.join("small.png"),
            large_png: root.join("large.png"),
            large_jpeg: root.join("large.jpg"),
            photo_jpeg: root.join("photo-12mp.jpg"),
            large_tiff: root.join("large.tif"),
            large_webp: root.join("large.webp"),
            large_svg: root.join("large.svg"),
            icc_png: root.join("display-p3.png"),
            icc_jpeg: root.join("display-p3.jpg"),
        }
    }
}

fn create_png(path: &Path, width: u32, height: u32, seed: u8) {
    let image = image::DynamicImage::ImageRgba8(gradient_rgba(width, height, seed));
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encode benchmark png");
    fs::write(path, bytes).expect("write benchmark png");
}

fn create_icc_png(path: &Path, width: u32, height: u32) {
    let image = gradient_rgba(width, height, 4);
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    encoder
        .set_icc_profile(display_p3_icc_profile())
        .expect("set benchmark png icc profile");
    encoder
        .write_image(
            image.as_raw(),
            width,
            height,
            image::ColorType::Rgba8.into(),
        )
        .expect("encode benchmark icc png");
    fs::write(path, bytes).expect("write benchmark icc png");
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

fn create_icc_jpeg(path: &Path, width: u32, height: u32) {
    let image = gradient_rgb(width, height, 5);
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90);
    encoder
        .set_icc_profile(display_p3_icc_profile())
        .expect("set benchmark jpeg icc profile");
    encoder
        .write_image(image.as_raw(), width, height, image::ColorType::Rgb8.into())
        .expect("encode benchmark icc jpeg");
    fs::write(path, bytes).expect("write benchmark icc jpeg");
}

fn create_webp(path: &Path, width: u32, height: u32) {
    let image = image::DynamicImage::ImageRgb8(gradient_rgb(width, height, 6));
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::WebP,
        )
        .expect("encode benchmark webp");
    fs::write(path, bytes).expect("write benchmark webp");
}

fn create_tiff(path: &Path, width: u32, height: u32) {
    let image = gradient_rgb(width, height, 7);
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

fn create_svg(path: &Path) {
    fs::write(
        path,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="1200"><defs><linearGradient id="g" x1="0" x2="1" y1="0" y2="1"><stop offset="0" stop-color="#db3939"/><stop offset="0.5" stop-color="#2984ce"/><stop offset="1" stop-color="#e0c43a"/></linearGradient></defs><rect width="1600" height="1200" fill="url(#g)"/><circle cx="1120" cy="420" r="260" fill="#ffffff" fill-opacity="0.35"/><path d="M 120 960 C 430 650 640 1120 940 810 S 1370 760 1520 980" fill="none" stroke="#1e2c36" stroke-width="72" stroke-linecap="round"/></svg>"##,
    )
    .expect("write benchmark svg");
}

fn display_p3_icc_profile() -> Vec<u8> {
    moxcms::ColorProfile::new_display_p3()
        .encode()
        .expect("encode Display P3 ICC profile")
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

fn open_image_for_benchmark(path: &Path, mode: ImageViewerBenchmarkIccMode) -> usize {
    let opened = open_image_viewer_for_benchmark(path, mode).expect("open benchmark image");
    assert!(opened.width > 0);
    assert!(opened.height > 0);
    opened.render_bytes
}

fn open_no_icc_image_for_benchmark(path: &Path) -> usize {
    let opened = open_image_viewer_for_benchmark(path, ImageViewerBenchmarkIccMode::Defer)
        .expect("open no-icc benchmark image");
    assert!(!opened.has_deferred_icc);
    opened.render_bytes
}

fn apply_deferred_icc_for_benchmark(input: ImageViewerDeferredIccBenchmarkInput) -> usize {
    apply_deferred_icc_input_for_benchmark(input).expect("apply deferred benchmark icc")
}

fn image_viewer_benchmarks(criterion: &mut Criterion) {
    let fixture = Fixture::get();

    let mut open_icc = criterion.benchmark_group("image_viewer/native_open_icc");
    open_icc.sample_size(10);
    open_icc.measurement_time(Duration::from_secs(5));
    for (fixture_name, path) in [
        ("png_display_p3", &fixture.icc_png),
        ("jpeg_display_p3", &fixture.icc_jpeg),
    ] {
        for (mode_name, mode) in [
            ("sync_icc", ImageViewerBenchmarkIccMode::ApplySynchronously),
            ("deferred_first_ready", ImageViewerBenchmarkIccMode::Defer),
            ("ignore_icc", ImageViewerBenchmarkIccMode::Ignore),
        ] {
            open_icc.throughput(Throughput::Bytes(image_file_len_for_benchmark(path)));
            open_icc.bench_function(BenchmarkId::new(mode_name, fixture_name), |bencher| {
                bencher
                    .iter(|| black_box(open_image_for_benchmark(black_box(path), black_box(mode))));
            });
        }
    }
    open_icc.finish();

    let mut no_icc = criterion.benchmark_group("image_viewer/native_open_no_icc");
    no_icc.sample_size(10);
    no_icc.measurement_time(Duration::from_secs(5));
    for (name, path) in [
        ("png_small", &fixture.small_png),
        ("png_large", &fixture.large_png),
        ("jpeg_large", &fixture.large_jpeg),
        ("jpeg_12mp", &fixture.photo_jpeg),
        ("tiff_large_uncompressed", &fixture.large_tiff),
        ("webp_large", &fixture.large_webp),
        ("svg_large", &fixture.large_svg),
    ] {
        no_icc.throughput(Throughput::Bytes(image_file_len_for_benchmark(path)));
        no_icc.bench_function(name, |bencher| {
            bencher.iter(|| black_box(open_no_icc_image_for_benchmark(black_box(path))));
        });
    }
    no_icc.finish();

    let deferred_png =
        deferred_icc_input_for_benchmark(&fixture.icc_png).expect("prepare deferred png icc input");
    let deferred_jpeg = deferred_icc_input_for_benchmark(&fixture.icc_jpeg)
        .expect("prepare deferred jpeg icc input");
    let mut deferred = criterion.benchmark_group("image_viewer/deferred_icc_correction");
    deferred.sample_size(10);
    deferred.measurement_time(Duration::from_secs(5));
    deferred.throughput(Throughput::Elements(
        u64::from(ICC_WIDTH) * u64::from(ICC_HEIGHT),
    ));
    for (name, input) in [
        ("png_display_p3", deferred_png),
        ("jpeg_display_p3", deferred_jpeg),
    ] {
        deferred.bench_with_input(
            BenchmarkId::from_parameter(name),
            &input,
            |bencher, input| {
                bencher.iter_batched(
                    || input.clone(),
                    |input| black_box(apply_deferred_icc_for_benchmark(input)),
                    BatchSize::SmallInput,
                );
            },
        );
    }
    deferred.finish();

    let small_rgba = gradient_rgba(SMALL_WIDTH, SMALL_HEIGHT, 8);
    let large_rgba = gradient_rgba(LARGE_WIDTH, LARGE_HEIGHT, 9);
    let mut render = criterion.benchmark_group("image_viewer/render_image_build");
    render.sample_size(20);
    render.measurement_time(Duration::from_secs(5));
    for (name, image, pixels) in [
        (
            "rgba_small",
            small_rgba,
            u64::from(SMALL_WIDTH) * u64::from(SMALL_HEIGHT),
        ),
        (
            "rgba_large",
            large_rgba,
            u64::from(LARGE_WIDTH) * u64::from(LARGE_HEIGHT),
        ),
    ] {
        render.throughput(Throughput::Elements(pixels));
        render.bench_with_input(
            BenchmarkId::from_parameter(name),
            &image,
            |bencher, image| {
                bencher.iter_batched(
                    || image.clone(),
                    |image| black_box(render_image_from_rgba_for_benchmark(image)),
                    BatchSize::SmallInput,
                );
            },
        );
    }
    render.finish();
}

criterion_group!(benches, image_viewer_benchmarks);
criterion_main!(benches);
