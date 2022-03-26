use std::borrow::Cow;
use std::io::Read;
use std::{convert::TryInto, str};

use anyhow::{anyhow, Result};
use encoding_rs::WINDOWS_1252;
use flate2::read::ZlibDecoder;
use nom::{
    branch::alt,
    bytes::complete::{take, take_while},
    combinator::{map, map_res, verify},
    number::complete::{le_f32, le_i32, le_u16, le_u32},
    IResult,
};
use serde::Serialize;

const RECORD_HEADER_SIZE: u32 = 24;
const FIELD_HEADER_SIZE: u32 = 6;

/// A parsed TES5 Skyrim plugin file
#[derive(Debug, PartialEq, Serialize)]
pub struct Plugin<'a> {
    /// Parsed [TES4 header record](https://en.uesp.net/wiki/Skyrim_Mod:Mod_File_Format/TES4) with metadata about the plugin
    pub header: PluginHeader<'a>,
    /// Parsed [WRLD records](https://en.uesp.net/wiki/Skyrim_Mod:Mod_File_Format/WRLD) contained in the plugin
    pub worlds: Vec<World>,
    /// Parsed [CELL records](https://en.uesp.net/wiki/Skyrim_Mod:Mod_File_Format/CELL) contained in the plugin
    pub cells: Vec<Cell>,
}

/// Parsed [TES4 header record](https://en.uesp.net/wiki/Skyrim_Mod:Mod_File_Format/TES4)
#[derive(Debug, PartialEq, Serialize)]
pub struct PluginHeader<'a> {
    pub version: f32,
    pub num_records_and_groups: i32,
    pub next_object_id: u32,
    pub author: Option<Cow<'a, str>>,
    pub description: Option<Cow<'a, str>>,
    pub masters: Vec<Cow<'a, str>>,
}

/// Parsed [CELL records](https://en.uesp.net/wiki/Skyrim_Mod:Mod_File_Format/CELL)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cell {
    pub form_id: u32,
    pub editor_id: Option<String>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    /// The [`World`] that this cell belongs to.
    pub world_form_id: Option<u32>,
    /// Indicates that this cell is a special persistent worldspace cell where all persistent references for the worldspace are stored
    pub is_persistent: bool,
}

#[derive(Debug)]
struct CellData {
    editor_id: Option<String>,
    x: Option<i32>,
    y: Option<i32>,
}

#[derive(Debug)]
pub struct UnparsedCell<'a> {
    form_id: u32,
    world_form_id: Option<u32>,
    is_compressed: bool,
    is_persistent: bool,
    data: &'a [u8],
}

/// A CELL record that has had it's header parsed and data decompressed, but not yet parsed into individual fields
#[derive(Debug)]
struct DecompressedCell {
    pub form_id: u32,
    world_form_id: Option<u32>,
    pub is_persistent: bool,
    pub data: Vec<u8>,
}

/// Parsed [WRLD records](https://en.uesp.net/wiki/Skyrim_Mod:Mod_File_Format/WRLD)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct World {
    /// Note that this `form_id` is relative to the plugin file, not what it would be in-game.
    /// The first byte of the `form_id` can be interpreted as an index into the `masters` array of the [`PluginHeader`].
    /// That master plugin is the "owner" of the `World` and this plugin is editing it.
    ///
    /// If the first byte of the `form_id` is the length of the `masters` array, then this plugin owns the `World`.
    pub form_id: u32,
    pub editor_id: String,
}

#[derive(Debug)]
struct GroupHeader<'a> {
    size: u32,
    label: &'a [u8; 4],
    group_type: i32,
    timestamp: u16,
    version_control_info: u16,
}

#[derive(Debug)]
struct RecordHeader<'a> {
    record_type: &'a str,
    size: u32,
    flags: RecordFlags,
    id: u32,
    timestamp: u16,
    version_control_info: u16,
    version: u16,
}

bitflags! {
    struct RecordFlags: u32 {
        const MASTER_FILE = 0x00000001;
        const DELETED_GROUP = 0x00000010;
        const DELETED_RECORD = 0x00000020;
        const CONSTANT = 0x00000040;
        const LOCALIZED = 0x00000080;
        const INACCESSIBLE = 0x00000100;
        const LIGHT_MASTER_FILE = 0x00000200;
        const PERSISTENT_REFR = 0x00000400;
        const INITIALLY_DISABLED = 0x00000800;
        const IGNORED = 0x00001000;
        const VISIBLE_WHEN_DISTANT = 0x00008000;
        const RANDOM_ANIM_START = 0x00010000;
        const OFF_LIMITS = 0x00020000;
        const COMPRESSED = 0x00040000;
        const CANT_WAIT = 0x00080000;
        const IGNORE_OBJECT_INTERACTION = 0x00100000;
        const IS_MARKER = 0x00800000;
        const NO_AI_ACQUIRE = 0x02000000;
        const NAVMESH_FILTER = 0x04000000;
        const NAVMESH_BOUNDING_BOX = 0x08000000;
        const REFLECTED_BY_AUTO_WATER = 0x10000000;
        const DONT_HAVOK_SETTLE = 0x20000000;
        const NO_RESPAWN = 0x40000000;
        const MULTI_BOUND = 0x80000000;
    }
}

#[derive(Debug)]
enum Header<'a> {
    Group(GroupHeader<'a>),
    Record(RecordHeader<'a>),
}

#[derive(Debug)]
struct FieldHeader<'a> {
    field_type: &'a str,
    size: u16,
}

/// Parses fields from the decompressed bytes of a CELL record. Returns remaining bytes of the input after parsing and the parsed Cell struct.
fn parse_cell<'a>(
    input: &'a [u8],
    form_id: u32,
    is_persistent: bool,
    world_form_id: Option<u32>,
) -> IResult<&'a [u8], Cell> {
    let (input, cell_data) = parse_cell_fields(input)?;
    Ok((
        input,
        Cell {
            form_id,
            editor_id: cell_data.editor_id,
            x: cell_data.x,
            y: cell_data.y,
            world_form_id,
            is_persistent,
        },
    ))
}

/// Maps the input `UnparsedCell`s to `DecompressedCell`s and decompresses the zlib compressed data sections of the record if necessary
fn decompress_cells(unparsed_cells: Vec<UnparsedCell>) -> Result<Vec<DecompressedCell>> {
    let mut decompressed_cells = Vec::new();
    for unparsed_cell in unparsed_cells {
        let decompressed_data = if unparsed_cell.is_compressed {
            let mut buf = Vec::new();
            let mut decoder = ZlibDecoder::new(&unparsed_cell.data[4..]);
            decoder.read_to_end(&mut buf)?;
            buf
        } else {
            unparsed_cell.data.to_vec()
        };
        decompressed_cells.push(DecompressedCell {
            form_id: unparsed_cell.form_id,
            world_form_id: unparsed_cell.world_form_id,
            is_persistent: unparsed_cell.is_persistent,
            data: decompressed_data,
        });
    }
    Ok(decompressed_cells)
}

/// Parses the plugin header and finds and extracts the headers and unparsed (and possibly compressed) data sections of every CELL record in the file.
fn parse_header_and_cell_bytes(
    input: &[u8],
) -> IResult<&[u8], (PluginHeader, Vec<World>, Vec<UnparsedCell>)> {
    let (input, header) = parse_plugin_header(input)?;
    let (input, (worlds, unparsed_cells)) = parse_group_data(input, input.len() as u32, 0, None)?;
    Ok((input, (header, worlds, unparsed_cells)))
}

/// Parses header and cell records from input bytes of a plugin file and outputs `Plugin` struct with extracted fields.
///
/// # Arguments
///
/// * `input` - A slice of bytes read from the plugin file
///
/// # Examples
///
/// ```
/// use skyrim_cell_dump::parse_plugin;
///
/// let plugin_contents = std::fs::read("Plugin.esp").unwrap();
/// let plugin = parse_plugin(&plugin_contents).unwrap();
/// ```
pub fn parse_plugin(input: &[u8]) -> Result<Plugin> {
    let (_, (header, worlds, unparsed_cells)) = parse_header_and_cell_bytes(&input)
        .map_err(|_err| anyhow!("Failed to parse plugin header and find CELL data"))?;
    let decompressed_cells = decompress_cells(unparsed_cells)?;

    let mut cells = Vec::new();
    for decompressed_cell in decompressed_cells {
        let (_, cell) = parse_cell(
            &decompressed_cell.data,
            decompressed_cell.form_id,
            decompressed_cell.is_persistent,
            decompressed_cell.world_form_id,
        )
        .unwrap();
        cells.push(cell);
    }

    Ok(Plugin {
        header,
        worlds,
        cells,
    })
}

fn parse_group_data<'a>(
    input: &'a [u8],
    remaining_bytes: u32,
    depth: usize,
    world_form_id: Option<u32>,
) -> IResult<&'a [u8], (Vec<World>, Vec<UnparsedCell>)> {
    let mut input = input;
    let mut worlds = vec![];
    let mut cells = vec![];
    let mut consumed_bytes = 0;
    let mut world_form_id = world_form_id;
    while !input.is_empty() && consumed_bytes < remaining_bytes {
        let (remaining, record_header) = parse_header(input)?;
        match record_header {
            Header::Group(group_header) => {
                if group_header.group_type == 0 {
                    // TODO: get rid of unwrap
                    let label = str::from_utf8(group_header.label).unwrap();
                    if label != "WRLD" && label != "CELL" {
                        let (remaining, _) =
                            take(group_header.size - RECORD_HEADER_SIZE)(remaining)?;
                        input = remaining;
                        consumed_bytes += group_header.size;
                        continue;
                    } else {
                        // reset world_form_id when entering new worldspace/cell group
                        world_form_id = None;
                    }
                } else if group_header.group_type == 7 {
                    // TODO: DRY
                    let (remaining, _) = take(group_header.size - RECORD_HEADER_SIZE)(remaining)?;
                    input = remaining;
                    consumed_bytes += group_header.size;
                    continue;
                }
                let (remaining, (mut inner_worlds, mut inner_cells)) = parse_group_data(
                    remaining,
                    group_header.size - RECORD_HEADER_SIZE,
                    depth + 1,
                    world_form_id,
                )?;
                worlds.append(&mut inner_worlds);
                cells.append(&mut inner_cells);
                input = remaining;
                consumed_bytes += group_header.size;
            }
            Header::Record(record_header) => match record_header.record_type {
                "CELL" => {
                    let (remaining, data) = take(record_header.size)(remaining)?;
                    cells.push(UnparsedCell {
                        form_id: record_header.id,
                        world_form_id,
                        is_compressed: record_header.flags.contains(RecordFlags::COMPRESSED),
                        is_persistent: record_header.flags.contains(RecordFlags::PERSISTENT_REFR),
                        data,
                    });
                    input = remaining;
                    consumed_bytes += record_header.size + RECORD_HEADER_SIZE;
                }
                "WRLD" => {
                    world_form_id = Some(record_header.id);
                    let (remaining, editor_id) = parse_world_fields(remaining, &record_header)?;
                    worlds.push(World {
                        form_id: record_header.id,
                        editor_id,
                    });
                    input = remaining;
                    consumed_bytes += record_header.size + RECORD_HEADER_SIZE;
                }
                _ => {
                    let (remaining, _) = take(record_header.size)(remaining)?;
                    input = remaining;
                    consumed_bytes += record_header.size + RECORD_HEADER_SIZE;
                }
            },
        }
    }
    Ok((input, (worlds, cells)))
}

fn parse_plugin_header(input: &[u8]) -> IResult<&[u8], PluginHeader> {
    let (mut input, tes4) = verify(parse_record_header, |record_header| {
        record_header.record_type == "TES4"
    })(input)?;
    let mut consumed_bytes = 0;
    let (remaining, hedr) = verify(parse_field_header, |field_header| {
        field_header.field_type == "HEDR"
    })(input)?;
    consumed_bytes += hedr.size as u32 + FIELD_HEADER_SIZE;
    input = remaining;
    let (remaining, (version, num_records_and_groups, next_object_id)) = parse_hedr_fields(input)?;
    input = remaining;
    let mut author = None;
    let mut description = None;
    let mut masters = vec![];
    let mut large_size = None;
    while consumed_bytes < tes4.size as u32 {
        let (remaining, field) = parse_field_header(input)?;
        if let Some(size) = large_size {
            consumed_bytes += size + FIELD_HEADER_SIZE;
        } else {
            consumed_bytes += field.size as u32 + FIELD_HEADER_SIZE;
        }
        input = remaining;
        match field.field_type {
            "CNAM" => {
                let (remaining, author_str) = parse_zstring(input)?;
                input = remaining;
                author = Some(author_str);
            }
            "SNAM" => {
                let (remaining, desc_str) = parse_zstring(input)?;
                input = remaining;
                description = Some(desc_str);
            }
            "MAST" => {
                let (remaining, master_str) = parse_zstring(input)?;
                input = remaining;
                masters.push(master_str);
            }
            "INTV" => {
                let (remaining, _) = take(field.size)(input)?;
                input = remaining;
            }
            "XXXX" => {
                let (remaining, size) = le_u32(input)?;
                input = remaining;
                large_size = Some(size);
            }
            _ => {
                if let Some(size) = large_size {
                    let (remaining, _) = take(size)(input)?;
                    input = remaining;
                    large_size = None;
                } else {
                    let (remaining, _) = take(field.size)(input)?;
                    input = remaining;
                }
            }
        }
    }
    Ok((
        input,
        PluginHeader {
            version,
            num_records_and_groups,
            next_object_id,
            author,
            description,
            masters,
        },
    ))
}

fn parse_group_header(input: &[u8]) -> IResult<&[u8], GroupHeader> {
    let (input, _record_type) =
        verify(parse_4char, |record_type: &str| record_type == "GRUP")(input)?;
    let (input, size) = le_u32(input)?;
    let (input, label) = map_res(take(4usize), |bytes: &[u8]| bytes.try_into())(input)?;
    let (input, group_type) = le_i32(input)?;
    let (input, timestamp) = le_u16(input)?;
    let (input, version_control_info) = le_u16(input)?;
    let (input, _) = take(4usize)(input)?;
    Ok((
        input,
        GroupHeader {
            size,
            label,
            group_type,
            timestamp,
            version_control_info,
        },
    ))
}

fn parse_record_header(input: &[u8]) -> IResult<&[u8], RecordHeader> {
    let (input, record_type) =
        verify(parse_4char, |record_type: &str| record_type != "GRUP")(input)?;
    let (input, size) = le_u32(input)?;
    let (input, flag_bits) = le_u32(input)?;
    // Okay to truncate since we only care about bits we know about and don't want to crash on unknown bits.
    let flags = RecordFlags::from_bits_truncate(flag_bits);
    let (input, id) = le_u32(input)?;
    let (input, timestamp) = le_u16(input)?;
    let (input, version_control_info) = le_u16(input)?;
    let (input, version) = le_u16(input)?;
    let (input, _) = take(2usize)(input)?;
    Ok((
        input,
        RecordHeader {
            record_type,
            size,
            flags,
            id,
            timestamp,
            version_control_info,
            version,
        },
    ))
}

fn parse_header(input: &[u8]) -> IResult<&[u8], Header> {
    alt((
        map(parse_group_header, |group_header| {
            Header::Group(group_header)
        }),
        map(parse_record_header, |record_header| {
            Header::Record(record_header)
        }),
    ))(input)
}

fn parse_field_header(input: &[u8]) -> IResult<&[u8], FieldHeader> {
    let (input, field_type) = parse_4char(input)?;
    let (input, size) = le_u16(input)?;
    Ok((input, FieldHeader { field_type, size }))
}

fn parse_hedr_fields(input: &[u8]) -> IResult<&[u8], (f32, i32, u32)> {
    let (input, version) = le_f32(input)?;
    let (input, num_records_and_groups) = le_i32(input)?;
    let (input, next_object_id) = le_u32(input)?;
    Ok((input, (version, num_records_and_groups, next_object_id)))
}

fn parse_cell_fields<'a>(input: &'a [u8]) -> IResult<&'a [u8], CellData> {
    let mut cell_data = CellData {
        editor_id: None,
        x: None,
        y: None,
    };
    let mut input = input;
    let mut large_size = None;
    while !input.is_empty() {
        let (remaining, field) = parse_field_header(input)?;
        input = remaining;
        match field.field_type {
            "EDID" => {
                let (remaining, editor_id) = parse_zstring(input)?;
                cell_data.editor_id = Some(editor_id.to_string());
                input = remaining;
            }
            "XCLC" => {
                let (remaining, x) = le_i32(input)?;
                let (remaining, y) = le_i32(remaining)?;
                cell_data.x = Some(x);
                cell_data.y = Some(y);
                if field.size == 12 {
                    // older (v. 0.94) files don't have the flags in this struct
                    let (remaining, _) = take(4usize)(remaining)?;
                    input = remaining;
                } else {
                    input = remaining;
                }
            }
            "XXXX" => {
                let (remaining, size) = le_u32(input)?;
                input = remaining;
                large_size = Some(size);
            }
            _ => {
                if let Some(size) = large_size {
                    let (remaining, _) = take(size)(input)?;
                    input = remaining;
                    large_size = None;
                } else {
                    let (remaining, _) = take(field.size)(input)?;
                    input = remaining;
                }
            }
        }
    }
    Ok((input, cell_data))
}

fn parse_world_fields<'a>(
    input: &'a [u8],
    record_header: &RecordHeader,
) -> IResult<&'a [u8], String> {
    let (remaining, field) = verify(parse_field_header, |field_header| {
        field_header.field_type == "EDID"
    })(input)?;
    let (remaining, editor_id) = parse_zstring(remaining)?;
    let record_bytes_left =
        record_header.size as usize - field.size as usize - FIELD_HEADER_SIZE as usize;
    let (remaining, _) = take(record_bytes_left)(remaining)?;
    Ok((remaining, editor_id.to_string()))
}

fn parse_4char(input: &[u8]) -> IResult<&[u8], &str> {
    map_res(take(4usize), |bytes: &[u8]| str::from_utf8(bytes))(input)
}

fn parse_zstring(input: &[u8]) -> IResult<&[u8], Cow<str>> {
    let (input, bytes) = take_while(|byte| byte != 0)(input)?;
    let (zstring, _, _) = WINDOWS_1252.decode(bytes);
    let (input, _) = take(1usize)(input)?;
    Ok((input, zstring))
}
