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
        if i % 250000 == 0 {
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
    let rms_gain = if rms_current > 0.0 { target_rms / rms_current } else { 0.0 };

    for (i, sample) in samples.iter_mut().enumerate() {
        *sample *= rms_gain;
        if i % 250000 == 0 {
            let current_percect = start_p + (range_percent * 0.4) + (i as f64 / total_samples as f64) * (range_percent * 0.6);
            let mut msg = format!("{}: {:>6.1}%", label, current_percect);
            if let Some(start_time) = start_time_rms {
                let elapsed = start_time.elapsed().as_secs_f64();
                if elapsed > 5.0 && current_percect > 0.0 {
                    let remaining = (elapsed / current_percect) * (100.0 - current_percect);
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

        if i % 10000 == 0 {
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

// REM: コンシューマーフィルタ選択入力
fn select_consumer_filter() -> AppResult<String> {
    println!("コンシューマーフィルタはどれを使用しますか？\n(Which consumer filter will you use?)");
    println!("1. MD   like");
    println!("2. PC88 like");
    println!("3. X68  like");
    println!("4. NES  (RF) like");
    println!("5. SNES (Composite) like");

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
    if ![16000, 22050, 32000, 44100, 48000, 88200, 96000].contains(&framerate) {
        show_message("Unsupported WAV freq");
        return Ok(());
    }

    println!("\nReading file...\n");
    let mut samples: Vec<f64> = Vec::new();

    match spec.sample_format {
        SampleFormat::Int => {
            let max_val: f64 = match spec.bits_per_sample {
                16 => 32768.0,
                24 => 8388608.0,
                32 => 2147483648.0,
                _ => {
                    show_message("Unsupported integer bit depth.");
                    return Ok(());
                }
            };

            for s in reader.samples::<i32>() {
                samples.push(s? as f64 / max_val);
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
            println!("1. 基本フィルタ   (Standard processing)");
            println!("2. コンシューマー + カセット フィルタのみ  (processing Consumer + Cassette Tape Filter)");
            println!("3. 詳細設定 (Advanced settings)");
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

        do_sec5 = false; // REM: 無効化中は有効に

        // REM: do_sec6 = false;

        match selected_no.as_str() {
            "1" => {
                do_sec2 = true;
                do_sec3 = true;
                do_sec4 = false;
                do_sec5 = false;
                do_sec6 = true;
            }
            "2" => {
                do_sec2 = true;
                do_sec3 = true;
                do_sec4 = false;
                do_sec5 = false;
                do_sec6 = false;
            }
            "3" => {
                // REM: 詳細設定処理
                do_sec2 = check_yn("コンシューマー フィルタを適用しますか？", "Do you want to apply the Consumer Filter?", true)?;
                if do_sec2 {
                    do_sec2_p1 = select_consumer_filter()?;
                    println!("Selected : {}", do_sec2_p1);
                    println!("");
                }
                do_sec3 = check_yn(
                    "カセットテープ フィルタを適用しますか？",
                    "Do you want to apply the Cassette Tape Filter?",
                    true,
                )?;
                do_sec4 = check_yn("ノイズを付加しますか？", "Do you want to add noise?", false)?;
                // REM: 無効化中 do_sec5 = check_yn("ヘッドフォン半挿しプラグ効果 フィルタ（ネタ）を適用しますか？\n", "Do you want to apply the Half-Plug in headphones Filter? (incomplete)", false)?;
                do_sec6 = check_yn("スピーカー フィルタを適用しますか？", "Do you want to apply the Speaker Filter?", true)?;
            }
            _ => unreachable!(),
        }

        // REM: 出力ファイル形式の選択
        if !arg_in_active {
            println!("\n出力するWavファイル形式を選択して下さい (Please select the output Wav file format)\n");
            println!(
                "1. 入力されたwavと同じ形式 (Same format as input wav) {}KHz / {}bit / Stereo",
                framerate as f64 / 1000.0,
                sampwidth * 8
            );
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
            println!(
                "Wave file format (Read / Save) : {}KHz - {}bit - {}ch",
                out_freq as f64 / 1000.0,
                out_bit * 8,
                n_channels
            );
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
                    io::stdout().write_all(b" [y/N]: ").map_err(|e| Box::new(e) as Box<dyn Error>)?;
                    io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
                }
                let mut u_cp = String::new();
                if arg_in_active {
                    u_cp = "n".to_string();
                } else {
                    io::stdin().read_line(&mut u_cp).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                }

                let u_cp = u_cp.trim().to_lowercase();

                if u_cp == "y" || u_cp == "1" {
                    clip_protect = true;
                    if arg_in_active {
                    } else {
                        println!("Selected : Yes");
                        println!("");
                    }
                    break;
                } else if u_cp.is_empty() || u_cp == "n" || u_cp == "2" {
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

    println!("\n\n<< Wav Retro Sound Filter v0.1 >>");

    // REM: Section 1. 初期処理（リサンプリング & RMS）
    // REM: 入力されたwavを下処理する場所
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

        if i % 100000 == 0 {
            let current_percent = (i as f64 / num_frames as f64) * 70.0;
            update_progress(label_s1, current_percent, 100.0)?;
        }
    }

    rms_change(&mut samples, n_channels, label_s1, 70.0, 100.0, None)?;

    update_progress(label_s1, 100.0, 100.0)?;

    let new_fs = framerate * 4;
    let dt = 1.0 / new_fs as f64;
    let total_res = resampled.len();

    // REM: Section 2. コンシューマーフィルタ（色々な機種の簡易フィルタ）
    if do_sec2 {
        println!("\n");
        // REM: あまり根拠がない雑な設定です。1次フィルタなので雰囲気。両端減衰弱め。
        // REM: 本来は2次フィルタを使ったり専用を作るべきですが重いので。
        let (hpf_fc, lpf_fc) = match do_sec2_p1.as_str() {
            "1" => (16.0, 3390.0),
            "2" => (8.0, 7992.0),
            "3" => (10.0, 8991.0),
            "4" => (100.0, 4995.0),
            "5" => (40.0, 5994.0),
            _ => (16.0, 3390.0),
        };
        let filters = [(hpf_fc, "HPF"), (lpf_fc, "LPF")];
        let start_time_s2 = Instant::now();
        let label_s2 = "Processing Consumer Filter… ";
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
                if i % 100000 == 0 {
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

        // REM: ギャップ損失
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
            if i % 250000 == 0 {
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

        // REM: テープ記録周波数簡易処理
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
                if i % 250000 == 0 {
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

        // REM: S/N比再現処理
        let sn_high = 10.0_f64.powf(-60.0 / 20.0);
        let sn_low = 10.0_f64.powf(-96.0 / 20.0);
        for i in 0..total_steps {
            for ch in 0..n_channels as usize {
                let idx = i * n_channels as usize + ch;
                let val = resampled[idx];
                let abs_v = val.abs();
                if abs_v < sn_low {
                    resampled[idx] = 0.0;
                } else if abs_v < sn_high {
                    // REM: -60dBから-96dBの間で線形補間的にゲインを落とす
                    let gain = (abs_v - sn_low) / (sn_high - sn_low);
                    resampled[idx] *= gain;
                }
            }
            if i % 250000 == 0 {
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

    // REM: Section 4. ノイズフィルタ (カセットテープ摺りノイズ的雰囲気用)
    if do_sec4 {
        println!("\n");
        let label_s4 = "Processing Noise Filter… ";
        let start_time_s4 = Instant::now();
        let mut cur_max = 0.0;
        let resampled_len = resampled.len() as f64;
        for (i, &s) in resampled.iter().enumerate() {
            if s.abs() > cur_max {
                cur_max = s.abs();
            }
            if i % 1000000 == 0 {
                let current_percent = (i as f64 / resampled_len) * 10.0;
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
        let n_gain = if cur_max > 0.0 { 10.0_f64.powf(-4.0 / 20.0) / cur_max } else { 1.0 };
        let resampled_len = resampled.len() as f64;
        for (i, s) in resampled.iter_mut().enumerate() {
            *s *= n_gain;
            if i % 1000000 == 0 {
                let current_percent = 10.0 + (i as f64 / resampled_len) * 10.0;
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
        let wn_amp = 10.0_f64.powf(-72.0 / 20.0); // REM: 強いノイズが好きなら-60辺りからお試しを
        let lpf_alpha = 0.15;
        let mut prev_wn = vec![0.0; n_channels as usize];
        let total_steps = total_res / n_channels as usize;
        let mut s4rng = rand::thread_rng();
        for i in 0..total_steps {
            for ch in 0..n_channels as usize {
                let idx = i * n_channels as usize + ch;
                let white = s4rng.gen_range(-1.0..1.0) * wn_amp;
                let filtered_wn = prev_wn[ch] + lpf_alpha * (white - prev_wn[ch]);
                resampled[idx] += filtered_wn;
                prev_wn[ch] = filtered_wn;
            }
            if i % 100000 == 0 {
                let current_percent = 20.0 + (i as f64 / total_steps as f64) * 80.0;
                let elapsed = start_time_s4.elapsed().as_secs_f64();
                let mut msg = format!("{}: {:>6.1}%", label_s4, current_percent);
                if elapsed > 5.0 && current_percent > 0.0 {
                    let remaining = (elapsed / current_percent) * (100.0 - current_percent);
                    msg += &format!("  ({:.0} seconds remaining)", remaining.max(0.0));
                }
                let row = cursor_row()?;
                execute!(io::stdout(), MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                print!("{}", msg);
                io::stdout().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
            }
        }
        update_progress(label_s4, 100.0, 100.0)?;
    }

    // REM: Section 5. ヘッドフォンプラグ半挿し効果フィルタ
    // REM:（ネタフィルタ/効果不完全かつ元のwavに左右されるので非推奨＆無効化中）
    // REM: (現象の原理は単純ですが、本当に再現するには回路再現並になるかも)
    /* REM: 無効化中
    let mut s5_rng = rand::thread_rng();
    if do_sec5 {
        println!("\n");
        let label_s5 = "Processing Half-Plug Headphones Filter… ";
        let s5_total_steps = total_res / n_channels as usize;

        // REM: MIX値
        const HALF_PLUG_MIX: f64 = 0.76;
        // REM: 元音保持量
        const HALF_DRY_GAIN: f64 = 0.73;
        // REM: 位相遅延(ms)
        const HALF_DELAY_MS: f64 = 0.17;

        let half_delay = ((out_freq as f64 / 1000.0) * HALF_DELAY_MS) as usize;
        let delay_offset = half_delay * 2;
        let mut mix_jitter = HALF_PLUG_MIX;

        for i in 0..s5_total_steps {
            let idx_l = i * n_channels as usize;
            let idx_r = idx_l + 1;
            if idx_r >= resampled.len() {
                break;
            }

            if i % 2048 == 0 {
                mix_jitter = HALF_PLUG_MIX + s5_rng.gen_range(-0.02..0.02);
            }

            let l = resampled[idx_l];
            let r = resampled[idx_r];

            // REM: 位相差処理
            let delayed_r = if idx_r >= delay_offset { resampled[idx_r - delay_offset] } else { r };
            let delayed_l = if idx_l >= delay_offset { resampled[idx_l - delay_offset] } else { l };

            resampled[idx_l] = (l * HALF_DRY_GAIN) - (delayed_r * mix_jitter);
            resampled[idx_r] = (r * HALF_DRY_GAIN) - (delayed_l * mix_jitter);

            if i % 100000 == 0 {
                let current_percent = (i as f64 / s5_total_steps as f64) * 100.0;
                update_progress(label_s5, current_percent, 100.0)?;
            }
        }

        update_progress(label_s5, 100.0, 100.0)?;
    } */

    // REM: Section 6. ラジカセスピーカー周波数特性フィルタ（RC-M70のスピーカー特性参考）
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
                    if i % 1000000 == 0 {
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

            let mut channel_data: Vec<Vec<f64>> =
                (0..n_channels as usize).map(|ch| resampled.iter().skip(ch).step_by(n_channels as usize).cloned().collect()).collect();

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
                let current_sum: usize = progress_list.lock().unwrap().iter().sum();
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
                channel_data[ch_idx] = handle.join().unwrap();
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
        if i % 100000 == 0 {
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
        if i % 1000000 == 0 {
            update_progress(prog_label, 50.0 + (i as f64 / resampled_len) * 25.0, 100.0)?;
        }
    }
    if max_p > 0.0 {
        let final_gain = 10.0_f64.powf(-1.0 / 20.0) / max_p;
        let resampled_len = resampled.len() as f64;
        for (i, s) in resampled.iter_mut().enumerate() {
            *s *= final_gain;
            if i % 1000000 == 0 {
                update_progress(prog_label, 75.0 + (i as f64 / resampled_len) * 25.0, 100.0)?;
            }
        }
    }
    update_progress(prog_label, 100.0, 100.0)?;

    // REM: Section 8. 保存処理 (自動命名・上書き確認)
    println!("\n");
    let base_name = in_path.file_stem().unwrap().to_str().unwrap();
    let ext_name = in_path.extension().unwrap().to_str().unwrap();
    let output_file_name = format!("{}_Processed.{}", base_name, ext_name);

    let output_path: PathBuf = in_path.parent().unwrap_or(Path::new(".")).join(output_file_name);

    if output_path.exists() {
        execute!(io::stdout(), Show).map_err(|e| Box::new(e) as Box<dyn Error>)?;
        println!("{}", output_path.file_name().unwrap().to_str().unwrap());
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
                        writer.write_sample((s * max_val).round() as i32).map_err(|e| Box::new(e) as Box<dyn Error>)?;
                    }
                }
                SampleFormat::Float => {
                    for s in resampled {
                        writer.write_sample(s as f32).map_err(|e| Box::new(e) as Box<dyn Error>)?;
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
        // REM: Linux系で先頭及び最後付与される場合があるクォート除去
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
                    println!(
                        "\nError: コマンドラインの引数が不正です。(Invalid argument) \n{} \n\nThe program will now exit.\n",
                        arg
                    );
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
