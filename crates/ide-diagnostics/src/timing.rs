//! 各项检查的累计耗时（设置环境变量 `JA_DIAG_TIMING` 时才统计），用来找慢的检查。

use std::{
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("JA_DIAG_TIMING").is_some())
}

static TOTALS: Mutex<Vec<(&'static str, Duration)>> = Mutex::new(Vec::new());

/// 运行一项检查；开启统计时把耗时记到 `name` 名下。
pub fn timed<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    if !enabled() {
        return f();
    }
    let start = Instant::now();
    let result = f();
    let elapsed = start.elapsed();
    if let Ok(mut totals) = TOTALS.lock() {
        match totals.iter_mut().find(|(n, _)| *n == name) {
            Some((_, total)) => *total += elapsed,
            None => totals.push((name, elapsed)),
        }
    }
    result
}

/// 目前为止各项检查的累计耗时，从多到少。
pub fn totals() -> Vec<(&'static str, Duration)> {
    let mut totals = TOTALS.lock().map(|t| t.clone()).unwrap_or_default();
    totals.sort_by_key(|t| std::cmp::Reverse(t.1));
    totals
}
