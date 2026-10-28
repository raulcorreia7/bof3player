use std::fs;
use std::path::PathBuf;

/// An empty folder for one test's output: `target/tmp/<test>`.
pub fn out_dir(test: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(test);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("the output folder");
    dir
}
