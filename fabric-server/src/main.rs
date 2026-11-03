fn main() {
    if let Err(e) = fabric_server::cli() {
        eprintln!("{e}");
        std::process::exit(1)
    }
}
