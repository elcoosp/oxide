use clap::Parser as _;

// SPEC-13 / §12.2: mimalloc as the global allocator — lower idle RSS and
// less fragmentation than the system allocator under Oxide's alloc-heavy
// request path.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let cli = oxide_fw_cli::Cli::parse();
    // P1 (readiness): startup env writes (OXIDE_DEV / OXIDE_ROOT /
    // OXIDE_DEV_MODE / OXIDE_DEV_NO_ESM / OXIDE_OFFLINE_PLUGINS) and the
    // in-process dev-mode atomic happen HERE — before the tokio runtime and
    // its worker threads exist, so `set_var` can never race a request
    // thread's `get_var`.
    oxide_fw_cli::apply_startup_env(&cli);
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    std::process::exit(rt.block_on(oxide_fw_cli::run(cli)).unwrap_or(1));
}
