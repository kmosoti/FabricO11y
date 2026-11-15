//! Schema-aware, stack-bounded Parquet Thrift footer and page-header preflight guards shared
//! across query execution (`fabric-query`) and storage maintenance (`fabric-store`).

use arrow::compute::can_cast_types;
use arrow::datatypes::{DataType, Field, IntervalUnit, Schema, SchemaRef};
use arrow::error::ArrowError;
use arrow::record_batch::{RecordBatch, RecordBatchReader};
use parquet::arrow::arrow_reader::{
    ArrowReaderMetadata, ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder,
};
use parquet::arrow::push_decoder::PushBuffers;
use parquet::arrow::{ARROW_SCHEMA_META_KEY, ProjectionMask};
use parquet::basic::{Compression, Encoding, Type as ParquetPhysicalType};
use parquet::column::page::{Page, PageReader};
use parquet::file::metadata::{ColumnChunkMetaData, ParquetMetaData, ParquetMetaDataReader};
use parquet::file::reader::{ChunkReader, SerializedPageReader};
use std::fmt;
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::Arc;

/// Maximum serialized Parquet Thrift footer bytes accepted across query and store paths.
pub const MAX_PARQUET_FOOTER_SERIALIZED_BYTES: usize = 1024 * 1024;
/// Maximum Thrift Compact nesting depth permitted during footer validation.
pub const MAX_THRIFT_FOOTER_DEPTH: usize = 64;
/// Maximum total Thrift Compact items visited per footer.
pub const MAX_THRIFT_FOOTER_ITEMS: usize = 500_000;
/// Maximum Parquet schema elements permitted per file footer.
pub const MAX_PARQUET_SCHEMA_ELEMENTS: usize = 1_024;
/// Maximum logical group nesting depth permitted in a Parquet schema tree.
pub const MAX_PARQUET_SCHEMA_DEPTH: usize = 32;
/// Maximum `ARROW:schema` key-value payload bytes permitted in footer metadata.
pub const MAX_ARROW_SCHEMA_KV_BYTES: usize = 512 * 1024;
/// Maximum DAG-expanded heap estimate permitted for embedded `ARROW:schema` IPC metadata.
pub const MAX_ARROW_SCHEMA_EXPANDED_BYTES: usize = 2 * 1024 * 1024;
const MAX_ARROW_SCHEMA_METADATA_ENTRIES: usize = 4_096;
/// Maximum single Parquet page compressed or uncompressed byte size permitted pre-decode.
pub const MAX_PARQUET_PAGE_UNCOMPRESSED_BYTES: usize = 64 * 1024 * 1024;
/// Maximum bytes scanned for a single Thrift `PageHeader`.
pub const MAX_PARQUET_PAGE_HEADER_BYTES: u64 = 1024 * 1024;
/// Maximum estimated decoded footer allocation allowed in maintenance paths.
pub const MAX_MAINTENANCE_FOOTER_DECODE_BYTES: usize = 32 * 1024 * 1024;
/// Maximum per-row-group decode memory allowed in maintenance paths.
pub const MAX_MAINTENANCE_ROW_GROUP_DECODE_BYTES: usize = 64 * 1024 * 1024;
/// Minimum memory pool floor retained per pinned Parquet footer.
pub const PINNED_FOOTER_MIN_BYTES: usize = 16 * 1024;

const FTYPE_STOP: u8 = 0;
const FTYPE_BOOL_TRUE: u8 = 1;
const FTYPE_BOOL_FALSE: u8 = 2;
const FTYPE_BYTE: u8 = 3;
const FTYPE_I16: u8 = 4;
const FTYPE_I32: u8 = 5;
const FTYPE_I64: u8 = 6;
const FTYPE_DOUBLE: u8 = 7;
const FTYPE_BINARY: u8 = 8;
const FTYPE_LIST: u8 = 9;
const FTYPE_SET: u8 = 10;
const FTYPE_MAP: u8 = 11;
const FTYPE_STRUCT: u8 = 12;
const FTYPE_UUID: u8 = 13;

/// Per-column-chunk page-header scan totals used to bound dictionary and page allocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnPageScanStats {
    effective_payload_bytes: usize,
    resident_bytes: usize,
}

impl ColumnPageScanStats {
    /// Returns the total effective uncompressed/compressed page payload bytes in the column chunk.
    #[must_use]
    pub const fn effective_payload_bytes(&self) -> usize {
        self.effective_payload_bytes
    }

    /// Returns the peak decoded-page + dictionary resident memory footprint in the column chunk.
    #[must_use]
    pub const fn resident_bytes(&self) -> usize {
        self.resident_bytes
    }
}

trait ThriftByteCursor {
    fn remaining_bound(&self) -> u64;
    fn read_byte(&mut self) -> Result<u8, String>;
    fn skip_bytes(&mut self, n: u64) -> Result<(), String>;

    fn read_vlq(&mut self) -> Result<u64, String> {
        let mut out = 0_u64;
        for step in 0..10_usize {
            let b = self.read_byte()?;
            if step == 9 && b > 1 {
                return Err("integer overflow decoding parquet Thrift varint".to_owned());
            }
            out |= u64::from(b & 0x7f) << (step * 7);
            if (b & 0x80) == 0 {
                return Ok(out);
            }
        }
        Err("overlong varint in parquet Thrift stream".to_owned())
    }

    fn read_zig_zag(&mut self) -> Result<i64, String> {
        let v = self.read_vlq()?;
        Ok((v >> 1) as i64 ^ -((v & 1) as i64))
    }

    fn read_i16(&mut self) -> Result<i16, String> {
        Ok(self.read_zig_zag()? as i16)
    }

    fn read_i32(&mut self) -> Result<i32, String> {
        Ok(self.read_zig_zag()? as i32)
    }

    fn read_field_header(&mut self, last_field_id: i16) -> Result<Option<(u8, i16)>, String> {
        let hdr = self.read_byte()?;
        let ftype = hdr & 0x0f;
        if ftype == FTYPE_STOP {
            return Ok(None);
        }
        if !(1..=13).contains(&ftype) {
            return Err(format!("invalid parquet Thrift struct field type {ftype}"));
        }
        let delta = i16::from((hdr & 0xf0) >> 4);
        let field_id = if delta != 0 {
            last_field_id
                .checked_add(delta)
                .ok_or_else(|| "field delta overflow in parquet Thrift stream".to_owned())?
        } else {
            self.read_i16()?
        };
        Ok(Some((ftype, field_id)))
    }

    fn read_list_begin(&mut self, expected_elem: Option<u8>) -> Result<(u8, usize), String> {
        let hdr = self.read_byte()?;
        if hdr == 0 {
            return Ok((FTYPE_BYTE, 0));
        }
        let elem_type = hdr & 0x0f;
        if !(1..=13).contains(&elem_type) {
            return Err(format!(
                "invalid parquet Thrift list element type {elem_type}"
            ));
        }
        if let Some(exp) = expected_elem {
            let matches = if exp == FTYPE_BOOL_TRUE || exp == FTYPE_BOOL_FALSE {
                elem_type == FTYPE_BOOL_TRUE || elem_type == FTYPE_BOOL_FALSE
            } else {
                elem_type == exp
            };
            if !matches {
                return Err(format!(
                    "parquet Thrift list element type {elem_type} mismatches expected {exp}"
                ));
            }
        }
        let raw_cnt = (hdr & 0xf0) >> 4;
        let count = if raw_cnt < 15 {
            usize::from(raw_cnt)
        } else {
            let n = i32::try_from(self.read_vlq()?)
                .map_err(|_| "parquet Thrift list length overflow".to_owned())?;
            usize::try_from(n)
                .map_err(|_| "negative list length in parquet Thrift stream".to_owned())?
        };
        if u64::try_from(count).is_err() || (count as u64) > self.remaining_bound() {
            return Err(format!(
                "parquet Thrift list count {count} exceeds remaining {} bytes",
                self.remaining_bound()
            ));
        }
        Ok((elem_type, count))
    }
}

fn account_thrift_items(visited: &mut usize, max_items: usize, count: usize) -> Result<(), String> {
    *visited = visited.saturating_add(count);
    if *visited > max_items {
        return Err(format!(
            "parquet Thrift item count exceeds limit {max_items}"
        ));
    }
    Ok(())
}

fn require_ftype(ftype: u8, expected: u8, ctx: &str) -> Result<(), String> {
    let ok = if expected == FTYPE_BOOL_TRUE || expected == FTYPE_BOOL_FALSE {
        ftype == FTYPE_BOOL_TRUE || ftype == FTYPE_BOOL_FALSE
    } else {
        ftype == expected
    };
    if !ok {
        return Err(format!(
            "parquet Thrift field {ctx} wire type {ftype} mismatches expected {expected}"
        ));
    }
    Ok(())
}

fn skip_thrift_elem<C: ThriftByteCursor>(
    cur: &mut C,
    elem_type: u8,
    depth: usize,
    visited: &mut usize,
    max_items: usize,
    est_bytes: &mut usize,
) -> Result<(), String> {
    match elem_type {
        FTYPE_BOOL_TRUE | FTYPE_BOOL_FALSE => {
            let b = cur.read_byte()?;
            if !matches!(b, 0..=2) {
                return Err(format!("invalid boolean element byte {b} in Thrift stream"));
            }
            Ok(())
        }
        _ => skip_thrift_field_value(cur, elem_type, depth, visited, max_items, est_bytes),
    }
}

fn skip_thrift_field_value<C: ThriftByteCursor>(
    cur: &mut C,
    ftype: u8,
    depth: usize,
    visited: &mut usize,
    max_items: usize,
    est_bytes: &mut usize,
) -> Result<(), String> {
    match ftype {
        FTYPE_BOOL_TRUE | FTYPE_BOOL_FALSE => Ok(()),
        FTYPE_BYTE => cur.skip_bytes(1),
        FTYPE_I16 | FTYPE_I32 | FTYPE_I64 => {
            let _ = cur.read_vlq()?;
            Ok(())
        }
        FTYPE_DOUBLE => cur.skip_bytes(8),
        FTYPE_BINARY => {
            let len = usize::try_from(cur.read_vlq()?)
                .map_err(|_| "Thrift binary length overflow".to_owned())?;
            *est_bytes = est_bytes.saturating_add(len);
            cur.skip_bytes(len as u64)
        }
        FTYPE_LIST | FTYPE_SET => {
            if depth == 0 {
                return Err("parquet Thrift collection exceeds max nesting depth".to_owned());
            }
            let (elem_type, count) = cur.read_list_begin(None)?;
            account_thrift_items(visited, max_items, count)?;
            let per_elem = match elem_type {
                FTYPE_STRUCT => 384usize,
                FTYPE_BINARY..=FTYPE_MAP => 64usize,
                _ => 16usize,
            };
            *est_bytes = est_bytes.saturating_add(count.saturating_mul(per_elem));
            for _ in 0..count {
                skip_thrift_elem(cur, elem_type, depth - 1, visited, max_items, est_bytes)?;
            }
            Ok(())
        }
        FTYPE_MAP => {
            if depth == 0 {
                return Err("parquet Thrift map exceeds max nesting depth".to_owned());
            }
            let n = i32::try_from(cur.read_vlq()?)
                .map_err(|_| "parquet Thrift map length overflow".to_owned())?;
            let count = usize::try_from(n)
                .map_err(|_| "negative map length in parquet Thrift stream".to_owned())?;
            if count == 0 {
                return Ok(());
            }
            let kv = cur.read_byte()?;
            let ktype = kv >> 4;
            let vtype = kv & 0x0f;
            if !(1..=13).contains(&ktype) || !(1..=13).contains(&vtype) {
                return Err(format!(
                    "invalid parquet Thrift map kv types ({ktype}, {vtype})"
                ));
            }
            if (count as u64).saturating_mul(2) > cur.remaining_bound() {
                return Err(format!(
                    "parquet Thrift map count {count} exceeds remaining {} bytes",
                    cur.remaining_bound()
                ));
            }
            account_thrift_items(visited, max_items, count)?;
            *est_bytes = est_bytes.saturating_add(count.saturating_mul(128));
            for _ in 0..count {
                skip_thrift_elem(cur, ktype, depth - 1, visited, max_items, est_bytes)?;
                skip_thrift_elem(cur, vtype, depth - 1, visited, max_items, est_bytes)?;
            }
            Ok(())
        }
        FTYPE_STRUCT => {
            if depth == 0 {
                return Err("parquet Thrift struct exceeds max nesting depth".to_owned());
            }
            *est_bytes = est_bytes.saturating_add(64);
            let mut last_id = 0_i16;
            while let Some((inner_type, fid)) = cur.read_field_header(last_id)? {
                account_thrift_items(visited, max_items, 1)?;
                skip_thrift_field_value(cur, inner_type, depth - 1, visited, max_items, est_bytes)?;
                last_id = fid;
            }
            Ok(())
        }
        FTYPE_UUID => cur.skip_bytes(16),
        _ => Err(format!("unsupported parquet Thrift field type {ftype}")),
    }
}

fn b64_val(b: u8) -> Result<u8, String> {
    match b {
        b'A'..=b'Z' => Ok(b - b'A'),
        b'a'..=b'z' => Ok(b - b'a' + 26),
        b'0'..=b'9' => Ok(b - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err("invalid base64 character in ARROW:schema metadata".to_owned()),
    }
}

fn decode_base64_standard_strict(input: &str) -> Result<Vec<u8>, String> {
    if input.len() > MAX_ARROW_SCHEMA_KV_BYTES {
        return Err(format!(
            "parquet ARROW:schema metadata ({} bytes) exceeds {MAX_ARROW_SCHEMA_KV_BYTES} bytes",
            input.len()
        ));
    }
    let raw = input.as_bytes();
    if !raw.len().is_multiple_of(4) {
        return Err("invalid base64 length in ARROW:schema metadata".to_owned());
    }
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let pad = if raw.ends_with(b"==") {
        2
    } else if raw.ends_with(b"=") {
        1
    } else {
        0
    };
    let out_len = (raw.len() / 4) * 3 - pad;
    let mut out = Vec::with_capacity(out_len);
    let last_idx = raw.len() / 4 - 1;
    for (idx, chunk) in raw.as_chunks::<4>().0.iter().enumerate() {
        let is_last = idx == last_idx;
        let v0 = b64_val(chunk[0])?;
        let v1 = b64_val(chunk[1])?;
        if is_last && chunk[2] == b'=' {
            if chunk[3] != b'=' || (v1 & 0x0f) != 0 {
                return Err("invalid base64 padding in ARROW:schema metadata".to_owned());
            }
            out.push((v0 << 2) | (v1 >> 4));
            break;
        }
        let v2 = b64_val(chunk[2])?;
        if is_last && chunk[3] == b'=' {
            if (v2 & 0x03) != 0 {
                return Err("invalid base64 padding in ARROW:schema metadata".to_owned());
            }
            out.push((v0 << 2) | (v1 >> 4));
            out.push((v1 << 4) | (v2 >> 2));
            break;
        }
        let v3 = b64_val(chunk[3])?;
        out.push((v0 << 2) | (v1 >> 4));
        out.push((v1 << 4) | (v2 >> 2));
        out.push((v2 << 6) | v3);
    }
    Ok(out)
}

fn fb_read_u16(slice: &[u8], pos: usize) -> Result<u16, String> {
    slice
        .get(pos..pos + 2)
        .and_then(|s| <[u8; 2]>::try_from(s).ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| "truncated ARROW:schema IPC flatbuffer u16".to_owned())
}

fn fb_read_u32(slice: &[u8], pos: usize) -> Result<u32, String> {
    slice
        .get(pos..pos + 4)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| "truncated ARROW:schema IPC flatbuffer u32".to_owned())
}

fn fb_read_i32(slice: &[u8], pos: usize) -> Result<i32, String> {
    slice
        .get(pos..pos + 4)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(i32::from_le_bytes)
        .ok_or_else(|| "truncated ARROW:schema IPC flatbuffer i32".to_owned())
}

fn fb_deref_uoffset(slice: &[u8], slot_pos: usize) -> Result<usize, String> {
    let uoff = usize::try_from(fb_read_u32(slice, slot_pos)?)
        .map_err(|_| "ARROW:schema IPC flatbuffer uoffset overflow".to_owned())?;
    if uoff < 4 {
        return Err("invalid ARROW:schema IPC flatbuffer uoffset".to_owned());
    }
    slot_pos
        .checked_add(uoff)
        .filter(|&p| p <= slice.len())
        .ok_or_else(|| "out-of-bounds ARROW:schema IPC flatbuffer uoffset".to_owned())
}

fn fb_table_vtable(slice: &[u8], tab_pos: usize) -> Result<usize, String> {
    let soffset = fb_read_i32(slice, tab_pos)?;
    let vt_signed = isize::try_from(tab_pos)
        .ok()
        .and_then(|p| isize::try_from(soffset).ok().and_then(|s| p.checked_sub(s)))
        .ok_or_else(|| "invalid ARROW:schema IPC flatbuffer vtable soffset".to_owned())?;
    let vt_pos = usize::try_from(vt_signed)
        .map_err(|_| "out-of-bounds ARROW:schema IPC flatbuffer vtable".to_owned())?;
    let vt_len = usize::from(fb_read_u16(slice, vt_pos)?);
    let obj_len = usize::from(fb_read_u16(slice, vt_pos + 2)?);
    if vt_len < 4
        || !vt_len.is_multiple_of(2)
        || vt_pos.checked_add(vt_len).is_none_or(|e| e > slice.len())
        || obj_len < 4
        || tab_pos.checked_add(obj_len).is_none_or(|e| e > slice.len())
    {
        return Err("invalid ARROW:schema IPC flatbuffer vtable bounds".to_owned());
    }
    Ok(vt_pos)
}

fn fb_slot_pos(
    slice: &[u8],
    tab_pos: usize,
    vt_pos: usize,
    vt_off: u16,
) -> Result<Option<usize>, String> {
    let vt_len = usize::from(fb_read_u16(slice, vt_pos)?);
    let off_idx = usize::from(vt_off);
    if off_idx + 2 > vt_len {
        return Ok(None);
    }
    let field_off = usize::from(fb_read_u16(slice, vt_pos + off_idx)?);
    if field_off == 0 {
        return Ok(None);
    }
    let obj_len = usize::from(fb_read_u16(slice, vt_pos + 2)?);
    if field_off < 4 || field_off >= obj_len {
        return Err("out-of-bounds ARROW:schema IPC flatbuffer field offset".to_owned());
    }
    tab_pos
        .checked_add(field_off)
        .filter(|&p| p <= slice.len())
        .map(Some)
        .ok_or_else(|| "overflow in ARROW:schema IPC flatbuffer slot".to_owned())
}

fn fb_vec_header(slice: &[u8], vec_pos: usize) -> Result<(usize, usize), String> {
    let len = usize::try_from(fb_read_u32(slice, vec_pos)?)
        .map_err(|_| "ARROW:schema IPC flatbuffer vector length overflow".to_owned())?;
    let body = vec_pos
        .checked_add(4)
        .ok_or_else(|| "ARROW:schema IPC flatbuffer vector body overflow".to_owned())?;
    Ok((body, len))
}

fn fb_str_len(slice: &[u8], slot_pos: usize) -> Result<usize, String> {
    let str_pos = fb_deref_uoffset(slice, slot_pos)?;
    let len = usize::try_from(fb_read_u32(slice, str_pos)?)
        .map_err(|_| "ARROW:schema IPC flatbuffer string length overflow".to_owned())?;
    if str_pos
        .checked_add(4)
        .and_then(|p| p.checked_add(len))
        .and_then(|p| p.checked_add(1))
        .is_none_or(|e| e > slice.len())
    {
        return Err("truncated ARROW:schema IPC flatbuffer string".to_owned());
    }
    Ok(len)
}

fn fb_account_kv_vec(
    slice: &[u8],
    kv_vec_slot: usize,
    total_entries: &mut usize,
    expanded: &mut usize,
) -> Result<(), String> {
    let vec_pos = fb_deref_uoffset(slice, kv_vec_slot)?;
    let (body, len) = fb_vec_header(slice, vec_pos)?;
    *total_entries = total_entries.saturating_add(len);
    if *total_entries > MAX_ARROW_SCHEMA_METADATA_ENTRIES {
        return Err(format!(
            "Resources exhausted: ARROW:schema custom_metadata entry count {} exceeds limit {MAX_ARROW_SCHEMA_METADATA_ENTRIES}",
            *total_entries
        ));
    }
    if body
        .checked_add(len.saturating_mul(4))
        .is_none_or(|e| e > slice.len())
    {
        return Err("truncated ARROW:schema custom_metadata vector".to_owned());
    }
    for i in 0..len {
        let kv_pos = fb_deref_uoffset(slice, body + i * 4)?;
        let kv_vt = fb_table_vtable(slice, kv_pos)?;
        let k_len =
            if let Some(s) = fb_slot_pos(slice, kv_pos, kv_vt, arrow::ipc::KeyValue::VT_KEY)? {
                fb_str_len(slice, s)?
            } else {
                0
            };
        let v_len =
            if let Some(s) = fb_slot_pos(slice, kv_pos, kv_vt, arrow::ipc::KeyValue::VT_VALUE)? {
                fb_str_len(slice, s)?
            } else {
                0
            };
        *expanded = expanded
            .saturating_add(64_usize)
            .saturating_add(k_len)
            .saturating_add(v_len);
        if *expanded > MAX_ARROW_SCHEMA_EXPANDED_BYTES {
            return Err(format!(
                "Resources exhausted: ARROW:schema expanded heap estimate {} exceeds limit {MAX_ARROW_SCHEMA_EXPANDED_BYTES}",
                *expanded
            ));
        }
    }
    Ok(())
}

/// Bounds FlatBuffer structural fan-out, nesting, and string expansion *before* invoking
/// `arrow::ipc::root_as_message`, preventing DAG blowup inside FlatBuffers verification.
fn preflight_arrow_schema_flatbuffer_slice(slice: &[u8]) -> Result<(), String> {
    let msg_pos = fb_deref_uoffset(slice, 0)?;
    let msg_vt = fb_table_vtable(slice, msg_pos)?;
    let hdr_ty = fb_slot_pos(slice, msg_pos, msg_vt, arrow::ipc::Message::VT_HEADER_TYPE)?
        .map(|p| slice[p])
        .unwrap_or(0);
    if hdr_ty != arrow::ipc::MessageHeader::Schema.0 {
        return Err("ARROW:schema IPC message header is not Schema".to_owned());
    }
    let mut total_md_entries = 0_usize;
    let mut expanded = 256_usize;
    if let Some(msg_md_slot) = fb_slot_pos(
        slice,
        msg_pos,
        msg_vt,
        arrow::ipc::Message::VT_CUSTOM_METADATA,
    )? {
        fb_account_kv_vec(slice, msg_md_slot, &mut total_md_entries, &mut expanded)?;
    }
    let schema_slot = fb_slot_pos(slice, msg_pos, msg_vt, arrow::ipc::Message::VT_HEADER)?
        .ok_or_else(|| "ARROW:schema IPC message header is not Schema".to_owned())?;
    let schema_pos = fb_deref_uoffset(slice, schema_slot)?;
    let schema_vt = fb_table_vtable(slice, schema_pos)?;
    if let Some(schema_md_slot) = fb_slot_pos(
        slice,
        schema_pos,
        schema_vt,
        arrow::ipc::Schema::VT_CUSTOM_METADATA,
    )? {
        fb_account_kv_vec(slice, schema_md_slot, &mut total_md_entries, &mut expanded)?;
    }
    if let Some(feat_slot) = fb_slot_pos(
        slice,
        schema_pos,
        schema_vt,
        arrow::ipc::Schema::VT_FEATURES,
    )? {
        let feat_pos = fb_deref_uoffset(slice, feat_slot)?;
        let (feat_body, feat_len) = fb_vec_header(slice, feat_pos)?;
        if feat_len > MAX_PARQUET_SCHEMA_ELEMENTS
            || feat_body
                .checked_add(feat_len.saturating_mul(8))
                .is_none_or(|e| e > slice.len())
        {
            return Err("invalid ARROW:schema features vector bounds".to_owned());
        }
    }
    let fields_slot = fb_slot_pos(slice, schema_pos, schema_vt, arrow::ipc::Schema::VT_FIELDS)?
        .ok_or_else(|| "ARROW:schema IPC Schema omits required fields vector".to_owned())?;
    let fields_vec_pos = fb_deref_uoffset(slice, fields_slot)?;
    let (fields_body, fields_len) = fb_vec_header(slice, fields_vec_pos)?;
    if fields_len > MAX_PARQUET_SCHEMA_ELEMENTS {
        return Err(format!(
            "ARROW:schema field count {fields_len} exceeds max {MAX_PARQUET_SCHEMA_ELEMENTS}"
        ));
    }
    if fields_body
        .checked_add(fields_len.saturating_mul(4))
        .is_none_or(|e| e > slice.len())
    {
        return Err("truncated ARROW:schema fields vector".to_owned());
    }
    for i in 0..fields_len {
        let field_pos = fb_deref_uoffset(slice, fields_body + i * 4)?;
        let field_vt = fb_table_vtable(slice, field_pos)?;
        let name_len =
            if let Some(s) = fb_slot_pos(slice, field_pos, field_vt, arrow::ipc::Field::VT_NAME)? {
                fb_str_len(slice, s)?
            } else {
                0
            };
        expanded = expanded.saturating_add(256_usize).saturating_add(name_len);
        if expanded > MAX_ARROW_SCHEMA_EXPANDED_BYTES {
            return Err(format!(
                "Resources exhausted: ARROW:schema expanded heap estimate {expanded} exceeds limit {MAX_ARROW_SCHEMA_EXPANDED_BYTES}"
            ));
        }
        if let Some(ch_slot) =
            fb_slot_pos(slice, field_pos, field_vt, arrow::ipc::Field::VT_CHILDREN)?
        {
            let ch_pos = fb_deref_uoffset(slice, ch_slot)?;
            let (_, ch_len) = fb_vec_header(slice, ch_pos)?;
            if ch_len != 0 {
                return Err(
                    "nested ARROW:schema field children rejected by flat schema contract"
                        .to_owned(),
                );
            }
        }
        if let Some(f_md_slot) = fb_slot_pos(
            slice,
            field_pos,
            field_vt,
            arrow::ipc::Field::VT_CUSTOM_METADATA,
        )? {
            fb_account_kv_vec(slice, f_md_slot, &mut total_md_entries, &mut expanded)?;
        }
        let type_ty = fb_slot_pos(slice, field_pos, field_vt, arrow::ipc::Field::VT_TYPE_TYPE)?
            .map(|p| slice[p])
            .unwrap_or(0);
        if type_ty == arrow::ipc::Type::Timestamp.0
            && let Some(ty_slot) =
                fb_slot_pos(slice, field_pos, field_vt, arrow::ipc::Field::VT_TYPE_)?
        {
            let ts_pos = fb_deref_uoffset(slice, ty_slot)?;
            let ts_vt = fb_table_vtable(slice, ts_pos)?;
            if let Some(tz_slot) =
                fb_slot_pos(slice, ts_pos, ts_vt, arrow::ipc::Timestamp::VT_TIMEZONE)?
            {
                let tz_len = fb_str_len(slice, tz_slot)?;
                expanded = expanded.saturating_add(tz_len);
                if expanded > MAX_ARROW_SCHEMA_EXPANDED_BYTES {
                    return Err(format!(
                        "Resources exhausted: ARROW:schema expanded heap estimate {expanded} exceeds limit {MAX_ARROW_SCHEMA_EXPANDED_BYTES}"
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_embedded_arrow_schema_ipc(encoded_b64: &str) -> Result<usize, String> {
    let bytes = decode_base64_standard_strict(encoded_b64)?;
    let slice = if bytes.len() > 8 && bytes[0..4] == [255_u8; 4] {
        &bytes[8..]
    } else {
        bytes.as_slice()
    };
    preflight_arrow_schema_flatbuffer_slice(slice)?;
    let msg = arrow::ipc::root_as_message(slice)
        .map_err(|e| format!("invalid ARROW:schema IPC flatbuffer: {e:?}"))?;
    let ipc_schema = msg
        .header_as_schema()
        .ok_or_else(|| "ARROW:schema IPC message header is not Schema".to_owned())?;
    if ipc_schema.endianness() != arrow::ipc::Endianness::Little {
        return Err("unsupported non-little-endian ARROW:schema IPC metadata".to_owned());
    }
    let fields = ipc_schema
        .fields()
        .ok_or_else(|| "ARROW:schema IPC Schema omits required fields vector".to_owned())?;
    if fields.len() > MAX_PARQUET_SCHEMA_ELEMENTS {
        return Err(format!(
            "ARROW:schema field count {} exceeds max {MAX_PARQUET_SCHEMA_ELEMENTS}",
            fields.len()
        ));
    }
    let mut expanded = 256_usize;
    let account = |acc: &mut usize, add: usize| -> Result<(), String> {
        *acc = acc.saturating_add(add);
        if *acc > MAX_ARROW_SCHEMA_EXPANDED_BYTES {
            return Err(format!(
                "Resources exhausted: ARROW:schema expanded heap estimate {} exceeds limit {MAX_ARROW_SCHEMA_EXPANDED_BYTES}",
                *acc
            ));
        }
        Ok(())
    };
    if let Some(md) = ipc_schema.custom_metadata() {
        if md.len() > MAX_ARROW_SCHEMA_METADATA_ENTRIES {
            return Err(format!(
                "Resources exhausted: ARROW:schema custom_metadata entry count {} exceeds limit {MAX_ARROW_SCHEMA_METADATA_ENTRIES}",
                md.len()
            ));
        }
        for i in 0..md.len() {
            let kv = md.get(i);
            let k_len = kv.key().map_or(0, str::len);
            let v_len = kv.value().map_or(0, str::len);
            account(
                &mut expanded,
                64_usize.saturating_add(k_len).saturating_add(v_len),
            )?;
        }
    }
    for i in 0..fields.len() {
        let f = fields.get(i);
        let name_len = f.name().map_or(0, str::len);
        account(&mut expanded, 256_usize.saturating_add(name_len))?;
        if let Some(f_md) = f.custom_metadata() {
            if f_md.len() > MAX_ARROW_SCHEMA_METADATA_ENTRIES {
                return Err(format!(
                    "Resources exhausted: ARROW:schema field metadata count {} exceeds limit {MAX_ARROW_SCHEMA_METADATA_ENTRIES}",
                    f_md.len()
                ));
            }
            for j in 0..f_md.len() {
                let kv = f_md.get(j);
                let k_len = kv.key().map_or(0, str::len);
                let v_len = kv.value().map_or(0, str::len);
                account(
                    &mut expanded,
                    64_usize.saturating_add(k_len).saturating_add(v_len),
                )?;
            }
        }
        if f.children().is_some_and(|c| !c.is_empty()) {
            return Err(
                "nested ARROW:schema field children rejected by flat schema contract".to_owned(),
            );
        }
        if f.dictionary().is_some() {
            return Err(format!(
                "dictionary-typed ARROW:schema field {:?} rejected by flat schema contract",
                f.name()
            ));
        }
        match f.type_type() {
            arrow::ipc::Type::Null
            | arrow::ipc::Type::Bool
            | arrow::ipc::Type::Binary
            | arrow::ipc::Type::BinaryView
            | arrow::ipc::Type::LargeBinary
            | arrow::ipc::Type::Utf8
            | arrow::ipc::Type::Utf8View
            | arrow::ipc::Type::LargeUtf8 => {}
            arrow::ipc::Type::Int => {
                let t = f
                    .type_as_int()
                    .ok_or_else(|| "ARROW:schema Int omits type table".to_owned())?;
                if !matches!(t.bitWidth(), 8 | 16 | 32 | 64) {
                    return Err(format!(
                        "unsupported ARROW:schema Int bitWidth {}",
                        t.bitWidth()
                    ));
                }
            }
            arrow::ipc::Type::FixedSizeBinary => {
                let t = f
                    .type_as_fixed_size_binary()
                    .ok_or_else(|| "ARROW:schema FixedSizeBinary omits type table".to_owned())?;
                if t.byteWidth() < 1 {
                    return Err(format!(
                        "invalid ARROW:schema FixedSizeBinary byteWidth {} (must be >= 1)",
                        t.byteWidth()
                    ));
                }
            }
            arrow::ipc::Type::FloatingPoint => {
                let t = f
                    .type_as_floating_point()
                    .ok_or_else(|| "ARROW:schema FloatingPoint omits type table".to_owned())?;
                if !matches!(
                    t.precision(),
                    arrow::ipc::Precision::HALF
                        | arrow::ipc::Precision::SINGLE
                        | arrow::ipc::Precision::DOUBLE
                ) {
                    return Err(format!(
                        "unsupported ARROW:schema FloatingPoint precision {:?}",
                        t.precision()
                    ));
                }
            }
            arrow::ipc::Type::Date => {
                let t = f
                    .type_as_date()
                    .ok_or_else(|| "ARROW:schema Date omits type table".to_owned())?;
                if !matches!(
                    t.unit(),
                    arrow::ipc::DateUnit::DAY | arrow::ipc::DateUnit::MILLISECOND
                ) {
                    return Err(format!("unsupported ARROW:schema Date unit {:?}", t.unit()));
                }
            }
            arrow::ipc::Type::Time => {
                let t = f
                    .type_as_time()
                    .ok_or_else(|| "ARROW:schema Time omits type table".to_owned())?;
                if !matches!(
                    (t.bitWidth(), t.unit()),
                    (
                        32,
                        arrow::ipc::TimeUnit::SECOND | arrow::ipc::TimeUnit::MILLISECOND
                    ) | (
                        64,
                        arrow::ipc::TimeUnit::MICROSECOND | arrow::ipc::TimeUnit::NANOSECOND
                    )
                ) {
                    return Err(format!(
                        "unsupported ARROW:schema Time ({}, {:?})",
                        t.bitWidth(),
                        t.unit()
                    ));
                }
            }
            arrow::ipc::Type::Timestamp => {
                let t = f
                    .type_as_timestamp()
                    .ok_or_else(|| "ARROW:schema Timestamp omits type table".to_owned())?;
                if let Some(tz) = t.timezone() {
                    account(&mut expanded, tz.len())?;
                }
                if !matches!(
                    t.unit(),
                    arrow::ipc::TimeUnit::SECOND
                        | arrow::ipc::TimeUnit::MILLISECOND
                        | arrow::ipc::TimeUnit::MICROSECOND
                        | arrow::ipc::TimeUnit::NANOSECOND
                ) {
                    return Err(format!(
                        "unsupported ARROW:schema Timestamp unit {:?}",
                        t.unit()
                    ));
                }
            }
            arrow::ipc::Type::Interval => {
                let t = f
                    .type_as_interval()
                    .ok_or_else(|| "ARROW:schema Interval omits type table".to_owned())?;
                if !matches!(
                    t.unit(),
                    arrow::ipc::IntervalUnit::YEAR_MONTH
                        | arrow::ipc::IntervalUnit::DAY_TIME
                        | arrow::ipc::IntervalUnit::MONTH_DAY_NANO
                ) {
                    return Err(format!(
                        "unsupported ARROW:schema Interval unit {:?}",
                        t.unit()
                    ));
                }
            }
            arrow::ipc::Type::Duration => {
                let t = f
                    .type_as_duration()
                    .ok_or_else(|| "ARROW:schema Duration omits type table".to_owned())?;
                if !matches!(
                    t.unit(),
                    arrow::ipc::TimeUnit::SECOND
                        | arrow::ipc::TimeUnit::MILLISECOND
                        | arrow::ipc::TimeUnit::MICROSECOND
                        | arrow::ipc::TimeUnit::NANOSECOND
                ) {
                    return Err(format!(
                        "unsupported ARROW:schema Duration unit {:?}",
                        t.unit()
                    ));
                }
            }
            arrow::ipc::Type::Decimal => {
                let t = f
                    .type_as_decimal()
                    .ok_or_else(|| "ARROW:schema Decimal omits type table".to_owned())?;
                if u8::try_from(t.precision()).is_err()
                    || i8::try_from(t.scale()).is_err()
                    || !matches!(t.bitWidth(), 32 | 64 | 128 | 256)
                {
                    return Err(format!(
                        "unsupported ARROW:schema Decimal ({}, {}, {})",
                        t.precision(),
                        t.scale(),
                        t.bitWidth()
                    ));
                }
            }
            arrow::ipc::Type::List
            | arrow::ipc::Type::LargeList
            | arrow::ipc::Type::ListView
            | arrow::ipc::Type::LargeListView
            | arrow::ipc::Type::FixedSizeList
            | arrow::ipc::Type::Struct_
            | arrow::ipc::Type::RunEndEncoded
            | arrow::ipc::Type::Map
            | arrow::ipc::Type::Union => {
                return Err(
                    "nested ARROW:schema column rejected by flat schema contract".to_owned(),
                );
            }
            other => {
                return Err(format!(
                    "unsupported or missing ARROW:schema field type {other:?}"
                ));
            }
        }
    }
    Ok(expanded)
}

struct ThriftSliceCursor<'a> {
    buf: &'a [u8],
    items_visited: usize,
    est_bytes: usize,
    arrow_schema_b64: Option<&'a str>,
    non_arrow_kv_bytes: usize,
}

impl<'a> ThriftSliceCursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self {
            buf,
            items_visited: 0,
            est_bytes: 0,
            arrow_schema_b64: None,
            non_arrow_kv_bytes: 0,
        }
    }

    fn read_str_slice(&mut self) -> Result<&'a str, String> {
        let len = usize::try_from(self.read_vlq()?)
            .map_err(|_| "Thrift string length overflow".to_owned())?;
        if self.buf.len() < len {
            return Err("truncated UTF-8 string in parquet Thrift footer".to_owned());
        }
        let (head, rest) = self.buf.split_at(len);
        self.buf = rest;
        self.est_bytes = self.est_bytes.saturating_add(len);
        std::str::from_utf8(head)
            .map_err(|_| "invalid UTF-8 string in parquet Thrift footer".to_owned())
    }

    fn account(&mut self, count: usize) -> Result<(), String> {
        account_thrift_items(&mut self.items_visited, MAX_THRIFT_FOOTER_ITEMS, count)
    }

    fn skip_unknown(&mut self, ftype: u8, depth: usize) -> Result<(), String> {
        let mut visited = self.items_visited;
        let mut est = self.est_bytes;
        let res = skip_thrift_field_value(
            self,
            ftype,
            depth,
            &mut visited,
            MAX_THRIFT_FOOTER_ITEMS,
            &mut est,
        );
        self.items_visited = visited;
        self.est_bytes = est;
        res
    }

    fn read_empty_struct(&mut self, ctx: &str) -> Result<(), String> {
        let stop = self.read_byte()?;
        if stop != FTYPE_STOP {
            return Err(format!("expected empty struct terminator for {ctx}"));
        }
        Ok(())
    }

    fn validate_key_value(&mut self, depth: usize, check_arrow_schema: bool) -> Result<(), String> {
        if depth == 0 {
            return Err("parquet KeyValue exceeds max Thrift depth".to_owned());
        }
        let mut last_id = 0_i16;
        let mut key: Option<&'a str> = None;
        let mut val_opt: Option<&'a str> = None;
        while let Some((ftype, fid)) = self.read_field_header(last_id)? {
            self.account(1)?;
            match fid {
                1 => {
                    require_ftype(ftype, FTYPE_BINARY, "KeyValue.key")?;
                    key = Some(self.read_str_slice()?);
                }
                2 => {
                    require_ftype(ftype, FTYPE_BINARY, "KeyValue.value")?;
                    val_opt = Some(self.read_str_slice()?);
                }
                _ => self.skip_unknown(ftype, depth - 1)?,
            }
            last_id = fid;
        }
        let key_str =
            key.ok_or_else(|| "missing required KeyValue.key in parquet footer".to_owned())?;
        if check_arrow_schema {
            if key_str == ARROW_SCHEMA_META_KEY {
                if self.arrow_schema_b64.is_some() {
                    return Err("duplicate ARROW:schema key in parquet footer metadata".to_owned());
                }
                if let Some(val_s) = val_opt {
                    if val_s.len() > MAX_ARROW_SCHEMA_KV_BYTES {
                        return Err(format!(
                            "parquet ARROW:schema metadata ({} bytes) exceeds {MAX_ARROW_SCHEMA_KV_BYTES} bytes",
                            val_s.len()
                        ));
                    }
                    if !val_s.len().is_multiple_of(4) {
                        return Err("invalid base64 length in ARROW:schema metadata".to_owned());
                    }
                    self.arrow_schema_b64 = Some(val_s);
                }
            } else if let Some(val_s) = val_opt {
                self.non_arrow_kv_bytes = self
                    .non_arrow_kv_bytes
                    .saturating_add(key_str.len().saturating_add(val_s.len()).saturating_add(64));
            }
        }
        self.est_bytes = self.est_bytes.saturating_add(96);
        Ok(())
    }

    fn validate_statistics_struct(&mut self, depth: usize) -> Result<(), String> {
        if depth == 0 {
            return Err("parquet Statistics exceeds max Thrift depth".to_owned());
        }
        let mut last_id = 0_i16;
        while let Some((ftype, fid)) = self.read_field_header(last_id)? {
            self.account(1)?;
            match fid {
                1 | 2 | 5 | 6 => {
                    require_ftype(ftype, FTYPE_BINARY, "Statistics.min_max")?;
                    let len = usize::try_from(self.read_vlq()?)
                        .map_err(|_| "Statistics binary overflow".to_owned())?;
                    self.est_bytes = self.est_bytes.saturating_add(len);
                    self.skip_bytes(len as u64)?;
                }
                3 | 4 => {
                    require_ftype(ftype, FTYPE_I64, "Statistics.count")?;
                    let _ = self.read_zig_zag()?;
                }
                7 | 8 => {
                    require_ftype(ftype, FTYPE_BOOL_TRUE, "Statistics.is_exact")?;
                }
                _ => self.skip_unknown(ftype, depth - 1)?,
            }
            last_id = fid;
        }
        self.est_bytes = self.est_bytes.saturating_add(64);
        Ok(())
    }

    fn validate_time_unit(&mut self, depth: usize) -> Result<(), String> {
        if depth == 0 {
            return Err("parquet TimeUnit exceeds max Thrift depth".to_owned());
        }
        let Some((ftype, fid)) = self.read_field_header(0)? else {
            return Err("empty TimeUnit union in parquet SchemaElement".to_owned());
        };
        self.account(1)?;
        match fid {
            1..=3 => {
                require_ftype(ftype, FTYPE_STRUCT, "TimeUnit")?;
                self.read_empty_struct("TimeUnit")?;
            }
            _ => return Err(format!("unexpected TimeUnit variant {fid}")),
        }
        if self.read_field_header(fid)?.is_some() {
            return Err("multiple fields in parquet TimeUnit union".to_owned());
        }
        Ok(())
    }

    fn validate_logical_type(&mut self, depth: usize) -> Result<(), String> {
        if depth == 0 {
            return Err("parquet LogicalType exceeds max Thrift depth".to_owned());
        }
        let Some((ftype, fid)) = self.read_field_header(0)? else {
            return Err("empty LogicalType union in parquet SchemaElement".to_owned());
        };
        self.account(1)?;
        match fid {
            1 | 2 | 3 | 4 | 6 | 11 | 12 | 13 | 14 | 15 => {
                require_ftype(ftype, FTYPE_STRUCT, "LogicalType empty variant")?;
                self.read_empty_struct("LogicalType")?;
            }
            5 => {
                require_ftype(ftype, FTYPE_STRUCT, "LogicalType.Decimal")?;
                let mut last_id = 0_i16;
                let (mut has_scale, mut has_prec) = (false, false);
                while let Some((sf, sfid)) = self.read_field_header(last_id)? {
                    self.account(1)?;
                    match sfid {
                        1 => {
                            require_ftype(sf, FTYPE_I32, "DecimalType.scale")?;
                            let _ = self.read_i32()?;
                            has_scale = true;
                        }
                        2 => {
                            require_ftype(sf, FTYPE_I32, "DecimalType.precision")?;
                            let _ = self.read_i32()?;
                            has_prec = true;
                        }
                        _ => self.skip_unknown(sf, depth - 1)?,
                    }
                    last_id = sfid;
                }
                if !has_scale || !has_prec {
                    return Err("missing required DecimalType fields in parquet schema".to_owned());
                }
            }
            7 | 8 => {
                require_ftype(ftype, FTYPE_STRUCT, "LogicalType.TimeOrTimestamp")?;
                let mut last_id = 0_i16;
                let (mut has_utc, mut has_unit) = (false, false);
                while let Some((sf, sfid)) = self.read_field_header(last_id)? {
                    self.account(1)?;
                    match sfid {
                        1 => {
                            require_ftype(sf, FTYPE_BOOL_TRUE, "TimeType.isAdjustedToUTC")?;
                            has_utc = true;
                        }
                        2 => {
                            require_ftype(sf, FTYPE_STRUCT, "TimeType.unit")?;
                            self.validate_time_unit(depth - 1)?;
                            has_unit = true;
                        }
                        _ => self.skip_unknown(sf, depth - 1)?,
                    }
                    last_id = sfid;
                }
                if !has_utc || !has_unit {
                    return Err(
                        "missing required Time/Timestamp fields in parquet schema".to_owned()
                    );
                }
            }
            10 => {
                require_ftype(ftype, FTYPE_STRUCT, "LogicalType.Integer")?;
                let mut last_id = 0_i16;
                let (mut has_width, mut has_signed) = (false, false);
                while let Some((sf, sfid)) = self.read_field_header(last_id)? {
                    self.account(1)?;
                    match sfid {
                        1 => {
                            require_ftype(sf, FTYPE_BYTE, "IntType.bitWidth")?;
                            let _ = self.read_byte()?;
                            has_width = true;
                        }
                        2 => {
                            require_ftype(sf, FTYPE_BOOL_TRUE, "IntType.isSigned")?;
                            has_signed = true;
                        }
                        _ => self.skip_unknown(sf, depth - 1)?,
                    }
                    last_id = sfid;
                }
                if !has_width || !has_signed {
                    return Err("missing required IntType fields in parquet schema".to_owned());
                }
            }
            16 => {
                require_ftype(ftype, FTYPE_STRUCT, "LogicalType.Variant")?;
                let mut last_id = 0_i16;
                while let Some((sf, sfid)) = self.read_field_header(last_id)? {
                    self.account(1)?;
                    match sfid {
                        1 => {
                            require_ftype(sf, FTYPE_BYTE, "VariantType.specification_version")?;
                            let _ = self.read_byte()?;
                        }
                        _ => self.skip_unknown(sf, depth - 1)?,
                    }
                    last_id = sfid;
                }
            }
            17 | 18 => {
                require_ftype(ftype, FTYPE_STRUCT, "LogicalType.Geo")?;
                let mut last_id = 0_i16;
                while let Some((sf, sfid)) = self.read_field_header(last_id)? {
                    self.account(1)?;
                    match sfid {
                        1 => {
                            require_ftype(sf, FTYPE_BINARY, "GeoType.crs")?;
                            let _ = self.read_str_slice()?;
                        }
                        2 if fid == 18 => {
                            require_ftype(sf, FTYPE_I32, "GeographyType.algorithm")?;
                            let _ = self.read_i32()?;
                        }
                        _ => self.skip_unknown(sf, depth - 1)?,
                    }
                    last_id = sfid;
                }
            }
            _ => self.skip_unknown(ftype, depth - 1)?,
        }
        if self.read_field_header(fid)?.is_some() {
            return Err("multiple fields in parquet LogicalType union".to_owned());
        }
        Ok(())
    }

    fn validate_schema_element(&mut self, depth: usize) -> Result<Option<i32>, String> {
        if depth == 0 {
            return Err("parquet SchemaElement exceeds max Thrift depth".to_owned());
        }
        let mut last_id = 0_i16;
        let mut has_name = false;
        let mut num_children: Option<i32> = None;
        while let Some((ftype, fid)) = self.read_field_header(last_id)? {
            self.account(1)?;
            match fid {
                1 => {
                    require_ftype(ftype, FTYPE_I32, "SchemaElement.type")?;
                    let _ = self.read_i32()?;
                }
                2 => {
                    require_ftype(ftype, FTYPE_I32, "SchemaElement.type_length")?;
                    let _ = self.read_i32()?;
                }
                3 => {
                    require_ftype(ftype, FTYPE_I32, "SchemaElement.repetition_type")?;
                    let _ = self.read_i32()?;
                }
                4 => {
                    require_ftype(ftype, FTYPE_BINARY, "SchemaElement.name")?;
                    let _ = self.read_str_slice()?;
                    has_name = true;
                }
                5 => {
                    require_ftype(ftype, FTYPE_I32, "SchemaElement.num_children")?;
                    num_children = Some(self.read_i32()?);
                }
                6 => {
                    require_ftype(ftype, FTYPE_I32, "SchemaElement.converted_type")?;
                    let _ = self.read_i32()?;
                }
                7 => {
                    require_ftype(ftype, FTYPE_I32, "SchemaElement.scale")?;
                    let _ = self.read_i32()?;
                }
                8 => {
                    require_ftype(ftype, FTYPE_I32, "SchemaElement.precision")?;
                    let _ = self.read_i32()?;
                }
                9 => {
                    require_ftype(ftype, FTYPE_I32, "SchemaElement.field_id")?;
                    let _ = self.read_i32()?;
                }
                10 => {
                    require_ftype(ftype, FTYPE_STRUCT, "SchemaElement.logicalType")?;
                    self.validate_logical_type(depth - 1)?;
                }
                _ => self.skip_unknown(ftype, depth - 1)?,
            }
            last_id = fid;
        }
        if !has_name {
            return Err("missing required SchemaElement.name in parquet footer".to_owned());
        }
        self.est_bytes = self.est_bytes.saturating_add(384);
        Ok(num_children)
    }

    fn validate_schema_list(&mut self, depth: usize) -> Result<usize, String> {
        let (_, count) = self.read_list_begin(Some(FTYPE_STRUCT))?;
        if count == 0 || count > MAX_PARQUET_SCHEMA_ELEMENTS {
            return Err(format!(
                "parquet SchemaElement list count {count} outside allowed range 1..={MAX_PARQUET_SCHEMA_ELEMENTS}"
            ));
        }
        self.account(count)?;
        let mut remaining_children_stack = [0_usize; MAX_PARQUET_SCHEMA_DEPTH];
        let mut stack_len = 0_usize;
        for idx in 0..count {
            let num_children = self.validate_schema_element(depth - 1)?;
            if idx == 0 {
                match num_children {
                    None | Some(0) => {
                        if count != 1 {
                            return Err(
                                "parquet root schema with 0 children has trailing SchemaElements"
                                    .to_owned(),
                            );
                        }
                    }
                    Some(n) if n < 0 => {
                        return Err(format!(
                            "negative parquet root SchemaElement.num_children {n}"
                        ));
                    }
                    Some(n) => {
                        let nc =
                            usize::try_from(n).map_err(|_| "invalid num_children".to_owned())?;
                        if nc > count - 1 {
                            return Err(format!(
                                "parquet root SchemaElement num_children {nc} exceeds elements {}",
                                count - 1
                            ));
                        }
                        remaining_children_stack[stack_len] = nc;
                        stack_len += 1;
                    }
                }
            } else {
                while stack_len > 0 && remaining_children_stack[stack_len - 1] == 0 {
                    stack_len -= 1;
                }
                if stack_len == 0 {
                    return Err("extra SchemaElement outside parquet root schema tree".to_owned());
                }
                remaining_children_stack[stack_len - 1] -= 1;
                if let Some(n) = num_children {
                    if n < 0 {
                        return Err(format!("negative parquet SchemaElement.num_children {n}"));
                    }
                    let nc = usize::try_from(n).map_err(|_| "invalid num_children".to_owned())?;
                    if nc > 0 {
                        if nc > count - 1 - idx {
                            return Err(format!(
                                "parquet SchemaElement {idx} num_children {nc} exceeds remaining {}",
                                count - 1 - idx
                            ));
                        }
                        if stack_len >= MAX_PARQUET_SCHEMA_DEPTH {
                            return Err(format!(
                                "parquet schema logical tree depth exceeds limit {MAX_PARQUET_SCHEMA_DEPTH}"
                            ));
                        }
                        remaining_children_stack[stack_len] = nc;
                        stack_len += 1;
                    }
                }
            }
        }
        while stack_len > 0 && remaining_children_stack[stack_len - 1] == 0 {
            stack_len -= 1;
        }
        if stack_len != 0 {
            return Err(
                "truncated parquet schema tree before SchemaElement children satisfied".to_owned(),
            );
        }
        Ok(count)
    }

    fn validate_column_metadata(&mut self, depth: usize) -> Result<(), String> {
        if depth == 0 {
            return Err("parquet ColumnMetaData exceeds max Thrift depth".to_owned());
        }
        let mut last_id = 0_i16;
        let mut seen = 0_u16;
        while let Some((ftype, fid)) = self.read_field_header(last_id)? {
            self.account(1)?;
            match fid {
                1 => {
                    require_ftype(ftype, FTYPE_I32, "ColumnMetaData.type")?;
                    let _ = self.read_i32()?;
                    seen |= 1 << 1;
                }
                2 => {
                    require_ftype(ftype, FTYPE_LIST, "ColumnMetaData.encodings")?;
                    let (_, count) = self.read_list_begin(Some(FTYPE_I32))?;
                    self.account(count)?;
                    for _ in 0..count {
                        let _ = self.read_i32()?;
                    }
                    seen |= 1 << 2;
                }
                3 => {
                    require_ftype(ftype, FTYPE_LIST, "ColumnMetaData.path_in_schema")?;
                    let (_, count) = self.read_list_begin(Some(FTYPE_BINARY))?;
                    self.account(count)?;
                    for _ in 0..count {
                        let _ = self.read_str_slice()?;
                    }
                }
                4 => {
                    require_ftype(ftype, FTYPE_I32, "ColumnMetaData.codec")?;
                    let _ = self.read_i32()?;
                    seen |= 1 << 4;
                }
                5 => {
                    require_ftype(ftype, FTYPE_I64, "ColumnMetaData.num_values")?;
                    let _ = self.read_zig_zag()?;
                    seen |= 1 << 5;
                }
                6 => {
                    require_ftype(ftype, FTYPE_I64, "ColumnMetaData.total_uncompressed_size")?;
                    let _ = self.read_zig_zag()?;
                    seen |= 1 << 6;
                }
                7 => {
                    require_ftype(ftype, FTYPE_I64, "ColumnMetaData.total_compressed_size")?;
                    let _ = self.read_zig_zag()?;
                    seen |= 1 << 7;
                }
                8 => {
                    require_ftype(ftype, FTYPE_LIST, "ColumnMetaData.key_value_metadata")?;
                    let (_, count) = self.read_list_begin(Some(FTYPE_STRUCT))?;
                    self.account(count)?;
                    for _ in 0..count {
                        self.validate_key_value(depth - 1, false)?;
                    }
                }
                9 => {
                    require_ftype(ftype, FTYPE_I64, "ColumnMetaData.data_page_offset")?;
                    let _ = self.read_zig_zag()?;
                    seen |= 1 << 9;
                }
                10 | 11 | 14 => {
                    require_ftype(ftype, FTYPE_I64, "ColumnMetaData.offset")?;
                    let _ = self.read_zig_zag()?;
                }
                12 => {
                    require_ftype(ftype, FTYPE_STRUCT, "ColumnMetaData.statistics")?;
                    self.validate_statistics_struct(depth - 1)?;
                }
                13 => {
                    require_ftype(ftype, FTYPE_LIST, "ColumnMetaData.encoding_stats")?;
                    let (_, count) = self.read_list_begin(Some(FTYPE_STRUCT))?;
                    self.account(count)?;
                    self.est_bytes = self.est_bytes.saturating_add(count.saturating_mul(64));
                    for _ in 0..count {
                        let mut pes_last = 0_i16;
                        let mut pes_mask = 0_u8;
                        while let Some((pf, pfid)) = self.read_field_header(pes_last)? {
                            self.account(1)?;
                            match pfid {
                                1..=3 => {
                                    require_ftype(pf, FTYPE_I32, "PageEncodingStats")?;
                                    let _ = self.read_i32()?;
                                    pes_mask |= 1 << pfid;
                                }
                                _ => self.skip_unknown(pf, depth - 2)?,
                            }
                            pes_last = pfid;
                        }
                        if pes_mask != 0b1110 {
                            return Err("missing required PageEncodingStats fields".to_owned());
                        }
                    }
                }
                15 => {
                    require_ftype(ftype, FTYPE_I32, "ColumnMetaData.bloom_filter_length")?;
                    let _ = self.read_i32()?;
                }
                16 => {
                    require_ftype(ftype, FTYPE_STRUCT, "ColumnMetaData.size_statistics")?;
                    let mut ss_last = 0_i16;
                    while let Some((sf, sfid)) = self.read_field_header(ss_last)? {
                        self.account(1)?;
                        match sfid {
                            1 => {
                                require_ftype(sf, FTYPE_I64, "SizeStatistics.unencoded")?;
                                let _ = self.read_zig_zag()?;
                            }
                            2 | 3 => {
                                require_ftype(sf, FTYPE_LIST, "SizeStatistics.histogram")?;
                                let (_, count) = self.read_list_begin(Some(FTYPE_I64))?;
                                self.account(count)?;
                                self.est_bytes =
                                    self.est_bytes.saturating_add(count.saturating_mul(16));
                                for _ in 0..count {
                                    let _ = self.read_zig_zag()?;
                                }
                            }
                            _ => self.skip_unknown(sf, depth - 2)?,
                        }
                        ss_last = sfid;
                    }
                }
                17 => {
                    require_ftype(ftype, FTYPE_STRUCT, "ColumnMetaData.geospatial_statistics")?;
                    let mut gs_last = 0_i16;
                    while let Some((gf, gfid)) = self.read_field_header(gs_last)? {
                        self.account(1)?;
                        match gfid {
                            1 => {
                                require_ftype(gf, FTYPE_STRUCT, "GeospatialStatistics.bbox")?;
                                let mut bb_last = 0_i16;
                                let mut bb_req = 0_u8;
                                while let Some((bf, bfid)) = self.read_field_header(bb_last)? {
                                    self.account(1)?;
                                    match bfid {
                                        1..=8 => {
                                            require_ftype(bf, FTYPE_DOUBLE, "BoundingBox")?;
                                            self.skip_bytes(8)?;
                                            if bfid <= 4 {
                                                bb_req |= 1 << bfid;
                                            }
                                        }
                                        _ => self.skip_unknown(bf, depth - 3)?,
                                    }
                                    bb_last = bfid;
                                }
                                if bb_req != 0b1_1110 {
                                    return Err(
                                        "missing required BoundingBox coordinates".to_owned()
                                    );
                                }
                            }
                            2 => {
                                require_ftype(gf, FTYPE_LIST, "GeospatialStatistics.types")?;
                                let (_, count) = self.read_list_begin(Some(FTYPE_I32))?;
                                self.account(count)?;
                                self.est_bytes =
                                    self.est_bytes.saturating_add(count.saturating_mul(16));
                                for _ in 0..count {
                                    let _ = self.read_i32()?;
                                }
                            }
                            _ => self.skip_unknown(gf, depth - 2)?,
                        }
                        gs_last = gfid;
                    }
                }
                _ => self.skip_unknown(ftype, depth - 1)?,
            }
            last_id = fid;
        }
        const REQ_MASK: u16 =
            (1 << 1) | (1 << 2) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7) | (1 << 9);
        if seen != REQ_MASK {
            return Err("missing required ColumnMetaData fields in parquet footer".to_owned());
        }
        self.est_bytes = self.est_bytes.saturating_add(384);
        Ok(())
    }

    fn validate_column_chunk(&mut self, depth: usize) -> Result<(), String> {
        if depth == 0 {
            return Err("parquet ColumnChunk exceeds max Thrift depth".to_owned());
        }
        let mut last_id = 0_i16;
        let (mut has_file_offset, mut has_meta) = (false, false);
        while let Some((ftype, fid)) = self.read_field_header(last_id)? {
            self.account(1)?;
            match fid {
                1 => {
                    require_ftype(ftype, FTYPE_BINARY, "ColumnChunk.file_path")?;
                    let _ = self.read_str_slice()?;
                }
                2 => {
                    require_ftype(ftype, FTYPE_I64, "ColumnChunk.file_offset")?;
                    let _ = self.read_zig_zag()?;
                    has_file_offset = true;
                }
                3 => {
                    require_ftype(ftype, FTYPE_STRUCT, "ColumnChunk.meta_data")?;
                    self.validate_column_metadata(depth - 1)?;
                    has_meta = true;
                }
                4 | 6 => {
                    require_ftype(ftype, FTYPE_I64, "ColumnChunk.index_offset")?;
                    let _ = self.read_zig_zag()?;
                }
                5 | 7 => {
                    require_ftype(ftype, FTYPE_I32, "ColumnChunk.index_length")?;
                    let _ = self.read_i32()?;
                }
                _ => self.skip_unknown(ftype, depth - 1)?,
            }
            last_id = fid;
        }
        if !has_file_offset || !has_meta {
            return Err("missing required ColumnChunk fields in parquet footer".to_owned());
        }
        self.est_bytes = self.est_bytes.saturating_add(384);
        Ok(())
    }

    fn validate_row_group(&mut self, depth: usize) -> Result<(), String> {
        if depth == 0 {
            return Err("parquet RowGroup exceeds max Thrift depth".to_owned());
        }
        let mut last_id = 0_i16;
        let (mut has_cols, mut has_bytes, mut has_rows) = (false, false, false);
        while let Some((ftype, fid)) = self.read_field_header(last_id)? {
            self.account(1)?;
            match fid {
                1 => {
                    require_ftype(ftype, FTYPE_LIST, "RowGroup.columns")?;
                    let (_, count) = self.read_list_begin(Some(FTYPE_STRUCT))?;
                    self.account(count)?;
                    for _ in 0..count {
                        self.validate_column_chunk(depth - 1)?;
                    }
                    has_cols = true;
                }
                2 => {
                    require_ftype(ftype, FTYPE_I64, "RowGroup.total_byte_size")?;
                    let _ = self.read_zig_zag()?;
                    has_bytes = true;
                }
                3 => {
                    require_ftype(ftype, FTYPE_I64, "RowGroup.num_rows")?;
                    let _ = self.read_zig_zag()?;
                    has_rows = true;
                }
                4 => {
                    require_ftype(ftype, FTYPE_LIST, "RowGroup.sorting_columns")?;
                    let (_, count) = self.read_list_begin(Some(FTYPE_STRUCT))?;
                    self.account(count)?;
                    self.est_bytes = self.est_bytes.saturating_add(count.saturating_mul(64));
                    for _ in 0..count {
                        let mut sc_last = 0_i16;
                        let mut sc_mask = 0_u8;
                        while let Some((sf, sfid)) = self.read_field_header(sc_last)? {
                            self.account(1)?;
                            match sfid {
                                1 => {
                                    require_ftype(sf, FTYPE_I32, "SortingColumn.column_idx")?;
                                    let _ = self.read_i32()?;
                                    sc_mask |= 1 << 1;
                                }
                                2 | 3 => {
                                    require_ftype(sf, FTYPE_BOOL_TRUE, "SortingColumn.bool")?;
                                    sc_mask |= 1 << sfid;
                                }
                                _ => self.skip_unknown(sf, depth - 2)?,
                            }
                            sc_last = sfid;
                        }
                        if sc_mask != 0b1110 {
                            return Err("missing required SortingColumn fields".to_owned());
                        }
                    }
                }
                5 | 6 => {
                    require_ftype(ftype, FTYPE_I64, "RowGroup.offset_or_comp_size")?;
                    let _ = self.read_zig_zag()?;
                }
                7 => {
                    require_ftype(ftype, FTYPE_I16, "RowGroup.ordinal")?;
                    let _ = self.read_i16()?;
                }
                _ => self.skip_unknown(ftype, depth - 1)?,
            }
            last_id = fid;
        }
        if !has_cols || !has_bytes || !has_rows {
            return Err("missing required RowGroup fields in parquet footer".to_owned());
        }
        self.est_bytes = self.est_bytes.saturating_add(384);
        Ok(())
    }

    fn validate_file_metadata(&mut self) -> Result<(), String> {
        let depth = MAX_THRIFT_FOOTER_DEPTH;
        let mut last_id = 0_i16;
        let (mut has_ver, mut has_schema, mut has_rows, mut has_rgs) = (false, false, false, false);
        while let Some((ftype, fid)) = self.read_field_header(last_id)? {
            self.account(1)?;
            match fid {
                1 => {
                    require_ftype(ftype, FTYPE_I32, "FileMetaData.version")?;
                    let _ = self.read_i32()?;
                    has_ver = true;
                }
                2 => {
                    require_ftype(ftype, FTYPE_LIST, "FileMetaData.schema")?;
                    let _ = self.validate_schema_list(depth - 1)?;
                    has_schema = true;
                }
                3 => {
                    require_ftype(ftype, FTYPE_I64, "FileMetaData.num_rows")?;
                    let _ = self.read_zig_zag()?;
                    has_rows = true;
                }
                4 => {
                    require_ftype(ftype, FTYPE_LIST, "FileMetaData.row_groups")?;
                    let (_, count) = self.read_list_begin(Some(FTYPE_STRUCT))?;
                    if count > i16::MAX as usize {
                        return Err(format!(
                            "parquet row group count {count} exceeds i16 max {}",
                            i16::MAX
                        ));
                    }
                    self.account(count)?;
                    for _ in 0..count {
                        self.validate_row_group(depth - 1)?;
                    }
                    has_rgs = true;
                }
                5 => {
                    require_ftype(ftype, FTYPE_LIST, "FileMetaData.key_value_metadata")?;
                    let (_, count) = self.read_list_begin(Some(FTYPE_STRUCT))?;
                    self.account(count)?;
                    for _ in 0..count {
                        self.validate_key_value(depth - 1, true)?;
                    }
                }
                6 => {
                    require_ftype(ftype, FTYPE_BINARY, "FileMetaData.created_by")?;
                    let _ = self.read_str_slice()?;
                }
                7 => {
                    require_ftype(ftype, FTYPE_LIST, "FileMetaData.column_orders")?;
                    let (_, count) = self.read_list_begin(Some(FTYPE_STRUCT))?;
                    self.account(count)?;
                    self.est_bytes = self.est_bytes.saturating_add(count.saturating_mul(64));
                    for _ in 0..count {
                        let Some((uf, ufid)) = self.read_field_header(0)? else {
                            return Err("empty ColumnOrder union in parquet footer".to_owned());
                        };
                        self.account(1)?;
                        if ufid == 1 {
                            require_ftype(uf, FTYPE_STRUCT, "ColumnOrder.TYPE_ORDER")?;
                            self.read_empty_struct("ColumnOrder")?;
                        } else {
                            self.skip_unknown(uf, depth - 2)?;
                        }
                        if self.read_field_header(ufid)?.is_some() {
                            return Err("multiple fields in ColumnOrder union".to_owned());
                        }
                    }
                }
                _ => self.skip_unknown(ftype, depth - 1)?,
            }
            last_id = fid;
        }
        if !has_ver || !has_schema || !has_rows || !has_rgs {
            return Err("missing required FileMetaData fields in parquet footer".to_owned());
        }
        Ok(())
    }
}

impl ThriftByteCursor for ThriftSliceCursor<'_> {
    fn remaining_bound(&self) -> u64 {
        self.buf.len() as u64
    }

    fn read_byte(&mut self) -> Result<u8, String> {
        let (&b, rest) = self
            .buf
            .split_first()
            .ok_or_else(|| "unexpected EOF validating parquet Thrift footer".to_owned())?;
        self.buf = rest;
        Ok(b)
    }

    fn skip_bytes(&mut self, n: u64) -> Result<(), String> {
        let n = usize::try_from(n).map_err(|_| "Thrift footer skip offset overflow".to_owned())?;
        if self.buf.len() < n {
            return Err("truncated payload in parquet Thrift footer".to_owned());
        }
        self.buf = &self.buf[n..];
        Ok(())
    }
}

/// Zero-allocation Thrift footer preflight summary carrying the borrowed `ARROW:schema` slice
/// so callers can account base64 decode memory inside a pool reservation before allocating.
#[derive(Debug, Clone, Copy)]
pub struct ParquetFooterPreflight<'a> {
    est_pq_meta_bytes: usize,
    arrow_schema_b64: Option<&'a str>,
    non_arrow_kv_bytes: usize,
}

impl<'a> ParquetFooterPreflight<'a> {
    /// Returns the estimated `ParquetMetaData` heap footprint from the Thrift scan.
    #[must_use]
    pub const fn est_pq_meta_bytes(&self) -> usize {
        self.est_pq_meta_bytes
    }

    /// Returns the exact transient buffer size required to base64-decode `ARROW:schema`.
    #[must_use]
    pub fn decoded_arrow_ipc_bytes(&self) -> usize {
        self.arrow_schema_b64
            .map_or(0, |s| (s.len() / 4).saturating_mul(3))
    }

    /// Validates any embedded `ARROW:schema` FlatBuffer IPC message and returns total retained
    /// Arrow metadata + schema bytes.
    pub fn validate_arrow_schema_ipc(&self) -> Result<usize, String> {
        let ipc_expanded = match self.arrow_schema_b64 {
            Some(b64) => validate_embedded_arrow_schema_ipc(b64)?,
            None => 0,
        };
        Ok(self.non_arrow_kv_bytes.saturating_add(ipc_expanded))
    }
}

/// Scans Thrift Compact wire types, field structures, collection bounds, and non-recursive
/// Parquet schema tree depth without allocating heap memory, deferring `ARROW:schema` base64
/// decoding until `ParquetFooterPreflight::validate_arrow_schema_ipc`.
pub fn scan_parquet_thrift_footer_slice(
    bytes: &[u8],
) -> Result<ParquetFooterPreflight<'_>, String> {
    let mut cur = ThriftSliceCursor::new(bytes);
    cur.validate_file_metadata()?;
    Ok(ParquetFooterPreflight {
        est_pq_meta_bytes: cur.est_bytes,
        arrow_schema_b64: cur.arrow_schema_b64,
        non_arrow_kv_bytes: cur.non_arrow_kv_bytes,
    })
}

/// Estimates transient HashMap clone memory allocated inside `parse_key_value_metadata` during
/// `ArrowReaderMetadata::try_new`.
#[must_use]
pub fn estimate_kv_metadata_clone_bytes(pq_meta: &ParquetMetaData) -> usize {
    pq_meta
        .file_metadata()
        .key_value_metadata()
        .map_or(0, |kvs| {
            kvs.iter()
                .filter_map(|kv| {
                    kv.value
                        .as_ref()
                        .map(|val| kv.key.len().saturating_add(val.len()).saturating_add(64))
                })
                .fold(0_usize, usize::saturating_add)
        })
}

/// Validates Thrift Compact wire types, field structures, collection bounds, `ARROW:schema` IPC
/// structure and expansion, and non-recursive Parquet schema tree depth on raw footer bytes,
/// returning `(est_pq_meta_bytes, est_arrow_ipc_bytes)`.
pub fn validate_parquet_thrift_footer_with_ipc(bytes: &[u8]) -> Result<(usize, usize), String> {
    let preflight = scan_parquet_thrift_footer_slice(bytes)?;
    let est_ipc = preflight.validate_arrow_schema_ipc()?;
    Ok((preflight.est_pq_meta_bytes(), est_ipc))
}

/// Validates Thrift Compact wire types, field structures, collection bounds, `ARROW:schema` size/IPC,
/// and non-recursive Parquet schema tree depth on raw serialized footer bytes before decoding.
pub fn validate_parquet_thrift_footer(bytes: &[u8]) -> Result<usize, String> {
    let (est_pq, est_ipc) = validate_parquet_thrift_footer_with_ipc(bytes)?;
    Ok(est_pq.saturating_add(est_ipc))
}

fn estimate_arrow_field_bytes(field: &Field) -> usize {
    let mut bytes = 256usize.saturating_add(field.name().len());
    for (k, v) in field.metadata() {
        bytes = bytes
            .saturating_add(k.len())
            .saturating_add(v.len())
            .saturating_add(64);
    }
    match field.data_type() {
        DataType::List(inner)
        | DataType::LargeList(inner)
        | DataType::ListView(inner)
        | DataType::LargeListView(inner)
        | DataType::FixedSizeList(inner, _)
        | DataType::Map(inner, _) => {
            bytes = bytes.saturating_add(estimate_arrow_field_bytes(inner));
        }
        DataType::Struct(children) => {
            for child in children {
                bytes = bytes.saturating_add(estimate_arrow_field_bytes(child));
            }
        }
        DataType::Union(fields, _) => {
            for (_, child) in fields.iter() {
                bytes = bytes.saturating_add(estimate_arrow_field_bytes(child));
            }
        }
        DataType::RunEndEncoded(r, v) => {
            bytes = bytes
                .saturating_add(estimate_arrow_field_bytes(r))
                .saturating_add(estimate_arrow_field_bytes(v));
        }
        _ => {}
    }
    bytes
}

/// Estimates heap memory retained by an Arrow `Schema` inside `ArrowReaderMetadata`.
#[must_use]
pub fn estimate_arrow_schema_bytes(schema: &Schema) -> usize {
    let mut total = 128usize;
    for (k, v) in schema.metadata() {
        total = total
            .saturating_add(k.len())
            .saturating_add(v.len())
            .saturating_add(64);
    }
    for field in schema.fields() {
        total = total.saturating_add(estimate_arrow_field_bytes(field));
    }
    total
}

/// Inspects Parquet leading/trailing `PAR1` magics and validates serialized footer byte bounds.
pub fn inspect_parquet_footer_bounds(file: &File) -> Result<(u64, usize), String> {
    let file_len = file.metadata().map_err(|e| e.to_string())?.len();
    if file_len < 12 {
        return Err(format!("corrupt parquet file too small ({file_len} bytes)"));
    }
    let mut reader = file.try_clone().map_err(|e| e.to_string())?;
    reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut head_magic = [0_u8; 4];
    reader
        .read_exact(&mut head_magic)
        .map_err(|e| e.to_string())?;
    if &head_magic != b"PAR1" {
        return Err("corrupt parquet leading magic".to_owned());
    }
    reader.seek(SeekFrom::End(-8)).map_err(|e| e.to_string())?;
    let mut tail = [0_u8; 8];
    reader.read_exact(&mut tail).map_err(|e| e.to_string())?;
    if &tail[4..8] != b"PAR1" {
        return Err("corrupt parquet trailing magic".to_owned());
    }
    let footer_len = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]) as usize;
    if footer_len == 0
        || u64::try_from(footer_len)
            .ok()
            .and_then(|n| n.checked_add(12))
            .is_none_or(|need| need > file_len)
        || footer_len > MAX_PARQUET_FOOTER_SERIALIZED_BYTES
    {
        return Err(format!(
            "parquet footer length {footer_len} exceeds safe bounds for {file_len}-byte file"
        ));
    }
    Ok((file_len, footer_len))
}

/// Reads exactly `footer_len` serialized Thrift footer bytes from an already bounds-checked file,
/// re-verifying file length and trailing `[footer_len, PAR1]` against concurrent modification.
pub fn read_parquet_footer_slice(
    file: &File,
    file_len: u64,
    footer_len: usize,
) -> Result<Vec<u8>, String> {
    let cur_len = file.metadata().map_err(|e| e.to_string())?.len();
    if cur_len != file_len {
        return Err(format!(
            "parquet file length changed concurrently ({cur_len} != {file_len}) before footer read"
        ));
    }
    let footer_start = file_len
        .checked_sub(8_u64.saturating_add(footer_len as u64))
        .ok_or_else(|| "invalid parquet footer start offset".to_owned())?;
    let mut reader = file.try_clone().map_err(|e| e.to_string())?;
    reader
        .seek(SeekFrom::Start(footer_start))
        .map_err(|e| e.to_string())?;
    let mut buf = vec![0_u8; footer_len.saturating_add(8)];
    reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
    let tail = &buf[footer_len..footer_len + 8];
    let observed_len = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]) as usize;
    if &tail[4..8] != b"PAR1" || observed_len != footer_len {
        return Err("parquet footer changed concurrently during read".to_owned());
    }
    buf.truncate(footer_len);
    Ok(buf)
}

/// Rejects nested or repeated raw `ParquetMetaData` schemas before `ArrowReaderMetadata::try_new`.
pub fn validate_non_nested_parquet_file_metadata(pq_meta: &ParquetMetaData) -> Result<(), String> {
    if pq_meta.offset_index().is_some() {
        return Err("unexpected Parquet offset index in verified metadata reader".to_owned());
    }
    let schema_descr = pq_meta.file_metadata().schema_descr();
    let root_cols = schema_descr.root_schema().get_fields().len();
    if root_cols != schema_descr.num_columns() {
        return Err(format!(
            "parquet schema root width {root_cols} mismatches leaf count {} (nested schema rejected)",
            schema_descr.num_columns()
        ));
    }
    for (leaf_idx, col_descr) in schema_descr.columns().iter().enumerate() {
        if col_descr.max_rep_level() > 0 {
            return Err(format!(
                "repeated parquet column {} (leaf {leaf_idx}) exceeds flat schema contract",
                col_descr.name()
            ));
        }
        if col_descr.max_def_level() > 1 {
            return Err(format!(
                "nested parquet column {} definition level {} (leaf {leaf_idx}) exceeds flat schema contract",
                col_descr.name(),
                col_descr.max_def_level()
            ));
        }
        if col_descr.physical_type() == ParquetPhysicalType::FIXED_LEN_BYTE_ARRAY
            && col_descr.type_length() < 1
        {
            return Err(format!(
                "invalid FIXED_LEN_BYTE_ARRAY length {} on parquet column {} (leaf {leaf_idx}; must be >= 1)",
                col_descr.type_length(),
                col_descr.name()
            ));
        }
    }
    Ok(())
}

/// Rejects nested, repeated, or mismatched-root Parquet/Arrow schemas before row-group decoding.
pub fn validate_non_nested_parquet_schema(reader_meta: &ArrowReaderMetadata) -> Result<(), String> {
    validate_non_nested_parquet_file_metadata(reader_meta.metadata())?;
    let schema_descr = reader_meta.metadata().file_metadata().schema_descr();
    for field in reader_meta.schema().fields() {
        if field.data_type().is_nested() {
            return Err(format!(
                "nested arrow column {} ({:?}) rejected by flat schema contract",
                field.name(),
                field.data_type()
            ));
        }
        if matches!(field.data_type(), DataType::Dictionary(_, _)) {
            return Err(format!(
                "dictionary arrow column {} ({:?}) rejected by flat schema contract",
                field.name(),
                field.data_type()
            ));
        }
        if matches!(field.data_type(), DataType::FixedSizeBinary(w) if *w < 1) {
            return Err(format!(
                "invalid FixedSizeBinary width in column {} ({:?})",
                field.name(),
                field.data_type()
            ));
        }
    }
    if reader_meta.schema().fields().len() != schema_descr.num_columns() {
        return Err(format!(
            "arrow schema width {} does not match parquet leaf count {}",
            reader_meta.schema().fields().len(),
            schema_descr.num_columns()
        ));
    }
    for ((leaf_idx, col_descr), field) in schema_descr
        .columns()
        .iter()
        .enumerate()
        .zip(reader_meta.schema().fields().iter())
    {
        match (col_descr.physical_type(), field.data_type()) {
            (ParquetPhysicalType::FIXED_LEN_BYTE_ARRAY, DataType::Decimal32(_, _))
                if !(1..=4).contains(&col_descr.type_length()) =>
            {
                return Err(format!(
                    "FIXED_LEN_BYTE_ARRAY width {} on Decimal32 parquet column {} (leaf {leaf_idx}) exceeds 4 bytes",
                    col_descr.type_length(),
                    field.name()
                ));
            }
            (ParquetPhysicalType::FIXED_LEN_BYTE_ARRAY, DataType::Decimal64(_, _))
                if !(1..=8).contains(&col_descr.type_length()) =>
            {
                return Err(format!(
                    "FIXED_LEN_BYTE_ARRAY width {} on Decimal64 parquet column {} (leaf {leaf_idx}) exceeds 8 bytes",
                    col_descr.type_length(),
                    field.name()
                ));
            }
            (ParquetPhysicalType::FIXED_LEN_BYTE_ARRAY, DataType::Decimal128(_, _))
                if !(1..=16).contains(&col_descr.type_length()) =>
            {
                return Err(format!(
                    "FIXED_LEN_BYTE_ARRAY width {} on Decimal128 parquet column {} (leaf {leaf_idx}) exceeds 16 bytes",
                    col_descr.type_length(),
                    field.name()
                ));
            }
            (ParquetPhysicalType::FIXED_LEN_BYTE_ARRAY, DataType::Decimal256(_, _))
                if !(1..=32).contains(&col_descr.type_length()) =>
            {
                return Err(format!(
                    "FIXED_LEN_BYTE_ARRAY width {} on Decimal256 parquet column {} (leaf {leaf_idx}) exceeds 32 bytes",
                    col_descr.type_length(),
                    field.name()
                ));
            }
            (
                ParquetPhysicalType::BYTE_ARRAY,
                DataType::Decimal32(_, _) | DataType::Decimal64(_, _),
            ) => {
                return Err(format!(
                    "unsupported BYTE_ARRAY Decimal32/Decimal64 parquet column {} (leaf {leaf_idx})",
                    field.name()
                ));
            }
            (ParquetPhysicalType::INT64, DataType::Decimal32(_, _)) => {
                return Err(format!(
                    "unsupported INT64 Decimal32 parquet column {} (leaf {leaf_idx})",
                    field.name()
                ));
            }
            (ParquetPhysicalType::FIXED_LEN_BYTE_ARRAY, DataType::Interval(_))
                if col_descr.type_length() != 12 =>
            {
                return Err(format!(
                    "FIXED_LEN_BYTE_ARRAY Interval width {} on parquet column {} (leaf {leaf_idx}) must equal 12",
                    col_descr.type_length(),
                    field.name()
                ));
            }
            (ParquetPhysicalType::FIXED_LEN_BYTE_ARRAY, DataType::Float16)
                if col_descr.type_length() != 2 =>
            {
                return Err(format!(
                    "FIXED_LEN_BYTE_ARRAY Float16 width {} on parquet column {} (leaf {leaf_idx}) must equal 2",
                    col_descr.type_length(),
                    field.name()
                ));
            }
            (_, DataType::Null) if col_descr.max_def_level() == 0 => {
                return Err(format!(
                    "required parquet column {} (leaf {leaf_idx}) cannot have DataType::Null",
                    field.name()
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Validates exact flat column names, non-nested layout, and cast compatibility against `expected`.
pub fn validate_flat_compatible_parquet_schema(
    reader_meta: &ArrowReaderMetadata,
    expected: &SchemaRef,
) -> Result<(), String> {
    validate_non_nested_parquet_schema(reader_meta)?;
    let schema_descr = reader_meta.metadata().file_metadata().schema_descr();
    if schema_descr.num_columns() != expected.fields().len() {
        return Err(format!(
            "parquet schema width {} does not match expected {}",
            schema_descr.num_columns(),
            expected.fields().len()
        ));
    }
    for ((leaf_idx, col_descr), exp_field) in schema_descr
        .columns()
        .iter()
        .enumerate()
        .zip(expected.fields().iter())
    {
        if col_descr.name() != exp_field.name() {
            return Err(format!(
                "parquet column {} at position {leaf_idx} does not match expected column {}",
                col_descr.name(),
                exp_field.name()
            ));
        }
    }
    for (idx, (pq_field, exp_field)) in reader_meta
        .schema()
        .fields()
        .iter()
        .zip(expected.fields().iter())
        .enumerate()
    {
        if pq_field.name() != exp_field.name() {
            return Err(format!(
                "arrow parquet column {} at position {idx} does not match expected column {}",
                pq_field.name(),
                exp_field.name()
            ));
        }
        if !can_cast_types(pq_field.data_type(), exp_field.data_type()) {
            return Err(format!(
                "incompatible parquet column {} type {:?} for expected {:?}",
                pq_field.name(),
                pq_field.data_type(),
                exp_field.data_type()
            ));
        }
    }
    Ok(())
}

/// Validates negative-free column chunk offsets and verifies both `dictionary_page_offset` and
/// `data_page_offset` reside inside the column chunk's data-section byte range.
pub fn checked_column_byte_range(
    col: &ColumnChunkMetaData,
    col_idx: usize,
    max_data_end: u64,
) -> Result<(u64, u64), String> {
    let data_off = col.data_page_offset();
    if data_off < 4 {
        return Err(format!(
            "parquet column chunk {col_idx} invalid data_page_offset {data_off}"
        ));
    }
    let data_off_u64 = u64::try_from(data_off)
        .map_err(|_| format!("parquet column chunk {col_idx} invalid data_page_offset"))?;
    let raw_comp = col.compressed_size();
    if raw_comp < 0 {
        return Err(format!(
            "parquet column chunk {col_idx} negative compressed size {raw_comp}"
        ));
    }
    let col_len = u64::try_from(raw_comp)
        .map_err(|_| format!("parquet column chunk {col_idx} invalid length"))?;
    let raw_start = match col.dictionary_page_offset() {
        Some(dict_off) => {
            if dict_off < 4 || dict_off > data_off || (col_len > 0 && dict_off >= data_off) {
                return Err(format!(
                    "parquet column chunk {col_idx} invalid dictionary_page_offset {dict_off} (data_page_offset={data_off})"
                ));
            }
            dict_off
        }
        None => data_off,
    };
    let col_start = u64::try_from(raw_start)
        .map_err(|_| format!("parquet column chunk {col_idx} invalid start offset"))?;
    let col_end = col_start
        .checked_add(col_len)
        .filter(|&end| end <= max_data_end)
        .ok_or_else(|| {
            format!(
                "parquet column chunk {col_idx} byte range {col_start}..+{col_len} exceeds data section end {max_data_end}"
            )
        })?;
    if data_off_u64 < col_start || data_off_u64 > col_end {
        return Err(format!(
            "parquet column chunk {col_idx} data_page_offset {data_off_u64} outside chunk {col_start}..{col_end}"
        ));
    }
    Ok((col_start, col_len))
}

/// Validates file/row-group row counts, non-negative sizes, chunk ranges, and unencoded byte stats.
pub fn validate_parquet_metadata_invariants(
    reader_meta: &ArrowReaderMetadata,
    file_len: u64,
    footer_len: usize,
) -> Result<(), String> {
    let raw_file_rows = reader_meta.metadata().file_metadata().num_rows();
    if raw_file_rows < 0 {
        return Err(format!(
            "invalid negative parquet file row count {raw_file_rows}"
        ));
    }
    let footer_start = file_len
        .checked_sub(8_u64.saturating_add(footer_len as u64))
        .ok_or_else(|| "invalid parquet footer offset bounds".to_owned())?;
    let mut sum_rg_rows = 0_i64;
    for (rg_idx, rg) in reader_meta.metadata().row_groups().iter().enumerate() {
        let rg_rows = rg.num_rows();
        if rg_rows < 0 {
            return Err(format!(
                "invalid negative row-group {rg_idx} row count {rg_rows}"
            ));
        }
        if rg.total_byte_size() < 0 {
            return Err(format!(
                "invalid negative row-group {rg_idx} total_byte_size {}",
                rg.total_byte_size()
            ));
        }
        sum_rg_rows = sum_rg_rows
            .checked_add(rg_rows)
            .ok_or_else(|| "parquet row count sum overflow across row groups".to_owned())?;
        for col_idx in 0..rg.num_columns() {
            let col = rg.column(col_idx);
            let num_values = col.num_values();
            if num_values < 0 || num_values != rg_rows {
                return Err(format!(
                    "parquet rg {rg_idx} column {col_idx} num_values {num_values} mismatches row group rows {rg_rows}"
                ));
            }
            if col.compressed_size() < 0 || col.uncompressed_size() < 0 {
                return Err(format!(
                    "parquet rg {rg_idx} column {col_idx} negative byte size (comp={}, uncomp={})",
                    col.compressed_size(),
                    col.uncompressed_size()
                ));
            }
            let _ = checked_column_byte_range(col, col_idx, footer_start)?;
            if !matches!(
                col.compression(),
                Compression::UNCOMPRESSED
                    | Compression::SNAPPY
                    | Compression::ZSTD(_)
                    | Compression::LZ4_RAW
            ) {
                return Err(format!(
                    "unsupported compression codec {:?} in parquet rg {rg_idx} column {col_idx} metadata (unbounded streaming codecs rejected)",
                    col.compression()
                ));
            }
            for enc in col.encodings() {
                #[allow(deprecated)]
                if !matches!(
                    enc,
                    Encoding::PLAIN
                        | Encoding::PLAIN_DICTIONARY
                        | Encoding::RLE
                        | Encoding::RLE_DICTIONARY
                ) {
                    return Err(format!(
                        "unsupported encoding {enc:?} in parquet rg {rg_idx} column {col_idx} metadata"
                    ));
                }
            }
            if let Some(raw_unenc) = col.unencoded_byte_array_data_bytes()
                && raw_unenc < 0
            {
                return Err(format!(
                    "parquet rg {rg_idx} column {col_idx} negative unencoded byte count {raw_unenc}"
                ));
            }
        }
    }
    if sum_rg_rows != raw_file_rows {
        return Err(format!(
            "parquet file metadata num_rows {raw_file_rows} does not match row-group sum {sum_rg_rows}"
        ));
    }
    Ok(())
}

struct StackBufReader<'a, R: Read + Seek> {
    inner: &'a mut R,
    buf: [u8; 4096],
    pos: usize,
    len: usize,
}

impl<'a, R: Read + Seek> StackBufReader<'a, R> {
    fn new(inner: &'a mut R, start: u64) -> Result<Self, String> {
        inner
            .seek(SeekFrom::Start(start))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            inner,
            buf: [0_u8; 4096],
            pos: 0,
            len: 0,
        })
    }

    fn read_byte(&mut self) -> Result<u8, String> {
        if self.pos < self.len {
            let b = self.buf[self.pos];
            self.pos += 1;
            return Ok(b);
        }
        self.pos = 0;
        self.len = self.inner.read(&mut self.buf).map_err(|e| e.to_string())?;
        if self.len == 0 {
            return Err("unexpected EOF reading parquet page header".to_owned());
        }
        let b = self.buf[0];
        self.pos = 1;
        Ok(b)
    }

    fn skip_bytes(&mut self, mut n: u64) -> Result<(), String> {
        let avail = (self.len - self.pos) as u64;
        if n <= avail {
            self.pos += n as usize;
            return Ok(());
        }
        n -= avail;
        self.pos = 0;
        self.len = 0;
        while n > 0 {
            let step = i64::try_from(n.min(i64::MAX as u64))
                .map_err(|_| "page skip offset overflow".to_owned())?;
            self.inner
                .seek(SeekFrom::Current(step))
                .map_err(|e| e.to_string())?;
            n -= step as u64;
        }
        Ok(())
    }
}

struct LimitedPageHeaderCursor<'r, 'b, R: Read + Seek> {
    reader: &'r mut StackBufReader<'b, R>,
    bytes_read: u64,
    max_bytes: u64,
    items_visited: usize,
}

impl<'r, 'b, R: Read + Seek> LimitedPageHeaderCursor<'r, 'b, R> {
    fn new(reader: &'r mut StackBufReader<'b, R>, max_bytes: u64) -> Self {
        Self {
            reader,
            bytes_read: 0,
            max_bytes,
            items_visited: 0,
        }
    }

    fn account(&mut self, count: usize) -> Result<(), String> {
        account_thrift_items(&mut self.items_visited, 65_536, count)
    }

    fn skip_unknown(&mut self, ftype: u8, depth: usize) -> Result<(), String> {
        let mut visited = self.items_visited;
        let mut dummy_est = 0_usize;
        let res = skip_thrift_field_value(self, ftype, depth, &mut visited, 65_536, &mut dummy_est);
        self.items_visited = visited;
        res
    }

    fn read_page_stats(&mut self, depth: usize) -> Result<(), String> {
        if depth == 0 {
            return Err("parquet PageStatistics exceeds max depth".to_owned());
        }
        let mut last_id = 0_i16;
        while let Some((ftype, fid)) = self.read_field_header(last_id)? {
            self.account(1)?;
            match fid {
                1 | 2 | 5 | 6 => {
                    require_ftype(ftype, FTYPE_BINARY, "PageStatistics.min_max")?;
                    let len = self.read_vlq()?;
                    self.skip_bytes(len)?;
                }
                3 | 4 => {
                    require_ftype(ftype, FTYPE_I64, "PageStatistics.count")?;
                    let _ = self.read_zig_zag()?;
                }
                7 | 8 => {
                    require_ftype(ftype, FTYPE_BOOL_TRUE, "PageStatistics.is_exact")?;
                }
                _ => self.skip_unknown(ftype, depth - 1)?,
            }
            last_id = fid;
        }
        Ok(())
    }

    fn read_data_page_v1_header(&mut self) -> Result<(i32, i32, i32, i32), String> {
        let mut last_id = 0_i16;
        let mut num_values: Option<i32> = None;
        let mut encoding: Option<i32> = None;
        let mut def_level_encoding: Option<i32> = None;
        let mut rep_level_encoding: Option<i32> = None;
        let mut saw_stats = false;
        while let Some((ftype, fid)) = self.read_field_header(last_id)? {
            self.account(1)?;
            match fid {
                1 => {
                    require_ftype(ftype, FTYPE_I32, "DataPageHeader.num_values")?;
                    if num_values.is_some() {
                        return Err("duplicate DataPageHeader.num_values".to_owned());
                    }
                    num_values = Some(self.read_i32()?);
                }
                2 => {
                    require_ftype(ftype, FTYPE_I32, "DataPageHeader.encoding")?;
                    if encoding.is_some() {
                        return Err("duplicate DataPageHeader.encoding".to_owned());
                    }
                    encoding = Some(self.read_i32()?);
                }
                3 => {
                    require_ftype(ftype, FTYPE_I32, "DataPageHeader.definition_level_encoding")?;
                    if def_level_encoding.is_some() {
                        return Err("duplicate DataPageHeader.definition_level_encoding".to_owned());
                    }
                    def_level_encoding = Some(self.read_i32()?);
                }
                4 => {
                    require_ftype(ftype, FTYPE_I32, "DataPageHeader.repetition_level_encoding")?;
                    if rep_level_encoding.is_some() {
                        return Err("duplicate DataPageHeader.repetition_level_encoding".to_owned());
                    }
                    rep_level_encoding = Some(self.read_i32()?);
                }
                5 => {
                    require_ftype(ftype, FTYPE_STRUCT, "DataPageHeader.statistics")?;
                    if saw_stats {
                        return Err("duplicate DataPageHeader.statistics".to_owned());
                    }
                    saw_stats = true;
                    self.read_page_stats(8)?;
                }
                _ => self.skip_unknown(ftype, 8)?,
            }
            last_id = fid;
        }
        Ok((
            num_values.ok_or_else(|| "missing DataPageHeader.num_values".to_owned())?,
            encoding.ok_or_else(|| "missing DataPageHeader.encoding".to_owned())?,
            def_level_encoding
                .ok_or_else(|| "missing DataPageHeader.definition_level_encoding".to_owned())?,
            rep_level_encoding
                .ok_or_else(|| "missing DataPageHeader.repetition_level_encoding".to_owned())?,
        ))
    }

    fn read_dictionary_page_header(&mut self) -> Result<(i32, i32), String> {
        let mut last_id = 0_i16;
        let mut num_values: Option<i32> = None;
        let mut encoding: Option<i32> = None;
        let mut saw_sorted = false;
        while let Some((ftype, fid)) = self.read_field_header(last_id)? {
            self.account(1)?;
            match fid {
                1 => {
                    require_ftype(ftype, FTYPE_I32, "DictionaryPageHeader.num_values")?;
                    if num_values.is_some() {
                        return Err("duplicate DictionaryPageHeader.num_values".to_owned());
                    }
                    num_values = Some(self.read_i32()?);
                }
                2 => {
                    require_ftype(ftype, FTYPE_I32, "DictionaryPageHeader.encoding")?;
                    if encoding.is_some() {
                        return Err("duplicate DictionaryPageHeader.encoding".to_owned());
                    }
                    encoding = Some(self.read_i32()?);
                }
                3 => {
                    require_ftype(ftype, FTYPE_BOOL_TRUE, "DictionaryPageHeader.is_sorted")?;
                    if saw_sorted {
                        return Err("duplicate DictionaryPageHeader.is_sorted".to_owned());
                    }
                    saw_sorted = true;
                }
                _ => self.skip_unknown(ftype, 8)?,
            }
            last_id = fid;
        }
        let n = num_values.ok_or_else(|| "missing DictionaryPageHeader.num_values".to_owned())?;
        let enc = encoding.ok_or_else(|| "missing DictionaryPageHeader.encoding".to_owned())?;
        Ok((n, enc))
    }

    fn read_data_page_v2_header(&mut self) -> Result<(i32, i32, i32, i32, i32, i32), String> {
        let mut last_id = 0_i16;
        let mut num_values: Option<i32> = None;
        let mut num_nulls: Option<i32> = None;
        let mut num_rows: Option<i32> = None;
        let mut encoding: Option<i32> = None;
        let mut def_len: Option<i32> = None;
        let mut rep_len: Option<i32> = None;
        let mut saw_compressed = false;
        let mut saw_stats = false;
        while let Some((ftype, fid)) = self.read_field_header(last_id)? {
            self.account(1)?;
            match fid {
                1 => {
                    require_ftype(ftype, FTYPE_I32, "DataPageHeaderV2.num_values")?;
                    if num_values.is_some() {
                        return Err("duplicate DataPageHeaderV2.num_values".to_owned());
                    }
                    num_values = Some(self.read_i32()?);
                }
                2 => {
                    require_ftype(ftype, FTYPE_I32, "DataPageHeaderV2.num_nulls")?;
                    if num_nulls.is_some() {
                        return Err("duplicate DataPageHeaderV2.num_nulls".to_owned());
                    }
                    num_nulls = Some(self.read_i32()?);
                }
                3 => {
                    require_ftype(ftype, FTYPE_I32, "DataPageHeaderV2.num_rows")?;
                    if num_rows.is_some() {
                        return Err("duplicate DataPageHeaderV2.num_rows".to_owned());
                    }
                    num_rows = Some(self.read_i32()?);
                }
                4 => {
                    require_ftype(ftype, FTYPE_I32, "DataPageHeaderV2.encoding")?;
                    if encoding.is_some() {
                        return Err("duplicate DataPageHeaderV2.encoding".to_owned());
                    }
                    encoding = Some(self.read_i32()?);
                }
                5 => {
                    require_ftype(ftype, FTYPE_I32, "DataPageHeaderV2.def_byte_length")?;
                    if def_len.is_some() {
                        return Err("duplicate DataPageHeaderV2.def_byte_length".to_owned());
                    }
                    def_len = Some(self.read_i32()?);
                }
                6 => {
                    require_ftype(ftype, FTYPE_I32, "DataPageHeaderV2.rep_byte_length")?;
                    if rep_len.is_some() {
                        return Err("duplicate DataPageHeaderV2.rep_byte_length".to_owned());
                    }
                    rep_len = Some(self.read_i32()?);
                }
                7 => {
                    require_ftype(ftype, FTYPE_BOOL_TRUE, "DataPageHeaderV2.is_compressed")?;
                    if saw_compressed {
                        return Err("duplicate DataPageHeaderV2.is_compressed".to_owned());
                    }
                    saw_compressed = true;
                }
                8 => {
                    require_ftype(ftype, FTYPE_STRUCT, "DataPageHeaderV2.statistics")?;
                    if saw_stats {
                        return Err("duplicate DataPageHeaderV2.statistics".to_owned());
                    }
                    saw_stats = true;
                    self.read_page_stats(8)?;
                }
                _ => self.skip_unknown(ftype, 8)?,
            }
            last_id = fid;
        }
        Ok((
            num_values.ok_or_else(|| "missing DataPageHeaderV2.num_values".to_owned())?,
            num_nulls.ok_or_else(|| "missing DataPageHeaderV2.num_nulls".to_owned())?,
            num_rows.ok_or_else(|| "missing DataPageHeaderV2.num_rows".to_owned())?,
            encoding.ok_or_else(|| "missing DataPageHeaderV2.encoding".to_owned())?,
            def_len.ok_or_else(|| "missing DataPageHeaderV2.def_byte_length".to_owned())?,
            rep_len.ok_or_else(|| "missing DataPageHeaderV2.rep_byte_length".to_owned())?,
        ))
    }
}

impl<R: Read + Seek> ThriftByteCursor for LimitedPageHeaderCursor<'_, '_, R> {
    fn remaining_bound(&self) -> u64 {
        self.max_bytes.saturating_sub(self.bytes_read)
    }

    fn read_byte(&mut self) -> Result<u8, String> {
        if self.bytes_read >= self.max_bytes {
            return Err(format!(
                "parquet page header exceeds {MAX_PARQUET_PAGE_HEADER_BYTES}-byte limit"
            ));
        }
        let b = self.reader.read_byte()?;
        self.bytes_read = self.bytes_read.saturating_add(1);
        Ok(b)
    }

    fn skip_bytes(&mut self, n: u64) -> Result<(), String> {
        if n > self.max_bytes.saturating_sub(self.bytes_read) {
            return Err("parquet page header skips beyond chunk/header bounds".to_owned());
        }
        self.reader.skip_bytes(n)?;
        self.bytes_read = self.bytes_read.saturating_add(n);
        Ok(())
    }
}

fn dict_type_sizes(
    col_type: ParquetPhysicalType,
    fixed_type_len: i32,
) -> Result<(usize, usize), String> {
    match col_type {
        ParquetPhysicalType::BOOLEAN => {
            Err("dictionary page not permitted on BOOLEAN parquet column".to_owned())
        }
        ParquetPhysicalType::INT32 | ParquetPhysicalType::FLOAT => Ok((4, 4)),
        ParquetPhysicalType::INT64 | ParquetPhysicalType::DOUBLE => Ok((8, 8)),
        ParquetPhysicalType::INT96 => Ok((12, 12)),
        ParquetPhysicalType::BYTE_ARRAY => Ok((4, 16)),
        ParquetPhysicalType::FIXED_LEN_BYTE_ARRAY => {
            let width = usize::try_from(fixed_type_len)
                .ok()
                .filter(|&w| w >= 1)
                .ok_or_else(|| {
                    format!("invalid FIXED_LEN_BYTE_ARRAY type_length {fixed_type_len}")
                })?;
            Ok((width, width))
        }
    }
}

fn validate_data_page_encoding(
    encoding: i32,
    col_type: ParquetPhysicalType,
    saw_dict: bool,
) -> Result<(), String> {
    match encoding {
        // PLAIN
        0 => Ok(()),
        // PLAIN_DICTIONARY | RLE_DICTIONARY
        2 | 8 => {
            if saw_dict && col_type != ParquetPhysicalType::BOOLEAN {
                Ok(())
            } else {
                Err(format!(
                    "dictionary data page encoding {encoding} invalid without prior non-BOOLEAN dictionary"
                ))
            }
        }
        // RLE
        3 => {
            if col_type == ParquetPhysicalType::BOOLEAN {
                Ok(())
            } else {
                Err(format!(
                    "RLE data page value encoding invalid for non-BOOLEAN column {col_type:?}"
                ))
            }
        }
        _ => Err(format!(
            "unsupported Parquet data page encoding {encoding} (delta and bit-packed encodings rejected)"
        )),
    }
}

/// Scans every Thrift `PageHeader` in a column chunk using a stack-only 4 KiB buffer, enforcing
/// wire-type checks, page byte caps, encoding allowlists, `DictionaryPageHeader.num_values` bounds,
/// dictionary decompression residency, post-level value slice non-emptiness, and value counts.
pub fn scan_column_chunk_page_headers<R: Read + Seek>(
    input: &mut R,
    col_start: u64,
    col_compressed_len: u64,
    col_type: ParquetPhysicalType,
    fixed_type_len: i32,
    max_def_level: i16,
    expected_rg_rows: usize,
) -> Result<ColumnPageScanStats, String> {
    let mut reader = StackBufReader::new(input, col_start)?;
    let mut remaining = col_compressed_len;
    let mut saw_dict = false;
    let mut saw_data = false;
    let mut dict_payload_bytes = 0_usize;
    let mut dict_resident_bytes = 0_usize;
    let mut data_payload_bytes = 0_usize;
    let mut max_page_resident = 0_usize;
    let mut seen_data_values = 0_usize;

    while remaining > 0 {
        let header_cap = remaining.min(MAX_PARQUET_PAGE_HEADER_BYTES);
        let (page_type, uncomp_sz, comp_sz, v1_hdr, idx_hdr, dict_hdr, v2_hdr, header_len) = {
            let mut tr = LimitedPageHeaderCursor::new(&mut reader, header_cap);
            let mut last_id = 0_i16;
            let mut page_type: Option<i32> = None;
            let mut uncomp_sz: Option<i32> = None;
            let mut comp_sz: Option<i32> = None;
            let mut saw_crc = false;
            let mut v1_hdr: Option<(i32, i32, i32, i32)> = None;
            let mut idx_hdr = false;
            let mut dict_hdr: Option<(i32, i32)> = None;
            let mut v2_hdr: Option<(i32, i32, i32, i32, i32, i32)> = None;

            while let Some((ftype, fid)) = tr.read_field_header(last_id)? {
                tr.account(1)?;
                match fid {
                    1 => {
                        require_ftype(ftype, FTYPE_I32, "PageHeader.type")?;
                        if page_type.is_some() {
                            return Err(
                                "duplicate PageHeader.type in parquet PageHeader".to_owned()
                            );
                        }
                        page_type = Some(tr.read_i32()?);
                    }
                    2 => {
                        require_ftype(ftype, FTYPE_I32, "PageHeader.uncompressed_page_size")?;
                        if uncomp_sz.is_some() {
                            return Err(
                                "duplicate PageHeader.uncompressed_page_size in parquet PageHeader"
                                    .to_owned(),
                            );
                        }
                        uncomp_sz = Some(tr.read_i32()?);
                    }
                    3 => {
                        require_ftype(ftype, FTYPE_I32, "PageHeader.compressed_page_size")?;
                        if comp_sz.is_some() {
                            return Err(
                                "duplicate PageHeader.compressed_page_size in parquet PageHeader"
                                    .to_owned(),
                            );
                        }
                        comp_sz = Some(tr.read_i32()?);
                    }
                    4 => {
                        require_ftype(ftype, FTYPE_I32, "PageHeader.crc")?;
                        if saw_crc {
                            return Err("duplicate PageHeader.crc in parquet PageHeader".to_owned());
                        }
                        saw_crc = true;
                        let _ = tr.read_i32()?;
                    }
                    5 => {
                        require_ftype(ftype, FTYPE_STRUCT, "PageHeader.data_page_header")?;
                        if v1_hdr.is_some() {
                            return Err("duplicate DataPageHeader in parquet PageHeader".to_owned());
                        }
                        v1_hdr = Some(tr.read_data_page_v1_header()?);
                    }
                    6 => {
                        require_ftype(ftype, FTYPE_STRUCT, "PageHeader.index_page_header")?;
                        if idx_hdr {
                            return Err(
                                "duplicate IndexPageHeader in parquet PageHeader".to_owned()
                            );
                        }
                        idx_hdr = true;
                        tr.skip_unknown(ftype, 8)?;
                    }
                    7 => {
                        require_ftype(ftype, FTYPE_STRUCT, "PageHeader.dictionary_page_header")?;
                        if dict_hdr.is_some() {
                            return Err(
                                "duplicate DictionaryPageHeader in parquet PageHeader".to_owned()
                            );
                        }
                        dict_hdr = Some(tr.read_dictionary_page_header()?);
                    }
                    8 => {
                        require_ftype(ftype, FTYPE_STRUCT, "PageHeader.data_page_header_v2")?;
                        if v2_hdr.is_some() {
                            return Err(
                                "duplicate DataPageHeaderV2 in parquet PageHeader".to_owned()
                            );
                        }
                        v2_hdr = Some(tr.read_data_page_v2_header()?);
                    }
                    _ => tr.skip_unknown(ftype, 16)?,
                }
                last_id = fid;
            }
            let pt =
                page_type.ok_or_else(|| "missing PageType in parquet page header".to_owned())?;
            let usz = uncomp_sz.ok_or_else(|| {
                "missing uncompressed_page_size in parquet page header".to_owned()
            })?;
            let csz = comp_sz
                .ok_or_else(|| "missing compressed_page_size in parquet page header".to_owned())?;
            (
                pt,
                usz,
                csz,
                v1_hdr,
                idx_hdr,
                dict_hdr,
                v2_hdr,
                tr.bytes_read,
            )
        };
        if header_len > remaining || comp_sz < 0 || uncomp_sz < 0 {
            return Err(format!(
                "corrupt parquet page header (header={header_len}, remaining={remaining}, comp={comp_sz}, uncomp={uncomp_sz})"
            ));
        }
        let comp_u64 = comp_sz as u64;
        if comp_u64 > remaining - header_len {
            return Err(format!(
                "parquet page compressed size {comp_u64} exceeds remaining column chunk bytes {}",
                remaining - header_len
            ));
        }
        let comp_usize = usize::try_from(comp_sz)
            .map_err(|_| "compressed_page_size overflows usize".to_owned())?;
        let uncomp_usize = usize::try_from(uncomp_sz)
            .map_err(|_| "uncompressed_page_size overflows usize".to_owned())?;
        if uncomp_usize > MAX_PARQUET_PAGE_UNCOMPRESSED_BYTES
            || comp_usize > MAX_PARQUET_PAGE_UNCOMPRESSED_BYTES
        {
            return Err(format!(
                "Resources exhausted: parquet page size (uncomp={uncomp_usize}, comp={comp_usize}) exceeds limit {MAX_PARQUET_PAGE_UNCOMPRESSED_BYTES}"
            ));
        }
        if idx_hdr && page_type != 1 {
            return Err("unexpected IndexPageHeader on non-INDEX_PAGE".to_owned());
        }

        match page_type {
            0 => {
                if dict_hdr.is_some() || v2_hdr.is_some() {
                    return Err(
                        "unexpected DictionaryPageHeader or DataPageHeaderV2 on V1 DATA_PAGE"
                            .to_owned(),
                    );
                }
                saw_data = true;
                let (nv, val_enc, def_enc, rep_enc) =
                    v1_hdr.ok_or_else(|| "missing DataPageHeader for DATA_PAGE".to_owned())?;
                if def_enc != 3 || rep_enc != 3 {
                    return Err(format!(
                        "unsupported Parquet V1 level encoding (def={def_enc}, rep={rep_enc}; only RLE permitted)"
                    ));
                }
                validate_data_page_encoding(val_enc, col_type, saw_dict)?;
                let nv_usize = usize::try_from(nv).map_err(|_| {
                    format!("negative DataPageHeader.num_values {nv} in parquet page")
                })?;
                if expected_rg_rows > 0 && nv_usize == 0 {
                    return Err("empty DataPageHeader.num_values in non-empty row group".to_owned());
                }
                let min_v1_payload = if max_def_level > 0 {
                    match val_enc {
                        2 | 8 => 5_usize,
                        3 => 8_usize,
                        _ => 4_usize,
                    }
                } else {
                    match val_enc {
                        2 | 8 => 1_usize,
                        3 => 4_usize,
                        _ => 0_usize,
                    }
                };
                if uncomp_usize < min_v1_payload {
                    return Err(format!(
                        "truncated Parquet V1 data page uncompressed_page_size {uncomp_usize} < required minimum {min_v1_payload} for encoding {val_enc}"
                    ));
                }
                seen_data_values = seen_data_values
                    .checked_add(nv_usize)
                    .ok_or_else(|| "parquet page value count overflow".to_owned())?;
                data_payload_bytes =
                    data_payload_bytes.saturating_add(uncomp_usize.max(comp_usize));
                max_page_resident = max_page_resident.max(comp_usize.saturating_add(uncomp_usize));
            }
            1 => {
                if v1_hdr.is_some() || dict_hdr.is_some() || v2_hdr.is_some() {
                    return Err(
                        "unexpected DataPage/Dictionary sub-header on parquet INDEX_PAGE"
                            .to_owned(),
                    );
                }
                if !idx_hdr {
                    return Err("missing IndexPageHeader for INDEX_PAGE".to_owned());
                }
                // INDEX_PAGE is skipped by SerializedPageReader without decompression.
            }
            2 => {
                if v1_hdr.is_some() || v2_hdr.is_some() {
                    return Err(
                        "unexpected DataPageHeader or DataPageHeaderV2 on DICTIONARY_PAGE"
                            .to_owned(),
                    );
                }
                if saw_dict || saw_data {
                    return Err(
                        "duplicate or out-of-order DictionaryPage in parquet column chunk"
                            .to_owned(),
                    );
                }
                saw_dict = true;
                let (dict_num_vals, dict_enc) = dict_hdr
                    .ok_or_else(|| "missing DictionaryPageHeader for DICTIONARY_PAGE".to_owned())?;
                if !matches!(dict_enc, 0 | 2) {
                    return Err(format!(
                        "unsupported DictionaryPage encoding {dict_enc} in parquet column chunk"
                    ));
                }
                let dict_vals_usize = usize::try_from(dict_num_vals).map_err(|_| {
                    format!("negative DictionaryPageHeader.num_values {dict_num_vals}")
                })?;
                let (min_plain_bytes, in_mem_bytes) = dict_type_sizes(col_type, fixed_type_len)?;
                if dict_vals_usize
                    .checked_mul(min_plain_bytes)
                    .is_none_or(|need| need > uncomp_usize)
                {
                    return Err(format!(
                        "Resources exhausted: DictionaryPageHeader num_values {dict_vals_usize} requires >= {} bytes, exceeding uncompressed_page_size {uncomp_usize}",
                        dict_vals_usize.saturating_mul(min_plain_bytes)
                    ));
                }
                let decoded_dict_bytes = match col_type {
                    ParquetPhysicalType::BYTE_ARRAY => uncomp_usize
                        .saturating_mul(2)
                        .saturating_add(dict_vals_usize.saturating_mul(in_mem_bytes)),
                    _ => dict_vals_usize.saturating_mul(in_mem_bytes),
                };
                dict_payload_bytes = uncomp_usize.max(comp_usize);
                dict_resident_bytes = comp_usize
                    .saturating_add(uncomp_usize)
                    .saturating_add(decoded_dict_bytes);
            }
            3 => {
                if v1_hdr.is_some() || dict_hdr.is_some() {
                    return Err(
                        "unexpected DataPageHeader or DictionaryPageHeader on DATA_PAGE_V2"
                            .to_owned(),
                    );
                }
                saw_data = true;
                let (nv, nulls, rows, val_enc, def_len, rep_len) =
                    v2_hdr.ok_or_else(|| "missing DataPageHeaderV2 for DATA_PAGE_V2".to_owned())?;
                validate_data_page_encoding(val_enc, col_type, saw_dict)?;
                if nv < 0 || nulls < 0 || rows < 0 || def_len < 0 || rep_len < 0 || nulls > nv {
                    return Err(format!(
                        "invalid DataPageHeaderV2 field (num_values={nv}, num_nulls={nulls}, num_rows={rows}, def={def_len}, rep={rep_len})"
                    ));
                }
                if expected_rg_rows > 0 && nv == 0 {
                    return Err(
                        "empty DataPageHeaderV2.num_values in non-empty row group".to_owned()
                    );
                }
                if rep_len != 0 {
                    return Err(format!(
                        "unexpected DataPageHeaderV2 repetition levels byte length {rep_len} in flat schema"
                    ));
                }
                let lvl_bytes = (def_len as u64).saturating_add(rep_len as u64);
                if lvl_bytes > comp_u64 || lvl_bytes > uncomp_usize as u64 {
                    return Err(format!(
                        "DataPageHeaderV2 level bytes {lvl_bytes} exceed page bounds (comp={comp_u64}, uncomp={uncomp_usize})"
                    ));
                }
                let min_v2_val_bytes = match val_enc {
                    2 | 8 => 1_usize,
                    3 => 4_usize,
                    _ => 0_usize,
                };
                if (lvl_bytes as usize).saturating_add(min_v2_val_bytes) > uncomp_usize {
                    return Err(format!(
                        "truncated Parquet V2 data page value payload (uncomp={uncomp_usize}, lvl={lvl_bytes}, enc={val_enc})"
                    ));
                }
                seen_data_values = seen_data_values
                    .checked_add(nv as usize)
                    .ok_or_else(|| "parquet page v2 value count overflow".to_owned())?;
                data_payload_bytes =
                    data_payload_bytes.saturating_add(uncomp_usize.max(comp_usize));
                max_page_resident = max_page_resident.max(comp_usize.saturating_add(uncomp_usize));
            }
            _ => {
                return Err(format!("invalid parquet PageType {page_type}"));
            }
        }
        remaining = remaining - header_len - comp_u64;
        if comp_u64 > 0 {
            reader.skip_bytes(comp_u64)?;
        }
    }

    if seen_data_values != expected_rg_rows {
        return Err(format!(
            "parquet column chunk data page num_values sum {seen_data_values} mismatches row group rows {expected_rg_rows}"
        ));
    }

    Ok(ColumnPageScanStats {
        effective_payload_bytes: dict_payload_bytes.saturating_add(data_payload_bytes),
        resident_bytes: dict_resident_bytes.saturating_add(max_page_resident),
    })
}

fn read_rle_vlq_header(stream: &[u8], pos: &mut usize) -> Result<u64, String> {
    let mut indicator = 0_u64;
    let mut vlq_len = 0_usize;
    for step in 0..5_usize {
        let Some(&b) = stream.get(*pos + step) else {
            return Err("truncated VLQ indicator in parquet RLE stream".to_owned());
        };
        if step == 4 && (b & 0x80) != 0 {
            return Err("overlong VLQ indicator in parquet RLE stream".to_owned());
        }
        indicator |= u64::from(b & 0x7f) << (step * 7);
        if (b & 0x80) == 0 {
            vlq_len = step + 1;
            break;
        }
    }
    if vlq_len == 0 {
        return Err("invalid VLQ indicator in parquet RLE stream".to_owned());
    }
    *pos += vlq_len;
    if indicator == 0 {
        return Err("zero RLE indicator in parquet RLE stream".to_owned());
    }
    Ok(indicator)
}

fn unpack_bit_packed_value_le(packed: &[u8], idx: usize, bit_width: u8) -> u32 {
    let bw = usize::from(bit_width);
    let bit_pos = idx * bw;
    let byte_idx = bit_pos / 8;
    let bit_off = bit_pos % 8;
    let mut raw = [0_u8; 8];
    let avail = (packed.len() - byte_idx).min(8);
    raw[..avail].copy_from_slice(&packed[byte_idx..byte_idx + avail]);
    let word = u64::from_le_bytes(raw) >> bit_off;
    let mask = if bw == 32 {
        u64::from(u32::MAX)
    } else {
        (1_u64 << bw) - 1
    };
    (word & mask) as u32
}

fn validate_def_level_rle_stream(stream: &[u8], expected_levels: usize) -> Result<usize, String> {
    let mut pos = 0_usize;
    let mut decoded_levels = 0_usize;
    let mut non_null_levels = 0_usize;
    let max_encoded = if expected_levels == 0 {
        0
    } else {
        expected_levels.saturating_add(7)
    };
    while pos < stream.len() {
        if decoded_levels >= expected_levels {
            return Err("surplus RLE run exceeds parquet definition level count".to_owned());
        }
        let indicator = read_rle_vlq_header(stream, &mut pos)?;
        let count = indicator >> 1;
        if count == 0 || count > u64::from(u32::MAX / 8) {
            return Err("parquet RLE definition level run count invalid".to_owned());
        }
        if (indicator & 1) == 1 {
            let packed_bytes = usize::try_from(count)
                .map_err(|_| "parquet RLE bit-packed byte count overflow".to_owned())?;
            let end = pos
                .checked_add(packed_bytes)
                .filter(|&e| e <= stream.len())
                .ok_or_else(|| "truncated bit-packed run in parquet RLE stream".to_owned())?;
            let run_vals = packed_bytes.saturating_mul(8);
            let next_decoded = decoded_levels
                .checked_add(run_vals)
                .ok_or_else(|| "parquet RLE definition level count overflow".to_owned())?;
            if next_decoded > max_encoded {
                return Err(format!(
                    "surplus bit-packed definition level run ({next_decoded} > {max_encoded}) exceeds page bound"
                ));
            }
            let take = expected_levels.saturating_sub(decoded_levels).min(run_vals);
            let packed = &stream[pos..end];
            let full_bytes = take / 8;
            for &b in &packed[..full_bytes] {
                non_null_levels = non_null_levels.saturating_add(b.count_ones() as usize);
            }
            let rem_bits = take % 8;
            if rem_bits > 0 {
                let mask = (1_u8 << rem_bits) - 1;
                non_null_levels = non_null_levels
                    .saturating_add((packed[full_bytes] & mask).count_ones() as usize);
            }
            decoded_levels = next_decoded;
            pos = end;
        } else {
            let Some(&val) = stream.get(pos) else {
                return Err("truncated RLE run value in parquet definition level stream".to_owned());
            };
            pos += 1;
            if val > 1 {
                return Err(format!(
                    "invalid definition level {val} > 1 in flat parquet RLE stream"
                ));
            }
            let run_vals = count as usize;
            let next_decoded = decoded_levels
                .checked_add(run_vals)
                .ok_or_else(|| "parquet RLE definition level count overflow".to_owned())?;
            if next_decoded > expected_levels {
                return Err(format!(
                    "surplus RLE definition level run ({next_decoded} > {expected_levels}) exceeds page bound"
                ));
            }
            if val == 1 {
                non_null_levels = non_null_levels.saturating_add(run_vals);
            }
            decoded_levels = next_decoded;
        }
    }
    if decoded_levels < expected_levels {
        return Err(format!(
            "insufficient definition levels {decoded_levels} < {expected_levels} in parquet RLE stream"
        ));
    }
    Ok(non_null_levels)
}

fn validate_value_rle_stream(
    stream: &[u8],
    bit_width: u8,
    needed_values: usize,
    max_val: u32,
) -> Result<(), String> {
    let mut pos = 0_usize;
    let mut decoded_vals = 0_usize;
    let max_encoded = if needed_values == 0 {
        0
    } else {
        needed_values.saturating_add(7)
    };
    while pos < stream.len() {
        if decoded_vals >= needed_values {
            return Err("surplus RLE/bit-packed run exceeds parquet page value count".to_owned());
        }
        let indicator = read_rle_vlq_header(stream, &mut pos)?;
        let count = indicator >> 1;
        if count == 0 || count > u64::from(u32::MAX / 8) {
            return Err("parquet RLE run count invalid".to_owned());
        }
        if (indicator & 1) == 1 {
            if bit_width == 0 {
                return Err("bit-packed run invalid with zero bit_width".to_owned());
            }
            let packed_bytes = usize::try_from(count)
                .ok()
                .and_then(|g| g.checked_mul(usize::from(bit_width)))
                .ok_or_else(|| "parquet RLE bit-packed run byte count overflow".to_owned())?;
            let end = pos
                .checked_add(packed_bytes)
                .filter(|&e| e <= stream.len())
                .ok_or_else(|| "truncated bit-packed run in parquet RLE stream".to_owned())?;
            let run_vals = (count as usize).saturating_mul(8);
            let next_decoded = decoded_vals
                .checked_add(run_vals)
                .ok_or_else(|| "parquet RLE value count overflow".to_owned())?;
            if next_decoded > max_encoded {
                return Err(format!(
                    "surplus bit-packed value run ({next_decoded} > {max_encoded}) exceeds page bound"
                ));
            }
            let max_possible = if bit_width == 32 {
                u32::MAX
            } else {
                (1_u32 << bit_width) - 1
            };
            if max_possible > max_val {
                let packed = &stream[pos..end];
                for idx in 0..run_vals {
                    let v = unpack_bit_packed_value_le(packed, idx, bit_width);
                    if v > max_val {
                        return Err(format!(
                            "parquet bit-packed value {v} exceeds maximum allowed {max_val}"
                        ));
                    }
                }
            }
            decoded_vals = next_decoded;
            pos = end;
        } else {
            let val_width = usize::from(bit_width).div_ceil(8);
            let end = pos
                .checked_add(val_width)
                .filter(|&e| e <= stream.len())
                .ok_or_else(|| "truncated RLE run value in parquet RLE stream".to_owned())?;
            let run_vals = count as usize;
            let next_decoded = decoded_vals
                .checked_add(run_vals)
                .ok_or_else(|| "parquet RLE value count overflow".to_owned())?;
            if next_decoded > needed_values {
                return Err(format!(
                    "surplus RLE value run ({next_decoded} > {needed_values}) exceeds page bound"
                ));
            }
            let mut raw = [0_u8; 4];
            raw[..val_width].copy_from_slice(&stream[pos..end]);
            let val = u32::from_le_bytes(raw);
            if val > max_val {
                return Err(format!(
                    "parquet RLE value {val} exceeds maximum allowed {max_val}"
                ));
            }
            decoded_vals = next_decoded;
            pos = end;
        }
    }
    if decoded_vals < needed_values {
        return Err(format!(
            "insufficient values {decoded_vals} < {needed_values} in parquet RLE stream"
        ));
    }
    Ok(())
}

fn column_value_constraints(arrow_type: &DataType) -> (Option<usize>, bool, bool) {
    let max_byte_array_item_len = match arrow_type {
        DataType::Decimal32(_, _) => Some(4),
        DataType::Decimal64(_, _) => Some(8),
        DataType::Decimal128(_, _) => Some(16),
        DataType::Decimal256(_, _) => Some(32),
        _ => None,
    };
    let require_utf8 = matches!(
        arrow_type,
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
    );
    let is_null_col = matches!(arrow_type, DataType::Null);
    (max_byte_array_item_len, require_utf8, is_null_col)
}

#[allow(clippy::too_many_arguments)]
fn validate_data_page_value_slice(
    encoding: Encoding,
    col_type: ParquetPhysicalType,
    fixed_len: i32,
    max_byte_array_item_len: Option<usize>,
    require_utf8: bool,
    dict_vals: Option<u32>,
    non_null_count: usize,
    is_v2: bool,
    val_slice: &[u8],
) -> Result<(), String> {
    #[allow(deprecated)]
    match encoding {
        Encoding::PLAIN => {
            if col_type == ParquetPhysicalType::BYTE_ARRAY {
                if is_v2 && non_null_count == 0 && !val_slice.is_empty() {
                    return Err(
                        "non-empty PLAIN BYTE_ARRAY payload for all-null DataPageV2".to_owned()
                    );
                }
                let mut pos = 0_usize;
                for idx in 0..non_null_count {
                    let Some(lb) = pos
                        .checked_add(4)
                        .and_then(|end| val_slice.get(pos..end))
                        .and_then(|s| <[u8; 4]>::try_from(s).ok())
                    else {
                        return Err("truncated PLAIN BYTE_ARRAY value length".to_owned());
                    };
                    let item_len = u32::from_le_bytes(lb) as usize;
                    let end = pos
                        .checked_add(4)
                        .and_then(|p| p.checked_add(item_len))
                        .filter(|&e| e <= val_slice.len())
                        .ok_or_else(|| "truncated PLAIN BYTE_ARRAY value payload".to_owned())?;
                    if let Some(max_len) = max_byte_array_item_len
                        && item_len > max_len
                    {
                        return Err(format!(
                            "PLAIN BYTE_ARRAY value {idx} length {item_len} exceeds Decimal byte bound {max_len}"
                        ));
                    }
                    if require_utf8 && std::str::from_utf8(&val_slice[pos + 4..end]).is_err() {
                        return Err(format!("invalid UTF-8 in PLAIN BYTE_ARRAY value {idx}"));
                    }
                    pos = end;
                }
            } else {
                let need = match col_type {
                    ParquetPhysicalType::BOOLEAN => Some(non_null_count.div_ceil(8)),
                    ParquetPhysicalType::INT32 | ParquetPhysicalType::FLOAT => {
                        non_null_count.checked_mul(4)
                    }
                    ParquetPhysicalType::INT64 | ParquetPhysicalType::DOUBLE => {
                        non_null_count.checked_mul(8)
                    }
                    ParquetPhysicalType::INT96 => non_null_count.checked_mul(12),
                    ParquetPhysicalType::FIXED_LEN_BYTE_ARRAY => usize::try_from(fixed_len)
                        .ok()
                        .filter(|&w| w >= 1)
                        .and_then(|w| non_null_count.checked_mul(w)),
                    ParquetPhysicalType::BYTE_ARRAY => unreachable!(),
                };
                if need.is_none_or(|bytes_needed| bytes_needed > val_slice.len()) {
                    return Err("truncated PLAIN fixed-width value payload".to_owned());
                }
            }
            Ok(())
        }
        Encoding::PLAIN_DICTIONARY | Encoding::RLE_DICTIONARY => {
            if col_type == ParquetPhysicalType::BOOLEAN {
                return Err(
                    "dictionary data page encoding invalid on BOOLEAN parquet column".to_owned(),
                );
            }
            let Some(dv) = dict_vals else {
                return Err("dictionary data page missing preceding DICTIONARY_PAGE".to_owned());
            };
            let Some((&bw, rest)) = val_slice.split_first() else {
                return Err("empty dictionary data page value payload".to_owned());
            };
            if bw > 32 || (dv == 0 && bw != 0) {
                return Err(format!(
                    "invalid dictionary RLE bit_width {bw} (max=32, dict_values={dv})"
                ));
            }
            if non_null_count == 0 {
                if !rest.is_empty() {
                    if dv == 0 {
                        return Err(
                            "non-empty dictionary index stream for empty DICTIONARY_PAGE"
                                .to_owned(),
                        );
                    }
                    validate_value_rle_stream(rest, bw, 0, dv - 1)?;
                }
                Ok(())
            } else {
                if dv == 0 {
                    return Err(
                        "dictionary data page has non-null values for empty DICTIONARY_PAGE"
                            .to_owned(),
                    );
                }
                validate_value_rle_stream(rest, bw, non_null_count, dv - 1)
            }
        }
        Encoding::RLE => {
            if col_type != ParquetPhysicalType::BOOLEAN {
                return Err(format!(
                    "RLE data page value encoding invalid for non-BOOLEAN column {col_type:?}"
                ));
            }
            let Some(len_bytes) = val_slice.get(..4).and_then(|s| <[u8; 4]>::try_from(s).ok())
            else {
                return Err("truncated length prefix in BOOLEAN RLE data page".to_owned());
            };
            let rle_len = usize::try_from(i32::from_le_bytes(len_bytes))
                .map_err(|_| "negative length prefix in BOOLEAN RLE data page".to_owned())?;
            let end = 4_usize
                .checked_add(rle_len)
                .filter(|&e| e <= val_slice.len())
                .ok_or_else(|| "truncated payload in BOOLEAN RLE data page".to_owned())?;
            validate_value_rle_stream(&val_slice[4..end], 1, non_null_count, 1)
        }
        other => Err(format!(
            "unsupported Parquet data page encoding {other:?} rejected pre-decode"
        )),
    }
}

/// Decompresses and validates every projected page inside a single row group from an immutable
/// `ChunkReader` snapshot (`PushBuffers` or `Bytes`) before constructing Arrow decoders.
pub fn validate_projected_row_group_pages<R: ChunkReader + 'static>(
    reader: &Arc<R>,
    reader_meta: &ArrowReaderMetadata,
    rg_idx: usize,
    mask: &ProjectionMask,
) -> Result<(), String> {
    let rg = reader_meta.metadata().row_group(rg_idx);
    let rg_rows = usize::try_from(rg.num_rows())
        .map_err(|_| format!("row group {rg_idx} invalid num_rows {}", rg.num_rows()))?;
    if rg_rows == 0 {
        return Ok(());
    }
    for col_idx in 0..rg.num_columns() {
        if !mask.leaf_included(col_idx) {
            continue;
        }
        let col = rg.column(col_idx);
        let col_type = col.column_type();
        let fixed_len = col.column_descr().type_length();
        let max_def = col.column_descr().max_def_level();
        let (max_byte_array_item_len, require_utf8, is_null_col) = reader_meta
            .schema()
            .fields()
            .get(col_idx)
            .map_or((None, false, false), |f| {
                column_value_constraints(f.data_type())
            });
        let mut page_reader = SerializedPageReader::new(Arc::clone(reader), col, rg_rows, None)
            .map_err(|e| e.to_string())?;
        let mut dict_vals: Option<u32> = None;
        let mut seen_vals = 0_usize;

        while let Some(page) = page_reader.get_next_page().map_err(|e| e.to_string())? {
            match page {
                Page::DictionaryPage {
                    buf,
                    num_values,
                    encoding,
                    ..
                } => {
                    if is_null_col {
                        return Err(
                            "DictionaryPage not permitted on DataType::Null parquet column"
                                .to_owned(),
                        );
                    }
                    #[allow(deprecated)]
                    let valid_dict_enc =
                        matches!(encoding, Encoding::PLAIN | Encoding::PLAIN_DICTIONARY);
                    if col_type == ParquetPhysicalType::BOOLEAN
                        || dict_vals.is_some()
                        || !valid_dict_enc
                    {
                        return Err(format!(
                            "invalid DictionaryPage metadata on column {col_idx} (enc={encoding:?})"
                        ));
                    }
                    if num_values > i32::MAX as u32 {
                        return Err(format!(
                            "DictionaryPage num_values {num_values} exceeds dictionary key capacity {}",
                            i32::MAX
                        ));
                    }
                    if col_type == ParquetPhysicalType::BYTE_ARRAY {
                        let raw = buf.as_ref();
                        let mut pos = 0_usize;
                        for entry in 0..num_values as usize {
                            let Some(lb) = pos
                                .checked_add(4)
                                .and_then(|end| raw.get(pos..end))
                                .and_then(|s| <[u8; 4]>::try_from(s).ok())
                            else {
                                return Err(format!(
                                    "truncated BYTE_ARRAY dictionary entry {entry}/{num_values}"
                                ));
                            };
                            let item_len = u32::from_le_bytes(lb) as usize;
                            let end = pos
                                .checked_add(4)
                                .and_then(|p| p.checked_add(item_len))
                                .filter(|&e| e <= raw.len())
                                .ok_or_else(|| {
                                    format!(
                                        "out-of-bounds BYTE_ARRAY dictionary entry {entry}/{num_values}"
                                    )
                                })?;
                            if let Some(max_len) = max_byte_array_item_len
                                && item_len > max_len
                            {
                                return Err(format!(
                                    "BYTE_ARRAY dictionary entry {entry}/{num_values} length {item_len} exceeds Decimal byte bound {max_len}"
                                ));
                            }
                            if require_utf8 && std::str::from_utf8(&raw[pos + 4..end]).is_err() {
                                return Err(format!(
                                    "invalid UTF-8 in BYTE_ARRAY dictionary entry {entry}/{num_values}"
                                ));
                            }
                            pos = end;
                        }
                    } else {
                        let (min_plain_bytes, _) = dict_type_sizes(col_type, fixed_len)?;
                        if (num_values as usize)
                            .checked_mul(min_plain_bytes)
                            .is_none_or(|need| need > buf.len())
                        {
                            return Err(format!("truncated {col_type:?} dictionary page"));
                        }
                    }
                    dict_vals = Some(num_values);
                }
                Page::DataPage {
                    buf,
                    num_values,
                    encoding,
                    def_level_encoding,
                    rep_level_encoding,
                    ..
                } => {
                    if def_level_encoding != Encoding::RLE
                        || rep_level_encoding != Encoding::RLE
                        || num_values == 0
                    {
                        return Err(
                            "invalid decompressed V1 DataPage level encoding or count".to_owned()
                        );
                    }
                    let mut slice = buf.as_ref();
                    let non_null = if max_def > 0 {
                        let Some(lb) = slice.get(..4).and_then(|s| <[u8; 4]>::try_from(s).ok())
                        else {
                            return Err("truncated V1 RLE definition level length".to_owned());
                        };
                        let lvl_len = usize::try_from(i32::from_le_bytes(lb))
                            .map_err(|_| "negative V1 RLE definition level length".to_owned())?;
                        let end = 4_usize
                            .checked_add(lvl_len)
                            .filter(|&e| e <= slice.len())
                            .ok_or_else(|| "truncated V1 RLE definition level stream".to_owned())?;
                        let nn =
                            validate_def_level_rle_stream(&slice[4..end], num_values as usize)?;
                        slice = &slice[end..];
                        nn
                    } else {
                        num_values as usize
                    };
                    if is_null_col && non_null > 0 {
                        return Err("non-null value in DataType::Null parquet column".to_owned());
                    }
                    validate_data_page_value_slice(
                        encoding,
                        col_type,
                        fixed_len,
                        max_byte_array_item_len,
                        require_utf8,
                        dict_vals,
                        non_null,
                        false,
                        slice,
                    )?;
                    seen_vals = seen_vals
                        .checked_add(num_values as usize)
                        .ok_or_else(|| "decompressed page num_values overflow".to_owned())?;
                }
                Page::DataPageV2 {
                    buf,
                    num_values,
                    encoding,
                    num_nulls,
                    def_levels_byte_len,
                    rep_levels_byte_len,
                    ..
                } => {
                    if num_values == 0 || num_nulls > num_values || rep_levels_byte_len != 0 {
                        return Err(
                            "invalid decompressed DataPageV2 counts or repetition levels"
                                .to_owned(),
                        );
                    }
                    let def_len = def_levels_byte_len as usize;
                    if def_len > buf.len() {
                        return Err("truncated DataPageV2 definition levels".to_owned());
                    }
                    let non_null = if max_def > 0 {
                        let nn = validate_def_level_rle_stream(
                            &buf.as_ref()[..def_len],
                            num_values as usize,
                        )?;
                        if nn != (num_values - num_nulls) as usize {
                            return Err(format!(
                                "DataPageV2 non-null count {nn} mismatches header num_values {num_values} - num_nulls {num_nulls}"
                            ));
                        }
                        nn
                    } else {
                        if def_len != 0 || num_nulls != 0 {
                            return Err(
                                "required parquet column DataPageV2 has non-zero definition levels or nulls"
                                    .to_owned(),
                            );
                        }
                        num_values as usize
                    };
                    if is_null_col && non_null > 0 {
                        return Err("non-null value in DataType::Null parquet column".to_owned());
                    }
                    validate_data_page_value_slice(
                        encoding,
                        col_type,
                        fixed_len,
                        max_byte_array_item_len,
                        require_utf8,
                        dict_vals,
                        non_null,
                        true,
                        &buf.as_ref()[def_len..],
                    )?;
                    seen_vals = seen_vals
                        .checked_add(num_values as usize)
                        .ok_or_else(|| "decompressed DataPageV2 num_values overflow".to_owned())?;
                }
            }
        }
        if seen_vals != rg_rows {
            return Err(format!(
                "decompressed column {col_idx} num_values {seen_vals} mismatches row group rows {rg_rows}"
            ));
        }
    }
    Ok(())
}

fn finish_verified_maintenance_metadata(
    pq_meta: ParquetMetaData,
    decoded_arrow_ipc_bytes: usize,
    est_arrow_ipc_bytes: usize,
    file_len: u64,
    footer_len: usize,
    expected_schema: Option<&SchemaRef>,
) -> Result<ArrowReaderMetadata, String> {
    validate_non_nested_parquet_file_metadata(&pq_meta)?;
    let pre_try_new = pq_meta
        .memory_size()
        .saturating_add(estimate_kv_metadata_clone_bytes(&pq_meta))
        .saturating_add(decoded_arrow_ipc_bytes)
        .saturating_add(est_arrow_ipc_bytes.saturating_mul(2));
    if pre_try_new > MAX_MAINTENANCE_FOOTER_DECODE_BYTES {
        return Err(format!(
            "parquet ARROW:schema expansion exceeds maintenance limit {MAX_MAINTENANCE_FOOTER_DECODE_BYTES}"
        ));
    }
    let reader_meta = ArrowReaderMetadata::try_new(Arc::new(pq_meta), Default::default())
        .map_err(|e| e.to_string())?;
    if reader_meta
        .metadata()
        .memory_size()
        .saturating_add(estimate_arrow_schema_bytes(reader_meta.schema()))
        > MAX_MAINTENANCE_FOOTER_DECODE_BYTES
    {
        return Err(format!(
            "parquet retained metadata exceeds maintenance limit {MAX_MAINTENANCE_FOOTER_DECODE_BYTES}"
        ));
    }
    match expected_schema {
        Some(expected) => validate_flat_compatible_parquet_schema(&reader_meta, expected)?,
        None => validate_non_nested_parquet_schema(&reader_meta)?,
    }
    validate_parquet_metadata_invariants(&reader_meta, file_len, footer_len)?;
    Ok(reader_meta)
}

/// Loads and fully validates Parquet footer Thrift structure, schema contract, and metadata invariants.
pub fn load_verified_parquet_metadata_from_file(
    file: &File,
    expected_schema: Option<&SchemaRef>,
) -> Result<(u64, usize, ArrowReaderMetadata), String> {
    let (file_len, footer_len) = inspect_parquet_footer_bounds(file)?;
    let footer_bytes = read_parquet_footer_slice(file, file_len, footer_len)?;
    let scan = scan_parquet_thrift_footer_slice(&footer_bytes)?;
    let decoded_arrow_ipc_bytes = scan.decoded_arrow_ipc_bytes();
    if footer_len
        .saturating_add(decoded_arrow_ipc_bytes)
        .saturating_add(scan.est_pq_meta_bytes())
        > MAX_MAINTENANCE_FOOTER_DECODE_BYTES
    {
        return Err(format!(
            "parquet footer estimated decode size exceeds maintenance limit {MAX_MAINTENANCE_FOOTER_DECODE_BYTES}"
        ));
    }
    let est_arrow_ipc_bytes = scan.validate_arrow_schema_ipc()?;
    if footer_len
        .saturating_add(scan.est_pq_meta_bytes())
        .saturating_add(est_arrow_ipc_bytes.saturating_mul(2))
        > MAX_MAINTENANCE_FOOTER_DECODE_BYTES
    {
        return Err(format!(
            "parquet footer estimated decode size exceeds maintenance limit {MAX_MAINTENANCE_FOOTER_DECODE_BYTES}"
        ));
    }
    let pq_meta =
        ParquetMetaDataReader::decode_metadata(&footer_bytes).map_err(|e| e.to_string())?;
    drop(footer_bytes);
    let reader_meta = finish_verified_maintenance_metadata(
        pq_meta,
        decoded_arrow_ipc_bytes,
        est_arrow_ipc_bytes,
        file_len,
        footer_len,
        expected_schema,
    )?;
    Ok((file_len, footer_len, reader_meta))
}

fn retained_reader_metadata_bytes(reader_meta: &ArrowReaderMetadata) -> usize {
    reader_meta
        .metadata()
        .memory_size()
        .saturating_add(estimate_arrow_schema_bytes(reader_meta.schema()))
        .max(PINNED_FOOTER_MIN_BYTES)
}

fn arrow_fixed_slot_bytes(dt: &DataType) -> usize {
    match dt {
        DataType::Decimal256(_, _) => 32,
        DataType::Decimal128(_, _)
        | DataType::Utf8View
        | DataType::BinaryView
        | DataType::Interval(IntervalUnit::MonthDayNano) => 16,
        DataType::LargeUtf8
        | DataType::LargeBinary
        | DataType::Decimal64(_, _)
        | DataType::Int64
        | DataType::UInt64
        | DataType::Float64
        | DataType::Date64
        | DataType::Time64(_)
        | DataType::Timestamp(_, _)
        | DataType::Duration(_)
        | DataType::Interval(IntervalUnit::DayTime) => 8,
        DataType::Utf8
        | DataType::Binary
        | DataType::Decimal32(_, _)
        | DataType::Int32
        | DataType::UInt32
        | DataType::Float32
        | DataType::Date32
        | DataType::Time32(_)
        | DataType::Interval(IntervalUnit::YearMonth) => 4,
        DataType::Int16 | DataType::UInt16 | DataType::Float16 => 2,
        DataType::FixedSizeBinary(w) => usize::try_from(*w).unwrap_or(1),
        _ => 1,
    }
}

/// Upper-bounds per-row fixed-width, simultaneous conversion-intermediate, and level/null-bitmap
/// bytes resident inside `parquet 59.2.0` column readers during `read_records` and `consume_batch`.
#[must_use]
pub fn column_peak_decode_fixed_row_bytes(
    phys: ParquetPhysicalType,
    type_len: i32,
    max_def_level: i16,
    arrow_type: Option<&DataType>,
) -> usize {
    let level_null = if max_def_level > 0 { 6_usize } else { 1_usize };
    if matches!(arrow_type, Some(DataType::Null)) {
        return 8_usize.saturating_add(level_null);
    }
    let slot = arrow_type.map_or(1, arrow_fixed_slot_bytes);
    match phys {
        ParquetPhysicalType::BOOLEAN => 2_usize.saturating_add(level_null),
        ParquetPhysicalType::FLOAT => 4_usize.saturating_add(level_null),
        ParquetPhysicalType::DOUBLE => 8_usize.saturating_add(level_null),
        ParquetPhysicalType::INT96 => 20_usize.saturating_add(level_null),
        ParquetPhysicalType::INT32 => {
            if matches!(
                arrow_type,
                Some(
                    DataType::Int32
                        | DataType::UInt32
                        | DataType::Date32
                        | DataType::Time32(_)
                        | DataType::Decimal32(_, _)
                )
            ) {
                4_usize.saturating_add(level_null)
            } else {
                4_usize.saturating_add(slot).saturating_add(level_null)
            }
        }
        ParquetPhysicalType::INT64 => {
            if matches!(
                arrow_type,
                Some(
                    DataType::Int64
                        | DataType::UInt64
                        | DataType::Date64
                        | DataType::Time64(_)
                        | DataType::Duration(_)
                        | DataType::Timestamp(_, _)
                        | DataType::Decimal64(_, _)
                )
            ) {
                8_usize.saturating_add(level_null)
            } else {
                8_usize.saturating_add(slot).saturating_add(level_null)
            }
        }
        ParquetPhysicalType::FIXED_LEN_BYTE_ARRAY => {
            let w = usize::try_from(type_len.max(1)).unwrap_or(usize::MAX);
            if matches!(arrow_type, Some(DataType::FixedSizeBinary(_))) {
                w.saturating_add(level_null)
            } else {
                w.saturating_add(slot).saturating_add(level_null)
            }
        }
        ParquetPhysicalType::BYTE_ARRAY => {
            let base = match arrow_type {
                Some(DataType::Decimal128(_, _)) => 20_usize,
                Some(DataType::Decimal256(_, _)) => 36_usize,
                Some(DataType::Utf8View | DataType::BinaryView) => 16_usize,
                Some(DataType::LargeUtf8 | DataType::LargeBinary) => 8_usize,
                _ => 4_usize,
            };
            base.saturating_add(level_null)
        }
    }
}

fn accumulate_maintenance_col_decode_stats(
    col: &ColumnChunkMetaData,
    arrow_type: Option<&DataType>,
    scan: &ColumnPageScanStats,
    fixed_row_bytes: &mut usize,
    varlen_eff_bytes: &mut usize,
    unenc_varlen: &mut usize,
) {
    let footer_uncomp = usize::try_from(col.uncompressed_size()).unwrap_or(0);
    let eff = footer_uncomp.max(scan.effective_payload_bytes());
    let peak_fixed = column_peak_decode_fixed_row_bytes(
        col.column_type(),
        col.column_descr().type_length(),
        col.column_descr().max_def_level(),
        arrow_type,
    );
    *fixed_row_bytes = fixed_row_bytes.saturating_add(peak_fixed);
    if col.column_type() == ParquetPhysicalType::BYTE_ARRAY {
        *varlen_eff_bytes = varlen_eff_bytes.saturating_add(eff.max(64));
        if let Some(u) = col
            .unencoded_byte_array_data_bytes()
            .and_then(|v| usize::try_from(v).ok())
        {
            *unenc_varlen = unenc_varlen.saturating_add(u);
        }
    }
}

fn compute_maintenance_rg_batch_rows(
    rg_idx: usize,
    rg_rows: usize,
    rg_resident: usize,
    fixed_row_bytes: usize,
    varlen_eff_bytes: usize,
    unenc_varlen: usize,
    max_batch_rows: usize,
) -> Result<usize, String> {
    if rg_resident > MAX_MAINTENANCE_ROW_GROUP_DECODE_BYTES {
        return Err(format!(
            "parquet row group {rg_idx} resident budget {rg_resident} exceeds maintenance cap {MAX_MAINTENANCE_ROW_GROUP_DECODE_BYTES}"
        ));
    }
    if rg_rows == 0 {
        return Ok(max_batch_rows.max(1));
    }
    let headroom = MAX_MAINTENANCE_ROW_GROUP_DECODE_BYTES - rg_resident;
    let avg_varlen_row = varlen_eff_bytes
        .max(unenc_varlen)
        .checked_div(rg_rows)
        .unwrap_or(0);
    let worst_row_bytes = fixed_row_bytes.saturating_add(varlen_eff_bytes.max(avg_varlen_row));
    if worst_row_bytes > headroom {
        return Err(format!(
            "parquet row group {rg_idx} estimated per-row decode size {worst_row_bytes} exceeds maintenance decode headroom {headroom}"
        ));
    }
    let row_cap = headroom
        .checked_div(worst_row_bytes)
        .unwrap_or_else(|| max_batch_rows.max(1));
    Ok(max_batch_rows.max(1).min(row_cap))
}

fn validate_chunk_pages_and_budget<R: Read + Seek>(
    reader: &mut R,
    reader_meta: &ArrowReaderMetadata,
    file_len: u64,
    footer_len: usize,
    retained_input_bytes: usize,
    max_batch_rows: usize,
) -> Result<usize, String> {
    let data_section_end = file_len
        .checked_sub(8_u64.saturating_add(footer_len as u64))
        .ok_or_else(|| "invalid parquet footer offset bounds".to_owned())?;
    let base_resident =
        retained_reader_metadata_bytes(reader_meta).saturating_add(retained_input_bytes);
    if base_resident > MAX_MAINTENANCE_ROW_GROUP_DECODE_BYTES {
        return Err(format!(
            "parquet retained buffer budget {base_resident} exceeds maintenance cap {MAX_MAINTENANCE_ROW_GROUP_DECODE_BYTES}"
        ));
    }
    let mut safe_batch_rows = max_batch_rows.max(1);
    for (rg_idx, rg) in reader_meta.metadata().row_groups().iter().enumerate() {
        let rg_rows = usize::try_from(rg.num_rows())
            .map_err(|_| format!("row group {rg_idx} invalid num_rows {}", rg.num_rows()))?;
        let mut rg_resident = base_resident.saturating_add(rg.num_columns().saturating_mul(256));
        let mut fixed_row_bytes = 0_usize;
        let mut varlen_eff_bytes = 0_usize;
        let mut unenc_varlen = 0_usize;
        for col_idx in 0..rg.num_columns() {
            let col = rg.column(col_idx);
            let (col_start, col_len) = checked_column_byte_range(col, col_idx, data_section_end)?;
            let scan = scan_column_chunk_page_headers(
                reader,
                col_start,
                col_len,
                col.column_type(),
                col.column_descr().type_length(),
                col.column_descr().max_def_level(),
                rg_rows,
            )?;
            rg_resident = rg_resident.saturating_add(scan.resident_bytes());
            let arrow_type = reader_meta
                .schema()
                .fields()
                .get(col_idx)
                .map(|f| f.data_type());
            accumulate_maintenance_col_decode_stats(
                col,
                arrow_type,
                &scan,
                &mut fixed_row_bytes,
                &mut varlen_eff_bytes,
                &mut unenc_varlen,
            );
        }
        let row_cap = compute_maintenance_rg_batch_rows(
            rg_idx,
            rg_rows,
            rg_resident,
            fixed_row_bytes,
            varlen_eff_bytes,
            unenc_varlen,
            max_batch_rows,
        )?;
        safe_batch_rows = safe_batch_rows.min(row_cap);
    }
    Ok(safe_batch_rows.max(1))
}

/// Maintenance `RecordBatchReader` that snapshots each Parquet row group once into immutable
/// `PushBuffers` and validates decompressed dictionary/RLE page payloads before decoding batches.
pub struct VerifiedParquetBatchReader {
    file: File,
    file_len: u64,
    footer_len: usize,
    reader_meta: ArrowReaderMetadata,
    max_batch_rows: usize,
    next_rg_idx: usize,
    current_rg_reader: Option<ParquetRecordBatchReader>,
}

impl fmt::Debug for VerifiedParquetBatchReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerifiedParquetBatchReader")
            .field("file_len", &self.file_len)
            .field("next_rg_idx", &self.next_rg_idx)
            .field(
                "num_row_groups",
                &self.reader_meta.metadata().num_row_groups(),
            )
            .finish()
    }
}

impl VerifiedParquetBatchReader {
    /// Returns the validated Arrow schema for this reader.
    #[must_use]
    pub fn schema(&self) -> SchemaRef {
        Arc::clone(self.reader_meta.schema())
    }

    fn advance_row_group(&mut self) -> Result<bool, String> {
        let data_section_end = self
            .file_len
            .checked_sub(8_u64.saturating_add(self.footer_len as u64))
            .ok_or_else(|| "invalid parquet footer offset bounds".to_owned())?;
        let retained_meta = retained_reader_metadata_bytes(&self.reader_meta);
        let num_row_groups = self.reader_meta.metadata().num_row_groups();
        while self.next_rg_idx < num_row_groups {
            let rg_idx = self.next_rg_idx;
            self.next_rg_idx += 1;
            let rg = self.reader_meta.metadata().row_group(rg_idx);
            let rg_rows = usize::try_from(rg.num_rows())
                .map_err(|_| format!("row group {rg_idx} invalid num_rows {}", rg.num_rows()))?;
            let cur_len = self.file.metadata().map_err(|e| e.to_string())?.len();
            if cur_len != self.file_len {
                return Err(format!(
                    "parquet file length changed concurrently ({cur_len} != {}) reading row group {rg_idx}",
                    self.file_len
                ));
            }
            let mut compressed_total = 0_usize;
            for col_idx in 0..rg.num_columns() {
                let col = rg.column(col_idx);
                let (_start, col_len) = checked_column_byte_range(col, col_idx, data_section_end)?;
                let len_usize = usize::try_from(col_len)
                    .map_err(|_| format!("column {col_idx} compressed length overflows usize"))?;
                compressed_total = compressed_total.saturating_add(len_usize);
            }
            if retained_meta.saturating_add(compressed_total)
                > MAX_MAINTENANCE_ROW_GROUP_DECODE_BYTES
            {
                return Err(format!(
                    "parquet row group {rg_idx} compressed bytes exceed maintenance cap {MAX_MAINTENANCE_ROW_GROUP_DECODE_BYTES}"
                ));
            }
            if rg_rows == 0 {
                continue;
            }

            let mut rg_buffers = PushBuffers::new(self.file_len);
            let mut rg_resident = retained_meta
                .saturating_add(compressed_total)
                .saturating_add(rg.num_columns().saturating_mul(256));
            let mut fixed_row_bytes = 0_usize;
            let mut varlen_eff_bytes = 0_usize;
            let mut unenc_varlen = 0_usize;
            for col_idx in 0..rg.num_columns() {
                let col = rg.column(col_idx);
                let (col_start, col_len) =
                    checked_column_byte_range(col, col_idx, data_section_end)?;
                let col_len_usize = usize::try_from(col_len)
                    .map_err(|_| format!("column {col_idx} compressed length overflows usize"))?;
                let col_bytes =
                    <File as ChunkReader>::get_bytes(&self.file, col_start, col_len_usize)
                        .map_err(|e| e.to_string())?;
                if col_bytes.len() != col_len_usize {
                    return Err(format!(
                        "short read buffering parquet column {col_idx} in row group {rg_idx}"
                    ));
                }
                let scan = scan_column_chunk_page_headers(
                    &mut Cursor::new(&col_bytes),
                    0,
                    col_len,
                    col.column_type(),
                    col.column_descr().type_length(),
                    col.column_descr().max_def_level(),
                    rg_rows,
                )?;
                rg_resident = rg_resident.saturating_add(scan.resident_bytes());
                let arrow_type = self
                    .reader_meta
                    .schema()
                    .fields()
                    .get(col_idx)
                    .map(|f| f.data_type());
                accumulate_maintenance_col_decode_stats(
                    col,
                    arrow_type,
                    &scan,
                    &mut fixed_row_bytes,
                    &mut varlen_eff_bytes,
                    &mut unenc_varlen,
                );
                rg_buffers.push_range(col_start..(col_start + col_len), col_bytes);
            }
            let batch_rows = compute_maintenance_rg_batch_rows(
                rg_idx,
                rg_rows,
                rg_resident,
                fixed_row_bytes,
                varlen_eff_bytes,
                unenc_varlen,
                self.max_batch_rows,
            )?;

            let rg_arc = Arc::new(rg_buffers);
            validate_projected_row_group_pages(
                &rg_arc,
                &self.reader_meta,
                rg_idx,
                &ProjectionMask::all(),
            )?;
            let immutable_rg = Arc::try_unwrap(rg_arc).unwrap_or_else(|a| (*a).clone());
            let reader = ParquetRecordBatchReaderBuilder::new_with_metadata(
                immutable_rg,
                self.reader_meta.clone(),
            )
            .with_row_groups(vec![rg_idx])
            .with_batch_size(batch_rows)
            .build()
            .map_err(|e| e.to_string())?;
            self.current_rg_reader = Some(reader);
            return Ok(true);
        }
        Ok(false)
    }
}

impl Iterator for VerifiedParquetBatchReader {
    type Item = Result<RecordBatch, ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(reader) = self.current_rg_reader.as_mut() {
                match reader.next() {
                    Some(item) => return Some(item),
                    None => self.current_rg_reader = None,
                }
            }
            match self.advance_row_group() {
                Ok(true) => continue,
                Ok(false) => return None,
                Err(err) => {
                    self.next_rg_idx = self.reader_meta.metadata().num_row_groups();
                    return Some(Err(ArrowError::ExternalError(Box::new(
                        std::io::Error::other(err),
                    ))));
                }
            }
        }
    }
}

impl RecordBatchReader for VerifiedParquetBatchReader {
    fn schema(&self) -> SchemaRef {
        Arc::clone(self.reader_meta.schema())
    }
}

/// Validates Parquet footer, schema, invariants, and row-group page/RLE/dictionary headers into an
/// immutable per-row-group `VerifiedParquetBatchReader` (eliminating post-check TOCTOU file reads).
pub fn open_verified_parquet_batch_reader_from_file(
    file: File,
    expected_schema: Option<&SchemaRef>,
    max_batch_rows: usize,
) -> Result<VerifiedParquetBatchReader, String> {
    let (file_len, footer_len, reader_meta) =
        load_verified_parquet_metadata_from_file(&file, expected_schema)?;
    let mut reader = VerifiedParquetBatchReader {
        file,
        file_len,
        footer_len,
        reader_meta,
        max_batch_rows,
        next_rg_idx: 0,
        current_rg_reader: None,
    };
    let _ = reader.advance_row_group()?;
    Ok(reader)
}

/// Validates Parquet footer, schema, invariants, and all page/dictionary/RLE headers on an immutable
/// in-memory `Bytes`-compatible `ChunkReader` buffer before constructing a `ParquetRecordBatchReader`.
pub fn open_verified_parquet_batch_reader_from_bytes<B>(
    bytes: B,
    expected_schema: Option<&SchemaRef>,
    max_batch_rows: usize,
) -> Result<ParquetRecordBatchReader, String>
where
    B: ChunkReader + AsRef<[u8]> + 'static,
{
    let slice = bytes.as_ref();
    let file_len = u64::try_from(slice.len()).map_err(|e| e.to_string())?;
    if file_len < 12 {
        return Err(format!(
            "corrupt parquet buffer too small ({file_len} bytes)"
        ));
    }
    if &slice[..4] != b"PAR1" || &slice[slice.len() - 4..] != b"PAR1" {
        return Err("corrupt parquet buffer magic".to_owned());
    }
    let tail_off = slice.len() - 8;
    let footer_len = u32::from_le_bytes([
        slice[tail_off],
        slice[tail_off + 1],
        slice[tail_off + 2],
        slice[tail_off + 3],
    ]) as usize;
    if footer_len == 0
        || u64::try_from(footer_len)
            .ok()
            .and_then(|n| n.checked_add(12))
            .is_none_or(|need| need > file_len)
        || footer_len > MAX_PARQUET_FOOTER_SERIALIZED_BYTES
    {
        return Err(format!(
            "parquet buffer footer length {footer_len} exceeds safe bounds for {file_len}-byte buffer"
        ));
    }
    let footer_start = slice.len() - 8 - footer_len;
    let footer_slice = &slice[footer_start..tail_off];
    let scan = scan_parquet_thrift_footer_slice(footer_slice)?;
    let decoded_arrow_ipc_bytes = scan.decoded_arrow_ipc_bytes();
    if footer_len
        .saturating_add(decoded_arrow_ipc_bytes)
        .saturating_add(scan.est_pq_meta_bytes())
        > MAX_MAINTENANCE_FOOTER_DECODE_BYTES
    {
        return Err(format!(
            "parquet buffer footer estimated decode size exceeds limit {MAX_MAINTENANCE_FOOTER_DECODE_BYTES}"
        ));
    }
    let est_arrow_ipc_bytes = scan.validate_arrow_schema_ipc()?;
    if footer_len
        .saturating_add(scan.est_pq_meta_bytes())
        .saturating_add(est_arrow_ipc_bytes.saturating_mul(2))
        > MAX_MAINTENANCE_FOOTER_DECODE_BYTES
    {
        return Err(format!(
            "parquet buffer footer estimated decode size exceeds limit {MAX_MAINTENANCE_FOOTER_DECODE_BYTES}"
        ));
    }
    let pq_meta =
        ParquetMetaDataReader::decode_metadata(footer_slice).map_err(|e| e.to_string())?;
    let reader_meta = finish_verified_maintenance_metadata(
        pq_meta,
        decoded_arrow_ipc_bytes,
        est_arrow_ipc_bytes,
        file_len,
        footer_len,
        expected_schema,
    )?;
    let mut cursor = Cursor::new(slice);
    let batch_rows = validate_chunk_pages_and_budget(
        &mut cursor,
        &reader_meta,
        file_len,
        footer_len,
        slice.len(),
        max_batch_rows,
    )?;
    let data_section_end = file_len
        .checked_sub(8_u64.saturating_add(footer_len as u64))
        .ok_or_else(|| "invalid parquet buffer footer offset bounds".to_owned())?;
    for (rg_idx, rg) in reader_meta.metadata().row_groups().iter().enumerate() {
        if rg.num_rows() == 0 {
            continue;
        }
        let mut rg_bufs = PushBuffers::new(file_len);
        for col_idx in 0..rg.num_columns() {
            let col = rg.column(col_idx);
            let (col_start, col_len) = checked_column_byte_range(col, col_idx, data_section_end)?;
            let col_len_usize = usize::try_from(col_len)
                .map_err(|_| format!("column {col_idx} length overflows usize"))?;
            let col_bytes = bytes
                .get_bytes(col_start, col_len_usize)
                .map_err(|e| e.to_string())?;
            rg_bufs.push_range(col_start..(col_start + col_len), col_bytes);
        }
        let rg_arc = Arc::new(rg_bufs);
        validate_projected_row_group_pages(&rg_arc, &reader_meta, rg_idx, &ProjectionMask::all())?;
    }
    ParquetRecordBatchReaderBuilder::new_with_metadata(bytes, reader_meta)
        .with_batch_size(batch_rows)
        .build()
        .map_err(|e| e.to_string())
}
