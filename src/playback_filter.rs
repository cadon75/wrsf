use crate::etc_process::rms_change;
use crate::progress::{progress_eta_range_if, update_progress};
use std::time::Instant;

// REM: 再生特性フィルタ（再生環境・時代感の簡易表現）
pub fn playback_filter(resampled: &mut [f64], n_channels: u16, dt: f64, do_playback_sel_01: &str, total_res: usize) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("\n");
    // REM: あまり根拠がない雑な設定です。1次フィルタなので雰囲気。両端減衰弱め。
    // REM: 大体は1でいいですが、高域や倍音が強いレトロチップ作製曲・現代的なマスタの曲は2がおすすめ
    let (hpf_fc, lpf_fc) = match do_playback_sel_01 {
        "1" => (10.0, 7992.0),
        "2" => (16.0, 3390.0),
        "3" => (10.0, 9000.0),
        "4" => (100.0, 4994.0),
        "5" => (40.0, 5994.0),
        _ => (10.0, 7992.0),
    };
    let hl_filters = [(hpf_fc, "HPF"), (lpf_fc, "LPF")];
    let start_time_playbf = Instant::now();
    let label_playbf = "Processing Playback Character Filter… ";
    for (idx_f, &(fc, label_f)) in hl_filters.iter().enumerate() {
        let rc = 1.0 / (2.0 * std::f64::consts::PI * fc);
        let alpha = if label_f == "HPF" { rc / (rc + dt) } else { dt / (rc + dt) };
        let mut p_x = vec![0.0; n_channels as usize];
        let mut p_y = vec![0.0; n_channels as usize];
        let total_steps = total_res / n_channels as usize;
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
            let phase_start = idx_f as f64 * 33.3;
            let phase_end = phase_start + 33.3;
            progress_eta_range_if(i, 2_000_000, total_steps, label_playbf, phase_start, phase_end, start_time_playbf)?;
        }
    }
    rms_change(resampled, n_channels, label_playbf, 66.6, 100.0, Some(start_time_playbf))?;
    update_progress(label_playbf, 100.0, 100.0)?;
    Ok(())
}
