fn main() {
    glib_build_tools::compile_resources(
        &["resources"],
        "resources/zelador.gresource.xml",
        "zelador.gresource",
    );
}
