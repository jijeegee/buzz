fn main() {
    if let Err(e) = dispatch() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn dispatch() -> Result<(), String> {
    let argv0 = std::env::args().next().unwrap_or_default();
    let cmd = std::path::Path::new(&argv0)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match cmd.as_str() {
        "kuzz-acp" | "git-credential-nostr" | "git-credential-kuzz" | "git-sign-nostr" => {
            kuzz_acp::run().map_err(|e| e.to_string())
        }
        "kuzz-agent" => kuzz_agent::run().map_err(|e| e.to_string()),
        "sprig" => match std::env::args().nth(1).as_deref() {
            Some("-V") | Some("--version") => {
                println!("sprig {}", env!("CARGO_PKG_VERSION"));
                Ok(())
            }
            Some("-h") | Some("--help") | None => {
                print_usage();
                if std::env::args().len() <= 1 {
                    Err("error: invoke Sprig via a personality symlink".into())
                } else {
                    Ok(())
                }
            }
            Some(other) => {
                print_usage();
                Err(format!(
                    "error: unknown Sprig option or personality: {other}"
                ))
            }
        },
        // kuzz-dev-mcp handles its utility multicall names: rg, tree, kuzz.
        _ => kuzz_dev_mcp::run().map_err(|e| e.to_string()),
    }
}

fn print_usage() {
    println!(
        "Sprig — all-in-one Kuzz ACP harness, agent, and developer MCP\n\n\
Sprig is a multicall binary. Invoke it through one of the personality names:\n\n\
  kuzz-acp       ACP harness\n  kuzz-agent     ACP-compliant agent\n  kuzz-dev-mcp   Developer MCP server\n\n\
Helper names are also supported: rg, tree, kuzz, git-credential-nostr, git-credential-kuzz, git-sign-nostr.\n\n\
Installers can create links with:\n  ln -s sprig kuzz-acp\n  ln -s sprig kuzz-agent\n  ln -s sprig kuzz-dev-mcp"
    );
}
