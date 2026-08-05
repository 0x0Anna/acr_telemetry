//! Version-aware rkyv readers driven by binary header + companion `.json` (see `format_meta`).

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use rkyv::de::deserializers::SharedDeserializeMap;
use rkyv::util::AlignedVec;
use rkyv::Deserialize;

use crate::format_meta::{
    self, FormatMetadataDoc, GRAPHICS_RECORD_SCHEMA_V1, GRAPHICS_RECORD_SCHEMA_V2,
    PHYSICS_RECORD_SCHEMA_V1, PHYSICS_RECORD_SCHEMA_V2, PHYSICS_RECORD_SCHEMA_V3,
};
use crate::record::disk_v2::PhysicsRecordDiskV2;
use crate::record::v1::{GraphicsRecordV1, PhysicsRecordV1};
use crate::record::{GraphicsRecord, PhysicsRecord, StaticsRecord};

pub const PHYSICS_MAGIC: &[u8; 4] = b"ACCR";
pub const GRAPHICS_MAGIC: &[u8; 4] = b"ACCG";

struct FileHeader {
    binary_version: u16,
    sample_rate_hz: u32,
}

fn companion_json_path(rkyv_path: &Path) -> PathBuf {
    let name = rkyv_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let base = name
        .strip_suffix(".graphics.rkyv")
        .or_else(|| name.strip_suffix(".rkyv"))
        .unwrap_or(name);
    rkyv_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{base}.json"))
}

fn read_file_header(reader: &mut BufReader<File>, expected_magic: &[u8; 4]) -> std::io::Result<FileHeader> {
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic)?;
    if magic != *expected_magic {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid magic: expected {expected_magic:?}, got {magic:?}"),
        ));
    }
    let mut version = [0u8; 2];
    reader.read_exact(&mut version)?;
    let binary_version = u16::from_le_bytes(version);
    let mut sample_rate = [0u8; 4];
    reader.read_exact(&mut sample_rate)?;
    let sample_rate_hz = u32::from_le_bytes(sample_rate);
    reader.read_exact(&mut [0u8; 6])?;
    Ok(FileHeader {
        binary_version,
        sample_rate_hz,
    })
}

fn load_sidecar(rkyv_path: &Path) -> std::io::Result<FormatMetadataDoc> {
    let json_path = companion_json_path(rkyv_path);
    if json_path.is_file() {
        format_meta::read_format_metadata(&json_path)
    } else {
        Ok(FormatMetadataDoc::inferred_from_binary_only(
            rkyv_path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
        ))
    }
}

fn resolve_physics_schema(header: &FileHeader, meta: &FormatMetadataDoc) -> std::io::Result<u32> {
  let schema = meta
        .physics_record_schema
        .unwrap_or_else(|| meta.infer_physics_schema(header.binary_version));
    meta.validate_physics(header.binary_version, schema)?;
    Ok(schema)
}

fn resolve_graphics_schema(header: &FileHeader, meta: &FormatMetadataDoc) -> std::io::Result<u32> {
    let schema = meta
        .graphics_record_schema
        .unwrap_or_else(|| meta.infer_graphics_schema(header.binary_version));
    meta.validate_graphics(header.binary_version, schema)?;
    Ok(schema)
}

fn deserialize_chunk<T>(chunk: &[u8]) -> Result<Vec<T>, std::io::Error>
where
    T: rkyv::Archive,
    Vec<T>: rkyv::Archive,
    <Vec<T> as rkyv::Archive>::Archived: Deserialize<Vec<T>, SharedDeserializeMap>,
{
    let mut aligned = AlignedVec::with_capacity(chunk.len());
    aligned.extend_from_slice(chunk);
    let archived = unsafe { rkyv::archived_root::<Vec<T>>(aligned.as_ref()) };
    let mut map = SharedDeserializeMap::new();
    archived
        .deserialize(&mut map)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

/// Load statics from companion JSON (schema v1 or v2).
pub fn load_statics(rkyv_path: &Path) -> Option<StaticsRecord> {
    let meta = load_sidecar(rkyv_path).ok()?;
    let statics_val = meta.statics.clone()?;
    let bv = meta.binary_file_version.unwrap_or(1);
    let schema = meta
        .statics_record_schema
        .unwrap_or_else(|| meta.infer_statics_schema(bv));
    match schema {
        format_meta::STATICS_RECORD_SCHEMA_V1 => {
            serde_json::from_value::<crate::record::v1::StaticsRecordV1>(statics_val)
                .ok()
                .map(Into::into)
        }
        format_meta::STATICS_RECORD_SCHEMA_V2 => serde_json::from_value(statics_val).ok(),
        _ => None,
    }
}

/// Read physics rkyv → current `PhysicsRecord` (v1 upgraded in memory).
pub fn read_physics(
    path: impl AsRef<Path>,
) -> std::io::Result<(u32, u16, Vec<PhysicsRecord>)> {
    let path = path.as_ref();
    let meta = load_sidecar(path)?;
    let f = File::open(path)?;
    let mut reader = BufReader::new(f);
    let header = read_file_header(&mut reader, PHYSICS_MAGIC)?;
    let schema = resolve_physics_schema(&header, &meta)?;
    let mut records = Vec::new();
    let mut len_buf = [0u8; 4];
    while reader.read_exact(&mut len_buf).is_ok() {
        let chunk_len = u32::from_le_bytes(len_buf) as usize;
        if chunk_len == 0 {
            break;
        }
        let mut chunk = vec![0u8; chunk_len];
        reader.read_exact(&mut chunk)?;
        match schema {
            PHYSICS_RECORD_SCHEMA_V1 => {
                let chunk_v1: Vec<PhysicsRecordV1> = deserialize_chunk(&chunk)?;
                records.extend(chunk_v1.into_iter().map(PhysicsRecord::from));
            }
            PHYSICS_RECORD_SCHEMA_V2 => {
                let chunk_v2: Vec<PhysicsRecordDiskV2> = deserialize_chunk(&chunk)?;
                records.extend(chunk_v2.into_iter().map(PhysicsRecord::from));
            }
            PHYSICS_RECORD_SCHEMA_V3 => {
                records.extend(deserialize_chunk::<PhysicsRecord>(&chunk)?);
            }
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unsupported physics_record_schema {other}"),
                ));
            }
        }
    }
    crate::record::ensure_capture_times(&mut records, header.sample_rate_hz);
    Ok((header.sample_rate_hz, header.binary_version, records))
}

/// Read graphics rkyv → current `GraphicsRecord`.
pub fn read_graphics(
    path: impl AsRef<Path>,
) -> std::io::Result<(u32, u16, Vec<GraphicsRecord>)> {
    let path = path.as_ref();
    let physics_json = companion_json_path(path);
    let meta = if physics_json.is_file() {
        format_meta::read_format_metadata(&physics_json)?
    } else {
        load_sidecar(path)?
    };
    let f = File::open(path)?;
    let mut reader = BufReader::new(f);
    let header = read_file_header(&mut reader, GRAPHICS_MAGIC)?;
    let schema = resolve_graphics_schema(&header, &meta)?;
    let mut records = Vec::new();
    let mut len_buf = [0u8; 4];
    while reader.read_exact(&mut len_buf).is_ok() {
        let chunk_len = u32::from_le_bytes(len_buf) as usize;
        if chunk_len == 0 {
            break;
        }
        let mut chunk = vec![0u8; chunk_len];
        reader.read_exact(&mut chunk)?;
        match schema {
            GRAPHICS_RECORD_SCHEMA_V1 => {
                let chunk_v1: Vec<GraphicsRecordV1> = deserialize_chunk(&chunk)?;
                records.extend(chunk_v1.into_iter().map(GraphicsRecord::from));
            }
            GRAPHICS_RECORD_SCHEMA_V2 => {
                records.extend(deserialize_chunk::<GraphicsRecord>(&chunk)?);
            }
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unsupported graphics_record_schema {other}"),
                ));
            }
        }
    }
    Ok((header.sample_rate_hz, header.binary_version, records))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format_meta::RKYV_BINARY_VERSION_V1;
    use std::path::PathBuf;

    fn calib_physics() -> PathBuf {
        PathBuf::from("telemetry_raw/acc_physics_1778921308.rkyv")
    }

    fn calib_graphics() -> PathBuf {
        PathBuf::from("telemetry_raw/acc_physics_1778921308.graphics.rkyv")
    }

    #[test]
    fn read_v1_calibration_physics() {
        let path = calib_physics();
        if !path.is_file() {
            eprintln!("skip: {} not found", path.display());
            return;
        }
        let (hz, ver, recs) = read_physics(&path).expect("physics");
        assert_eq!(ver, RKYV_BINARY_VERSION_V1);
        assert!(hz >= 300);
        assert!(recs.len() > 10_000, "expected long run, got {}", recs.len());
    }

    #[test]
    fn read_v1_calibration_graphics() {
        let path = calib_graphics();
        if !path.is_file() {
            eprintln!("skip: {} not found", path.display());
            return;
        }
        let (hz, ver, recs) = read_graphics(&path).expect("graphics");
        assert_eq!(ver, RKYV_BINARY_VERSION_V1);
        assert!(hz >= 50);
        assert!(recs.len() > 1_000, "expected graphics samples, got {}", recs.len());
        assert!(recs[100].car_coordinates_x.abs() > 1.0 || recs[100].car_coordinates_z.abs() > 1.0);
    }

    #[test]
    fn sidecar_resolves_v1_schema() {
        let path = calib_physics();
        if !path.is_file() {
            return;
        }
        let meta = load_sidecar(&path).unwrap();
        assert_eq!(
            meta.infer_physics_schema(RKYV_BINARY_VERSION_V1),
            PHYSICS_RECORD_SCHEMA_V1
        );
    }
}
