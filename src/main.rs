use crate::config::RequestCollection;

mod config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let collection = RequestCollection::load(None).await?;
    println!("{collection:#?}");
    Ok(())
}
