#!/usr/bin/env -S cargo +nightly -Zscript
---
[package]
edition = "2024"

[dependencies]
# Script doesn't support workspaces yet so we have to redefine deps
schemars = "1.0"
slumber_config = {path = "../crates/config", features = ["schema"]}
slumber_core = {path = "../crates/core", features = ["schema"]}
serde_json = "1.0"
---
//! Script to generate JSON Schemas for Slumber entities. This is a separate
//! script because:
//! - It's only needed for developers, so it doesn't belong in the CLI
//! - Script minimizes the amount of code needed

use slumber_config::Config;
use slumber_core::collection::Collection;

fn main() {
    const OPTIONS: &str = "`collection` or `config`";

    let schema = match std::env::args().nth(1).as_deref() {
        Some("collection") => schemars::schema_for!(Collection),
        Some("config") => schemars::schema_for!(Config),
        Some(other) => {
            panic!("Unexpected target `{other}`; must be one of {OPTIONS}")
        }
        None => panic!("Missing target; specify {OPTIONS}"),
    };

    let json = serde_json::to_string_pretty(&schema).unwrap();
    println!("{json}");
}
