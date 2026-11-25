//! CLI 入口的退出码/错误提示冒烟：不连 qB、不碰真实配置。
//! 目的只是把「命令处理函数拿不到配置时会安静成功」这类回归挡住。

use std::process::Command;

fn bsend() -> Command {
    Command::new(env!("CARGO_BIN_EXE_bsend"))
}

fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("bsend-cli-{}-{name}", std::process::id()))
}

#[test]
fn version_prints_and_exits_zero() {
    let out = bsend().arg("--version").output().expect("run bsend");
    assert!(out.status.success(), "{:?}", out);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("bsend"), "{text}");
}

#[test]
fn status_with_missing_config_exits_nonzero() {
    let path = temp_path("missing.json");
    let _ = std::fs::remove_file(&path);
    let out = bsend()
        .args(["--config", path.to_str().unwrap(), "status"])
        .output()
        .expect("run bsend");
    assert!(!out.status.success(), "缺配置不能当成功：{:?}", out);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("配置") || err.contains("boxsend"), "{err}");
}

#[test]
fn status_with_bad_config_json_exits_nonzero() {
    let path = temp_path("bad.json");
    std::fs::write(&path, "{not json").unwrap();
    let out = bsend()
        .args(["--config", path.to_str().unwrap(), "status"])
        .output()
        .expect("run bsend");
    assert!(!out.status.success(), "坏配置不能当成功：{:?}", out);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("解析") || err.contains("JSON"), "{err}");
    let _ = std::fs::remove_file(path);
}
