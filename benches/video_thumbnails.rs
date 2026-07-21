use std::{
    fs,
    hint::black_box,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use explorer::benchmark_support::{
    load_video_hover_first_frame_for_benchmark, load_video_properties_frames_for_benchmark,
    load_video_thumbnail_batch_for_benchmark, load_video_thumbnail_for_benchmark,
};

const FIXTURE_VERSION: &str = "video-thumbnails-benchmark-v1";
const THUMBNAIL_SIZE: u32 = 128;
const FOLDER_VIDEO_COUNT: usize = 24;

struct VideoFixtures {
    sub_second: PathBuf,
    ordinary: PathBuf,
    long: PathBuf,
    malformed: PathBuf,
}

fn fixtures() -> &'static VideoFixtures {
    static FIXTURES: OnceLock<VideoFixtures> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(FIXTURE_VERSION);
        let marker = root.join(".complete");
        if !marker.is_file() {
            if root.exists() {
                fs::remove_dir_all(&root).expect("remove incomplete video thumbnail fixture");
            }
            fs::create_dir_all(&root).expect("create video thumbnail fixture directory");
            generate_video(&root.join("sub-second.mp4"), "0.4", "640x360");
            generate_video(&root.join("ordinary.mp4"), "12", "1280x720");
            generate_video(&root.join("long.mp4"), "120", "640x360");
            fs::write(root.join("malformed.mp4"), b"not a video")
                .expect("write malformed video fixture");
            fs::write(&marker, FIXTURE_VERSION).expect("write video fixture marker");
        }
        VideoFixtures {
            sub_second: root.join("sub-second.mp4"),
            ordinary: root.join("ordinary.mp4"),
            long: root.join("long.mp4"),
            malformed: root.join("malformed.mp4"),
        }
    })
}

fn generate_video(path: &Path, duration: &str, size: &str) {
    let input = format!("testsrc2=size={size}:rate=30");
    let status = Command::new("ffmpeg")
        .args([
            "-v", "error", "-f", "lavfi", "-i", &input, "-t", duration, "-an", "-c:v", "mpeg4",
            "-q:v", "8", "-y",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .unwrap_or_else(|error| panic!("start ffmpeg fixture generation: {error}"));
    assert!(
        status.success(),
        "ffmpeg could not generate {}",
        path.display()
    );
}

fn video_thumbnail_benchmarks(criterion: &mut Criterion) {
    let fixture = fixtures();
    let mut single = criterion.benchmark_group("video_thumbnails/single_uncached");
    for (name, path) in [
        ("sub_second", &fixture.sub_second),
        ("ordinary", &fixture.ordinary),
        ("long", &fixture.long),
        ("malformed", &fixture.malformed),
    ] {
        single.throughput(Throughput::Bytes(
            fs::metadata(path)
                .map(|metadata| metadata.len())
                .unwrap_or_default(),
        ));
        single.bench_with_input(name, path, |bencher, path| {
            bencher.iter(|| {
                black_box(load_video_thumbnail_for_benchmark(
                    black_box(path),
                    black_box(THUMBNAIL_SIZE),
                ))
            });
        });
    }
    single.finish();

    let paths = vec![fixture.ordinary.as_path(); FOLDER_VIDEO_COUNT];
    criterion.bench_function("video_thumbnails/folder_24_time_to_ready", |bencher| {
        bencher.iter(|| {
            black_box(load_video_thumbnail_batch_for_benchmark(
                black_box(&paths),
                black_box(THUMBNAIL_SIZE),
            ))
        });
    });

    criterion.bench_function("video_thumbnails/properties_20_frames", |bencher| {
        bencher.iter(|| {
            black_box(load_video_properties_frames_for_benchmark(black_box(
                fixture.long.as_path(),
            )))
        });
    });

    criterion.bench_function("video_thumbnails/hover_first_frame", |bencher| {
        bencher.iter(|| {
            black_box(load_video_hover_first_frame_for_benchmark(black_box(
                fixture.ordinary.as_path(),
            )))
        });
    });
}

criterion_group!(benches, video_thumbnail_benchmarks);
criterion_main!(benches);
