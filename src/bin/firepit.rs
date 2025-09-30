use console::Style;
use firepit::{cli, panic};

#[tokio::main]
async fn main() {
    std::panic::set_hook(Box::new(panic::panic_handler));

    match cli::run().await {
        Ok(ret) => {
            std::process::exit(ret);
        }
        Err(e) => {
            eprintln!("{}", Style::new().red().apply_to(format!("Error: {:?}", e)));
            std::process::exit(1);
        }
    }
}
