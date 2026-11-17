use anyhow::Result;
use msync_core::{
    adb::{self, Adb},
    log::Log,
};
use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};
fn main() -> Result<()> {
    let log = Log::new(&PathBuf::from("target/adb-probe/logs"))?;
    let stop = Arc::new(AtomicBool::new(false));
    println!(
        "{}",
        serde_json::to_string_pretty(&adb::devices(log.clone(), stop.clone())?)?
    );
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() == 2 {
        let device = Adb::open(&args[0], log, stop)?;
        let root = device.canonical_root(&args[1])?;
        println!(
            "目录：{root}\n{}",
            serde_json::to_string_pretty(&device.scan(&root)?)?
        );
    }
    Ok(())
}
