//! Quick utility to list all pallets in a runtime

use subxt::{OnlineClient, PolkadotConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "wss://westend-rpc.polkadot.io:443".to_string());

    println!("Connecting to {}...", url);
    let client = OnlineClient::<PolkadotConfig>::from_url(&url).await?;

    println!("\nAvailable pallets:");
    let metadata = client.metadata();

    for pallet in metadata.pallets() {
        let name = pallet.name();
        // Highlight migration-related pallets
        if name.to_lowercase().contains("migrat")
            || name.to_lowercase().contains("trie")
            || name.to_lowercase().contains("state")
        {
            println!("  >>> {} <<<", name);
        } else {
            println!("  {}", name);
        }
    }

    Ok(())
}
