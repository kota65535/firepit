use firepit::{cli, panic};

#[tokio::main]
async fn main() {
    std::panic::set_hook(Box::new(panic::panic_handler));

    cli::run().await.unwrap();
    std::process::exit(0);
}
