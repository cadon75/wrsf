use crate::etc_process::rms_change;
use crate::progress::{progress_eta_range_if, update_progress};
use std::error::Error;
use std::time::Instant;

// REM: 初期処理（リサンプリング & RMS / 入力されたwavを下処理）
pub fn pre_process(samples: &[f64], n_channels: u16, framerate: u32) -> Result<(Vec<f64>, u32, f64, usize), Box<dyn Error>> {
    eprintln!("\n");
    let label_pre_process = "Preparing & Pre-processing… ";
    let start_time_pre_process = Instant::now();
    let mut resampled: Vec<f64> = Vec::with_capacity(samples.len() * 4);
    let num_frames = samples.len() / n_channels as usize;
    for i in 0..num_frames {
        let frame_start = i * n_channels as usize;
        let frame_end = frame_start + n_channels as usize;
        let frame = &samples[frame_start..frame_end];
        // REM:線形補間リサンプリング
        let next_i = if i + 1 < num_frames { i + 1 } else { i };
        let next_start = next_i * n_channels as usize;
        let next_end = next_start + n_channels as usize;
        let next_frame = &samples[next_start..next_end];
        for step in 0..4 {
            let t = step as f64 / 4.0;
            for ch in 0..n_channels as usize {
                let pre_resamp0 = frame[ch];
                let pre_resamp1 = next_frame[ch];
                let interp = pre_resamp0 + (pre_resamp1 - pre_resamp0) * t;
                resampled.push(interp);
            }
        }
        progress_eta_range_if(i, 2_000_000, num_frames, label_pre_process, 0.0, 70.0, start_time_pre_process)?;
    }
    rms_change(&mut resampled, n_channels, label_pre_process, 70.0, 100.0, None)?;
    update_progress(label_pre_process, 100.0, 100.0)?;

    let new_fs = framerate * 4;
    let dt = 1.0 / new_fs as f64;
    let total_res = resampled.len();

    Ok((resampled, new_fs, dt, total_res))
}
