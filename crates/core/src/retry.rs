use crate::log::Log;
use anyhow::{ensure, Result};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub const ATTEMPTS: usize = 3;

pub fn check(stop: &AtomicBool) -> Result<()> {
    ensure!(!stop.load(Ordering::Relaxed), "用户已停止同步");
    Ok(())
}

pub fn run<T>(
    log: &Log,
    stop: &AtomicBool,
    label: &str,
    mut action: impl FnMut() -> Result<T>,
) -> Result<T> {
    for attempt in 1..=ATTEMPTS {
        check(stop)?;
        match action() {
            Ok(value) => return Ok(value),
            Err(error) => {
                check(stop)?;
                if attempt == ATTEMPTS {
                    return Err(error);
                }
                log.write(
                    "WARN",
                    format!("{label}：第 {attempt}/{ATTEMPTS} 次失败，自动重试；{error:#}"),
                );
                // 等待可取消，停止同步不必等退避时间结束。
                for _ in 0..attempt * 10 {
                    check(stop)?;
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
    unreachable!()
}
