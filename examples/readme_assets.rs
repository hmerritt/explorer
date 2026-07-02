use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use filetime::{FileTime, set_file_mtime};
use image::{ImageBuffer, Rgba};
use serde_json::{Value, json};

const FIXED_MTIME: i64 = 1_767_225_600; // 2026-01-01T00:00:00Z
const WINDOW_WIDTH: f32 = 1200.0;
const WINDOW_HEIGHT: f32 = 820.0;

const SCREENSHOTS: &[(&str, &str)] = &[
    ("explorer-overview", "Explorer overview/details view"),
    ("large-icons", "Explorer large-icons media view"),
    ("image-viewer", "Explorer image viewer"),
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repo = env::current_dir()?;
    let target = repo.join("target").join("readme-assets");

    match env::args().nth(1).as_deref() {
        Some("prepare") => prepare(&repo, &target)?,
        Some("optimize") => optimize(&repo, &target)?,
        Some("help" | "--help" | "-h") | None => print_help(),
        Some(command) => {
            eprintln!("unknown command: {command}");
            print_help();
            std::process::exit(2);
        }
    }

    Ok(())
}

fn print_help() {
    eprintln!("usage:");
    eprintln!("  cargo run --example readme_assets -- prepare");
    eprintln!("  cargo run --example readme_assets -- optimize");
}

fn prepare(repo: &Path, target: &Path) -> io::Result<()> {
    let fixture = target.join("fixture").join("Explorer Demo");
    let captures = target.join("captures");

    if target.exists() {
        fs::remove_dir_all(target)?;
    }

    fs::create_dir_all(&fixture)?;
    fs::create_dir_all(&captures)?;

    let documents = fixture.join("Documents");
    let media = fixture.join("Media");
    let projects = fixture.join("Projects");
    let archives = fixture.join("Archives");
    let shared = fixture.join("Shared");

    for dir in [&documents, &media, &projects, &archives, &shared] {
        fs::create_dir_all(dir)?;
    }

    write_text(
        &fixture.join("Project brief.md"),
        "# Project brief\n\nExplorer README screenshot fixture.\n",
    )?;
    write_text(
        &fixture.join("Release checklist.txt"),
        "Download links\nScreenshots\nConfiguration example\nDevelopment docs\n",
    )?;
    write_text(&fixture.join("Budget.xlsx"), "placeholder spreadsheet\n")?;
    write_text(
        &fixture.join("Presentation.pptx"),
        "placeholder presentation\n",
    )?;
    write_text(&fixture.join("Archive bundle.zip"), "placeholder archive\n")?;

    write_text(
        &documents.join("Notes.txt"),
        "A neutral text document used by README screenshots.\n",
    )?;
    write_text(&documents.join("Invoice.pdf"), "placeholder pdf\n")?;
    write_text(
        &documents.join("Design draft.docx"),
        "placeholder document\n",
    )?;

    write_text(
        &projects.join("README.md"),
        "# Demo project\n\nA deterministic project folder for screenshots.\n",
    )?;
    write_text(
        &projects.join("main.rs"),
        "fn main() {\n    println!(\"demo\");\n}\n",
    )?;
    write_text(
        &projects.join("config.json"),
        "{\n  \"name\": \"demo\"\n}\n",
    )?;

    write_text(&archives.join("Photos 2026.zip"), "placeholder archive\n")?;
    write_text(&archives.join("Backup.tar.zst"), "placeholder archive\n")?;
    write_text(&archives.join("Logs.7z"), "placeholder archive\n")?;

    write_text(
        &shared.join("Team plan.md"),
        "# Team plan\n\nShared screenshot content.\n",
    )?;
    write_text(&shared.join("Meeting audio.flac"), "placeholder audio\n")?;
    write_text(&shared.join("Walkthrough.mp4"), "placeholder video\n")?;

    write_png(
        &media.join("Harbor.png"),
        960,
        540,
        [32, 107, 128],
        [239, 203, 121],
    )?;
    write_png(
        &media.join("Workspace.png"),
        800,
        800,
        [80, 108, 78],
        [236, 237, 218],
    )?;
    write_png(
        &media.join("Blueprint.png"),
        1024,
        640,
        [45, 75, 122],
        [200, 221, 247],
    )?;
    write_png(
        &media.join("Viewer sample.png"),
        320,
        180,
        [32, 107, 128],
        [239, 203, 121],
    )?;
    write_text(&media.join("Launch poster.svg"), demo_svg())?;
    write_text(&media.join("Clip preview.mp4"), "placeholder video\n")?;

    let icon_source = repo.join("assets").join("explorer.png");
    if icon_source.is_file() {
        fs::copy(&icon_source, media.join("Explorer app icon.png"))?;
    }

    touch_tree(&fixture, FIXED_MTIME)?;

    let sidebar_items = vec![
        fixture.clone(),
        documents.clone(),
        media.clone(),
        projects.clone(),
    ];
    write_scenario_config(target, "overview", &fixture, "details", &sidebar_items)?;
    write_scenario_config(target, "large-icons", &media, "large_icons", &sidebar_items)?;
    write_scenario_config(
        target,
        "image-viewer",
        &media,
        "large_icons",
        &sidebar_items,
    )?;
    write_window_state(&target.join("window-state.json"))?;

    println!("Prepared README asset fixture at {}", fixture.display());
    println!();
    println!("Capture targets:");
    for (name, description) in SCREENSHOTS {
        println!("  {name}.png - {description}");
    }
    println!();
    println!("Windows example:");
    println!(
        "  $env:APPDATA='{}'; cargo run",
        scenario_root(target, "overview").display()
    );
    println!();
    println!("Linux example:");
    println!(
        "  XDG_CONFIG_HOME='{}' cargo run",
        scenario_root(target, "overview").display()
    );
    println!();
    println!("macOS example:");
    println!(
        "  HOME='{}' cargo run",
        scenario_root(target, "overview").join("home").display()
    );
    println!();
    println!(
        "Image viewer sample: {}",
        media.join("Viewer sample.png").display()
    );

    Ok(())
}

fn optimize(repo: &Path, target: &Path) -> io::Result<()> {
    let captures = target.join("captures");
    let docs = repo.join("docs").join("assets").join("readme");
    fs::create_dir_all(&docs)?;

    let mut missing = Vec::new();
    for (name, description) in SCREENSHOTS {
        let source = captures.join(format!("{name}.png"));
        let destination = docs.join(format!("{name}.png"));
        if !source.is_file() {
            missing.push(source);
            continue;
        }

        validate_png(&source, description)?;
        fs::copy(&source, &destination)?;

        let size = fs::metadata(&destination)?.len();
        if size > 1_000_000 {
            eprintln!(
                "warning: {} is {:.1} MB; consider cropping or external PNG optimization",
                destination.display(),
                size as f64 / 1_000_000.0
            );
        }

        println!("wrote {}", destination.display());
    }

    if !missing.is_empty() {
        eprintln!("missing staged screenshots:");
        for path in missing {
            eprintln!("  {}", path.display());
        }
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "capture all README screenshots before optimizing",
        ));
    }

    Ok(())
}

fn write_scenario_config(
    target: &Path,
    scenario: &str,
    start_path: &Path,
    view_mode: &str,
    sidebar_items: &[PathBuf],
) -> io::Result<()> {
    let settings = settings_json(start_path, view_mode, sidebar_items);
    let state = window_state_json();
    let root = scenario_root(target, scenario);

    let windows_dir = root.join("com.hmerritt.explorer");
    let linux_dir = root.join("explorer");
    let mac_dir = root.join("home").join(".config").join("explorer");

    for dir in [&windows_dir, &linux_dir, &mac_dir] {
        fs::create_dir_all(dir)?;
        write_json(&dir.join("settings.json"), &settings)?;
        write_json(&dir.join("window-state.json"), &state)?;
    }

    Ok(())
}

fn scenario_root(target: &Path, scenario: &str) -> PathBuf {
    target.join("config").join(scenario)
}

fn settings_json(start_path: &Path, view_mode: &str, sidebar_items: &[PathBuf]) -> Value {
    let sidebar_items = sidebar_items
        .iter()
        .map(|path| Value::String(path_string(path)))
        .collect::<Vec<_>>();

    json!({
        "app": {
            "cache_cleanup_interval_days": 30,
            "start": {
                "kind": "custom",
                "path": path_string(start_path)
            }
        },
        "contextmenu": [],
        "sidebar": {
            "hide": [],
            "items": sidebar_items,
            "width": 225
        },
        "tabs": {
            "focus_new": false
        },
        "view": {
            "date_format": "%Y/%m/%d %H:%M",
            "file_columns": {
                "order": ["date_modified", "type", "size"],
                "widths": {
                    "date_modified": 150,
                    "type": 120,
                    "size": 96
                }
            },
            "font": "default",
            "mode": view_mode,
            "mode_media": "large_icons",
            "remote_mode_media": "details",
            "remote_thumbnails": false,
            "native_icons": true,
            "show_extensions": true,
            "show_folder_sizes": false,
            "show_dotfiles": true,
            "show_hidden": false,
            "sort": {
                "column": "name",
                "direction": "ascending"
            }
        }
    })
}

fn window_state_json() -> Value {
    json!({
        "x": 80.0,
        "y": 80.0,
        "width": WINDOW_WIDTH,
        "height": WINDOW_HEIGHT,
        "state": "windowed"
    })
}

fn write_window_state(path: &Path) -> io::Result<()> {
    write_json(path, &window_state_json())
}

fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    fs::write(path, json)?;
    set_file_mtime(path, FileTime::from_unix_time(FIXED_MTIME, 0))?;
    Ok(())
}

fn write_text(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    set_file_mtime(path, FileTime::from_unix_time(FIXED_MTIME, 0))?;
    Ok(())
}

fn write_png(path: &Path, width: u32, height: u32, start: [u8; 3], end: [u8; 3]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let image = ImageBuffer::from_fn(width, height, |x, y| {
        let fx = x as f32 / width.max(1) as f32;
        let fy = y as f32 / height.max(1) as f32;
        let mix = ((fx + fy) / 2.0).clamp(0.0, 1.0);
        let channel = |index: usize| {
            (start[index] as f32 + (end[index] as f32 - start[index] as f32) * mix).round() as u8
        };
        let stripe = if (x / 80 + y / 80) % 2 == 0 { 12 } else { 0 };
        Rgba([
            channel(0).saturating_add(stripe),
            channel(1).saturating_add(stripe),
            channel(2).saturating_add(stripe),
            255,
        ])
    });

    image.save(path).map_err(io::Error::other)?;
    set_file_mtime(path, FileTime::from_unix_time(FIXED_MTIME, 0))?;
    Ok(())
}

fn touch_tree(root: &Path, timestamp: i64) -> io::Result<()> {
    let time = FileTime::from_unix_time(timestamp, 0);
    let mut paths = Vec::new();
    collect_paths(root, &mut paths)?;

    paths.sort_by_key(|path| path.components().count());
    for path in paths.into_iter().rev() {
        set_file_mtime(path, time)?;
    }

    Ok(())
}

fn collect_paths(root: &Path, paths: &mut Vec<PathBuf>) -> io::Result<()> {
    paths.push(root.to_path_buf());
    if root.is_dir() {
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            collect_paths(&entry.path(), paths)?;
        }
    }
    Ok(())
}

fn validate_png(path: &Path, description: &str) -> io::Result<()> {
    let image = image::open(path).map_err(io::Error::other)?;
    let width = image.width();
    let height = image.height();
    if width < 800 || height < 500 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{description} is too small for the README: {}x{}",
                width, height
            ),
        ));
    }
    println!("{description}: {}x{}", width, height);
    Ok(())
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn demo_svg() -> &'static str {
    r##"<svg xmlns="http://www.w3.org/2000/svg" width="900" height="540" viewBox="0 0 900 540">
  <rect width="900" height="540" fill="#f5f7f8"/>
  <rect x="80" y="80" width="740" height="380" fill="#dbe7ef"/>
  <circle cx="250" cy="225" r="90" fill="#557a95"/>
  <rect x="390" y="160" width="260" height="42" fill="#5f8b6b"/>
  <rect x="390" y="230" width="330" height="34" fill="#c69c55"/>
  <rect x="390" y="292" width="210" height="34" fill="#7f6d9a"/>
</svg>
"##
}
