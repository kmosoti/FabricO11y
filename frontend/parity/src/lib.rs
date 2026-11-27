#[path = "../../fixtures/scenario.rs"]
mod scenario;
pub fn recorded_output() -> String {
    scenario::run()
}
#[cfg(target_arch = "wasm32")]
fn output() -> &'static String {
    static OUTPUT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    OUTPUT.get_or_init(recorded_output)
}
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn fixture_ptr() -> *const u8 {
    output().as_ptr()
}
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn fixture_len() -> usize {
    output().len()
}
