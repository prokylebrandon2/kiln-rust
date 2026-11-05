fn main() {
    // No `std-widgets`: nothing in ui/ imports a stock style, so no style is selected either.
    slint_build::compile("ui/main.slint").expect("the Slint UI failed to compile");
}
