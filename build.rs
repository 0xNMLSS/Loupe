fn main() {
    // Compile and link `app.rc` into the binary so the icon (resource id 1)
    // is embedded in lens.exe and visible to LoadIconW at runtime.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_resource::compile("app.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("failed to compile app.rc");
    }
}
