fn main() -> Result<(), Box<dyn std::error::Error>> {
    kuzz_core::legacy_env::adopt_legacy_env();
    kuzz_dev_mcp::run()
}
