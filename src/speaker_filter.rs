use crate::progress::{progress_eta, progress_range_if, set_thread_progress, thread_progress_if, update_progress};
use std::{
    error::Error,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

fn calc_peaking(fs: f64, freq: f64, q: f64, db_gain: f64) -> (f64, f64, f64, f64, f64) {
    let f2k0 = 10.0_f64.powf(db_gain / 40.0);
    let f2o = 2.0 * std::f64::consts::PI * freq / fs;
    let f2sn = f2o.sin();
    let f2cs = f2o.cos();
    let f2a = f2sn / (2.0 * q);
    let f2m0 = 1.0 + f2a * f2k0;
    let f2m1 = -2.0 * f2cs;
    let f2m2 = 1.0 - f2a * f2k0;
    let f2k1 = 1.0 + f2a / f2k0;
    let f2k2 = -2.0 * f2cs;
    let f2k3 = 1.0 - f2a / f2k0;
    (f2m0 / f2k1, f2m1 / f2k1, f2m2 / f2k1, f2k2 / f2k1, f2k3 / f2k1)
}

fn speakerfilter_core(
    resampled_ptr: *mut f64,
    total_samples: usize,
    n_channels: u16,
    channel_index: usize,
    hpf_coeffs: &[f64],
    eq_f: &[(f64, f64, f64, f64, f64)],
    lpf_coeffs: &[f64],
    progress_list: Arc<Mutex<Vec<usize>>>,
    thread_idx: usize,
) {
    let mut hpf_w = [0.0, 0.0];
    let mut eq_w: Vec<[f64; 2]> = (0..eq_f.len()).map(|_| [0.0, 0.0]).collect();
    let mut lpf_w = [[0.0, 0.0]; 2];
    let step = n_channels as usize;
    let mut i = channel_index;
    let mut sample_count = 0;

    // REM: チャンネルのインデックス（Lなら0, 2, 4... Rなら1, 3, 5...）で直接ループ
    while i < total_samples {
        // REM: unsafe LとRでポインタが衝突することは絶対にない
        let mut v = unsafe { *resampled_ptr.add(i) };
        let h1 = hpf_coeffs[0] * v + hpf_w[0];
        hpf_w[0] = hpf_coeffs[1] * v - hpf_coeffs[3] * h1 + hpf_w[1];
        hpf_w[1] = hpf_coeffs[2] * v - hpf_coeffs[4] * h1;
        v = h1;
        for (k, f) in eq_f.iter().enumerate() {
            let e1 = f.0 * v + eq_w[k][0];
            eq_w[k][0] = f.1 * v - f.3 * e1 + eq_w[k][1];
            eq_w[k][1] = f.2 * v - f.4 * e1;
            v = e1;
        }
        for lpf1 in 0..2 {
            let l1 = lpf_coeffs[0] * v + lpf_w[lpf1][0];
            lpf_w[lpf1][0] = lpf_coeffs[1] * v - lpf_coeffs[3] * l1 + lpf_w[lpf1][1];
            lpf_w[lpf1][1] = lpf_coeffs[2] * v - lpf_coeffs[4] * l1;
            v = l1;
        }
        // REM: 結果を元のメモリへ直接書き戻す
        unsafe { *resampled_ptr.add(i) = v };
        thread_progress_if(sample_count, 1_000_000, &progress_list, thread_idx);
        sample_count += 1;
        i += step; // REM: L..R..と飛ばす
    }
    set_thread_progress(&progress_list, thread_idx, sample_count);
}

// REM: スピーカー周波数特性フィルタ（RC-M70のスピーカー特性参考）
pub fn speaker_filter(resampled: &mut Vec<f64>, new_fs: u32, n_channels: u16, clip_protect: bool) -> Result<(), Box<dyn Error>> {
    let label_spkf = "Processing Speaker Filter… ";
    let resampled_len = resampled.len();
    eprintln!("\n");
    let mut cur_max = 0.0;
    for (i, &s) in resampled.iter().enumerate() {
        if s.abs() > cur_max {
            cur_max = s.abs();
        }
        progress_range_if(i, 2_000_000, resampled_len, label_spkf, 0.0, 2.5)?;
    }
    let current_n_gain = Some(if cur_max > 0.0 { 10.0_f64.powf(-4.0 / 20.0) / cur_max } else { 1.0 });
    for (i, s) in resampled.iter_mut().enumerate() {
        if let Some(gain) = current_n_gain {
            *s *= gain;
        }
        progress_range_if(i, 2_000_000, resampled_len, label_spkf, 2.5, 5.0)?;
    }

    let start_time_spkf = Instant::now();
    let spk_omega = 2.0 * std::f64::consts::PI * 85.0 / new_fs as f64;
    let spk_sn = spk_omega.sin();
    let spk_cs = spk_omega.cos();
    let spk2 = spk_sn / (2.0 * 0.707);
    let hpf_coeffs_spkf = [
        (1.0 + spk_cs) / 2.0 / (1.0 + spk2),
        -(1.0 + spk_cs) / (1.0 + spk2),
        (1.0 + spk_cs) / 2.0 / (1.0 + spk2),
        -2.0 * spk_cs / (1.0 + spk2),
        (1.0 - spk2) / (1.0 + spk2),
    ];
    let eq_set = [
        (200.0, 1.0, 0.5),
        (400.0, 1.0, 0.0),
        (600.0, 2.0, -1.5),
        (800.0, 2.0, -2.0),
        (1000.0, 1.5, -1.0),
        (2000.0, 2.5, -2.5),
        (3000.0, 1.5, -1.2),
        (4000.0, 1.2, 0.5),
        (5000.0, 1.0, 1.8),
        (6000.0, 1.0, 2.2),
        (7000.0, 1.2, 2.0),
        (8000.0, 1.5, 1.5),
        (9000.0, 2.0, -2.0),
        (10000.0, 2.0, -5.0),
    ];
    let eq_spk_vec: Vec<(f64, f64, f64, f64, f64)> = eq_set.iter().map(|&(f, q, g)| calc_peaking(new_fs as f64, f, q, g)).collect();
    let lpf_spk_omega = 2.0 * std::f64::consts::PI * 10500.0 / new_fs as f64;
    let lpf_spk_sn = lpf_spk_omega.sin();
    let lpf_spk_cs = lpf_spk_omega.cos();
    let lpf_spk_k0 = lpf_spk_sn / (2.0 * 0.707);
    let lpf_spk_m0 = (1.0 - lpf_spk_cs) / 2.0;
    let lpf_spk_m1 = 1.0 - lpf_spk_cs;
    let lpf_spk_m2 = (1.0 - lpf_spk_cs) / 2.0;
    let lpf_spk_k1 = 1.0 + lpf_spk_k0;
    let lpf_spk_k2 = -2.0 * lpf_spk_cs;
    let lpf_spk_k3 = 1.0 - lpf_spk_k0;
    let lpf_coeffs_spkf = [
        lpf_spk_m0 / lpf_spk_k1,
        lpf_spk_m1 / lpf_spk_k1,
        lpf_spk_m2 / lpf_spk_k1,
        lpf_spk_k2 / lpf_spk_k1,
        lpf_spk_k3 / lpf_spk_k1,
    ];
    let hpf_coeffs_spkf_arc = Arc::new(hpf_coeffs_spkf);
    let lpf_coeffs_spkf_arc = Arc::new(lpf_coeffs_spkf);
    let eq_spk_spkf_arc = Arc::new(eq_spk_vec);
    let resampled_ptr = resampled.as_mut_ptr() as usize;
    let total_samples = resampled.len();
    let progress_list = Arc::new(Mutex::new(vec![0; n_channels as usize]));
    let mut handles = Vec::new();

    for ch_idx in 0..n_channels as usize {
        let hpf_coeffs_spkf_clone = Arc::clone(&hpf_coeffs_spkf_arc);
        let eq_spk_spkf_clone = Arc::clone(&eq_spk_spkf_arc);
        let lpf_coeffs_spkf_clone = Arc::clone(&lpf_coeffs_spkf_arc);
        let progress_list_clone = Arc::clone(&progress_list);
        let n_channels_worker = n_channels;
        let handle = thread::spawn(move || {
            let ptr = resampled_ptr as *mut f64;
            speakerfilter_core(
                ptr,
                total_samples,
                n_channels_worker,
                ch_idx,
                &hpf_coeffs_spkf_clone[..],
                &eq_spk_spkf_clone[..],
                &lpf_coeffs_spkf_clone[..],
                progress_list_clone,
                ch_idx,
            );
        });
        handles.push(handle);
    }
    let total_ch_samples = resampled.len() / n_channels as usize;
    while handles.iter().any(|h| !h.is_finished()) {
        let current_sum: usize = if let Ok(list) = progress_list.lock() { list.iter().sum() } else { 0 };
        let total_sum = total_ch_samples * n_channels as usize;
        let percent_in_f = (current_sum as f64 / total_sum as f64) * 100.0;
        let total_percent = 5.0 + (percent_in_f * 0.95);
        progress_eta(label_spkf, total_percent, start_time_spkf)?;
        thread::sleep(Duration::from_millis(50));
    }
    for handle in handles {
        handle.join().map_err(|_| "Speaker filter thread panicked.")?;
    }
    update_progress(label_spkf, 100.0, 100.0)?;

    /* REM: クリッピングテストコード
    (f64 仮数有効精度 53bit/約320dB / 表現可能レンジ 約6160db (20 log10(1e308)) とは別)
    これで異常なクリッピング（スピーカーに音量上げ上げ的な）が作れますが
    +40dB程度（100倍）とか過大入力すると、
    補正後耳で僅かな変化として認識できる場合アリ（多分 誤差約0.32%）
    f64精度自体は十分高いですが、他フィルタ処理や量子化の累積誤差は発生します
    耳が鋭い方で+6db～10db辺りまでが知覚できない範囲（多分part2）
    +6dbでも通常使用で中々発生しないと思うので実用範囲としてます。
    ↓ +14db (x5) */
    /*
    for val in resampled.iter_mut() {
        *val *= 5.0;
    }
    */

    // REM: クリッピング対策が有効時にクリッピングチェック
    let mut max_p: f64 = 0.0;
    if clip_protect {
        for &val in resampled.iter() {
            if val.abs() > max_p {
                max_p = val.abs();
            }
        }
        // REM: クリッピング発生時ゲイン下げ
        if max_p > 1.0 {
            let db_over = 20.0 * max_p.log10();
            eprintln!("\n\nThere was clipping (+{:.2} dB over). Correcting...", db_over);
            let gain_reduction = 0.99 / max_p;
            for val in resampled.iter_mut() {
                *val *= gain_reduction;
            }
        }
    }
    Ok(())
}
