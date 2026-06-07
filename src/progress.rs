use crossterm::{
    execute,
    terminal::{Clear, ClearType},
};
use std::error::Error;
use std::io::{self, Write};
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

// REM: プログレス更新
pub fn update_progress(label: &str, current: f64, total: f64) -> Result<(), Box<dyn Error>> {
    let percent = if total > 0.0 { (current / total) * 100.0 } else { 0.0 };
    execute!(io::stderr(), crossterm::cursor::MoveToColumn(0), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
    eprint!("{}: {:>6.1}%", label, percent);
    io::stderr().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
    Ok(())
}

// REM: ETA付きプログレス更新
pub fn update_progress_eta(label: &str, current_percent: f64, start_time: Instant) -> Result<(), Box<dyn Error>> {
    let elapsed = start_time.elapsed().as_secs_f64();
    let mut msg = format!("{}: {:>6.1}%", label, current_percent);
    if elapsed > 5.0 && current_percent > 0.0 {
        let remaining = (elapsed / current_percent) * (100.0 - current_percent);
        msg += &format!(" ({:.0} seconds remaining)", remaining.max(0.0));
    }
    execute!(io::stderr(), crossterm::cursor::MoveToColumn(0), Clear(ClearType::CurrentLine)).map_err(|e| Box::new(e) as Box<dyn Error>)?;
    eprint!("{}", msg);
    io::stderr().flush().map_err(|e| Box::new(e) as Box<dyn Error>)?;
    Ok(())
}

// REM: ラッパー群
#[inline]
pub fn should_update_progress(index: usize, interval: usize) -> bool {
    interval != 0 && index % interval == 0
}

#[inline]
pub fn calc_progress_percent(current: usize, total: usize, start_percent: f64, end_percent: f64) -> f64 {
    if total == 0 {
        return end_percent;
    }
    start_percent + (current as f64 / total as f64) * (end_percent - start_percent)
}

#[inline]
pub fn progress_range_if(current: usize, interval: usize, total: usize, label: &str, start_percent: f64, end_percent: f64) -> Result<(), Box<dyn Error>> {
    let percent = calc_progress_percent(current, total, start_percent, end_percent);
    progress_if(current, interval, label, percent)
}

#[inline]
pub fn progress_eta_range_if(current: usize, interval: usize, total: usize, label: &str, start_percent: f64, end_percent: f64, start_time: Instant) -> Result<(), Box<dyn Error>> {
    let percent = calc_progress_percent(current, total, start_percent, end_percent);
    progress_eta_if(current, interval, label, percent, start_time)
}

#[inline]
pub fn set_thread_progress(progress_list: &Arc<Mutex<Vec<usize>>>, thread_idx: usize, value: usize) {
    if let Ok(mut list) = progress_list.lock() {
        list[thread_idx] = value;
    }
}

#[inline]
pub fn thread_progress_if(index: usize, interval: usize, progress_list: &Arc<Mutex<Vec<usize>>>, thread_idx: usize) {
    if should_update_progress(index, interval) {
        set_thread_progress(progress_list, thread_idx, index);
    }
}

#[inline]
pub fn progress_eta(label: &str, current_percent: f64, start_time: Instant) -> Result<(), Box<dyn Error>> {
    update_progress_eta(label, current_percent, start_time)
}

#[inline]
pub fn progress_eta_if(index: usize, interval: usize, label: &str, current_percent: f64, start_time: Instant) -> Result<(), Box<dyn Error>> {
    if should_update_progress(index, interval) {
        progress_eta(label, current_percent, start_time)?;
    }
    Ok(())
}

#[inline]
pub fn progress_if(index: usize, interval: usize, label: &str, current_percent: f64) -> Result<(), Box<dyn Error>> {
    if should_update_progress(index, interval) {
        update_progress(label, current_percent, 100.0)?;
    }
    Ok(())
}
