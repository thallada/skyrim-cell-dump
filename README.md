# skyrim-cell-dump

Library and binary for parsing Skyrim plugin files and extracting CELL data.

The main objective of this library is to extract the form ID and X and Y coordinates of every exterior cell a plugin edits as fast as possible, ignoring the rest of the plugin.

## Install

```
cargo install skyrim-cell-dump
```

Or, build yourself by checking out the repository and running:

```
cargo build --release --features build-binary
```

## Usage

```
Usage: skyrim-cell-dump.exe <plugin> [-f <format>] [-p]

Extracts cell edits from a TES5 Skyrim plugin file

Options:
  -f, --format      format of the output (json or text)
  -p, --pretty      pretty print json output
  --help            display usage information
```

The pretty JSON format looks something like:

```json
{
  "header": {
    "version": 1.0,
    "num_records_and_groups": 792,
    "next_object_id": 221145,
    "author": "Critterman",
    "description": "An example plugin",
    "masters": [
      "Skyrim.esm",
      "Update.esm",
      "Dawnguard.esm",
      "HearthFires.esm",
      "Dragonborn.esm"
    ]
  },
  "worlds": [
    {
      "form_id": 60,
      "editor_id": "Tamriel"
    }
  ],
  "cells": [
    {
      "form_id": 100000001,
      "editor_id": "SomeInterior",
      "x": null,
      "y": null,
      "world_form_id": null,
      "is_persistent": false
    },
    {
      "form_id": 3444,
      "editor_id": null,
      "x": 0,
      "y": 0,
      "world_form_id": 60,
      "is_persistent": true
    },
    {
      "form_id": 46432,
      "editor_id": "SomeExterior01",
      "x": 32,
      "y": 3,
      "world_form_id": 60,
      "is_persistent": false
    },
    {
      "form_id": 46464,
      "editor_id": "SomeExterior02",
      "x": 33,
      "y": 2,
      "world_form_id": 60,
      "is_persistent": false
    },
    {
      "form_id": 46498,
      "editor_id": null,
      "x": 32,
      "y": 1,
      "world_form_id": 60,
      "is_persistent": false
    }
  ]
}
```

Note: I have only tested parsing Skyrim Special Edition `.esp`, `.esm`, and `.esl` files.

## Import

You can include this crate in your `Cargo.toml` and get the parsed `Plugin` struct with:

```rust
use skyrim_cell_dump::parse_plugin;

let plugin_contents = std::fs::read("Plugin.esp").unwrap();
let plugin = parse_plugin(&plugin_contents).unwrap();
```
