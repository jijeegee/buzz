fn main() {
    kuzz_core::legacy_env::adopt_legacy_env();
    async_main();
}

#[tokio::main]
async fn async_main() {
    std::process::exit(kuzz_cli::run_from_args(std::env::args()).await);
}
