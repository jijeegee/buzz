fn main() -> anyhow::Result<()> {
    kuzz_core::legacy_env::adopt_legacy_env();
    kuzz_acp::run()
}
