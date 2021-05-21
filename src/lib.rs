//! # Skyrim Cell Dump
//!
//! `skyrim-cell-dump` is a library for parsing Skyrim plugin files and extracing CELL data into Rust structs.
#[macro_use]
extern crate bitflags;

mod parser;

pub use parser::{parse_plugin, Cell, Plugin, PluginHeader};
