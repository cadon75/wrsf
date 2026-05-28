use crossterm::{
    cursor::{Hide, MoveTo, Show},
    execute,
    terminal::{Clear, ClearType},
};
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use rand::Rng;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageLevel};
use std::{
    env,
    error::Error,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

// REM: エラー処理
type AppResult<T> = Result<T, Box<dyn Error>>;
fn show_message(text: &str) {
    eprintln!("Error: {}", text);
}

fn check_terminal() {
    if !io::stdout().is_terminal() {
        MessageDialog::new()
            .set_level(MessageLevel::Error)
            .set_title("Error")
            .set_description("ターミナル内で起動して下さい。\n(Please launch it within the terminal.)")
            .set_buttons(MessageButtons::Ok)
            .show();
        std::process::exit(0);
    }
}

// REM: ファイル選択 (ここだけファイル選択のGUIダイアログが出ます。利便性とった結果)
// REM: Linux では GUIダイアログは、ユーザーの環境に libgtk-3-dev が 必要になる事アリ
fn select_file(mode: &str) -> AppResult<String> {
    let path = if mode == "open" {
        FileDialog::new().add_filter("WAV audio", &["wav"]).set_title("Select WAV file").pick_file()
    } else {
        FileDialog::new().add_filter("WAV audio", &["wav"]).set_title("Save processed WAV file").set_file_name("output.wav").save_file()
    };
    match path {
        Some(p) => Ok(p.display().to_string()),
        None => Err("File selection cancelled.".into()),
    }
}

// REM: プログレス表示
fn update_progress(label: &str, current: f64, total: f64) -> AppResult<()> {
    if total <= 0.0 {
        return Ok(());
    }
    let mut current_val = current;
    if current_val > total {
        current_val = total;
    }
    let percent = (current_val / total) * 100.0;
    let row = cursor_row()?;
    execute!(io::stdout(), MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
    print!("{}: {:>6.1}%", label, percent);
    io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
    Ok(())
}

// REM: 現在のカーソル行を取得
fn cursor_row() -> AppResult<u16> {
    crossterm::cursor::position().map(|(_, row)| row).map_err(|e| Box::new(e) as Box<dyn Error>)
}

// REM: RMS
fn rms_change(samples: &mut [f64], _n_channels: u16, label: &str, start_p: f64, end_p: f64, start_time_rms: Option<Instant>) -> AppResult<()> {
    let total_samples = samples.len();
    if total_samples == 0 {
        return Ok(());
    }
    let mut sum_sq = 0.0;
    let range_percent = end_p - start_p;
    for (i, &sample) in samples.iter().enumerate() {
        sum_sq += sample * sample;
        if i % 250_000 == 0 {
            let current_percent = start_p + (i as f64 / total_samples as f64) * (range_percent * 0.4);
            let mut msg = format!("{}: {:>6.1}%", label, current_percent);
            if let Some(start_time) = start_time_rms {
                let elapsed = start_time.elapsed().as_secs_f64();
                if elapsed > 5.0 && current_percent > 0.0 {
                    let remaining = (elapsed / current_percent) * (100.0 - current_percent);
                    msg += &format!(" ({:.0} seconds remaining)", remaining.max(0.0));
                }
            }
            let row = cursor_row()?;
            execute!(io::stdout(), MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
            print!("{}", msg);
            io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
        }
    }

    let rms_current = (sum_sq / total_samples as f64).sqrt();
    let target_rms = 0.1;
    let rms_floor = 10.0_f64.powf(-90.0 / 20.0); // REM : 爆音化対策
    let rms_gain = if rms_current > rms_floor { target_rms / rms_current } else { 1.0 };
    for (i, sample) in samples.iter_mut().enumerate() {
        *sample *= rms_gain;
        if i % 250_000 == 0 {
            let current_percent = start_p + (range_percent * 0.4) + (i as f64 / total_samples as f64) * (range_percent * 0.6);
            let mut msg = format!("{}: {:>6.1}%", label, current_percent);
            if let Some(start_time) = start_time_rms {
                let elapsed = start_time.elapsed().as_secs_f64();
                if elapsed > 5.0 && current_percent > 0.0 {
                    let remaining = (elapsed / current_percent) * (100.0 - current_percent);
                    msg += &format!(" ({:.0} seconds remaining)", remaining.max(0.0));
                }
            }
            let row = cursor_row()?;
            execute!(io::stdout(), MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
            print!("{}", msg);
            io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
        }
    }
    update_progress(label, end_p, 100.0)?;
    Ok(())
}

// REM: 2次フィルタ係数
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

// REM: スピーカーフィルタ用
fn speakerfilter_core(
    ch_samples: &mut [f64],
    _n_channels: u16,
    _channel_index: usize,
    hpf_coeffs: &[f64],
    eq_f: &[(f64, f64, f64, f64, f64)],
    lpf_coeffs: &[f64],
    progress_list: Arc<Mutex<Vec<usize>>>,
    thread_idx: usize,
) {
    let mut hpf_w = [0.0, 0.0];
    let mut eq_w: Vec<[f64; 2]> = (0..eq_f.len()).map(|_| [0.0, 0.0]).collect();
    let mut lpf_w = [[0.0, 0.0]; 2];
    let total_len = ch_samples.len();
    for i in 0..total_len {
        let mut v = ch_samples[i];
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
        ch_samples[i] = v * 1.35;

        if i % 10_000 == 0 {
            if let Ok(mut list) = progress_list.lock() {
                list[thread_idx] = i;
            }
        }
    }
    if let Ok(mut list) = progress_list.lock() {
        list[thread_idx] = total_len;
    }
}

// REM: 汎用入力
fn check_yn(question_ja: &str, question_en: &str, default_yes: bool) -> AppResult<bool> {
    let yn_suffix = if default_yes { " [Y/n]: " } else { " [y/N]: " };
    let prompt = format!("\n{} ({}){}", question_ja, question_en, yn_suffix);
    loop {
        io::stdout().write_all(prompt.as_bytes()).map_err(|e| Box::new(e) as Box<dyn Error>)?;
        io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
        let mut usr_in1 = String::new();
        io::stdin().read_line(&mut usr_in1).map_err(|e| Box::new(e) as Box<dyn Error>)?;
        let usr_in1 = usr_in1.trim().to_lowercase();
        let res = if usr_in1.is_empty() {
            default_yes
        } else if usr_in1 == "y" || usr_in1 == "1" {
            true
        } else if usr_in1 == "n" || usr_in1 == "2" {
            false
        } else {
            continue; // REM: 無効な入力の場合は再入力
        };
        println!("Selected : {}", if res { "Yes" } else { "No" });
        return Ok(res);
    }
}

// REM: 再生特性フィルタ選択入力
fn select_consumer_filter() -> AppResult<String> {
    println!("再生特性フィルタを選択して下さい\n(Please select the Playback Character Filter)");
    println!("1. Balanced Retro Hi-Fi  (default)");
    println!("2. Muffled  Retro Lo-Fi");
    println!("3. Bright   Retro Hi-Fi");
    println!("4. RF        CRT-style");
    println!("5. Composite CRT-style");

    loop {
        let mut usr_input2 = String::new();
        io::stdin().read_line(&mut usr_input2)?;

        let usr_input2 = usr_input2.trim().to_string();

        if usr_input2.is_empty() {
            return Ok("1".to_string());
        }

        if ["1", "2", "3", "4", "5"].contains(&usr_input2.as_str()) {
            return Ok(usr_input2);
        }
    }
}

// REM: WAV処理のメイン（ほぼ本体）
fn wav_process_main(arg_input_path: Option<PathBuf>, arg_mode: Option<String>) -> AppResult<()> {
    let arg_in_active = arg_input_path.is_some();
    let in_path_buf = if let Some(p) = arg_input_path {
        p
    } else {
        println!("読み込むwavファイルを選択して下さい");
        println!("(Please enter the path to the wav file to load)");
        let in_path_str = select_file("open")?;
        if in_path_str.is_empty() {
            return Ok(());
        }
        PathBuf::from(in_path_str)
    };

    let in_path = in_path_buf.as_path();
    if in_path.extension().map_or(true, |ext| ext.to_ascii_lowercase() != "wav") {
        show_message("Only WAV files are supported");
        return Ok(());
    }

    let mut reader = match WavReader::open(in_path) {
        Ok(r) => r,
        Err(e) => {
            show_message(&format!("Only WAV files are supported or file could not be opened: {}", e));
            return Ok(());
        }
    };

    let spec = reader.spec();
    let n_channels = spec.channels;
    let sampwidth = match spec.bits_per_sample {
        8 => 1,
        16 => 2,
        24 => 3,
        32 => 4,
        _ => {
            show_message("Unsupported WAV bits per sample.");
            return Ok(());
        }
    };
    let framerate = spec.sample_rate;
    let _n_frames = reader.len() as u32 / (sampwidth as u32 * n_channels as u32);

    if n_channels != 2 {
        show_message("Only 2ch-Stereo is supported");
        return Ok(());
    }
    if ![22050, 32000, 44100, 48000, 88200, 96000].contains(&framerate) {
        show_message("Unsupported WAV freq");
        return Ok(());
    }

    println!("\nReading file...\n");
    let mut samples: Vec<f64> = Vec::new();

    match spec.sample_format {
        SampleFormat::Int => {
            let max_val: f64 = match spec.bits_per_sample {
                16 => 32767.0,
                24 => 8388607.0,
                32 => 2147483647.0,
                _ => {
                    show_message("Unsupported integer bit depth.");
                    return Ok(());
                }
            };

            for s in reader.samples::<i32>() {
                let raw_s = s? as f64;
                let mut val = raw_s / max_val;
                if val.abs() > 1.0 {
                    val = raw_s / 2147483647.0;
                }
                samples.push(val.clamp(-1.0, 1.0));
            }
        }
        SampleFormat::Float => {
            for s in reader.samples::<f32>() {
                samples.push(s? as f64);
            }
        }
    }

    // REM: ユーザーによる処理選択セクションのループ開始
    let (mut do_sec2, mut do_sec3, mut do_sec4, mut do_sec5, mut do_sec6);
    let (mut out_freq, mut out_bit);
    let mut clip_protect;
    let mut do_sec2_p1 = "1".to_string();

    loop {
        // REM: ユーザー入力を受け付けるためカーソルを表示
        execute!(io::stdout(), Show).map_err(|e| Box::new(e) as Box<dyn Error>)?;

        if !arg_in_active {
            println!("どのフィルタ処理を行いますか？ (Which filter processing do you want to perform?)\n");
            println!("1. 基本フィルタ (Standard processing)");
            println!("2. 全フィルタ   (All Filters)");
            println!("3. 詳細設定     (Advanced settings)");
        }

        let selected_no = if let Some(mode) = arg_mode.clone() {
            mode
        } else {
            loop {
                let mut u_in = String::new();
                io::stdin().read_line(&mut u_in)?;
                let u_in = u_in.trim().to_string();
                if u_in.is_empty() {
                    break "1".to_string();
                }
                if ["1", "2", "3"].contains(&u_in.as_str()) {
                    break u_in;
                }
            }
        };

        if !arg_in_active {
            println!("Selected : {}", selected_no);
            println!("");
        }

        if selected_no == "1" || selected_no == "2" {
            if arg_in_active {
                do_sec2_p1 = "1".to_string();
            } else {
                do_sec2_p1 = select_consumer_filter()?;
                println!("Selected : {}", do_sec2_p1);
                println!("");
            }
        }

        // REM: 機能無効化用
        // REM: do_sec2 = false;
        // REM: do_sec3 = false;
        // REM: do_sec4 = false;
        // REM: do_sec5 = false;
        // REM: do_sec6 = false;

        match selected_no.as_str() {
            "1" => {
                do_sec2 = true;
                do_sec3 = true;
                do_sec4 = false;
                do_sec5 = true;
                do_sec6 = true;
            }
            "2" => {
                do_sec2 = true;
                do_sec3 = true;
                do_sec4 = true;
                do_sec5 = true;
                do_sec6 = true;
            }
            "3" => {
                // REM: 詳細設定処理
                println!("\n注意：詳細設定では組み合わせ次第で結果がおかしくなる可能性があります。\n（Note: Depending on the combination of settings used in the advanced settings, the results may be unexpected.）\n");
                do_sec2 = check_yn("再生特性フィルタを適用しますか？", "Do you want to apply the Playback Character Filter?", true)?;
                if do_sec2 {
                    do_sec2_p1 = select_consumer_filter()?;
                    println!("Selected : {}\n", do_sec2_p1);
                }
                do_sec3 = check_yn("カセットテープ フィルタを適用しますか？", "Do you want to apply the Cassette Tape Filter?", true)?;
                do_sec4 = check_yn("レコードノイズを付加しますか？", "Do you want to add Record Noise?", false)?;
                do_sec5 = check_yn("アナログ特性フィルタを適用しますか？", "Do you want to apply the Analog Characteristics Filter?", true)?;
                do_sec6 = check_yn("スピーカー フィルタを適用しますか？", "Do you want to apply the Speaker Filter?", true)?;
            }
            _ => unreachable!(),
        }

        // REM: 出力ファイル形式の選択
        if !arg_in_active {
            println!("\n出力するWavファイル形式を選択して下さい (Please select the output Wav file format)\n");
            println!("1. 入力されたwavと同じ形式 (Same format as input wav) {}KHz / {}bit / Stereo", framerate as f64 / 1000.0, sampwidth * 8);
            println!("2. 96KHz / 24bit / Stereo");
            println!("3. 48KHz / 16bit / Stereo");
        }

        let u_fmt = loop {
            let mut u_fmt_str = String::new();
            if arg_in_active {
                u_fmt_str = "1".to_string();
            } else {
                io::stdin().read_line(&mut u_fmt_str)?;
            }

            let u_fmt_str = u_fmt_str.trim().to_string();
            if u_fmt_str.is_empty() {
                break "1".to_string();
            }
            if ["1", "2", "3"].contains(&u_fmt_str.as_str()) {
                break u_fmt_str;
            }
        };

        match u_fmt.as_str() {
            "1" => {
                out_freq = framerate;
                out_bit = sampwidth;
            }
            "2" => {
                out_freq = 96000;
                out_bit = 3;
            }
            _ => {
                out_freq = 48000;
                out_bit = 2;
            }
        }

        // REM: 周波数表示
        if arg_in_active {
            println!("Wave file format (Read / Save) : {}KHz - {}bit - {}ch", out_freq as f64 / 1000.0, out_bit * 8, n_channels);
        } else {
            println!("Selected : {}KHz - {}bit - {}ch", out_freq as f64 / 1000.0, out_bit * 8, n_channels);
            println!("");
        }

        // REM: スピーカーフィルタが有効な場合のみクリッピング対策の選択肢を表示
        clip_protect = false;
        if do_sec6 {
            if arg_in_active {
            } else {
                println!("\nクリッピング対策を行いますか？（時間がかかる場合があります）");
                println!("(Do you want to perform clipping protection? (It may take some time))");
            }
            loop {
                if arg_in_active {
                } else {
                    io::stdout().write_all(b" [Y/n]: ").map_err(|e| Box::new(e) as Box<dyn Error>)?;
                    io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
                }
                let mut u_cp = String::new();
                if arg_in_active {
                    u_cp = "y".to_string();
                } else {
                    io::stdin().read_line(&mut u_cp).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                }

                let u_cp = u_cp.trim().to_lowercase();

                if u_cp.is_empty() || u_cp == "y" || u_cp == "1" {
                    clip_protect = true;
                    if arg_in_active {
                    } else {
                        println!("Selected : Yes");
                        println!("");
                    }
                    break;
                } else if u_cp == "n" || u_cp == "2" {
                    clip_protect = false;
                    if arg_in_active {
                    } else {
                        println!("Selected : No");
                        println!("");
                    }
                    break;
                }
            }
        }

        // REM: 対話選択時は必ず開始確認を行う
        let start_confirm = if arg_in_active {
            true
        } else {
            check_yn("処理を開始しますか？", "Do you want to start processing?", true)?
        };
        if !start_confirm {
            println!("\n");
            continue; // REM: 一番最初に戻る
        }

        // REM: フィルタが一つも選択されていない場合の安全な終了
        if !(do_sec2 || do_sec3 || do_sec4 || do_sec5 || do_sec6) {
            println!("No processing was performed.");
            return Ok(());
        }

        // REM: ループを抜けて処理を開始
        break;
    }

    // REM: 以降、処理のためカーソルを非表示
    execute!(io::stdout(), Hide).map_err(|e| Box::new(e) as Box<dyn Error>)?;

    println!("\n\n<< Wav Retro Sound Filter v0.2.1 >>");

    // REM: Section 1. 初期処理（リサンプリング & RMS）
    // REM: 入力されたwavを下処理
    println!("\n");
    let label_s1 = "Preparing & Pre-processing… ";

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
                let s0 = frame[ch];
                let s1 = next_frame[ch];
                let interp = s0 + (s1 - s0) * t;
                resampled.push(interp);
            }
        }

        if i % 100_000 == 0 {
            let current_percent = (i as f64 / num_frames as f64) * 70.0;
            update_progress(label_s1, current_percent, 100.0)?;
        }
    }
    rms_change(&mut resampled, n_channels, label_s1, 70.0, 100.0, None)?;
    update_progress(label_s1, 100.0, 100.0)?;

    let new_fs = framerate * 4;
    let dt = 1.0 / new_fs as f64;
    let total_res = resampled.len();

    // REM: Section 2. 再生特性フィルタ（再生環境・時代感の簡易表現）
    if do_sec2 {
        println!("\n");
        // REM: あまり根拠がない雑な設定です。1次フィルタなので雰囲気。両端減衰弱め。
        // REM: 本来は2次フィルタを使ったり専用を作るべきですが重いので。
        // REM: m2v等で作製したレトロチップ作製曲や昔の曲は大体1でいいですが
        // REM: 高域や倍音が強いレトロチップ作製曲・現代的なマスタの曲は2がおすすめ
        let (hpf_fc, lpf_fc) = match do_sec2_p1.as_str() {
            "1" => (8.0, 7992.0),
            "2" => (16.0, 3390.0),
            "3" => (10.0, 8991.0),
            "4" => (100.0, 4995.0),
            "5" => (40.0, 5994.0),
            _ => (8.0, 7992.0),
        };
        let filters = [(hpf_fc, "HPF"), (lpf_fc, "LPF")];
        let start_time_s2 = Instant::now();
        let label_s2 = "Processing Playback Character Filter… ";
        for (idx_f, &(fc, label_f)) in filters.iter().enumerate() {
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

                if i % 100_000 == 0 {
                    let current_percent = (idx_f as f64 * 33.3) + (i as f64 / total_steps as f64) * 33.3;
                    let elapsed = start_time_s2.elapsed().as_secs_f64();
                    let mut msg = format!("{}: {:>6.1}%", label_s2, current_percent);
                    if elapsed > 5.0 && current_percent > 0.0 {
                        let remaining = (elapsed / current_percent) * (100.0 - current_percent);
                        msg += &format!(" ({:.0} seconds remaining)", remaining.max(0.0));
                    }
                    let row = cursor_row()?;
                    execute!(io::stdout(), MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                    print!("{}", msg);
                    io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
                }
            }
        }
        rms_change(&mut resampled, n_channels, label_s2, 66.6, 100.0, Some(start_time_s2))?;
        update_progress(label_s2, 100.0, 100.0)?;
    }

    // REM: Section 3. カセットテープフィルタ（ギャップ損失＆テープ記録周波数＆S/N比）
    if do_sec3 {
        println!("\n");
        let label_s3 = "Processing Cassette Tape Filter… ";
        let start_time_s3 = Instant::now();
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

            if i % 250_000 == 0 {
                let current_percent = (i as f64 / total_steps as f64) * 25.0;
                let elapsed = start_time_s3.elapsed().as_secs_f64();
                let mut msg = format!("{}: {:>6.1}%", label_s3, current_percent);
                if elapsed > 5.0 && current_percent > 0.0 {
                    let remaining = (elapsed / current_percent) * (100.0 - current_percent);
                    msg += &format!(" ({:.0} seconds remaining)", remaining.max(0.0));
                }
                let row = cursor_row()?;
                execute!(io::stdout(), MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                print!("{}", msg);
                io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
            }
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

                if i % 250_000 == 0 {
                    let current_percent = 25.0 + (idx_f as f64 * 25.0) + (i as f64 / total_steps as f64) * 25.0;
                    let elapsed = start_time_s3.elapsed().as_secs_f64();
                    let mut msg = format!("{}: {:>6.1}%", label_s3, current_percent);
                    if elapsed > 5.0 && current_percent > 0.0 {
                        let remaining = (elapsed / current_percent) * (100.0 - current_percent);
                        msg += &format!(" ({:.0} seconds remaining)", remaining.max(0.0));
                    }
                    let row = cursor_row()?;
                    execute!(io::stdout(), MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                    print!("{}", msg);
                    io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
                }
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

            if i % 250_000 == 0 {
                let current_percent = 75.0 + (i as f64 / total_steps as f64) * 25.0;
                let elapsed = start_time_s3.elapsed().as_secs_f64();
                let mut msg = format!("{}: {:>6.1}%", label_s3, current_percent);
                if elapsed > 5.0 && current_percent > 0.0 {
                    let remaining = (elapsed / current_percent) * (100.0 - current_percent);
                    msg += &format!(" ({:.0} seconds remaining)", remaining.max(0.0));
                }
                let row = cursor_row()?;
                execute!(io::stdout(), MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                print!("{}", msg);
                io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
            }
        }
        update_progress(label_s3, 100.0, 100.0)?;
    }

    // REM: Section 4. レコードノイズ付加（摺り音＋プチプチ）
    if do_sec4 {
        println!("\n");
        let label_s4 = "Processing Record Noise Filter… ";
        let start_time_s4 = Instant::now();
        let total_steps = total_res / n_channels as usize;
        let mut s4_rng = rand::thread_rng();

        // REM: 4-1. RMS解析
        let mut s4_rms_acc = 0.0_f64;
        for (idx, &s) in resampled.iter().enumerate() {
            s4_rms_acc += s * s;
            if idx % 500_000 == 0 {
                let current_percent = (idx as f64 / resampled.len() as f64) * 20.0;
                let elapsed = start_time_s4.elapsed().as_secs_f64();
                let mut msg = format!("{}: {:>6.1}%", label_s4, current_percent);
                if elapsed > 5.0 && current_percent > 0.0 {
                    let remaining = (elapsed / current_percent) * (100.0 - current_percent);
                    msg += &format!(" ({:.0} seconds remaining)", remaining.max(0.0));
                }
                let row = cursor_row()?;
                execute!(io::stdout(), MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                print!("{}", msg);
                io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
            }
        }
        let mut s4_rms = (s4_rms_acc / resampled.len().max(1) as f64).sqrt();

        // REM: 無音入力時の保護
        let rms_floor = 10.0_f64.powf(-90.0 / 20.0);
        if s4_rms < rms_floor {
            s4_rms = rms_floor;
        }
        let loudness_db = 20.0 * s4_rms.log10();

        // REM: 4-2. レコード摺りノイズ
        // REM: 実効-65dB前後狙いだけど入力されたwavに左右されまふ
        let expected_max_p = resampled.iter().map(|&s| s.abs()).fold(0.0_f64, |a, b| a.max(b));
        let peak_floor = 10.0_f64.powf(-90.0 / 20.0);
        let mut expected_final_gain = if expected_max_p > peak_floor { 10.0_f64.powf(-1.0 / 20.0) / expected_max_p } else { 1.0 };
        if do_sec6 {
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

        // REM: ノイズを生成してリサンプリングしてのせる
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
                    let white_low = s4_rng.gen_range(-1.0..1.0) * low_gain * low_sr_scale;
                    low_state[ch] = (1.0 - low_a) * white_low + low_a * low_state[ch];
                    // REM: 高域ヒス
                    let white_hiss = s4_rng.gen_range(-1.0..1.0) * hiss_gain * hiss_sr_scale;
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

            if i % 100_000 == 0 {
                let current_percent = 20.0 + ((i as f64 / total_steps as f64) * 40.0);
                let elapsed = start_time_s4.elapsed().as_secs_f64();
                let mut msg = format!("{}: {:>6.1}%", label_s4, current_percent);
                if elapsed > 5.0 && current_percent > 0.0 {
                    let remaining = (elapsed / current_percent) * (100.0 - current_percent);
                    msg += &format!(" ({:.0} seconds remaining)", remaining.max(0.0));
                }
                let row = cursor_row()?;
                execute!(io::stdout(), MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                print!("{}", msg);
                io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
            }
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
            if s4_rng.gen_bool(click_probability) {
                let center_idx_l = i * n_channels as usize;
                let center_idx_r = center_idx_l + 1;
                let stereo_offset = ((0.000010 * new_fs as f64) as isize).max(1);
                let l_offset = s4_rng.gen_range(-stereo_offset..=stereo_offset) * n_channels as isize;
                let r_offset = s4_rng.gen_range(-stereo_offset..=stereo_offset) * n_channels as isize;
                let noise_roll = s4_rng.gen_range(0.0..1.0);

                let noise_type = if noise_roll < 0.72 {
                    0
                } else if noise_roll < 0.94 {
                    1
                } else {
                    2
                };

                let strong_noise = s4_rng.gen_bool(0.06);
                let click_sign = if s4_rng.gen_bool(0.5) { 1.0 } else { -1.0 };

                let base_amp = match noise_type {
                    0 => {
                        if strong_noise {
                            s4_rng.gen_range(0.010..0.024)
                        } else {
                            s4_rng.gen_range(0.0020..0.0060)
                        }
                    }
                    1 => {
                        if strong_noise {
                            s4_rng.gen_range(0.018..0.045)
                        } else {
                            s4_rng.gen_range(0.0045..0.0125)
                        }
                    }
                    _ => {
                        if strong_noise {
                            s4_rng.gen_range(0.028..0.065)
                        } else {
                            s4_rng.gen_range(0.008..0.018)
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
                            if r_pos >= 0 && (r_pos as usize) < resampled.len() {
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
                            if r_pos >= 0 && (r_pos as usize) < resampled.len() {
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
                            if r_pos >= 0 && (r_pos as usize) < resampled.len() {
                                let val = resampled[r_pos as usize] + shaped;
                                resampled[r_pos as usize] = val.clamp(-1.0, 1.0);
                            }
                        }
                    }
                }
            }

            if i % 100_000 == 0 {
                let current_percent = 60.0 + ((i as f64 / total_steps as f64) * 40.0);
                let elapsed = start_time_s4.elapsed().as_secs_f64();
                let mut msg = format!("{}: {:>6.1}%", label_s4, current_percent);
                if elapsed > 5.0 && current_percent > 0.0 {
                    let remaining = (elapsed / current_percent) * (100.0 - current_percent);
                    msg += &format!(" ({:.0} seconds remaining)", remaining.max(0.0));
                }
                let row = cursor_row()?;
                execute!(io::stdout(), MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                print!("{}", msg);
                io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
            }
        }
        update_progress(label_s4, 100.0, 100.0)?;
    }

    // REM: Section 5. アナログ特性フィルタ　（アナログっぽさを出すカナメ）
    let mut s5_rng = rand::thread_rng();
    if do_sec5 {
        println!("\n");
        let label_s5 = "Processing Analog Characteristics Filter… ";
        let start_time_s5 = Instant::now();
        let s5_total_steps = total_res / n_channels as usize;

        // REM: 5-1. TPDF 16bit 約-93dB / 24・32bit 約-120dB　左右独立
        // REM: このパートに対策を入れれば砂嵐生成は止まるはずですが面白いのｄ（
        let dither_amp = match out_bit {
            2 => 10.0_f64.powf(-93.0 / 20.0),
            3 | 4 => 10.0_f64.powf(-120.0 / 20.0),
            _ => 10.0_f64.powf(-93.0 / 20.0),
        };

        for i in 0..s5_total_steps {
            let idx_l = i * 2;
            let idx_r = idx_l + 1;
            // REM: 左右独立TPDF
            let tpdf_l = (s5_rng.gen_range(-1.0..1.0) + s5_rng.gen_range(-1.0..1.0)) * 0.5 * dither_amp;
            let tpdf_r = (s5_rng.gen_range(-1.0..1.0) + s5_rng.gen_range(-1.0..1.0)) * 0.5 * dither_amp;
            resampled[idx_l] += tpdf_l;
            resampled[idx_r] += tpdf_r;

            if i % 100_000 == 0 {
                let current_percent = (i as f64 / s5_total_steps as f64) * 50.0;
                let elapsed = start_time_s5.elapsed().as_secs_f64();
                let mut msg = format!("{}: {:>6.1}%", label_s5, current_percent);
                if elapsed > 5.0 && current_percent > 0.0 {
                    let remaining = (elapsed / current_percent) * (100.0 - current_percent);
                    msg += &format!(" ({:.0} seconds remaining)", remaining.max(0.0));
                }
                let row = cursor_row()?;
                execute!(io::stdout(), MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                print!("{}", msg);
                io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
            }
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

        for i in 0..s5_total_steps {
            let idx_l = i * 2;
            let idx_r = idx_l + 1;
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
            resampled[idx_r] = low_out_r + mid_r + high_out_r;

            if i % 100_000 == 0 {
                let current_percent = 50.0 + (i as f64 / s5_total_steps as f64) * 50.0;
                let elapsed = start_time_s5.elapsed().as_secs_f64();
                let mut msg = format!("{}: {:>6.1}%", label_s5, current_percent);
                if elapsed > 5.0 && current_percent > 0.0 {
                    let remaining = (elapsed / current_percent) * (100.0 - current_percent);
                    msg += &format!(" ({:.0} seconds remaining)", remaining.max(0.0));
                }
                let row = cursor_row()?;
                execute!(io::stdout(), MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                print!("{}", msg);
                io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
            }
        }
        update_progress(label_s5, 100.0, 100.0)?;
    }

    // REM: Section 6. スピーカー周波数特性フィルタ（RC-M70のスピーカー特性参考）
    if do_sec6 {
        let backup_s6 = resampled.clone();
        let label_s6 = "Processing Speaker Filter… ";

        let mut current_n_gain: Option<f64> = None;
        for attempt in 0..2 {
            println!("\n");
            if attempt == 0 {
                let mut cur_max = 0.0;
                let resampled_len = resampled.len() as f64;
                for (i, &s) in resampled.iter().enumerate() {
                    if s.abs() > cur_max {
                        cur_max = s.abs();
                    }
                    if i % 1_000_000 == 0 {
                        let current_percent = (i as f64 / resampled_len) * 2.5;
                        update_progress(label_s6, current_percent, 100.0)?;
                    }
                }
                current_n_gain = Some(if cur_max > 0.0 { 10.0_f64.powf(-4.0 / 20.0) / cur_max } else { 1.0 });
            }

            let resampled_len = resampled.len() as f64;
            for (i, s) in resampled.iter_mut().enumerate() {
                if let Some(gain) = current_n_gain {
                    *s *= gain;
                }
                if attempt == 0 && i % 1000000 == 0 {
                    let current_percent = 2.5 + (i as f64 / resampled_len) * 2.5;
                    update_progress(label_s6, current_percent, 100.0)?;
                }
            }

            let start_time_s5 = Instant::now();
            let spk_omega = 2.0 * std::f64::consts::PI * 85.0 / new_fs as f64;
            let spk_sn = spk_omega.sin();
            let spk_cs = spk_omega.cos();
            let spk2 = spk_sn / (2.0 * 0.707);
            let hpf_coeffs_s5 = [
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
            let lpf_coeffs_s5 = [
                lpf_spk_m0 / lpf_spk_k1,
                lpf_spk_m1 / lpf_spk_k1,
                lpf_spk_m2 / lpf_spk_k1,
                lpf_spk_k2 / lpf_spk_k1,
                lpf_spk_k3 / lpf_spk_k1,
            ];

            let hpf_coeffs_s5_arc = Arc::new(hpf_coeffs_s5);
            let eq_spk_arc = Arc::new(eq_spk_vec);
            let lpf_coeffs_arc = Arc::new(lpf_coeffs_s5);
            let mut channel_data: Vec<Vec<f64>> = (0..n_channels as usize).map(|ch| resampled.iter().skip(ch).step_by(n_channels as usize).cloned().collect()).collect();
            let progress_list = Arc::new(Mutex::new(vec![0; n_channels as usize]));
            let mut handles = Vec::new();

            for ch_idx in 0..n_channels as usize {
                let mut ch_samples_cloned = channel_data[ch_idx].clone(); // REM: スレッドに所有権を渡すためにクローン
                let hpf_coeffs_clone_thread = Arc::clone(&hpf_coeffs_s5_arc);
                let eq_spk_clone_thread = Arc::clone(&eq_spk_arc);
                let lpf_coeffs_clone_thread = Arc::clone(&lpf_coeffs_arc);
                let progress_list_arc = Arc::clone(&progress_list);
                let n_channels_worker = n_channels;

                let handle = thread::spawn(move || {
                    speakerfilter_core(
                        &mut ch_samples_cloned,
                        n_channels_worker,
                        ch_idx,
                        &hpf_coeffs_clone_thread[..],
                        &eq_spk_clone_thread[..],
                        &lpf_coeffs_clone_thread[..],
                        progress_list_arc,
                        ch_idx,
                    );
                    ch_samples_cloned
                });
                handles.push(handle);
            }

            let total_ch_samples = channel_data[0].len();
            while handles.iter().any(|h| !h.is_finished()) {
                let current_sum: usize = if let Ok(list) = progress_list.lock() { list.iter().sum() } else { 0 };
                let total_sum = total_ch_samples * n_channels as usize;
                let percent_in_f = (current_sum as f64 / total_sum as f64) * 100.0;
                let total_percent = 5.0 + (percent_in_f * 0.95);
                let elapsed_filter = start_time_s5.elapsed().as_secs_f64();
                let mut msg = format!("{}: {:>6.1}%", label_s6, total_percent);
                if elapsed_filter > 5.0 && percent_in_f > 0.0 {
                    let remaining = (elapsed_filter / percent_in_f) * (100.0 - percent_in_f);
                    msg += &format!(" ({:.0} seconds remaining)", remaining.max(0.0));
                }
                let row = cursor_row()?;
                execute!(io::stdout(), MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                print!("{}", msg);
                io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
                thread::sleep(Duration::from_millis(100));
            }

            for (ch_idx, handle) in handles.into_iter().enumerate() {
                match handle.join() {
                    Ok(result) => {
                        channel_data[ch_idx] = result;
                    }
                    Err(_) => {
                        return Err("Speaker filter thread panicked.".into());
                    }
                }
            }

            // REM: クリッピングチェック
            let mut max_p = 0.0;
            for ch in 0..n_channels as usize {
                for &val in &channel_data[ch] {
                    if val.abs() > max_p {
                        max_p = val.abs();
                    }
                }
            }

            // REM: クリッピング対策が有効時にチェック
            // REM: 下処理しているので中々発生しないと思うものの組み合わせ次第の予防線
            if clip_protect && max_p > 1.0 && attempt == 0 {
                println!("\nreprocessing... ");
                if let Some(ref mut gain) = current_n_gain {
                    *gain *= 0.99 / max_p;
                }
                resampled = backup_s6.clone(); // REM: リトライ復元
                continue;
            }

            for i in 0..total_ch_samples {
                for ch in 0..n_channels as usize {
                    resampled[i * n_channels as usize + ch] = channel_data[ch][i];
                }
            }
            break; // REM: 処理が完了したらループを抜ける
        }
        update_progress(label_s6, 100.0, 100.0)?;
    }

    // REM: Section 7. 最終処理 リサンプリング＋ノーマライズ（ピークを-1dBに調整）
    println!("\n");
    let prog_label = "Finalizing Resample & Normalization… ";

    let ratio = new_fs as f64 / out_freq as f64;
    let target_len_frames = (resampled.len() as f64 / n_channels as f64 / ratio) as usize;
    let mut final_samples: Vec<f64> = Vec::with_capacity(target_len_frames * n_channels as usize);

    for i in 0..target_len_frames {
        let pos = i as f64 * ratio;
        let idx = pos as usize;
        let frac = pos - idx as f64;
        for ch in 0..n_channels as usize {
            let s1 = resampled[idx * n_channels as usize + ch];
            let s2 = if (idx + 1) * n_channels as usize + ch < resampled.len() {
                resampled[(idx + 1) * n_channels as usize + ch]
            } else {
                s1
            };
            final_samples.push(s1 + frac * (s2 - s1));
        }
        if i % 100_000 == 0 {
            let current_percent = (i as f64 / target_len_frames as f64) * 50.0;
            update_progress(prog_label, current_percent, 100.0)?;
        }
    }
    resampled = final_samples;
    let mut max_p = 0.0;
    let resampled_len = resampled.len() as f64;
    for (i, &s) in resampled.iter().enumerate() {
        if s.abs() > max_p {
            max_p = s.abs();
        }
        if i % 1_000_000 == 0 {
            update_progress(prog_label, 50.0 + (i as f64 / resampled_len) * 25.0, 100.0)?;
        }
    }
    let peak_floor = 10.0_f64.powf(-90.0 / 20.0); // REM: 爆音化対策
    if max_p > peak_floor {
        let final_gain = 10.0_f64.powf(-1.0 / 20.0) / max_p;
        let resampled_len = resampled.len() as f64;
        for (i, s) in resampled.iter_mut().enumerate() {
            *s *= final_gain;
            if i % 1_000_000 == 0 {
                update_progress(prog_label, 75.0 + (i as f64 / resampled_len) * 25.0, 100.0)?;
            }
        }
    }
    update_progress(prog_label, 100.0, 100.0)?;

    // REM: Section 8. 保存処理 (自動命名・上書き確認)
    println!("\n");
    let base_name = in_path.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let ext_name = in_path.extension().and_then(|s| s.to_str()).unwrap_or("wav");
    let output_file_name = format!("{}_Processed.{}", base_name, ext_name);
    let output_dir = in_path.parent().unwrap_or_else(|| Path::new("."));
    let output_path: PathBuf = output_dir.join(output_file_name);

    if output_path.exists() {
        execute!(io::stdout(), Show).map_err(|e| Box::new(e) as Box<dyn Error>)?;
        if let Some(name) = output_path.file_name().and_then(|s| s.to_str()) {
            println!("{}", name);
        } else {
            println!("output file");
        }
        if arg_in_active {
            println!("\nForce overwrite and save...");
        } else {
            io::stdout().write_all(b"File already exists. Overwrite? [Y/n]: ").map_err(|e| Box::new(e) as Box<dyn Error>)?;
            io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
            let mut ans_s8 = String::new();
            io::stdin().read_line(&mut ans_s8).map_err(|e| Box::new(e) as Box<dyn Error>)?;
            execute!(io::stdout(), Hide).map_err(|e| Box::new(e) as Box<dyn Error>)?;
            if ans_s8.trim().to_lowercase() == "n" {
                println!("\nSaving cancelled. \n\nProcessing finished. The program will now exit.\n");
                return Ok(());
            }
        }
    }
    println!("\nSaving file...\nOutput Path: {}", output_path.display());

    let output_spec = WavSpec {
        channels: n_channels,
        sample_rate: out_freq,
        bits_per_sample: match out_bit {
            2 => 16,
            3 => 24,
            4 => 32,
            _ => 16,
        },
        sample_format: if out_bit == 4 && spec.sample_format == SampleFormat::Float {
            SampleFormat::Float
        } else {
            SampleFormat::Int
        },
    };

    match WavWriter::create(output_path, output_spec) {
        Ok(mut writer) => {
            match output_spec.sample_format {
                SampleFormat::Int => {
                    let max_val = match output_spec.bits_per_sample {
                        16 => 32767.0,
                        24 => 8388607.0,
                        32 => 2147483647.0,
                        _ => 32767.0,
                    };
                    for s in resampled {
                        let clamped = s.clamp(-1.0, 1.0);
                        writer.write_sample((clamped * max_val).round() as i32).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                    }
                }
                SampleFormat::Float => {
                    for s in resampled {
                        let clamped = s.clamp(-1.0, 1.0);
                        writer.write_sample(clamped as f32).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                    }
                }
            }
            writer.flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
            println!("Saving completed. \n\nProcessing finished. The program will now exit.\n");
        }
        Err(e) => {
            println!("Error occurred during saving: {} \n\nThe program will now exit.\n", e);
        }
    }

    Ok(())
}

fn main() -> AppResult<()> {
    check_terminal();

    // REM: コマンドライン引数解析
    // REM: 引数ありの挙動は表示しないで入力を内部で入れてるだけなので
    // REM: 挙動を変えたい場合は増設なりの対処が必要
    let mut arg_input_path: Option<PathBuf> = None;
    let mut cl_mode: Option<String> = None;

    for arg in env::args().skip(1) {
        // REM: Linux系で先頭及び最後に付与される場合があるクォート除去
        let arg = if arg.len() >= 2 && ((arg.starts_with('\'') && arg.ends_with('\'')) || (arg.starts_with('"') && arg.ends_with('"'))) {
            arg[1..arg.len() - 1].to_string()
        } else {
            arg.to_string()
        };

        match arg.as_str() {
            "-1" => {
                cl_mode = Some("1".to_string());
            }
            "-2" => {
                cl_mode = Some("2".to_string());
            }
            _ => {
                if arg_input_path.is_none() {
                    arg_input_path = Some(PathBuf::from(&arg));
                } else {
                    println!("\nError: コマンドラインの引数が不正です。(Invalid argument) \n{} \n\nThe program will now exit.\n", arg);
                    return Ok(());
                }
            }
        }
    }

    // REM: オプション未指定時は-1扱い
    if arg_input_path.is_some() && cl_mode.is_none() {
        cl_mode = Some("1".to_string());
    }

    let result = wav_process_main(arg_input_path, cl_mode);

    // REM: 処理終了時にカーソルを表示に戻す
    execute!(io::stdout(), Show).map_err(|e| Box::new(e) as Box<dyn Error>)?;
    io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;

    if let Err(e) = result {
        eprintln!("\nProcessing interrupted or error\n {}\n", e);
    }
    Ok(())
}
