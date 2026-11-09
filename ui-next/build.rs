fn main() {
    // lib.slint exports the app window and the preview window; compiling both checks both.
    //
    // The only stock widget is `ListView` (list.slint). It needs a style, and the default one
    // (Fluent) embeds SVG images that need a decoder this build leaves out, so rustc fails on the
    // generated code. Cupertino draws nothing from images, and its scrollbar is the thin overlay
    // one that a touch screen expects.
    let config = slint_build::CompilerConfiguration::new().with_style("cupertino".into());
    slint_build::compile_with_config("ui/lib.slint", config)
        .expect("the Slint UI failed to compile");
}
