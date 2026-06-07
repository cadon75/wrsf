use crate::etc_process::{check_wav_memory_usage, select_file, show_message};
use hound::{SampleFormat, WavReader, WavSpec};
use std::{error::Error, fs, io::Cursor, path::PathBuf};

pub struct LoadedInput {
    pub samples: Vec<f64>,
    pub spec: WavSpec,
    pub n_channels: u16,
    pub sampwidth: u16,
    pub framerate: u32,
    pub arg_in_active: bool,
    pub in_path_buf: PathBuf,
}

pub fn load_input(arg_input_path: Option<PathBuf>, piped_data: Option<Vec<u8>>) -> Result<Option<LoadedInput>, Box<dyn Error>> {
    let is_pipe_mode = piped_data.is_some();
    let arg_in_active = arg_input_path.is_some() || is_pipe_mode;

    // REM: 引数あり ＞ パイプのみ ＞ 引数なしGUI の順で解決
    let in_path_buf = if let Some(p) = arg_input_path {
        p
    } else if is_pipe_mode {
        // REM: パイプ用ダミー
        PathBuf::from("Pipe_sound.wav")
    } else {
        eprintln!("読み込むwavファイルを選択して下さい");
        eprintln!("(Please enter the path to the wav file to load)");
        let in_path_str = select_file("open")?;
        if in_path_str.is_empty() {
            return Ok(None);
        }
        eprintln!("\nReading file...");
        PathBuf::from(in_path_str)
    };

    let in_path = in_path_buf.as_path();

    // REM: 色々チェック
    let ext = in_path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default();
    if is_pipe_mode {
        let valid_pipe_extensions = ["wav", "vgm", "vgz"];
        if !valid_pipe_extensions.contains(&ext.as_str()) {
            show_message("\nUnsupported file extension for pipe input.");
            return Ok(None);
        }
        if ext == "wav" && in_path.file_name().unwrap_or_default() != "Pipe_sound.wav" {
            show_message("\nAmbiguous input: Both file path and pipe stream provided.");
            return Ok(None);
        }
    } else {
        // REM: 通常時
        if ext != "wav" {
            show_message("Unsupported file extension.");
            return Ok(None);
        }
    }

    // REM: ファイルまたはパイプからWAVデータをメモリ上のバッファに取得
    let wav_buffer = if let Some(data) = piped_data {
        eprintln!("\nLoaded Data from pipe.");
        data
    } else {
        fs::read(in_path).map_err(|e| format!("File read error: {}", e))?
    };

    // REM: WAVヘッダの確認
    if wav_buffer.len() < 12 || &wav_buffer[0..4] != b"RIFF" || &wav_buffer[8..12] != b"WAVE" {
        show_message("\nInvalid WAV signature.");
        return Ok(None);
    }

    // REM: メモリ上のバッファをWavReaderに渡す
    let mut reader = match WavReader::new(Cursor::new(wav_buffer)) {
        Ok(r) => r,
        Err(e) => {
            show_message(&format!("\nOnly WAV files are supported or file could not be opened: {}", e));
            return Ok(None);
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
            return Ok(None);
        }
    };

    let framerate = spec.sample_rate;
    let _n_frames = reader.len() / n_channels as u32;
    if n_channels != 2 {
        show_message("Only 2ch-Stereo is supported");
        return Ok(None);
    }

    if ![22050, 32000, 44100, 48000, 88200, 96000].contains(&framerate) {
        show_message("Unsupported WAV freq");
        return Ok(None);
    }

    let mut samples = Vec::<f64>::new();
    match spec.sample_format {
        SampleFormat::Int => {
            let max_val: f64 = match spec.bits_per_sample {
                16 => 32767.0,
                24 => 8388607.0,
                32 => 2147483647.0,
                _ => {
                    show_message("Unsupported integer bit depth.");
                    return Ok(None);
                }
            };
            for s in reader.samples::<i32>() {
                let raw_s = s? as f64;
                let val = raw_s / max_val;
                samples.push(val.clamp(-1.0, 1.0));
            }
        }
        SampleFormat::Float => {
            for s in reader.samples::<f32>() {
                samples.push(s? as f64);
            }
        }
    }

    if !check_wav_memory_usage(samples.len())? {
        return Ok(None);
    }

    Ok(Some(LoadedInput {
        samples,
        spec,
        n_channels,
        sampwidth,
        framerate,
        arg_in_active,
        in_path_buf,
    }))
}
