//! # Skyrim Cell Dump
//!
//! `skyrim-cell-dump` is a library for parsing Skyrim plugin files and extracting CELL data into Rust structs.
#[macro_use]
extern crate bitflags;

mod parser;

pub use parser::{parse_plugin, Cell, Plugin, PluginHeader};
