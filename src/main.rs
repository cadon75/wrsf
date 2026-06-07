use crate::analog_filter::analog_filter;
use crate::etc_process::check_terminal;
use crate::load_input::{load_input, LoadedInput};
use crate::playback_filter::playback_filter;
use crate::pre_process::pre_process;
use crate::record_noise::record_noise;
use crate::save_output::save_output;
use crate::speaker_filter::speaker_filter;
use crate::startup::{startup_args, StartupArgs};
use crate::tape_filter::tape_filter;
use crate::usrsel::{run_usr_input, UsrInput};
use crossterm::{
    cursor::{Hide, Show},
    execute,
};
use std::{
    error::Error,
    io::{self, Write},
    path::PathBuf,
};
mod analog_filter;
mod etc_process;
mod load_input;
mod playback_filter;
mod pre_process;
mod progress;
mod record_noise;
mod save_output;
mod speaker_filter;
mod startup;
mod tape_filter;
mod usrsel;

type AppResult<T> = Result<T, Box<dyn Error>>;

pub const WRSFVER: &str = env!("CARGO_PKG_VERSION");
// REM: 処理のメイン
fn wav_process_main(arg_input_path: Option<PathBuf>, arg_mode: Option<String>, piped_data: Option<Vec<u8>>, is_output_pipe: bool) -> AppResult<()> {
    // REM: ファイルロード処理
    let loaded = match load_input(arg_input_path, piped_data)? {
        Some(v) => v,
        None => return Ok(()),
    };
    let LoadedInput {
        samples,
        spec,
        n_channels,
        sampwidth,
        framerate,
        arg_in_active,
        in_path_buf,
    } = loaded;
    let in_path = in_path_buf.as_path();

    // REM: ユーザー選択入力
    let usr_sel = run_usr_input(arg_in_active, arg_mode.clone(), framerate, sampwidth, n_channels)?;
    let UsrInput {
        do_playbackf,
        do_tapefilter,
        do_record_noise,
        do_analogfilter,
        do_speakerfilter,
        do_playback_sel_01,
        out_freq,
        out_bit,
        clip_protect,
    } = usr_sel;
    // REM: フィルタが一つも選択されていない場合の安全な終了
    if !(do_playbackf || do_tapefilter || do_record_noise || do_analogfilter || do_speakerfilter) {
        eprintln!("No processing was performed.");
        return Ok(());
    }

    // REM: 以降、処理のためカーソルを非表示
    if !is_output_pipe {
        execute!(io::stderr(), Hide).map_err(|e| Box::new(e) as Box<dyn Error>)?;
    }

    eprintln!("\n\n<< Wav Retro Sound Filter v{} >>", WRSFVER);

    // REM: 1. 初期処理
    let (mut resampled, new_fs, dt, total_res) = pre_process(&samples, n_channels, framerate)?;
    // REM: 2. 再生特性フィルタ
    if do_playbackf {
        playback_filter(&mut resampled, n_channels, dt, &do_playback_sel_01, total_res)?;
    }
    // REM: 3. カセットテープフィルタ
    if do_tapefilter {
        tape_filter(&mut resampled, total_res, n_channels, new_fs, dt)?;
    }
    // REM: 4. レコードノイズ付加
    if do_record_noise {
        record_noise(&mut resampled, total_res, n_channels, new_fs, do_speakerfilter)?;
    }
    // REM: 5. アナログ特性フィルタ
    if do_analogfilter {
        analog_filter(&mut resampled, total_res, n_channels, out_bit, dt)?;
    }
    // REM: 6. スピーカー周波数特性フィルタ
    if do_speakerfilter {
        speaker_filter(&mut resampled, new_fs, n_channels, clip_protect)?;
    }
    // REM: 7. 最終処理＆保存処理
    save_output(&mut resampled, new_fs, is_output_pipe, n_channels, out_freq, out_bit, &spec, in_path, arg_in_active)?;
    Ok(())
}

fn main() -> AppResult<()> {
    // REM: 端末内か確認（Win11のcmdはダブルクリックでも端末内になる模様）
    check_terminal();

    // REM: パイプ確認や引数処理など
    let startup = match startup_args()? {
        Some(v) => v,
        None => return Ok(()),
    };
    let StartupArgs {
        arg_input_path,
        cl_mode,
        piped_data,
        is_output_pipe,
    } = startup;

    // REM: 処理本体
    let result = wav_process_main(arg_input_path, cl_mode, piped_data, is_output_pipe);

    // REM: エラー表示
    if let Err(ref e) = result {
        eprintln!("\nProcessing interrupted or error\n {}\n", e);
    }
    // REM: 処理終了時にカーソルを表示に戻す
    if !is_output_pipe {
        let _ = execute!(io::stderr(), Show);
        let _ = io::stderr().flush();
    }
    result
}
