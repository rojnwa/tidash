fn main() {
    // English is the source language; any `po/<tag>.po` added later is compiled in.
    fastframe_i18n::build::compile_catalogs("po");
    // The exe's name in Task Manager, the volume mixer and the media overlay, and its icon.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let icon = "../../packaging/windows/tidash.ico";
        println!("cargo:rerun-if-changed={icon}");
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon(icon)
            .set("ProductName", "Tidash")
            .set("FileDescription", "Tidash");
        if let Err(e) = resource.compile() {
            panic!("Windows resources not embedded: {e}");
        }
    }
}
