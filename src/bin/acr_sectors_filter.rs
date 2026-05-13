//! Filter dense sector boundaries in a LineString SHP.
//!
//! Keeps sector lines per track in ascending sector id, dropping a "following"
//! neighbor when center-to-center distance is below a threshold.
//!
//! Usage:
//!   acr_sectors_filter --input timing/sectors.shp --output timing/sectors.filtered.shp
//!   acr_sectors_filter --input timing/sectors.shp --output timing/sectors.filtered.shp --min-dist 50 --track-field src_layer --id-field seg_id

use std::collections::HashMap;
use std::convert::TryInto;
use std::path::PathBuf;

use shapefile::dbase::{FieldValue, Record, TableWriterBuilder};

#[derive(Clone, Copy, Debug)]
struct P2 {
    x: f64,
    y: f64,
}

#[derive(Clone, Debug)]
struct SectorLine {
    track: String,
    id: i32,
    a: P2,
    b: P2,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (input, output, min_dist, track_field, id_field) = parse_args(std::env::args().collect())?;
    let mut reader = shapefile::Reader::from_path(&input)?;

    let mut by_track: HashMap<String, Vec<SectorLine>> = HashMap::new();
    let mut total_in = 0usize;
    for item in reader.iter_shapes_and_records() {
        let (shape, rec) = item?;
        let track = field_value_to_string(rec.get(&track_field))
            .ok_or_else(|| format!("Missing/invalid track field '{}'", track_field))?;
        let id =
            field_value_to_i32(rec.get(&id_field)).ok_or_else(|| format!("Missing/invalid id field '{}'", id_field))?;

        let (a, b) = match shape {
            shapefile::Shape::Polyline(pl) => endpoints_polyline_points(pl.parts())?,
            shapefile::Shape::PolylineM(pl) => endpoints_polyline_m(pl.parts())?,
            shapefile::Shape::PolylineZ(pl) => endpoints_polyline_z(pl.parts())?,
            _ => continue,
        };

        by_track
            .entry(track.clone())
            .or_default()
            .push(SectorLine { track, id, a, b });
        total_in += 1;
    }

    let mut kept: Vec<SectorLine> = Vec::new();
    let mut dropped = 0usize;
    for (_track, mut lines) in by_track {
        lines.sort_by_key(|s| s.id);
        if lines.is_empty() {
            continue;
        }
        let mut out_track: Vec<SectorLine> = Vec::with_capacity(lines.len());
        out_track.push(lines[0].clone());
        for s in lines.iter().skip(1) {
            let prev = out_track.last().expect("non-empty");
            let d = dist(center(prev), center(s));
            if d < min_dist {
                dropped += 1;
                continue; // drop following neighbor
            }
            out_track.push(s.clone());
        }
        kept.extend(out_track);
    }

    // stable output by track then id
    kept.sort_by(|a, b| a.track.cmp(&b.track).then_with(|| a.id.cmp(&b.id)));

    let table_builder = TableWriterBuilder::new()
        .add_character_field(track_field.as_str().try_into()?, 64)
        .add_numeric_field(id_field.as_str().try_into()?, 12, 0);
    let mut writer = shapefile::Writer::from_path(&output, table_builder)?;
    for s in &kept {
        let shape = shapefile::Polyline::new(vec![
            shapefile::Point::new(s.a.x, s.a.y),
            shapefile::Point::new(s.b.x, s.b.y),
        ]);
        let mut rec = Record::default();
        rec.insert(track_field.clone(), FieldValue::Character(Some(s.track.clone())));
        rec.insert(id_field.clone(), FieldValue::Numeric(Some(s.id as f64)));
        writer.write_shape_and_record(&shape, &rec)?;
    }

    eprintln!(
        "Filtered sectors: in={} kept={} dropped={} min_dist={}m -> {}",
        total_in,
        kept.len(),
        dropped,
        min_dist,
        output.display()
    );
    Ok(())
}

fn parse_args(
    args: Vec<String>,
) -> Result<(PathBuf, PathBuf, f64, String, String), Box<dyn std::error::Error>> {
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut min_dist = 50.0f64;
    let mut track_field = "src_layer".to_string();
    let mut id_field = "seg_id".to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--input" => {
                input = Some(PathBuf::from(args.get(i + 1).ok_or("--input needs path")?));
                i += 1;
            }
            "--output" => {
                output = Some(PathBuf::from(args.get(i + 1).ok_or("--output needs path")?));
                i += 1;
            }
            "--min-dist" => {
                min_dist = args
                    .get(i + 1)
                    .ok_or("--min-dist needs meters value")?
                    .parse::<f64>()?;
                i += 1;
            }
            "--track-field" => {
                track_field = args.get(i + 1).ok_or("--track-field needs name")?.clone();
                i += 1;
            }
            "--id-field" => {
                id_field = args.get(i + 1).ok_or("--id-field needs name")?.clone();
                i += 1;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            _ => {}
        }
        i += 1;
    }

    let input = input.ok_or("Need --input <sectors.shp>")?;
    let output = output.ok_or("Need --output <sectors.filtered.shp>")?;
    Ok((input, output, min_dist, track_field, id_field))
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  acr_sectors_filter --input timing/sectors.shp --output timing/sectors.filtered.shp");
    eprintln!("  [--min-dist 50] [--track-field src_layer] [--id-field seg_id]");
}

fn field_value_to_string(v: Option<&FieldValue>) -> Option<String> {
    match v? {
        FieldValue::Character(Some(s)) => Some(s.trim().to_string()),
        FieldValue::Numeric(Some(n)) => Some(format!("{n:.0}")),
        FieldValue::Float(Some(f)) => Some(format!("{f:.0}")),
        FieldValue::Integer(i) => Some(i.to_string()),
        FieldValue::Double(d) => Some(format!("{d:.0}")),
        FieldValue::Logical(Some(b)) => Some(if *b { "1".into() } else { "0".into() }),
        _ => None,
    }
}

fn field_value_to_i32(v: Option<&FieldValue>) -> Option<i32> {
    match v? {
        FieldValue::Numeric(Some(n)) => Some(*n as i32),
        FieldValue::Float(Some(f)) => Some(*f as i32),
        FieldValue::Integer(i) => Some(*i),
        FieldValue::Double(d) => Some(*d as i32),
        FieldValue::Character(Some(s)) => s.trim().parse::<i32>().ok(),
        _ => None,
    }
}

fn endpoints_polyline_points(parts: &[Vec<shapefile::Point>]) -> Result<(P2, P2), Box<dyn std::error::Error>> {
    let part = parts.first().ok_or("Empty polyline part")?;
    let a = part.first().ok_or("Empty polyline")?;
    let b = part.last().ok_or("Empty polyline")?;
    Ok((P2 { x: a.x, y: a.y }, P2 { x: b.x, y: b.y }))
}

fn endpoints_polyline_m(parts: &[Vec<shapefile::PointM>]) -> Result<(P2, P2), Box<dyn std::error::Error>> {
    let part = parts.first().ok_or("Empty polylineM part")?;
    let a = part.first().ok_or("Empty polylineM")?;
    let b = part.last().ok_or("Empty polylineM")?;
    Ok((P2 { x: a.x, y: a.y }, P2 { x: b.x, y: b.y }))
}

fn endpoints_polyline_z(parts: &[Vec<shapefile::PointZ>]) -> Result<(P2, P2), Box<dyn std::error::Error>> {
    let part = parts.first().ok_or("Empty polylineZ part")?;
    let a = part.first().ok_or("Empty polylineZ")?;
    let b = part.last().ok_or("Empty polylineZ")?;
    Ok((P2 { x: a.x, y: a.y }, P2 { x: b.x, y: b.y }))
}

fn center(s: &SectorLine) -> P2 {
    P2 {
        x: (s.a.x + s.b.x) * 0.5,
        y: (s.a.y + s.b.y) * 0.5,
    }
}

fn dist(a: P2, b: P2) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt()
}
