use std::io::Read;
use std::{convert::TryInto, str};

use anyhow::{anyhow, Result};
use flate2::read::ZlibDecoder;
use nom::{
    branch::alt,
    bytes::complete::{take, take_while},
    combinator::{map, map_res, verify},
    number::complete::{le_f32, le_i32, le_u16, le_u32},
    IResult,
};
use serde::Serialize;

const HEADER_SIZE: u32 = 24;

#[derive(Debug, PartialEq, Serialize)]
pub struct Plugin<'a> {
    pub header: PluginHeader<'a>,
    pub cells: Vec<Cell>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct PluginHeader<'a> {
    pub version: f32,
    pub num_records_and_groups: i32,
    pub next_object_id: u32,
    pub author: Option<&'a str>,
    pub description: Option<&'a str>,
    pub masters: Vec<&'a str>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Cell {
    pub form_id: u32,
    pub editor_id: Option<String>,
    pub x: Option<i32>,
    pub y: Option<i32>,
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
    is_compressed: bool,
    is_persistent: bool,
    data: &'a [u8],
}

#[derive(Debug)]
pub struct DecompressedCell {
    pub form_id: u32,
    pub is_persistent: bool,
    pub data: Vec<u8>,
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

pub fn parse_cell<'a>(
    input: &'a [u8],
    form_id: u32,
    is_persistent: bool,
) -> IResult<&'a [u8], Cell> {
    let (input, cell_data) = parse_cell_fields(input)?;
    Ok((
        input,
        Cell {
            form_id,
            editor_id: cell_data.editor_id,
            x: cell_data.x,
            y: cell_data.y,
            is_persistent,
        },
    ))
}

pub fn decompress_cells(unparsed_cells: Vec<UnparsedCell>) -> Result<Vec<DecompressedCell>> {
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
            is_persistent: unparsed_cell.is_persistent,
            data: decompressed_data,
        });
    }
    Ok(decompressed_cells)
}

pub fn parse_header_and_cell_bytes(
    input: &[u8],
) -> IResult<&[u8], (PluginHeader, Vec<UnparsedCell>)> {
    let (input, header) = parse_plugin_header(input)?;
    let (input, unparsed_cells) = parse_group_data(input, input.len() as u32, 0)?;
    Ok((input, (header, unparsed_cells)))
}

pub fn parse_plugin(input: &[u8]) -> Result<Plugin> {
    let (_, (header, unparsed_cells)) = parse_header_and_cell_bytes(&input)
        .map_err(|_err| anyhow!("Failed to parse plugin header and find CELL data"))?;
    let decompressed_cells = decompress_cells(unparsed_cells)?;

    let mut cells = Vec::new();
    for decompressed_cell in decompressed_cells {
        let (_, cell) = parse_cell(
            &decompressed_cell.data,
            decompressed_cell.form_id,
            decompressed_cell.is_persistent,
        )
        .unwrap();
        cells.push(cell);
    }

    Ok(Plugin { header, cells })
}

fn parse_group_data<'a>(
    input: &'a [u8],
    remaining_bytes: u32,
    depth: usize,
) -> IResult<&'a [u8], Vec<UnparsedCell>> {
    let mut input = input;
    let mut cells = vec![];
    let mut consumed_bytes = 0;
    while !input.is_empty() && consumed_bytes < remaining_bytes {
        let (remaining, record_header) = parse_header(input)?;
        match record_header {
            Header::Group(group_header) => {
                if group_header.group_type == 0 {
                    // TODO: get rid of unwrap
                    let label = str::from_utf8(group_header.label).unwrap();
                    if label != "WRLD" && label != "CELL" {
                        let (remaining, _) = take(group_header.size - HEADER_SIZE)(remaining)?;
                        input = remaining;
                        consumed_bytes += group_header.size;
                        continue;
                    }
                } else if group_header.group_type == 7 {
                    // TODO: DRY
                    let (remaining, _) = take(group_header.size - HEADER_SIZE)(remaining)?;
                    input = remaining;
                    consumed_bytes += group_header.size;
                    continue;
                }
                let (remaining, mut inner_cells) =
                    parse_group_data(remaining, group_header.size - HEADER_SIZE, depth + 1)?;
                cells.append(&mut inner_cells);
                input = remaining;
                consumed_bytes += group_header.size;
            }
            Header::Record(record_header) => match record_header.record_type {
                "CELL" => {
                    let (remaining, data) = take(record_header.size)(remaining)?;
                    cells.push(UnparsedCell {
                        form_id: record_header.id,
                        is_compressed: record_header.flags.contains(RecordFlags::COMPRESSED),
                        is_persistent: record_header.flags.contains(RecordFlags::PERSISTENT_REFR),
                        data,
                    });
                    input = remaining;
                    consumed_bytes += record_header.size + HEADER_SIZE;
                }
                _ => {
                    let (remaining, _) = take(record_header.size)(remaining)?;
                    input = remaining;
                    consumed_bytes += record_header.size + HEADER_SIZE;
                }
            },
        }
    }
    Ok((input, cells))
}

fn parse_plugin_header(input: &[u8]) -> IResult<&[u8], PluginHeader> {
    let (mut input, _tes4) = verify(parse_record_header, |record_header| {
        record_header.record_type == "TES4"
    })(input)?;
    let (remaining, _hedr) = verify(parse_field_header, |field_header| {
        field_header.field_type == "HEDR"
    })(input)?;
    input = remaining;
    let (remaining, (version, num_records_and_groups, next_object_id)) = parse_hedr_fields(input)?;
    input = remaining;
    let mut author = None;
    let mut description = None;
    let mut masters = vec![];
    loop {
        let (remaining, field) = parse_field_header(input)?;
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
                break;
            }
            _ => {
                let (remaining, _) = take(field.size)(input)?;
                input = remaining;
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
    let (input, flags) = map_res(le_u32, |bits| {
        RecordFlags::from_bits(bits).ok_or("bad record flag")
    })(input)?;
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
    if field_type == "XXXX" {
        todo!()
    }
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
                let (remaining, _) = take(4usize)(remaining)?;
                input = remaining;
            }
            _ => {
                let (remaining, _) = take(field.size)(input)?;
                input = remaining;
            }
        }
    }
    Ok((input, cell_data))
}

fn parse_4char(input: &[u8]) -> IResult<&[u8], &str> {
    map_res(take(4usize), |bytes: &[u8]| str::from_utf8(bytes))(input)
}

fn parse_zstring(input: &[u8]) -> IResult<&[u8], &str> {
    let (input, zstring) = map_res(take_while(|byte| byte != 0), |bytes: &[u8]| {
        str::from_utf8(bytes)
    })(input)?;
    let (input, _) = take(1usize)(input)?;
    Ok((input, zstring))
}
