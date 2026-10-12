use clap::Parser as _;

fn main() {
    let cli = oxide_cli::Cli::parse();
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    std::process::exit(rt.block_on(oxide_cli::run(cli)).unwrap_or(1));
}
