#[macro_use]
extern crate bitflags;

mod parser;

pub use parser::{decompress_cells, parse_cell, parse_plugin};
