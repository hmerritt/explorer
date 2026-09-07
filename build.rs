fn main() {
    #[cfg(target_os = "windows")]
    {
        use std::{env, fs, path::PathBuf};

        let resource_template = "assets/windows/app.rc";
        let version = env::var("CARGO_PKG_VERSION").expect("Cargo package version is available");
        let mut parts = version
            .split('.')
            .take(3)
            .map(|part| part.parse::<u16>().unwrap_or(0))
            .collect::<Vec<_>>();
        parts.resize(3, 0);
        let version_commas = format!("{},{},{},0", parts[0], parts[1], parts[2]);
        let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
        let icon_path = manifest_dir
            .join("assets")
            .join("explorer.ico")
            .display()
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        let resource = fs::read_to_string(resource_template)
            .expect("failed to read Windows resource template")
            .replace("@ICON_PATH@", &icon_path)
            .replace("@VERSION_COMMAS@", &version_commas)
            .replace("@VERSION@", &version);
        let generated_resource = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("app.rc");
        fs::write(&generated_resource, resource).expect("failed to generate Windows resources");

        println!("cargo:rerun-if-changed={resource_template}");
        println!("cargo:rerun-if-changed=assets/explorer.ico");

        embed_resource::compile(&generated_resource, embed_resource::NONE)
            .manifest_optional()
            .expect("failed to embed Windows resources");
    }
}
