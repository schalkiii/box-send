//! 覆盖索引加载/保存基准：复制一份真实 coverage.json 到临时目录，只测 Rust 读写。
//! 用法：cargo run -p bsend --example coverage-bench -- <coverage.json>

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;
use std::time::Instant;

use bsend::index::ContentIndex;

fn main() {
    let src = env::args()
        .nth(1)
        .expect("用法：coverage-bench <coverage.json>");
    let src = PathBuf::from(src);
    let tmp = env::temp_dir().join(format!("bsend-coverage-bench-{}.json", process::id()));
    fs::copy(&src, &tmp).expect("复制覆盖索引失败");
    let src_bytes = fs::metadata(&tmp).expect("读文件大小失败").len();

    let t = Instant::now();
    let index = ContentIndex::load(&tmp);
    let load_ms = t.elapsed().as_millis();

    let t = Instant::now();
    index.save().expect("保存覆盖索引失败");
    let save_ms = t.elapsed().as_millis();
    let out_bytes = fs::metadata(&tmp).expect("读输出大小失败").len();

    println!(
        "src_bytes={src_bytes} out_bytes={out_bytes} contents={} load_ms={load_ms} save_ms={save_ms}",
        index.contents.len()
    );

    let _ = fs::remove_file(&tmp);
}
