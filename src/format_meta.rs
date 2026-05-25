//! Format metadata JSON for rkyv recordings.
//!
//! Companion `.json` is the **contract** for readers: `binary_file_version` and `*_record_schema`
//! must match the on-disk rkyv layout. All tools read rkyv via `export::rkyv_format` (not raw structs).

use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Binary header `version` field written by `recorder` (little-endian u16 after magic).
pub const RKYV_BINARY_VERSION_V1: u16 = 1;
pub const RKYV_BINARY_VERSION_V2: u16 = 2;
/// Current JSON sidecar document version.
pub const FORMAT_JSON_VERSION: u16 = 2;

pub const PHYSICS_RECORD_SCHEMA_V1: u32 = 1;
pub const PHYSICS_RECORD_SCHEMA_V2: u32 = 2;
pub const PHYSICS_RECORD_SCHEMA_V3: u32 = 3;
pub const GRAPHICS_RECORD_SCHEMA_V1: u32 = 1;
pub const GRAPHICS_RECORD_SCHEMA_V2: u32 = 2;
pub const STATICS_RECORD_SCHEMA_V1: u32 = 1;
pub const STATICS_RECORD_SCHEMA_V2: u32 = 2;

/// Parsed companion metadata (read by all rkyv consumers).
#[derive(Debug, Clone, Deserialize)]
pub struct FormatMetadataDoc {
    pub format_version: u16,
    pub binary_file: String,
    #[serde(default)]
    pub binary_file_version: Option<u16>,
    #[serde(default)]
    pub physics_record_schema: Option<u32>,
    #[serde(default)]
    pub graphics_record_schema: Option<u32>,
    #[serde(default)]
    pub statics_record_schema: Option<u32>,
    #[serde(default)]
    pub sample_rate_hz: Option<u32>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub statics: Option<serde_json::Value>,
}

impl FormatMetadataDoc {
    pub fn inferred_from_binary_only(binary_file: &str) -> Self {
        Self {
            format_version: 1,
            binary_file: binary_file.to_string(),
            binary_file_version: None,
            physics_record_schema: None,
            graphics_record_schema: None,
            statics_record_schema: None,
            sample_rate_hz: None,
            source: None,
            statics: None,
        }
    }

    pub fn infer_physics_schema(&self, binary_version: u16) -> u32 {
        self.physics_record_schema.unwrap_or_else(|| match binary_version {
            RKYV_BINARY_VERSION_V1 => PHYSICS_RECORD_SCHEMA_V1,
            _ => PHYSICS_RECORD_SCHEMA_V2,
        })
    }

    pub fn infer_graphics_schema(&self, binary_version: u16) -> u32 {
        self.graphics_record_schema.unwrap_or_else(|| match binary_version {
            RKYV_BINARY_VERSION_V1 => GRAPHICS_RECORD_SCHEMA_V1,
            _ => GRAPHICS_RECORD_SCHEMA_V2,
        })
    }

    pub fn infer_statics_schema(&self, binary_version: u16) -> u32 {
        self.statics_record_schema.unwrap_or_else(|| match binary_version {
            RKYV_BINARY_VERSION_V1 => STATICS_RECORD_SCHEMA_V1,
            _ => STATICS_RECORD_SCHEMA_V2,
        })
    }

    pub fn validate_physics(&self, binary_version: u16, schema: u32) -> std::io::Result<()> {
        if let Some(expected) = self.physics_record_schema {
            if expected != schema {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "physics_record_schema mismatch: json={expected}, inferred={schema}"
                    ),
                ));
            }
        }
        if let Some(bv) = self.binary_file_version {
            if bv != binary_version {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "binary_file_version mismatch: json={bv}, header={binary_version}"
                    ),
                ));
            }
        }
        Ok(())
    }

    pub fn validate_graphics(&self, binary_version: u16, schema: u32) -> std::io::Result<()> {
        if let Some(expected) = self.graphics_record_schema {
            if expected != schema {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "graphics_record_schema mismatch: json={expected}, inferred={schema}"
                    ),
                ));
            }
        }
        if let Some(bv) = self.binary_file_version {
            if bv != binary_version {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "binary_file_version mismatch: json={bv}, header={binary_version}"
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// Load companion `<stem>.json` for a physics or graphics rkyv path.
pub fn read_format_metadata(json_path: &Path) -> std::io::Result<FormatMetadataDoc> {
    let raw = std::fs::read_to_string(json_path)?;
    serde_json::from_str(&raw)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
}

#[derive(Serialize)]
struct FormatMetadata<'a> {
    format_version: u16,
    binary_file: &'a str,
    binary_file_version: u16,
    physics_record_schema: u32,
    graphics_record_schema: u32,
    statics_record_schema: u32,
    created_at: String,
    sample_rate_hz: u32,
    source: &'static str,
    file_format: FileFormat,
    schema: Schema,
    #[serde(skip_serializing_if = "Option::is_none")]
    statics: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct FileFormat {
    header: HeaderFormat,
    chunks: ChunkFormat,
    serialization: &'static str,
}

#[derive(Serialize)]
struct HeaderFormat {
    size_bytes: u32,
    layout: Vec<HeaderField>,
    byte_order: &'static str,
}

#[derive(Serialize)]
struct HeaderField {
    offset: u32,
    size: u32,
    name: &'static str,
    r#type: &'static str,
    description: &'static str,
}

#[derive(Serialize)]
struct ChunkFormat {
    structure: &'static str,
    length_prefix: LengthPrefix,
    payload: &'static str,
}

#[derive(Serialize)]
struct LengthPrefix {
    size_bytes: u32,
    r#type: &'static str,
    byte_order: &'static str,
}

#[derive(Serialize)]
struct Schema {
    root_type: &'static str,
    root_description: &'static str,
    types: Vec<TypeDef>,
}

#[derive(Serialize)]
struct TypeDef {
    name: &'static str,
    description: &'static str,
    fields: Vec<FieldDef>,
}

#[derive(Serialize)]
struct FieldDef {
    name: &'static str,
    r#type: &'static str,
    unit: Option<&'static str>,
}

/// Write format metadata JSON alongside the rkyv file.
pub fn write_format_metadata(
    rkyv_path: &Path,
    statics: Option<&crate::record::StaticsRecord>,
) -> std::io::Result<()> {
    let binary_name = rkyv_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("recording.rkyv");

    let created_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let created_at = format_iso8601(created_at);

    let meta = FormatMetadata {
        format_version: FORMAT_JSON_VERSION,
        binary_file: binary_name,
        binary_file_version: RKYV_BINARY_VERSION_V2,
        physics_record_schema: PHYSICS_RECORD_SCHEMA_V3,
        graphics_record_schema: GRAPHICS_RECORD_SCHEMA_V2,
        statics_record_schema: STATICS_RECORD_SCHEMA_V2,
        created_at,
        sample_rate_hz: 333,
        source: "ACC/AC Rally shared memory (acc_shared_memory_rs)",
        file_format: FileFormat {
            header: HeaderFormat {
                size_bytes: 16,
                byte_order: "little-endian",
                layout: vec![
                    HeaderField {
                        offset: 0,
                        size: 4,
                        name: "magic",
                        r#type: "bytes",
                        description: "File signature, must be b\"ACCR\"",
                    },
                    HeaderField {
                        offset: 4,
                        size: 2,
                        name: "version",
                        r#type: "u16",
                        description: "Format version",
                    },
                    HeaderField {
                        offset: 6,
                        size: 4,
                        name: "sample_rate",
                        r#type: "u32",
                        description: "Target sample rate in Hz (typically 333)",
                    },
                    HeaderField {
                        offset: 10,
                        size: 6,
                        name: "reserved",
                        r#type: "bytes",
                        description: "Reserved for future use",
                    },
                ],
            },
            chunks: ChunkFormat {
                structure: "Repeated: [length_prefix][payload] from offset 16 until EOF",
                length_prefix: LengthPrefix {
                    size_bytes: 4,
                    r#type: "u32",
                    byte_order: "little-endian",
                },
                payload: "rkyv-serialized Vec<PhysicsRecord>",
            },
            serialization: "rkyv 0.7. Read via acr_recorder::export::rkyv_format using binary_file_version and *_record_schema from this file.",
        },
        schema: Schema {
            root_type: "Vec<PhysicsRecord> | Vec<GraphicsRecord>",
            root_description: "Chunk payloads; layout version = physics_record_schema / graphics_record_schema",
            types: schema_types_all(),
        },
        statics: statics.and_then(|s| serde_json::to_value(s).ok()),
    };

    let json_path = rkyv_path.with_extension("json");
    let json = serde_json::to_string_pretty(&meta).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;
    std::fs::write(json_path, json)
}

fn format_iso8601(secs: u64) -> String {
    let days = secs / 86400;
    let (y, m, d) = days_to_ymd(days);
    let h = (secs / 3600) % 24;
    let min = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, h, min, s)
}

fn days_to_ymd(days: u64) -> (u32, u32, u32) {
    const EPOCH: i64 = 719163;
    let j = days as i64 + EPOCH;
    let a = (4 * j + 3) / 146097;
    let b = j - (146097 * a) / 4;
    let c = (4 * b + 3) / 1461;
    let d = b - (1461 * c) / 4;
    let e = (5 * d + 2) / 153;
    let day = (d - (153 * e + 2) / 5) as u32 + 1;
    let month = ((e + 2) % 12) as u32 + 1;
    let year = (100 * a + c) as u32;
    (year, month, day)
}

fn schema_types_all() -> Vec<TypeDef> {
    let mut types = schema_types_physics();
    types.extend(schema_types_graphics());
    types
}

fn schema_types_physics() -> Vec<TypeDef> {
    vec![
        TypeDef {
            name: "PhysicsRecord",
            description: "Physics schema v2 (~333 Hz); v1 omits tyre_temp_extra (see record::v1)",
            fields: vec![
                FieldDef { name: "packet_id", r#type: "i32", unit: None },
                FieldDef { name: "gas", r#type: "f32", unit: Some("0–1") },
                FieldDef { name: "brake", r#type: "f32", unit: Some("0–1") },
                FieldDef { name: "clutch", r#type: "f32", unit: Some("0–1") },
                FieldDef { name: "steer_angle", r#type: "f32", unit: Some("deg") },
                FieldDef { name: "gear", r#type: "i32", unit: None },
                FieldDef { name: "rpm", r#type: "i32", unit: None },
                FieldDef { name: "autoshifter_on", r#type: "bool", unit: None },
                FieldDef { name: "ignition_on", r#type: "bool", unit: None },
                FieldDef { name: "starter_engine_on", r#type: "bool", unit: None },
                FieldDef { name: "is_engine_running", r#type: "bool", unit: None },
                FieldDef { name: "speed_kmh", r#type: "f32", unit: Some("km/h") },
                FieldDef { name: "velocity", r#type: "Vector3fRecord", unit: None },
                FieldDef { name: "local_velocity", r#type: "Vector3fRecord", unit: None },
                FieldDef { name: "local_angular_vel", r#type: "Vector3fRecord", unit: None },
                FieldDef { name: "g_force", r#type: "Vector3fRecord", unit: None },
                FieldDef { name: "heading", r#type: "f32", unit: Some("rad") },
                FieldDef { name: "pitch", r#type: "f32", unit: Some("rad") },
                FieldDef { name: "roll", r#type: "f32", unit: Some("rad") },
                FieldDef { name: "final_ff", r#type: "f32", unit: None },
                FieldDef { name: "wheel_slip", r#type: "WheelsRecord", unit: None },
                FieldDef { name: "wheel_load", r#type: "WheelsRecord", unit: None },
                FieldDef { name: "wheel_pressure", r#type: "WheelsRecord", unit: Some("psi") },
                FieldDef { name: "wheel_angular_speed", r#type: "WheelsRecord", unit: Some("rad/s") },
                FieldDef { name: "tyre_wear", r#type: "WheelsRecord", unit: None },
                FieldDef { name: "tyre_dirty_level", r#type: "WheelsRecord", unit: None },
                FieldDef { name: "tyre_core_temp", r#type: "WheelsRecord", unit: Some("°C") },
                FieldDef { name: "camber_rad", r#type: "WheelsRecord", unit: Some("rad") },
                FieldDef { name: "suspension_travel", r#type: "WheelsRecord", unit: Some("mm") },
                FieldDef { name: "brake_temp", r#type: "WheelsRecord", unit: Some("°C") },
                FieldDef { name: "brake_pressure", r#type: "WheelsRecord", unit: Some("bar") },
                FieldDef { name: "suspension_damage", r#type: "WheelsRecord", unit: None },
                FieldDef { name: "slip_ratio", r#type: "WheelsRecord", unit: None },
                FieldDef { name: "slip_angle", r#type: "WheelsRecord", unit: Some("deg") },
                FieldDef { name: "pad_life", r#type: "WheelsRecord", unit: Some("%") },
                FieldDef { name: "disc_life", r#type: "WheelsRecord", unit: Some("%") },
                FieldDef { name: "front_brake_compound", r#type: "i32", unit: None },
                FieldDef { name: "rear_brake_compound", r#type: "i32", unit: None },
                FieldDef { name: "tyre_temp_i", r#type: "WheelsRecord", unit: Some("°C") },
                FieldDef { name: "tyre_temp_m", r#type: "WheelsRecord", unit: Some("°C") },
                FieldDef { name: "tyre_temp_o", r#type: "WheelsRecord", unit: Some("°C") },
                FieldDef {
                    name: "tyre_temp_extra",
                    r#type: "WheelsRecord",
                    unit: Some("°C (4th SHM block; see PhysicsMap)"),
                },
                FieldDef { name: "tyre_contact_point", r#type: "ContactPointRecord", unit: None },
                FieldDef { name: "tyre_contact_normal", r#type: "ContactPointRecord", unit: None },
                FieldDef { name: "tyre_contact_heading", r#type: "ContactPointRecord", unit: None },
                FieldDef { name: "fuel", r#type: "f32", unit: Some("L") },
                FieldDef { name: "tc", r#type: "f32", unit: None },
                FieldDef { name: "abs", r#type: "f32", unit: None },
                FieldDef { name: "pit_limiter_on", r#type: "bool", unit: None },
                FieldDef { name: "turbo_boost", r#type: "f32", unit: Some("bar") },
                FieldDef { name: "air_temp", r#type: "f32", unit: Some("°C") },
                FieldDef { name: "road_temp", r#type: "f32", unit: Some("°C") },
                FieldDef { name: "water_temp", r#type: "f32", unit: Some("°C") },
                FieldDef { name: "car_damage", r#type: "CarDamageRecord", unit: None },
                FieldDef { name: "is_ai_controlled", r#type: "bool", unit: None },
                FieldDef { name: "brake_bias", r#type: "f32", unit: None },
                FieldDef { name: "tc_in_action", r#type: "bool", unit: None },
                FieldDef { name: "abs_in_action", r#type: "bool", unit: None },
                FieldDef { name: "drs", r#type: "i32", unit: None },
                FieldDef { name: "cg_height", r#type: "f32", unit: None },
                FieldDef { name: "number_of_tyres_out", r#type: "i32", unit: None },
                FieldDef { name: "kers_charge", r#type: "f32", unit: None },
                FieldDef { name: "kers_input", r#type: "f32", unit: None },
                FieldDef { name: "ride_height_front", r#type: "f32", unit: None },
                FieldDef { name: "ride_height_rear", r#type: "f32", unit: None },
                FieldDef { name: "ballast", r#type: "f32", unit: None },
                FieldDef { name: "air_density", r#type: "f32", unit: None },
                FieldDef { name: "performance_meter", r#type: "f32", unit: None },
                FieldDef { name: "engine_brake", r#type: "i32", unit: None },
                FieldDef { name: "ers_recovery_level", r#type: "i32", unit: None },
                FieldDef { name: "ers_power_level", r#type: "i32", unit: None },
                FieldDef { name: "ers_heat_charging", r#type: "i32", unit: None },
                FieldDef { name: "ers_is_charging", r#type: "i32", unit: None },
                FieldDef { name: "kers_current_kj", r#type: "f32", unit: None },
                FieldDef { name: "drs_available", r#type: "i32", unit: None },
                FieldDef { name: "drs_enabled", r#type: "i32", unit: None },
                FieldDef { name: "p2p_activation", r#type: "i32", unit: None },
                FieldDef { name: "p2p_status", r#type: "i32", unit: None },
                FieldDef { name: "current_max_rpm", r#type: "i32", unit: None },
                FieldDef { name: "mz", r#type: "WheelsRecord", unit: None },
                FieldDef { name: "fz", r#type: "WheelsRecord", unit: None },
                FieldDef { name: "my", r#type: "WheelsRecord", unit: None },
                FieldDef { name: "kerb_vibration", r#type: "f32", unit: None },
                FieldDef { name: "slip_vibration", r#type: "f32", unit: None },
                FieldDef { name: "g_vibration", r#type: "f32", unit: None },
                FieldDef { name: "abs_vibration", r#type: "f32", unit: None },
            ],
        },
        TypeDef {
            name: "Vector3fRecord",
            description: "3D vector (x, y, z)",
            fields: vec![
                FieldDef { name: "x", r#type: "f32", unit: None },
                FieldDef { name: "y", r#type: "f32", unit: None },
                FieldDef { name: "z", r#type: "f32", unit: None },
            ],
        },
        TypeDef {
            name: "WheelsRecord",
            description: "Per-wheel values (front_left, front_right, rear_left, rear_right)",
            fields: vec![
                FieldDef { name: "front_left", r#type: "f32", unit: None },
                FieldDef { name: "front_right", r#type: "f32", unit: None },
                FieldDef { name: "rear_left", r#type: "f32", unit: None },
                FieldDef { name: "rear_right", r#type: "f32", unit: None },
            ],
        },
        TypeDef {
            name: "ContactPointRecord",
            description: "3D contact points for all four tyres",
            fields: vec![
                FieldDef { name: "front_left", r#type: "Vector3fRecord", unit: None },
                FieldDef { name: "front_right", r#type: "Vector3fRecord", unit: None },
                FieldDef { name: "rear_left", r#type: "Vector3fRecord", unit: None },
                FieldDef { name: "rear_right", r#type: "Vector3fRecord", unit: None },
            ],
        },
        TypeDef {
            name: "CarDamageRecord",
            description: "Car damage (front, rear, left, right, center)",
            fields: vec![
                FieldDef { name: "front", r#type: "f32", unit: None },
                FieldDef { name: "rear", r#type: "f32", unit: None },
                FieldDef { name: "left", r#type: "f32", unit: None },
                FieldDef { name: "right", r#type: "f32", unit: None },
                FieldDef { name: "center", r#type: "f32", unit: None },
            ],
        },
    ]
}

fn schema_types_graphics() -> Vec<TypeDef> {
    vec![TypeDef {
        name: "GraphicsRecord",
        description: "Graphics snapshot schema v2 (~60 Hz); v1 omits replay_time_multiplier, surface_grip, i_split",
        fields: vec![
            FieldDef { name: "packet_id", r#type: "i32", unit: None },
            FieldDef { name: "car_coordinates_x", r#type: "f32", unit: Some("game world") },
            FieldDef { name: "car_coordinates_y", r#type: "f32", unit: Some("game world") },
            FieldDef { name: "car_coordinates_z", r#type: "f32", unit: Some("game world") },
            FieldDef { name: "distance_traveled", r#type: "f32", unit: Some("m") },
            FieldDef { name: "speed_kmh", r#type: "f32", unit: None },
            FieldDef { name: "replay_time_multiplier", r#type: "f32", unit: Some("v2 only") },
            FieldDef { name: "surface_grip", r#type: "f32", unit: Some("v2 only") },
            FieldDef { name: "i_split", r#type: "i32", unit: Some("v2 only") },
        ],
    }]
}
