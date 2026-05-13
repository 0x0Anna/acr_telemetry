//! Demo split sounds via default audio device (same path as `acr_track_match`).

fn main() {
    eprintln!("Split audio demo (delta in seconds, same as acr_track_match).");
    let cfg = acr_recorder::split_beep::SplitBeepConfig::default();
    let scenarios = [
        ("faster 120ms", -0.12),
        ("faster 380ms", -0.38),
        ("faster 780ms", -0.78),
        ("slower 120ms", 0.12),
        ("slower 380ms", 0.38),
        ("slower 780ms", 0.78),
    ];
    for (label, delta) in scenarios {
        eprintln!("-> {label}");
        acr_recorder::split_beep::play_split_feedback(delta, &cfg);
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    eprintln!("Done.");
}
