use crate::progress::{progress_eta_range_if, update_progress};
use std::{error::Error, time::Instant};

// REM: カセットテープフィルタ（ギャップ損失＆テープ記録周波数＆S/N比）
pub fn tape_filter(resampled: &mut [f64], total_res: usize, n_channels: u16, new_fs: u32, dt: f64) -> Result<(), Box<dyn Error>> {
    eprintln!("\n");
    let label_tapef = "Processing Cassette Tape Filter… ";
    let start_time_tapef = Instant::now();
    let total_steps = total_res / n_channels as usize;

    // REM: 3-1. ギャップ損失
    let gap_samples = (2.0_f64).max((1.0e-6 / 0.0476) * new_fs as f64) as usize;
    let mut gap_buf = vec![vec![0.0; gap_samples]; n_channels as usize];
    let mut gap_sum = vec![0.0; n_channels as usize];
    let mut gap_ptr = 0;
    for i in 0..total_steps {
        for ch in 0..n_channels as usize {
            let idx = i * n_channels as usize + ch;
            let val = resampled[idx];
            gap_sum[ch] -= gap_buf[ch][gap_ptr];
            gap_buf[ch][gap_ptr] = val;
            gap_sum[ch] += val;
            resampled[idx] = gap_sum[ch] / gap_samples as f64;
        }
        gap_ptr = (gap_ptr + 1) % gap_samples;
        progress_eta_range_if(i, 2_000_000, total_steps, label_tapef, 0.0, 25.0, start_time_tapef)?;
    }

    // REM: 3-2. テープ記録周波数簡易処理
    let tape_filters = [(40.0, "HPF"), (12000.0, "LPF")];
    for (idx_f, &(fc, label_f)) in tape_filters.iter().enumerate() {
        let rc = 1.0 / (2.0 * std::f64::consts::PI * fc);
        let alpha = if label_f == "HPF" { rc / (rc + dt) } else { dt / (rc + dt) };
        let mut p_x = vec![0.0; n_channels as usize];
        let mut p_y = vec![0.0; n_channels as usize];
        for i in 0..total_steps {
            for ch in 0..n_channels as usize {
                let idx = i * n_channels as usize + ch;
                let y = if label_f == "HPF" {
                    let temp_y = p_y[ch] + resampled[idx] - p_x[ch];
                    p_x[ch] = resampled[idx];
                    alpha * temp_y
                } else {
                    p_y[ch] + alpha * (resampled[idx] - p_y[ch])
                };
                resampled[idx] = y;
                p_y[ch] = y;
            }
            let phase_start = 25.0 + idx_f as f64 * 25.0;
            let phase_end = phase_start + 25.0;
            progress_eta_range_if(i, 2_000_000, total_steps, label_tapef, phase_start, phase_end, start_time_tapef)?;
        }
    }

    // REM: 3-3. S/N比再現処理
    let sn_high = 10.0_f64.powf(-60.0 / 20.0);
    let sn_low = 10.0_f64.powf(-96.0 / 20.0);
    let env_attack = 1.0 - (-1.0 / (0.01 * new_fs as f64)).exp(); // REM: 波形潰れ防止
    let env_release = 1.0 - (-1.0 / (0.1 * new_fs as f64)).exp();
    let mut env_state = vec![0.0; n_channels as usize];
    for i in 0..total_steps {
        for ch in 0..n_channels as usize {
            let idx = i * n_channels as usize + ch;
            let val = resampled[idx];
            let abs_v = val.abs();
            if abs_v > env_state[ch] {
                env_state[ch] += env_attack * (abs_v - env_state[ch]);
            } else {
                env_state[ch] += env_release * (abs_v - env_state[ch]);
            }
            let current_env = env_state[ch];
            if current_env < sn_low {
                resampled[idx] = 0.0;
            } else if current_env < sn_high {
                // REM: -60dBから-96dBの間で線形補間的にゲインを落とす
                let gain = (current_env - sn_low) / (sn_high - sn_low);
                resampled[idx] *= gain;
            }
        }
        progress_eta_range_if(i, 2_000_000, total_steps, label_tapef, 75.0, 100.0, start_time_tapef)?;
    }
    update_progress(label_tapef, 100.0, 100.0)?;
    Ok(())
}
