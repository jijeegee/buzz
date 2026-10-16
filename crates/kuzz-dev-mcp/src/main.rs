fn main() -> Result<(), Box<dyn std::error::Error>> {
    kuzz_core::legacy_env::adopt_legacy_env();
    buzz_dev_mcp::run()
}
