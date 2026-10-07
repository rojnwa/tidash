fn main() {
    // English is the source language; any `po/<tag>.po` added later is compiled in.
    fastframe_i18n::build::compile_catalogs("po");
}
