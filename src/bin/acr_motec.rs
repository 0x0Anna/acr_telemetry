//! Direct-to-MoTeC live recorder (`acr_motec` binary).
//!
//! Thin wrapper around [`acr_recorder::motec_live`]. Same behaviour as `acr_recorder --motec`.

use std::sync::atomic::{AtomicBool, Ordering};

static RUNNING: AtomicBool = AtomicBool::new(true);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    ctrlc::set_handler(|| RUNNING.store(false, Ordering::Relaxed))
        .expect("could not set Ctrl+C handler");

    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return Ok(());
    }

    acr_recorder::motec_live::run(
        acr_recorder::motec_live::Options {
            out_dir: acr_recorder::motec_live::parse_out_dir(&args),
        },
        &RUNNING,
    )
}

fn print_help() {
    println!("acr_motec — Direct-to-MoTeC live recorder for ACC / AC Rally");
    println!();
    println!("Same as: acr_recorder --motec");
    println!();
    println!("Records physics data live from ACC shared memory and writes a MoTeC LD");
    println!("file on stop. No intermediate rkyv files are created.");
    println!();
    println!("USAGE:");
    println!("    acr_motec [OPTIONS]");
    println!();
    println!("OPTIONS:");
    println!("    --out <dir>      Output directory for the LD file");
    println!("                     Default: raw_output_dir from acr_recorder.toml");
    println!("    --out=<dir>      Same as --out <dir>");
    println!("    --help, -h       Show this help message and exit");
    println!();
    println!("OUTPUT:");
    println!("    acr_motec_<unix_timestamp>.ld   MoTeC LD file, openable in MoTeC i2");
    println!();
    println!("STARTUP:");
    println!("    If ACC is not running yet, waits for shared memory (polls every 0.5s).");
    println!();
    println!("STOPPING:");
    println!("    Ctrl+C                          Stop recording");
    println!("    Create the stop file            Default: acr_stop (see config)");
    println!();
    println!("EXAMPLES:");
    println!("    acr_motec                       Record to default output dir");
    println!("    acr_motec --out C:\\Telemetry    Record to a custom directory");
}
