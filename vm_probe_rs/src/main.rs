use anyhow::Result;
use vim_rs::core::Client;
use vim_rs::vim25;

#[tokio::main]
async fn main() -> Result<()> {
    // Client creation
    println!("vim_rs version: {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
