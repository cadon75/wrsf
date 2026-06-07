use crate::progress::{progress_eta_range_if, update_progress};
use crossterm::{
    cursor::{Hide, Show},
    execute,
};
use hound::{SampleFormat, WavSpec, WavWriter};
use std::{
    error::Error,
    io::{self, Cursor, Write},
    path::{Path, PathBuf},
    time::Instant,
};

// REM: 最終処理＆保存処理 (リサンプリング＋ノーマライズ（ピークを-1dBに調整）・自動命名・上書き確認)
pub fn save_output(
    resampled: &mut Vec<f64>,
    new_fs: u32,
    is_output_pipe: bool,
    n_channels: u16,
    out_freq: u32,
    out_bit: u16,
    spec: &WavSpec,
    in_path: &Path,
    arg_in_active: bool,
) -> Result<(), Box<dyn Error>> {
    eprintln!("\n");
    let label_finalsection = "Finalizing Resample & Normalization… ";
    let start_time_finalsection = Instant::now();
    let ratio = new_fs as f64 / out_freq as f64;
    let target_len_frames = (resampled.len() as f64 / n_channels as f64 / ratio) as usize;
    let mut final_samples: Vec<f64> = Vec::with_capacity(target_len_frames * n_channels as usize);
    let resampled_len = resampled.len();
    for i in 0..target_len_frames {
        let pos = i as f64 * ratio;
        let idx = pos as usize;
        let frac = pos - idx as f64;
        for ch in 0..n_channels as usize {
            let finalsamp1 = resampled[idx * n_channels as usize + ch];
            let finalsamp2 = if (idx + 1) * n_channels as usize + ch < resampled.len() {
                resampled[(idx + 1) * n_channels as usize + ch]
            } else {
                finalsamp1
            };
            final_samples.push(finalsamp1 + frac * (finalsamp2 - finalsamp1));
        }
        progress_eta_range_if(i, 2_000_000, target_len_frames, label_finalsection, 0.0, 50.0, start_time_finalsection)?;
    }
    *resampled = final_samples;
    let mut max_p = 0.0;
    for (i, &s) in resampled.iter().enumerate() {
        if s.abs() > max_p {
            max_p = s.abs();
        }
        progress_eta_range_if(i, 2_000_000, resampled_len, label_finalsection, 50.0, 75.0, start_time_finalsection)?;
    }
    let peak_floor = 10.0_f64.powf(-90.0 / 20.0); // REM: 爆音化対策
    if max_p > peak_floor {
        let final_gain = 10.0_f64.powf(-1.0 / 20.0) / max_p;
        for (i, s) in resampled.iter_mut().enumerate() {
            *s *= final_gain;
            progress_eta_range_if(i, 2_000_000, resampled_len, label_finalsection, 75.0, 100.0, start_time_finalsection)?;
        }
    }
    update_progress(label_finalsection, 100.0, 100.0)?;

    // REM: パイプ出力モード
    if is_output_pipe {
        eprintln!("\n");
        // REM: houndはファイルサイズ確定のためにSeekを要求するため、構築してから標準出力へ流す
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
        let mut cursor = Cursor::new(Vec::new());
        match WavWriter::new(&mut cursor, output_spec) {
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
                // REM: finalize()でヘッダのファイルサイズ等を確定させる
                writer.finalize().map_err(|e| Box::new(e) as Box<dyn Error>)?;
            }
            Err(e) => {
                eprintln!("Error occurred during pipe output encoding: {}", e);
                return Ok(());
            }
        }
        // REM: 構築したWAVデータを標準出力へ書き込み
        let wav_data = cursor.into_inner();
        let mut stdout = io::stdout().lock();
        stdout.write_all(&wav_data).map_err(|e| Box::new(e) as Box<dyn Error>)?;
        stdout.flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
        return Ok(()); // パイプ出力時はここで終了
    }

    eprintln!("\n");
    let base_name = in_path.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let ext_name = "wav";
    let output_file_name = format!("{}_Processed.{}", base_name, ext_name);
    let output_dir = in_path.parent().unwrap_or_else(|| Path::new("."));
    let output_path: PathBuf = output_dir.join(output_file_name);
    if output_path.exists() {
        execute!(io::stderr(), Show).map_err(|e| Box::new(e) as Box<dyn Error>)?;
        if let Some(name) = output_path.file_name().and_then(|s| s.to_str()) {
            eprintln!("{}", name);
        } else {
            eprintln!("output file");
        }
        if arg_in_active {
            eprintln!("\nForce overwrite and save...");
        } else {
            io::stderr().write_all(b"File already exists. Overwrite? [Y/n]: ").map_err(|e| Box::new(e) as Box<dyn Error>)?;
            io::stderr().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
            let mut ans_s8 = String::new();
            io::stdin().read_line(&mut ans_s8).map_err(|e| Box::new(e) as Box<dyn Error>)?;
            execute!(io::stderr(), Hide).map_err(|e| Box::new(e) as Box<dyn Error>)?;
            if ans_s8.trim().to_lowercase() == "n" {
                eprintln!("\nSaving cancelled. \n\nProcessing finished. The program will now exit.\n");
                return Ok(());
            }
        }
    }
    eprintln!("\nSaving file...\nOutput Path: {}", output_path.display());
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
            eprintln!("Saving completed. \n\nProcessing finished. The program will now exit.\n");
        }
        Err(e) => {
            eprintln!("Error occurred during saving: {} \n\nThe program will now exit.\n", e);
        }
    }
    Ok(())
}
