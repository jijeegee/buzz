fn main() {
    if let Err(e) = kuzz_agent::run() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}
