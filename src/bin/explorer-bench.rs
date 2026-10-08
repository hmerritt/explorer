fn main() {
    if let Err(error) = explorer::performance::main() {
        eprintln!("explorer-bench: {error}");
        std::process::exit(1);
    }
}
