use crate::progress::{progress_eta_range_if, progress_range_if, update_progress};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use std::{
    error::Error,
    io::{self, IsTerminal},
    time::Instant,
};

// REM: 分類が微妙なものを入れておく所

// REM: エラー処理
pub fn show_message(text: &str) {
    eprintln!("Error: {}", text);
}

pub fn check_terminal() {
    if !io::stderr().is_terminal() {
        MessageDialog::new()
            .set_level(MessageLevel::Error)
            .set_title("Error")
            .set_description(
                "ターミナル内で起動して下さい。\n(Please launch it within the terminal.)",
            )
            .set_buttons(MessageButtons::Ok)
            .show();
        std::process::exit(0);
    }
}

// REM: ファイル選択 (ここだけファイル選択のGUIダイアログが出ます。利便性とった結果)
// REM: Linux では GUIダイアログは、ユーザーの環境に libgtk-3-dev が 必要になる事アリ
pub fn select_file(mode: &str) -> Result<String, Box<dyn Error>> {
    let path = if mode == "open" {
        FileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .set_title("Select WAV file")
            .pick_file()
    } else {
        FileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .set_title("Save processed WAV file")
            .set_file_name("output.wav")
            .save_file()
    };
    match path {
        Some(p) => Ok(p.display().to_string()),
        None => Err("File selection cancelled.".into()),
    }
}

// REM: RMS
pub fn rms_change(
    samples: &mut [f64],
    _n_channels: u16,
    label: &str,
    start_p: f64,
    end_p: f64,
    start_time_rms: Option<Instant>,
) -> Result<(), Box<dyn Error>> {
    let total_samples = samples.len();
    if total_samples == 0 {
        return Ok(());
    }

    let mut sum_sq = 0.0;
    let mid_percent = start_p + ((end_p - start_p) * 0.5);

    for (i, &sample) in samples.iter().enumerate() {
        sum_sq += sample * sample;

        if let Some(start_time) = start_time_rms {
            progress_eta_range_if(
                i,
                2_000_000,
                total_samples,
                label,
                start_p,
                mid_percent,
                start_time,
            )?;
        } else {
            progress_range_if(i, 2_000_000, total_samples, label, start_p, mid_percent)?;
        }
    }

    let rms_current = (sum_sq / total_samples as f64).sqrt();
    let target_rms = 0.1;
    let rms_floor = 10.0_f64.powf(-90.0 / 20.0);
    let rms_gain = if rms_current > rms_floor {
        target_rms / rms_current
    } else {
        1.0
    };

    for (i, sample) in samples.iter_mut().enumerate() {
        *sample *= rms_gain;

        if let Some(start_time) = start_time_rms {
            progress_eta_range_if(
                i,
                2_000_000,
                total_samples,
                label,
                mid_percent,
                end_p,
                start_time,
            )?;
        } else {
            progress_range_if(i, 2_000_000, total_samples, label, mid_percent, end_p)?;
        }
    }

    update_progress(label, end_p, 100.0)?;
    Ok(())
}

pub fn check_wav_memory_usage(samples_len: usize) -> Result<bool, Box<dyn Error>> {
    let est_max_bytes = samples_len as f64 * 44.4 + 52_428_880.0; // REM: 約50MBはソフト本体の雑な使用量
    let mb = est_max_bytes / 1_048_576.0;
    let (disp_val, unit) = if mb >= 1024.0 {
        (mb / 1024.0, "GB")
    } else {
        (mb, "MB")
    };
    let val_trunc = (disp_val * 100.0).trunc() / 100.0;
    eprintln!(
        "\nEstimated maximum memory usage: ~{:.2}{}\n",
        val_trunc, unit
    );

    // REM: 予想が 6GB（6 * 1024 = 6144MB）を超えたら一応の安全措置で終了
    // REM: 96KHzで約12分半、44.1KHzで約27分くらいです。ファイルサイズより周波数と時間です
    // REM: オンメモリ動作なので。ブロック処理にするの面ｄ（
    if mb > 6144.0 {
        show_message("Estimated maximum memory usage limit exceeded (6GB safety threshold).\nTry shortening the playback time of the WAV file or lowering the frequency.\nOperation aborted.");
        return Ok(false);
    }
    Ok(true)
}
