use std::path::PathBuf;
use std::{fs::read, str::FromStr};

use anyhow::{anyhow, Error, Result};
#[cfg(feature = "build-binary")]
use argh::FromArgs;

use skyrim_cell_dump::parse_plugin;

enum Format {
    Json,
    PlainText,
}

impl FromStr for Format {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_lowercase().as_str() {
            "json" => Ok(Format::Json),
            "text" | "plain" | "plain_text" | "plaintext" => Ok(Format::PlainText),
            _ => Err(anyhow!("Unrecognized format {}", s)),
        }
    }
}

#[derive(FromArgs)]
/// Extracts cell edits from a TES5 Skyrim plugin file
struct Args {
    /// path to the plugin to parse
    #[argh(positional)]
    plugin: PathBuf,
    /// format of the output (json or text)
    #[argh(option, short = 'f', default = "Format::PlainText")]
    format: Format,
    /// pretty print json output
    #[argh(switch, short = 'p')]
    pretty: bool,
}

fn main() {
    let args: Args = argh::from_env();
    let plugin_contents = match read(&args.plugin) {
        Ok(contents) => contents,
        Err(error) => {
            return eprintln!(
                "Failed to read from plugin file {}: {}",
                &args.plugin.to_string_lossy(),
                error
            )
        }
    };
    let plugin = match parse_plugin(&plugin_contents) {
        Ok(plugin) => plugin,
        Err(error) => {
            return eprintln!(
                "Failed to parse plugin file {}: {}",
                &args.plugin.to_string_lossy(),
                error
            )
        }
    };

    match args.format {
        Format::PlainText => println!("{:#?}", &plugin),
        Format::Json if args.pretty => {
            println!("{}", serde_json::to_string_pretty(&plugin).unwrap())
        }
        Format::Json => println!("{}", serde_json::to_string(&plugin).unwrap()),
    }
}
