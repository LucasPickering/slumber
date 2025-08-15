#!/usr/bin/env -S cargo +nightly-2025-06-20 -Zscript
---
[package]
edition = "2024"

[dependencies]
doc_utils = {path = "../crates/doc_utils"}
---
//! Mdbook preprocessor for generating docs from Rust code. This is set up as a
//! mdbook preprocessor, and will replace any occurrences of the string `<!--
//! template_functions -->` with the generated markdown.
//!
//! This is implemented in Rust because we need:
//! - syn for parsing some content
//! - mdbook library for handling the book

use std::process::ExitCode;

fn main() -> ExitCode {
    match doc_utils::mdbook() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}
