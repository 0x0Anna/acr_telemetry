//! Standalone OSD demo: simulates timing start + 1 Hz sub splits + new sector every 5 s.
//!
//! Run (console):
//!   cargo run --bin acr_timing_presenter_demo
//! With RTSS (Windows):
//!   cargo run --bin acr_timing_presenter_demo -- --rtss --rtss-owner acr_timing_demo

use std::thread;
use std::time::{Duration, Instant};

use acr_timing_presenter::{compose_osd_message, PresenterState};
use acr_timing_protocol::{
    SectorCompleted, SectorIncomplete, SectorStarted, SubSplit, TimingEvent, TimingEventBody,
    TimingStarted,
};

const SECTOR_SEC: u64 = 5;
const SUBS_PER_SECTOR: u32 = 4;
const SECTOR_COUNT: u32 = 4;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rtss = args.iter().any(|a| a == "--rtss");
    let rtss_owner = args
        .iter()
        .position(|a| a == "--rtss-owner")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("acr_timing_demo");

    let mut presenter = PresenterState::default();
    let run_start = Instant::now();
    let mut timing_started = false;
    let mut active_sector: Option<u32> = None;
    let mut last_tick_sec: u64 = u64::MAX;
    let mut sub_times: Vec<Option<f64>> = vec![None; SUBS_PER_SECTOR as usize];

    eprintln!(
        "acr_timing_presenter_demo: 1 sub/s, new sector every {SECTOR_SEC}s (S1..S{SECTOR_COUNT})"
    );
    eprintln!("Ctrl+C to stop.");

    loop {
        let elapsed = run_start.elapsed().as_secs();
        if elapsed >= SECTOR_COUNT as u64 * SECTOR_SEC {
            break;
        }

        if elapsed != last_tick_sec {
            last_tick_sec = elapsed;
            let sector_ix = (elapsed / SECTOR_SEC) as u32;
            let sec_in_sector = (elapsed % SECTOR_SEC) as u32;

            if !timing_started && elapsed == 0 {
                presenter.apply(&TimingEvent::new(TimingEventBody::TimingStarted(
                    TimingStarted {
                        reference_track: "demo".into(),
                        stage_slug: "demo".into(),
                        reference_stage_tot_sec: None,
                    },
                )));
                timing_started = true;
            }

            if sec_in_sector == 0 {
                if let Some(prev) = active_sector {
                    if prev < sector_ix || (prev == sector_ix && elapsed > 0) {
                        finish_sector(&mut presenter, prev, &sub_times);
                    }
                }
                if sector_ix < SECTOR_COUNT && active_sector != Some(sector_ix) {
                    sub_times = vec![None; SUBS_PER_SECTOR as usize];
                    start_sector(&mut presenter, sector_ix);
                    active_sector = Some(sector_ix);
                }
            } else if active_sector == Some(sector_ix) && sec_in_sector <= SUBS_PER_SECTOR {
                let sub_ix = (sec_in_sector - 1) as usize;
                let leg = sec_in_sector as f64;
                sub_times[sub_ix] = Some(leg);
                let sub_id = (sub_ix + 1) as i32;
                let delta = leg - (sub_id as f64);
                presenter.apply(&TimingEvent::new(TimingEventBody::SubSplit(SubSplit {
                    sector_index: sector_ix,
                    sub_id,
                    leg_time_sec: leg,
                    delta_i_sec: Some(delta),
                    cum_delta_sec: delta,
                })));
                eprintln!("tick {elapsed}s: sub CP {sub_id} in S{}", sector_ix + 1);
            }
        }

        let status = if timing_started {
            format!("demo timing +{}s", elapsed)
        } else {
            "demo waiting".into()
        };
        let lines = presenter.osd_lines(
            rtss,
            &acr_timing::DeltaDisplayConfig::default(),
            Some(&acr_timing::OsdTemplateConfig::default()),
        );
        let msg = compose_osd_message(&status, &lines);
        print_osd_block(&msg);

        if rtss {
            #[cfg(windows)]
            {
                let safe = acr_timing::rtss_osd::sanitize_multiline_osd_text(
                    &msg,
                    acr_timing::rtss_osd::DEFAULT_MAX_OSD_LINES,
                );
                let _ = acr_timing::rtss_osd::update(rtss_owner, &safe, 0);
            }
            #[cfg(not(windows))]
            let _ = rtss_owner;
        }

        thread::sleep(Duration::from_millis(200));
    }

    if let Some(prev) = active_sector {
        finish_sector(&mut presenter, prev, &sub_times);
    }
    let status = "demo finished";
    let lines = presenter.osd_lines(
        rtss,
        &acr_timing::DeltaDisplayConfig::default(),
        Some(&acr_timing::OsdTemplateConfig::default()),
    );
    let msg = compose_osd_message(status, &lines);
    print_osd_block(&msg);
    #[cfg(windows)]
    if rtss {
        let _ = acr_timing::rtss_osd::release(rtss_owner);
    }
    eprintln!("done.");
}

fn start_sector(presenter: &mut PresenterState, sector_index: u32) {
    let ref_subs: Vec<i32> = (1..=SUBS_PER_SECTOR as i32).collect();
    let ref_times: Vec<f64> = ref_subs.iter().map(|&id| id as f64).collect();
    presenter.apply(&TimingEvent::new(TimingEventBody::SectorStarted(SectorStarted {
        sector_index,
        reference_run_id: None,
        reference_sub_ids: ref_subs,
        reference_sub_times_sec: ref_times,
        reference_tot_sec: SUBS_PER_SECTOR as f64,
    })));
    eprintln!("sector S{} started (expect lower line S{})", sector_index + 1, sector_index + 1);
}

fn finish_sector(presenter: &mut PresenterState, sector_index: u32, sub_times: &[Option<f64>]) {
    let any = sub_times.iter().any(|t| t.is_some());
    if any {
        let sub_ids: Vec<i32> = (1..=SUBS_PER_SECTOR as i32).collect();
        let tot: f64 = SECTOR_SEC as f64;
        presenter.apply(&TimingEvent::new(TimingEventBody::SectorCompleted(SectorCompleted {
            sector_index,
            cum_delta_sec: 0.0,
            tot_sec: tot,
            sub_ids,
            sub_times_sec: sub_times.to_vec(),
            sub_delta_sec: vec![None; SUBS_PER_SECTOR as usize],
            reference_tot_sec: SUBS_PER_SECTOR as f64,
        })));
    } else {
        presenter.apply(&TimingEvent::new(TimingEventBody::SectorIncomplete(
            SectorIncomplete {
                sector_index,
                tot_sec: SECTOR_SEC as f64,
            },
        )));
    }
    eprintln!(
        "sector S{} completed -> upper line should show S{}, lower S{}",
        sector_index + 1,
        sector_index + 1,
        sector_index + 2
    );
}

fn print_osd_block(msg: &str) {
    eprint!("\x1b[2J\x1b[H");
    for line in msg.lines() {
        eprintln!("{line}");
    }
}
