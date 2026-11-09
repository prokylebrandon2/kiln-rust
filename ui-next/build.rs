fn main() {
    // lib.slint exports the app window and the preview window; compiling both checks both.
    slint_build::compile("ui/lib.slint").expect("the Slint UI failed to compile");
}
