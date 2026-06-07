use std::{
    env,
    error::Error,
    io::{self, IsTerminal, Read},
    path::PathBuf,
};

pub struct StartupArgs {
    pub arg_input_path: Option<PathBuf>,
    pub cl_mode: Option<String>,
    pub piped_data: Option<Vec<u8>>,
    pub is_output_pipe: bool,
}

pub fn startup_args() -> Result<Option<StartupArgs>, Box<dyn Error>> {
    // REM: パイプ入力の自動検出とバッファ処理
    let mut piped_data: Option<Vec<u8>> = None;

    if !io::stdin().is_terminal() {
        let mut buffer = Vec::new();
        io::stdin().read_to_end(&mut buffer).map_err(|e| Box::new(e) as Box<dyn Error>)?;
        if !buffer.is_empty() {
            piped_data = Some(buffer);
        }
    }

    // REM: コマンドライン引数解析
    // REM: 引数ありの挙動は表示しないで入力を内部で入れてるだけなので
    // REM: 挙動を変えたい場合は対処が必要
    let mut arg_input_path: Option<PathBuf> = None;
    let mut cl_mode: Option<String> = None;
    let mut is_output_pipe = false;

    for arg in env::args().skip(1) {
        // REM: Linux系で先頭及び最後に付与される場合があるクォート除去
        let arg = if arg.len() >= 2 && ((arg.starts_with('\'') && arg.ends_with('\'')) || (arg.starts_with('"') && arg.ends_with('"'))) {
            arg[1..arg.len() - 1].to_string()
        } else {
            arg.to_string()
        };

        let arg_opt_check = arg.trim();
        match arg_opt_check {
            "-1" => {
                cl_mode = Some("1".to_string());
            }
            "-2" => {
                cl_mode = Some("2".to_string());
            }
            "-" => {
                is_output_pipe = true;
            }
            _ => {
                if arg_input_path.is_none() {
                    arg_input_path = Some(PathBuf::from(&arg));
                } else {
                    eprintln!("\nError: コマンドラインの引数が不正です。(Invalid argument) \n{} \n\nThe program will now exit.\n", arg);
                    return Ok(None);
                }
            }
        }
    }

    // REM: パイプ出力の指示がある場合、OS側でパイプの確認
    if is_output_pipe && io::stdout().is_terminal() {
        eprintln!("\nパイプ出力が接続されていません。");
        eprintln!("(pipe output is not connected.)\n\nThe program will now exit.\n");
        return Ok(None);
    }

    // REM: パイプ入力時と通常ファイル名引数時のみでオプション未指定時は-1を入れる
    if (arg_input_path.is_some() || piped_data.is_some()) && cl_mode.is_none() {
        cl_mode = Some("1".to_string());
    }

    Ok(Some(StartupArgs {
        arg_input_path,
        cl_mode,
        piped_data,
        is_output_pipe,
    }))
}
