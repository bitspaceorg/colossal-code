mod app;

use color_eyre::Result;

#[tokio::main]
async fn main() -> Result<()> {
    app::run().await
}
