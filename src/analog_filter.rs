use crate::progress::{progress_eta_range_if, update_progress};
use rand::Rng;
use std::{error::Error, time::Instant};

// REM: アナログ特性フィルタ　（アナログっぽさを出すカナメ）
pub fn analog_filter(resampled: &mut [f64], total_res: usize, n_channels: u16, out_bit: u16, dt: f64) -> Result<(), Box<dyn Error>> {
    eprintln!("\n");
    let mut analogf_rng = rand::thread_rng();
    let label_analogf = "Processing Analog Characteristics Filter… ";
    let start_time_analogf = Instant::now();
    let analogf_total_steps = total_res / n_channels as usize;

    // REM: 5-1. TPDF 16bit 約-93dB / 24・32bit 約-114dB　左右独立
    // REM: このパートに対策を入れれば砂嵐生成は止まるはずですが面白いのｄ（
    let dither_amp = match out_bit {
        2 => 10.0_f64.powf(-93.0 / 20.0),
        3 | 4 => 10.0_f64.powf(-114.0 / 20.0),
        _ => 10.0_f64.powf(-93.0 / 20.0),
    };
    for i in 0..analogf_total_steps {
        let idx_l = i * n_channels as usize;
        // REM: 左右独立TPDF
        let tpdf_l = (analogf_rng.gen_range(-1.0..1.0) + analogf_rng.gen_range(-1.0..1.0)) * 0.5 * dither_amp;
        resampled[idx_l] += tpdf_l;
        if n_channels > 1 {
            let idx_r = idx_l + 1;
            let tpdf_r = (analogf_rng.gen_range(-1.0..1.0) + analogf_rng.gen_range(-1.0..1.0)) * 0.5 * dither_amp;
            resampled[idx_r] += tpdf_r;
        }
        progress_eta_range_if(i, 2_000_000, analogf_total_steps, label_analogf, 0.0, 50.0, start_time_analogf)?;
    }

    // REM: 5-2. クロストーク
    // REM: 低域側は2.0%狭まる 700Hz付近で0% 14KHz付近で3.16%広がる
    // REM: 当時の機器のマニュアルから大体で算出した雑な値と簡易処理
    let low_fc = 700.0;
    let high_fc = 14000.0;
    let low_rc = 1.0 / (2.0 * std::f64::consts::PI * low_fc);
    let high_rc = 1.0 / (2.0 * std::f64::consts::PI * high_fc);
    let low_alpha = dt / (low_rc + dt);
    let high_alpha = high_rc / (high_rc + dt);
    // REM: 低域 LPF 状態
    let mut low_l = 0.0;
    let mut low_r = 0.0;
    // REM: 高域 HPF 状態
    let mut high_prev_in_l = 0.0;
    let mut high_prev_in_r = 0.0;
    let mut high_prev_out_l = 0.0;
    let mut high_prev_out_r = 0.0;
    for i in 0..analogf_total_steps {
        let idx_l = i * 2;
        let idx_r = idx_l + if n_channels > 1 { 1 } else { 0 };
        let in_l = resampled[idx_l];
        let in_r = resampled[idx_r];
        // REM: 低域分離
        low_l += low_alpha * (in_l - low_l);
        low_r += low_alpha * (in_r - low_r);
        let low_band_l = low_l;
        let low_band_r = low_r;
        // REM: 高域分離
        let high_l = high_alpha * (high_prev_out_l + in_l - high_prev_in_l);
        let high_r = high_alpha * (high_prev_out_r + in_r - high_prev_in_r);
        high_prev_in_l = in_l;
        high_prev_in_r = in_r;
        high_prev_out_l = high_l;
        high_prev_out_r = high_r;
        // REM: 中域
        let mid_l = in_l - low_band_l - high_l;
        let mid_r = in_r - low_band_r - high_r;
        // REM: クロストークの量
        let low_mix = 0.02;
        let high_mix = -0.0316;
        // REM: 低域
        let low_out_l = low_band_l + (low_band_r - low_band_l) * low_mix;
        let low_out_r = low_band_r + (low_band_l - low_band_r) * low_mix;
        // REM: 高域
        let high_out_l = high_l + (high_r - high_l) * high_mix;
        let high_out_r = high_r + (high_l - high_r) * high_mix;
        // REM: 結合
        resampled[idx_l] = low_out_l + mid_l + high_out_l;
        if n_channels > 1 {
            resampled[idx_r] = low_out_r + mid_r + high_out_r;
        }
        progress_eta_range_if(i, 2_000_000, analogf_total_steps, label_analogf, 50.0, 100.0, start_time_analogf)?;
    }
    update_progress(label_analogf, 100.0, 100.0)?;
    Ok(())
}
