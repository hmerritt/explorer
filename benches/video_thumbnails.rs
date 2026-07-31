use std::{
    env, fs,
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

const FIXTURE_VERSION: &str = "video-thumbnails-benchmark-v2";
const REAL_FOLDER_ENV: &str = "EXPLORER_VIDEO_THUMBNAIL_BENCH_DIR";
const THUMBNAIL_SIZE: u32 = 128;
const FOLDER_VIDEO_COUNT: usize = 24;

struct VideoFixtures {
    sub_second: PathBuf,
    ordinary: PathBuf,
    long: PathBuf,
    sparse_keyframes: PathBuf,
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
            generate_long_gop_video(&root.join("sparse-keyframes.mkv"));
            fs::write(root.join("malformed.mp4"), b"not a video")
                .expect("write malformed video fixture");
            fs::write(&marker, FIXTURE_VERSION).expect("write video fixture marker");
        }
        VideoFixtures {
            sub_second: root.join("sub-second.mp4"),
            ordinary: root.join("ordinary.mp4"),
            long: root.join("long.mp4"),
            sparse_keyframes: root.join("sparse-keyframes.mkv"),
            malformed: root.join("malformed.mp4"),
        }
    })
}

fn generate_long_gop_video(path: &Path) {
    let input = "testsrc2=size=1920x1080:rate=30";
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            input,
            "-t",
            "12",
            "-an",
            "-c:v",
            "mpeg4",
            "-q:v",
            "8",
            "-g",
            "300",
            "-sc_threshold",
            "0",
            "-y",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .unwrap_or_else(|error| panic!("start long-GOP fixture generation: {error}"));
    assert!(
        status.success(),
        "ffmpeg could not generate {}",
        path.display()
    );
}

fn real_folder_videos() -> Option<Vec<PathBuf>> {
    let directory = env::var_os(REAL_FOLDER_ENV).map(PathBuf::from)?;
    let mut paths = fs::read_dir(&directory)
        .unwrap_or_else(|error| {
            panic!(
                "could not read {REAL_FOLDER_ENV} directory {}: {error}",
                directory.display()
            )
        })
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_video_path(path))
        .collect::<Vec<_>>();
    paths.sort();
    assert!(
        !paths.is_empty(),
        "{REAL_FOLDER_ENV} did not contain any recognized video files"
    );
    Some(paths)
}

fn is_video_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "webm"
                    | "mkv"
                    | "flv"
                    | "vob"
                    | "ogv"
                    | "ogg"
                    | "mov"
                    | "avi"
                    | "wmv"
                    | "m2ts"
                    | "mts"
                    | "ts"
                    | "mp4"
                    | "m4v"
                    | "mpg"
                    | "mpeg"
                    | "m2v"
                    | "3gp"
                    | "3g2"
                    | "mxf"
            )
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
    for (name, path, should_succeed) in [
        ("sub_second", &fixture.sub_second, true),
        ("ordinary", &fixture.ordinary, true),
        ("long", &fixture.long, true),
        ("sparse_keyframes", &fixture.sparse_keyframes, true),
        ("malformed", &fixture.malformed, false),
    ] {
        single.throughput(Throughput::Bytes(
            fs::metadata(path)
                .map(|metadata| metadata.len())
                .unwrap_or_default(),
        ));
        single.bench_with_input(name, path, |bencher, path| {
            bencher.iter(|| {
                let pixels =
                    load_video_thumbnail_for_benchmark(black_box(path), black_box(THUMBNAIL_SIZE));
                assert_eq!(pixels > 0, should_succeed);
                black_box(pixels)
            });
        });
    }
    single.finish();

    let paths = vec![fixture.ordinary.as_path(); FOLDER_VIDEO_COUNT];
    criterion.bench_function("video_thumbnails/folder_24_time_to_ready", |bencher| {
        bencher.iter(|| {
            let ready = load_video_thumbnail_batch_for_benchmark(
                black_box(&paths),
                black_box(THUMBNAIL_SIZE),
            );
            assert_eq!(ready, paths.len());
            black_box(ready)
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

    if let Some(real_paths) = real_folder_videos() {
        let path_refs = real_paths.iter().map(PathBuf::as_path).collect::<Vec<_>>();
        let mut real_folder = criterion.benchmark_group("video_thumbnails/real_folder");
        real_folder.throughput(Throughput::Elements(path_refs.len() as u64));
        real_folder.bench_function("time_to_ready", |bencher| {
            bencher.iter(|| {
                let ready = load_video_thumbnail_batch_for_benchmark(
                    black_box(&path_refs),
                    black_box(THUMBNAIL_SIZE),
                );
                assert_eq!(ready, path_refs.len());
                black_box(ready)
            });
        });
        real_folder.finish();
    }
}

criterion_group!(benches, video_thumbnail_benchmarks);
criterion_main!(benches);
