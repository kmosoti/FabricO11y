#[cfg(target_arch = "wasm32")]
fn main() {
    leptos::mount::mount_to_body(fabric_ui::app::App);
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!(
        "The console runs in a browser. Build with tools/ui/build.py under the resource launcher."
    );
}
