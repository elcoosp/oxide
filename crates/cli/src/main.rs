use clap::Parser as _;

// SPEC-13 / §12.2: mimalloc as the global allocator — lower idle RSS and
// less fragmentation than the system allocator under Oxide's alloc-heavy
// request path.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let cli = oxide_fw_cli::Cli::parse();
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    std::process::exit(rt.block_on(oxide_fw_cli::run(cli)).unwrap_or(1));
}
