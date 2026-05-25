//! MoTeC .ld file writer.
//!
//! Format ported from Python ldparser (gotzl/ldparser) - reverse-engineered ACC MoTeC export.
//! Channel names/units come from TOML profiles (`motec_profiles/<profile>.toml`).

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use crate::record::{GraphicsRecord, PhysicsRecord};

use super::motec_profile::{self, MotecProfile};

/// Write physics records to MoTeC .ld using the configured or default profile.
pub fn write_ld(
    path: impl AsRef<Path>,
    records: &[PhysicsRecord],
    sample_rate_hz: u32,
) -> std::io::Result<()> {
    write_ld_with_profile(path, records, sample_rate_hz, None, None)
}

/// Write physics records (+ optional graphics sidecar channels) to MoTeC .ld format.
pub fn write_ld_with_graphics(
    path: impl AsRef<Path>,
    records: &[PhysicsRecord],
    sample_rate_hz: u32,
    graphics: Option<(&[GraphicsRecord], u32)>,
) -> std::io::Result<()> {
    write_ld_with_profile(path, records, sample_rate_hz, graphics, None)
}

/// Write `.ld` with an explicit MoTeC channel profile (from TOML).
pub fn write_ld_with_profile(
    path: impl AsRef<Path>,
    records: &[PhysicsRecord],
    sample_rate_hz: u32,
    graphics: Option<(&[GraphicsRecord], u32)>,
    profile: Option<&MotecProfile>,
) -> std::io::Result<()> {
    let profile = match profile {
        Some(p) => p.clone(),
        None => motec_profile::load_profile_from_config(
            &default_profile_name(),
            default_profiles_dir().as_deref(),
        )
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?,
    };
    write_ld_profile(path, records, sample_rate_hz, graphics, &profile)
}

fn default_profile_name() -> String {
    crate::config::load_config()
        .export
        .motec
        .profile
        .clone()
}

fn default_profiles_dir() -> Option<String> {
    let dir = crate::config::load_config().export.motec.profiles_dir.clone();
    if dir.trim().is_empty() {
        None
    } else {
        Some(dir)
    }
}

fn write_ld_profile(
    path: impl AsRef<Path>,
    records: &[PhysicsRecord],
    sample_rate_hz: u32,
    graphics: Option<(&[GraphicsRecord], u32)>,
    profile: &MotecProfile,
) -> std::io::Result<()> {
    let channels = motec_profile::build_ld_channels(profile, records, sample_rate_hz, graphics)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    if channels.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("MoTeC profile '{}' produced no channels", profile.id),
        ));
    }

    let mut f = File::create(path)?;
    let rec_freq: u16 = sample_rate_hz.min(u16::MAX as u32) as u16;

    let head_size = 1762u32;
    let event_ptr: u32 = head_size;
    let event_size = 1154u32;
    let chan_head_size = 124u32;

    let meta_ptr = head_size + event_size;
    let data_ptr = meta_ptr + channels.len() as u32 * chan_head_size;

    let mut data_offsets = Vec::with_capacity(channels.len());
    let mut offset = data_ptr;
    for (_, _, data) in &channels {
        data_offsets.push(offset);
        offset += data.len() as u32 * 4;
    }

    write_ld_head(
        &mut f,
        meta_ptr,
        data_ptr,
        event_ptr,
        channels.len() as u32,
        &profile.id,
    )?;

    f.seek(SeekFrom::Start(event_ptr as u64))?;
    write_ld_event(&mut f, &profile.description)?;

    f.seek(SeekFrom::Start(meta_ptr as u64))?;
    for (i, ((name, unit, data), &data_off)) in channels.iter().zip(data_offsets.iter()).enumerate()
    {
        let prev = if i == 0 {
            0u32
        } else {
            meta_ptr + (i - 1) as u32 * chan_head_size
        };
        let next = if i + 1 < channels.len() {
            meta_ptr + (i + 1) as u32 * chan_head_size
        } else {
            0
        };
        write_ld_chan(&mut f, prev, next, data_off, data.len() as u32, name, unit, i, rec_freq)?;
    }

    for (_, _, data) in &channels {
        for &v in data {
            f.write_all(&v.to_le_bytes())?;
        }
    }

    Ok(())
}

fn pad(w: &mut impl Write, n: usize) -> std::io::Result<()> {
    w.write_all(&vec![0u8; n])
}

fn write_str_fixed(w: &mut impl Write, s: &str, len: usize) -> std::io::Result<()> {
    let bytes = s.as_bytes();
    let n = bytes.len().min(len);
    w.write_all(&bytes[..n])?;
    pad(w, len - n)
}

fn write_ld_head(
    f: &mut File,
    meta_ptr: u32,
    data_ptr: u32,
    event_ptr: u32,
    n_chans: u32,
    profile_id: &str,
) -> std::io::Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = secs / 86400;
    let (y, m, d) = days_to_ymd(days);
    let h = (secs / 3600) % 24;
    let min = (secs / 60) % 60;
    let s = secs % 60;
    let date = format!("{:02}/{:02}/{:04}", d, m, y);
    let time = format!("{:02}:{:02}:{:02}", h, min, s);
    let comment = format!("acr_recorder export profile={profile_id}");

    f.write_all(&0x40u32.to_le_bytes())?;
    pad(f, 4)?;
    f.write_all(&meta_ptr.to_le_bytes())?;
    f.write_all(&data_ptr.to_le_bytes())?;
    pad(f, 20)?;
    f.write_all(&event_ptr.to_le_bytes())?;
    pad(f, 24)?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&0x4240u16.to_le_bytes())?;
    f.write_all(&0xfu16.to_le_bytes())?;
    f.write_all(&0x1f44u32.to_le_bytes())?;
    write_str_fixed(f, "ADL", 8)?;
    f.write_all(&420u16.to_le_bytes())?;
    f.write_all(&0xadb0u16.to_le_bytes())?;
    f.write_all(&n_chans.to_le_bytes())?;
    pad(f, 4)?;
    write_str_fixed(f, &date, 16)?;
    pad(f, 16)?;
    write_str_fixed(f, &time, 16)?;
    pad(f, 16)?;
    write_str_fixed(f, "ACR", 64)?;
    write_str_fixed(f, "AC Rally", 64)?;
    pad(f, 64)?;
    write_str_fixed(f, "Telemetry", 64)?;
    pad(f, 64)?;
    pad(f, 1024)?;
    f.write_all(&0xc81a4u32.to_le_bytes())?;
    pad(f, 66)?;
    write_str_fixed(f, &comment, 64)?;
    pad(f, 126)?;

    Ok(())
}

fn write_ld_event(f: &mut File, description: &str) -> std::io::Result<()> {
    let comment = if description.is_empty() {
        "acr_recorder export".to_string()
    } else {
        format!("acr_recorder: {description}")
    };
    write_str_fixed(f, "ACR Session", 64)?;
    write_str_fixed(f, "0", 64)?;
    write_str_fixed(f, &comment, 1024)?;
    f.write_all(&0u16.to_le_bytes())?;
    Ok(())
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

fn write_ld_chan(
    f: &mut File,
    prev: u32,
    next: u32,
    data_ptr: u32,
    n_data: u32,
    name: &str,
    unit: &str,
    idx: usize,
    rec_freq: u16,
) -> std::io::Result<()> {
    let counter = 0x2ee1u16 + idx as u16;
    let dtype_a: u16 = 0x07;
    let dtype: u16 = 4;
    let shift: i16 = 0;
    let mul: i16 = 1;
    let scale: i16 = 1;
    let dec: i16 = 0;

    f.write_all(&prev.to_le_bytes())?;
    f.write_all(&next.to_le_bytes())?;
    f.write_all(&data_ptr.to_le_bytes())?;
    f.write_all(&n_data.to_le_bytes())?;
    f.write_all(&counter.to_le_bytes())?;
    f.write_all(&dtype_a.to_le_bytes())?;
    f.write_all(&dtype.to_le_bytes())?;
    f.write_all(&rec_freq.to_le_bytes())?;
    f.write_all(&shift.to_le_bytes())?;
    f.write_all(&mul.to_le_bytes())?;
    f.write_all(&scale.to_le_bytes())?;
    f.write_all(&dec.to_le_bytes())?;
    write_str_fixed(f, name, 32)?;
    write_str_fixed(f, &name.chars().take(8).collect::<String>(), 8)?;
    write_str_fixed(f, unit, 12)?;
    pad(f, 40)?;

    Ok(())
}
