use crate::config::Config;

mod config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::load(None)?;
    println!("{config:#?}");
    Ok(())
}
