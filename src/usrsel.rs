// REM: ユーザー入力処理
use crossterm::{cursor::Show, execute};
use std::{
    error::Error,
    io::{self, Write},
};
type AppResult<T> = Result<T, Box<dyn Error>>;

pub struct UsrInput {
    pub do_playbackf: bool,
    pub do_tapefilter: bool,
    pub do_record_noise: bool,
    pub do_analogfilter: bool,
    pub do_speakerfilter: bool,
    pub do_playback_sel_01: String,
    pub out_freq: u32,
    pub out_bit: u16,
    pub clip_protect: bool,
}

// REM: 汎用入力
fn check_yn(question_ja: &str, question_en: &str, default_yes: bool) -> AppResult<bool> {
    let yn_suffix = if default_yes { " [Y/n]: " } else { " [y/N]: " };
    let prompt = format!("\n{} ({}){}", question_ja, question_en, yn_suffix);
    loop {
        io::stderr().write_all(prompt.as_bytes()).map_err(|e| Box::new(e) as Box<dyn Error>)?;
        io::stderr().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
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
        eprintln!("Selected : {}", if res { "Yes" } else { "No" });
        return Ok(res);
    }
}

// REM: 再生特性フィルタ選択入力
fn select_playback_filter() -> AppResult<String> {
    eprintln!("再生特性フィルタを選択して下さい\n(Please select the Playback Character Filter)");
    eprintln!("1. Balanced Retro Hi-Fi  (default)");
    eprintln!("2. Muffled  Retro Lo-Fi");
    eprintln!("3. Bright   Retro Hi-Fi");
    eprintln!("4. RF        CRT-style");
    eprintln!("5. Composite CRT-style");

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

// REM: ユーザーの選択などの入力を扱う所
pub fn run_usr_input(arg_in_active: bool, arg_mode: Option<String>, framerate: u32, sampwidth: u16, n_channels: u16) -> AppResult<UsrInput> {
    // REM: ユーザーによる処理選択セクションのループ開始
    let (mut do_playbackf, mut do_tapefilter, mut do_record_noise, mut do_analogfilter, mut do_speakerfilter);
    let (mut out_freq, mut out_bit);
    let mut clip_protect;
    let mut do_playback_sel_01 = "1".to_string();

    loop {
        // REM: ユーザー入力を受け付けるためカーソルを表示
        execute!(io::stderr(), Show).map_err(|e| Box::new(e) as Box<dyn Error>)?;

        if !arg_in_active {
            eprintln!("どのフィルタ処理を行いますか？ (Which filter processing do you want to perform?)\n");
            eprintln!("1. 基本フィルタ (Standard processing)");
            eprintln!("2. 全フィルタ   (All Filters)");
            eprintln!("3. 詳細設定     (Advanced settings)");
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
            eprintln!("Selected : {}", selected_no);
            eprintln!("");
        }

        if selected_no == "1" || selected_no == "2" {
            if arg_in_active {
                do_playback_sel_01 = "1".to_string();
            } else {
                do_playback_sel_01 = select_playback_filter()?;
                eprintln!("Selected : {}", do_playback_sel_01);
                eprintln!("");
            }
        }

        // REM: 機能無効化用とフィルター順番メモ
        // REM: do_playbackf = false;
        // REM: do_tapefilter = false;
        // REM: do_record_noise = false;
        // REM: do_analogfilter = false;
        // REM: do_speakerfilter = false;

        match selected_no.as_str() {
            "1" => {
                do_playbackf = true;
                do_tapefilter = true;
                do_record_noise = false;
                do_analogfilter = true;
                do_speakerfilter = true;
            }
            "2" => {
                do_playbackf = true;
                do_tapefilter = true;
                do_record_noise = true;
                do_analogfilter = true;
                do_speakerfilter = true;
            }
            "3" => {
                // REM: 詳細設定処理
                eprintln!("\n注意：詳細設定では組み合わせ次第で結果がおかしくなる可能性があります。\n（Note: Depending on the combination of settings used in the advanced settings, the results may be unexpected.）\n");
                do_playbackf = check_yn("再生特性 フィルタを適用しますか？", "Do you want to apply the Playback Character Filter?", true)?;
                if do_playbackf {
                    do_playback_sel_01 = select_playback_filter()?;
                    eprintln!("Selected : {}\n", do_playback_sel_01);
                }
                do_tapefilter = check_yn("カセットテープ フィルタを適用しますか？", "Do you want to apply the Cassette Tape Filter?", true)?;
                do_record_noise = check_yn("レコードノイズ を付加しますか？", "Do you want to add Record Noise?", false)?;
                do_analogfilter = check_yn("アナログ特性 フィルタを適用しますか？", "Do you want to apply the Analog Characteristics Filter?", true)?;
                do_speakerfilter = check_yn("スピーカー フィルタを適用しますか？", "Do you want to apply the Speaker Filter?", true)?;
            }
            _ => unreachable!(),
        }

        // REM: 出力ファイル形式の選択
        if !arg_in_active {
            eprintln!("\n出力するWavファイル形式を選択して下さい (Please select the output Wav file format)\n");
            eprintln!("1. 入力されたwavと同じ形式 (Same format as input wav) {}KHz / {}bit / Stereo", framerate as f64 / 1000.0, sampwidth * 8);
            eprintln!("2. 96KHz / 24bit / Stereo");
            eprintln!("3. 48KHz / 16bit / Stereo");
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
            eprintln!("Wave file format (Read / Save) : {}KHz - {}bit - {}ch", out_freq as f64 / 1000.0, out_bit * 8, n_channels);
        } else {
            eprintln!("Selected : {}KHz - {}bit - {}ch", out_freq as f64 / 1000.0, out_bit * 8, n_channels);
            eprintln!("");
        }

        // REM: スピーカーフィルタが有効な場合のみクリッピング対策の選択肢を表示
        clip_protect = false;
        if do_speakerfilter {
            if arg_in_active {
            } else {
                eprintln!("\nスピーカーフィルタのクリッピング対策を行いますか？");
                eprintln!("(Do you want to perform speker filter clipping protection?)");
            }
            loop {
                if arg_in_active {
                } else {
                    io::stderr().write_all(b" [Y/n]: ").map_err(|e| Box::new(e) as Box<dyn Error>)?;
                    io::stderr().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
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
                        eprintln!("Selected : Yes");
                        eprintln!("");
                    }
                    break;
                } else if u_cp == "n" || u_cp == "2" {
                    clip_protect = false;
                    if arg_in_active {
                    } else {
                        eprintln!("Selected : No");
                        eprintln!("");
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
            eprintln!("\n");
            continue; // REM: 一番最初に戻る
        }

        // REM: ループを抜けて処理を開始
        break;
    }

    Ok(UsrInput {
        do_playbackf,
        do_tapefilter,
        do_record_noise,
        do_analogfilter,
        do_speakerfilter,
        do_playback_sel_01,
        out_freq,
        out_bit,
        clip_protect,
    })
}
