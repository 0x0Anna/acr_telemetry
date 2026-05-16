fn main() {
    if let Err(e) = acr_recorder::track_match_app::run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
