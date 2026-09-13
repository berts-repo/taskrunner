#[tokio::main]
async fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = taskrunner::cli::main(&argv).await;
    std::process::exit(code);
}
