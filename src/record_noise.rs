use crate::progress::{progress_eta_range_if, update_progress};
use rand::Rng;
use std::{error::Error, time::Instant};

// REM: レコードノイズ付加（摺り音＋プチプチ）
pub fn record_noise(resampled: &mut [f64], total_res: usize, n_channels: u16, new_fs: u32, do_speakerfilter: bool) -> Result<(), Box<dyn Error>> {
    eprintln!("\n");
    let label_r_noise = "Processing Record Noise Filter… ";
    let start_time_r_noise = Instant::now();
    let total_steps = total_res / n_channels as usize;
    let mut r_noise_rng = rand::thread_rng();

    // REM: 4-1. RMS解析
    let mut r_noise_rms_acc = 0.0_f64;
    for (i, &s) in resampled.iter().enumerate() {
        r_noise_rms_acc += s * s;
        progress_eta_range_if(i, 2_000_000, resampled.len(), label_r_noise, 0.0, 20.0, start_time_r_noise)?;
    }
    let mut r_noise_rms = (r_noise_rms_acc / resampled.len().max(1) as f64).sqrt();
    // REM: 無音入力時の保護
    let rms_floor = 10.0_f64.powf(-90.0 / 20.0);
    if r_noise_rms < rms_floor {
        r_noise_rms = rms_floor;
    }
    let loudness_db = 20.0 * r_noise_rms.log10();

    // REM: 4-2. レコード摺りノイズ
    // REM: 実効-65dB前後狙いだけど入力されたwavに左右されまふ
    let expected_max_p = resampled.iter().map(|&s| s.abs()).fold(0.0_f64, |a, b| a.max(b));
    let peak_floor = 10.0_f64.powf(-90.0 / 20.0);
    let mut expected_final_gain = if expected_max_p > peak_floor { 10.0_f64.powf(-1.0 / 20.0) / expected_max_p } else { 1.0 };
    if do_speakerfilter {
        if expected_max_p <= peak_floor {
            expected_final_gain = 10.0_f64.powf(29.0 / 20.0); // REM: スピーカーフィルタで膨れ上がる事がある先回り対策
        }
    }
    let target_noise_db = -60.0;
    let base_noise_gain = 10.0_f64.powf(target_noise_db / 20.0) / expected_final_gain;
    let low_gain = base_noise_gain * 0.80;
    let hiss_gain = base_noise_gain * 0.20;
    let low_cutoff_hz = 1400.0;
    let hiss_cutoff_hz = 6200.0;
    let process_fs = new_fs as f64 * 4.0;
    let ref_fs = 176400.0; // REM: 常に44.1kHzオーバーサンプリング176.4kHzを基準としてノイズを生成する
    let low_w0 = 2.0 * std::f64::consts::PI * low_cutoff_hz / ref_fs;
    let low_a = (2.0 - low_w0.cos()) - ((2.0 - low_w0.cos()).powi(2) - 1.0).sqrt();
    let hiss_w0 = 2.0 * std::f64::consts::PI * hiss_cutoff_hz / ref_fs;
    let hiss_a = (2.0 - hiss_w0.cos()) - ((2.0 - hiss_w0.cos()).powi(2) - 1.0).sqrt();
    let low_sr_scale = ((1.0 + low_a) / (1.0 - low_a)).sqrt() * 0.2916;
    let hiss_sr_scale = ((1.0 + hiss_a) / (1.0 - hiss_a)).sqrt() * 0.5988;
    let mut low_state = vec![0.0_f64; n_channels as usize];
    let mut hiss_state = vec![0.0_f64; n_channels as usize];
    let phase_step = ref_fs / process_fs;
    let mut phase = 1.0_f64;
    let mut prev_noise = vec![0.0_f64; n_channels as usize];
    let mut next_noise = vec![0.0_f64; n_channels as usize];
    for i in 0..total_steps {
        while phase >= 1.0 {
            phase -= 1.0;
            for ch in 0..n_channels as usize {
                prev_noise[ch] = next_noise[ch];
                // REM: 低域摺り
                let white_low = r_noise_rng.gen_range(-1.0..1.0) * low_gain * low_sr_scale;
                low_state[ch] = (1.0 - low_a) * white_low + low_a * low_state[ch];
                // REM: 高域ヒス
                let white_hiss = r_noise_rng.gen_range(-1.0..1.0) * hiss_gain * hiss_sr_scale;
                hiss_state[ch] = (1.0 - hiss_a) * white_hiss + hiss_a * hiss_state[ch];
                next_noise[ch] = low_state[ch] + hiss_state[ch];
            }
        }
        for ch in 0..n_channels as usize {
            let out_idx = i * n_channels as usize + ch;
            // REM: 線形補間でノイズ値を算出
            let final_noise = prev_noise[ch] + phase * (next_noise[ch] - prev_noise[ch]);
            resampled[out_idx] += final_noise;
        }
        phase += phase_step;
        progress_eta_range_if(i, 2_000_000, total_steps, label_r_noise, 20.0, 60.0, start_time_r_noise)?;
    }

    // REM: 4-3. クリックノイズ
    let click_scale = if loudness_db < -32.0 {
        0.88
    } else if loudness_db < -24.0 {
        1.05
    } else if loudness_db < -16.0 {
        1.35
    } else {
        1.60
    };
    let clicks_per_second = 1.8;
    let click_probability = (clicks_per_second / new_fs as f64).clamp(0.0, 1.0);
    let sr_ratio = new_fs as f64 / 44100.0;
    let tick_len_44k = 2.0_f64;
    let pop_len_44k = 4.0_f64;
    let thump_len_44k = 8.0_f64;
    let tick_len = (tick_len_44k * sr_ratio).ceil() as usize;
    let pop_len = (pop_len_44k * sr_ratio).ceil() as usize;
    let thump_len = (thump_len_44k * sr_ratio).ceil() as usize;
    for i in 0..total_steps {
        if r_noise_rng.gen_bool(click_probability) {
            let center_idx_l = i * n_channels as usize;
            let center_idx_r = center_idx_l + if n_channels > 1 { 1 } else { 0 };
            let stereo_offset = ((0.000010 * new_fs as f64) as isize).max(1);
            let l_offset = r_noise_rng.gen_range(-stereo_offset..=stereo_offset) * n_channels as isize;
            let r_offset = r_noise_rng.gen_range(-stereo_offset..=stereo_offset) * n_channels as isize;
            let noise_roll = r_noise_rng.gen_range(0.0..1.0);
            let noise_type = if noise_roll < 0.72 {
                0
            } else if noise_roll < 0.94 {
                1
            } else {
                2
            };
            let strong_noise = r_noise_rng.gen_bool(0.06);
            let click_sign = if r_noise_rng.gen_bool(0.5) { 1.0 } else { -1.0 };
            let base_amp = match noise_type {
                0 => {
                    if strong_noise {
                        r_noise_rng.gen_range(0.010..0.024)
                    } else {
                        r_noise_rng.gen_range(0.0020..0.0060)
                    }
                }
                1 => {
                    if strong_noise {
                        r_noise_rng.gen_range(0.018..0.045)
                    } else {
                        r_noise_rng.gen_range(0.0045..0.0125)
                    }
                }
                _ => {
                    if strong_noise {
                        r_noise_rng.gen_range(0.028..0.065)
                    } else {
                        r_noise_rng.gen_range(0.008..0.018)
                    }
                }
            } * click_scale;
            let idx_l = center_idx_l as isize + l_offset;
            let idx_r = center_idx_r as isize + r_offset;
            match noise_type {
                0 => {
                    for k in 0..tick_len {
                        let k_44k = k as f64 / sr_ratio;
                        let env = (-5.5 * k_44k / tick_len_44k).exp();
                        let shaped = base_amp * env * click_sign;
                        let pos = (k * n_channels as usize) as isize;
                        let l_pos = idx_l + pos;
                        let r_pos = idx_r + pos;
                        if l_pos >= 0 && (l_pos as usize) < resampled.len() {
                            let val = resampled[l_pos as usize] + shaped;
                            resampled[l_pos as usize] = val.clamp(-1.0, 1.0);
                        }
                        if n_channels > 1 && r_pos >= 0 && (r_pos as usize) < resampled.len() {
                            let val = resampled[r_pos as usize] + shaped;
                            resampled[r_pos as usize] = val.clamp(-1.0, 1.0);
                        }
                    }
                }
                1 => {
                    for k in 0..pop_len {
                        let k_44k = k as f64 / sr_ratio;
                        let t = (k_44k / pop_len_44k).min(1.0);
                        let env = (1.0 - t).powf(2.2);
                        let shaped = base_amp * env * click_sign;
                        let pos = (k * n_channels as usize) as isize;
                        let l_pos = idx_l + pos;
                        let r_pos = idx_r + pos;
                        if l_pos >= 0 && (l_pos as usize) < resampled.len() {
                            let val = resampled[l_pos as usize] + shaped;
                            resampled[l_pos as usize] = val.clamp(-1.0, 1.0);
                        }
                        if n_channels > 1 && r_pos >= 0 && (r_pos as usize) < resampled.len() {
                            let val = resampled[r_pos as usize] + shaped;
                            resampled[r_pos as usize] = val.clamp(-1.0, 1.0);
                        }
                    }
                }
                _ => {
                    for k in 0..thump_len {
                        let k_44k = k as f64 / sr_ratio;
                        let t = (k_44k / thump_len_44k).min(1.0);
                        let body = (1.0 - t).powf(1.6);
                        let ripple = 1.0 - (t * 8.0).sin().abs() * 0.12;
                        let env = body * ripple;
                        let shaped = base_amp * env * click_sign;
                        let pos = (k * n_channels as usize) as isize;
                        let l_pos = idx_l + pos;
                        let r_pos = idx_r + pos;
                        if l_pos >= 0 && (l_pos as usize) < resampled.len() {
                            let val = resampled[l_pos as usize] + shaped;
                            resampled[l_pos as usize] = val.clamp(-1.0, 1.0);
                        }
                        if n_channels > 1 && r_pos >= 0 && (r_pos as usize) < resampled.len() {
                            let val = resampled[r_pos as usize] + shaped;
                            resampled[r_pos as usize] = val.clamp(-1.0, 1.0);
                        }
                    }
                }
            }
        }
        progress_eta_range_if(i, 2_000_000, total_steps, label_r_noise, 60.0, 100.0, start_time_r_noise)?;
    }
    update_progress(label_r_noise, 100.0, 100.0)?;
    Ok(())
}
