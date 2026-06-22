use std::{
    collections::HashSet,
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const MIN_GO_VERSION: (u32, u32, u32) = (1, 25, 0);
const RCLONE_VERSION: &str = "v1.74.3";
const DEFAULT_WINFSP_FUSE_INCLUDE: &str = r"C:\Program Files (x86)\WinFsp\inc\fuse";

fn main() {
    let target_triple = env::var("TARGET").expect("TARGET is set by Cargo");
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR is set by Cargo");
    let out_path = PathBuf::from(&out_dir);

    if env::var("DOCS_RS").is_ok() {
        fs::write(out_path.join("bindings.rs"), "").expect("write docs.rs bindings stub");
        return;
    }

    println!("cargo:rerun-if-changed=go.mod");
    println!("cargo:rerun-if-changed=go.sum");
    println!("cargo:rerun-if-changed=librclone.go");
    println!("cargo:rerun-if-env-changed=GO");
    println!("cargo:rerun-if-env-changed=CPATH");
    println!("cargo:rerun-if-env-changed=LIBRCLONE_DLL_PATH");
    println!("cargo:rustc-env=LIBRCLONE_BUILD_OUT_DIR={}", out_dir);

    let go = go_executable();

    if target_triple.contains("windows") {
        build_windows_shared_library(&go, &out_path);
    } else {
        build_static_library(&go, &target_triple, &out_path);
    }
}

fn build_static_library(go: &Path, target_triple: &str, out_path: &Path) {
    let ldflags = rclone_ldflags(false);
    let mut command = Command::new(go);
    command
        .args(["build", "--buildmode=c-archive", "-ldflags"])
        .arg(&ldflags)
        .arg("-o")
        .arg(out_path.join("librclone.a"))
        .arg("github.com/rclone/rclone/librclone");
    run_go_build(&mut command);

    println!("cargo:rustc-link-search=native={}", out_path.display());
    println!("cargo:rustc-link-lib=static=rclone");

    if target_triple.ends_with("darwin") {
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=IOKit");
        println!("cargo:rustc-link-lib=framework=Security");
        println!("cargo:rustc-link-lib=resolv");
    }

    let bindings = bindgen::Builder::default()
        .header(out_path.join("librclone.h").display().to_string())
        .allowlist_function("RcloneRPC")
        .allowlist_function("RcloneInitialize")
        .allowlist_function("RcloneFinalize")
        .allowlist_function("RcloneFreeString")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("unable to generate librclone bindings; make sure libclang is installed");

    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("write librclone bindings");
}

fn build_windows_shared_library(go: &Path, out_path: &Path) {
    let cpath = windows_winfsp_cpath();
    let dll_path = out_path.join("librclone.dll");
    let ldflags = rclone_ldflags(true);

    let mut command = Command::new(go);
    command
        .env("CGO_ENABLED", "1")
        .env("CPATH", &cpath)
        .args([
            "build",
            "--buildmode=c-shared",
            "-tags",
            "cmount",
            "-ldflags",
        ])
        .arg(&ldflags)
        .arg("-o")
        .arg(&dll_path)
        .arg("github.com/rclone/rclone/librclone");
    run_go_build(&mut command);

    copy_windows_dll_to_cargo_output_dirs(&dll_path);
}

fn run_go_build(command: &mut Command) {
    let status = command
        .status()
        .expect("failed to run `go build`; install Go 1.25.x and make sure `go` is on PATH");
    assert!(
        status.success(),
        "`go build` failed with status {status}; librclone requires Go 1.25.x and platform C toolchain prerequisites"
    );
}

fn rclone_ldflags(strip_symbols: bool) -> String {
    let version_flag = format!("-X github.com/rclone/rclone/fs.Version={RCLONE_VERSION}");
    if strip_symbols {
        format!("-s {version_flag}")
    } else {
        version_flag
    }
}

fn go_executable() -> PathBuf {
    let mut errors = Vec::new();

    for candidate in go_candidates() {
        match go_version(&candidate) {
            Ok((_version_output, parsed)) if parsed >= MIN_GO_VERSION => return candidate,
            Ok((version_output, _)) => errors.push(format!(
                "{} reported `{}`",
                candidate.display(),
                version_output.trim()
            )),
            Err(error) => errors.push(error),
        }
    }

    panic!(
        "failed to find Go 1.25.x or newer for librclone; tried {}",
        errors.join("; ")
    );
}

fn go_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    if let Some(go) = env::var_os("GO").filter(|value| !value.is_empty()) {
        push_unique_candidate(&mut candidates, &mut seen, PathBuf::from(go));
    }

    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            push_unique_candidate(&mut candidates, &mut seen, directory.join(go_binary_name()));
        }
    }

    push_unique_candidate(&mut candidates, &mut seen, PathBuf::from("go"));
    candidates
}

fn push_unique_candidate(
    candidates: &mut Vec<PathBuf>,
    seen: &mut HashSet<OsString>,
    candidate: PathBuf,
) {
    let key = candidate.as_os_str().to_owned();
    if seen.insert(key) {
        candidates.push(candidate);
    }
}

fn go_binary_name() -> &'static str {
    if cfg!(windows) { "go.exe" } else { "go" }
}

fn go_version(candidate: &Path) -> Result<(String, (u32, u32, u32)), String> {
    let output = Command::new(candidate)
        .arg("version")
        .output()
        .map_err(|error| format!("failed to run `{}`: {error}", candidate.display()))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let combined = format!("{stdout}{stderr}");
    if !output.status.success() {
        return Err(format!(
            "`{} version` failed with status {}: {}",
            candidate.display(),
            output.status,
            combined.trim()
        ));
    }

    let parsed = parse_go_version(&combined).ok_or_else(|| {
        format!(
            "unable to parse `{}` version output `{}`",
            candidate.display(),
            combined.trim()
        )
    })?;

    Ok((combined, parsed))
}

fn parse_go_version(output: &str) -> Option<(u32, u32, u32)> {
    output.split_whitespace().find_map(|token| {
        let version = token.strip_prefix("go")?;
        if !version
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
        {
            return None;
        }

        let version = version
            .chars()
            .take_while(|character| character.is_ascii_digit() || *character == '.')
            .collect::<String>();
        let mut parts = version.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next().unwrap_or("0").parse().ok()?;
        Some((major, minor, patch))
    })
}

fn windows_winfsp_cpath() -> OsString {
    match env::var_os("CPATH").filter(|value| !value.is_empty()) {
        Some(cpath) => {
            assert!(
                cpath_has_winfsp_headers(&cpath),
                "CPATH is set but does not contain WinFsp FUSE headers. Install WinFsp with the Developer feature and include `{DEFAULT_WINFSP_FUSE_INCLUDE}` in CPATH."
            );
            cpath
        }
        None => {
            let default = PathBuf::from(DEFAULT_WINFSP_FUSE_INCLUDE);
            assert!(
                default.join("fuse.h").is_file(),
                "WinFsp FUSE headers were not found at `{DEFAULT_WINFSP_FUSE_INCLUDE}`. Install WinFsp with the Developer feature selected, or set CPATH to the WinFsp fuse include directory."
            );
            default.into_os_string()
        }
    }
}

fn cpath_has_winfsp_headers(cpath: &OsString) -> bool {
    env::split_paths(cpath).any(|path| path.join("fuse.h").is_file())
}

fn copy_windows_dll_to_cargo_output_dirs(dll_path: &Path) {
    let Some(profile_dir) = cargo_profile_dir_from_out_dir(dll_path) else {
        println!(
            "cargo:warning=Unable to infer Cargo profile directory from {}; librclone.dll remains in OUT_DIR",
            dll_path.display()
        );
        return;
    };

    copy_file(dll_path, &profile_dir.join("librclone.dll"));

    let deps_dir = profile_dir.join("deps");
    fs::create_dir_all(&deps_dir).expect("create Cargo deps output directory for librclone.dll");
    copy_file(dll_path, &deps_dir.join("librclone.dll"));
}

fn cargo_profile_dir_from_out_dir(dll_path: &Path) -> Option<PathBuf> {
    let out_dir = dll_path.parent()?;
    out_dir.ancestors().nth(3).map(Path::to_path_buf)
}

fn copy_file(source: &Path, destination: &Path) {
    fs::copy(source, destination).unwrap_or_else(|error| {
        panic!(
            "failed to copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    });
}
