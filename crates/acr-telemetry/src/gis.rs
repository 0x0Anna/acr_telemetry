//! Flat-map GIS I/O for ACC world XZ.
//!
//! Shapefiles and GeoJSON store swapped coordinates (`[game_z, game_x]`) so
//! curve direction matches typical map viewers. Live telemetry stays in game XZ.

/// Shapefile / GeoJSON first and second coordinate components.
pub fn game_xz_to_file(game_x: f64, game_z: f64) -> (f64, f64) {
    (game_z, game_x)
}

/// Recover game XZ from a shapefile point or GeoJSON coordinate pair.
pub fn file_to_game_xz(file_x: f64, file_y: f64) -> (f64, f64) {
    (file_y, file_x)
}
