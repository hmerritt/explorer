fn main() {
    #[cfg(target_os = "windows")]
    {
        let resource = "assets/windows/app.rc";

        println!("cargo:rerun-if-changed={resource}");
        println!("cargo:rerun-if-changed=assets/explorer.ico");

        embed_resource::compile(resource, embed_resource::NONE)
            .manifest_optional()
            .expect("failed to embed Windows resources");
    }
}
