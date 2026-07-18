mod binary;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    binary::run().await
}
