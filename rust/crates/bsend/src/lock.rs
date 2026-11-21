//! 跨进程运行锁：同一个 dataDir 只允许一个长写任务（跑一轮 / 自动发种）在跑。
//!
//! 为什么需要：日限额是「读审计 → 判定 → 发布 → 追加审计」，两个进程（计划任务 CLI +
//! 开着的 GUI）会各自读到同样的 posted_today，把额度用超；coverage.json 并发写则是
//! 后写覆盖先写，丢格子结论。
//!
//! 实现用 `create_new` 独占创建锁文件（跨平台、无额外依赖）；Drop 删除。
//! 进程崩溃会留下陈旧锁：超过 `STALE_SECS` 视为陈旧，自动接管（PT 运营是低频任务，
//! 2 小时足够覆盖最长的校验等待）。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context as _, Result};

/// 陈旧锁判定：锁文件超过该时长未更新即视为进程已崩溃
const STALE_SECS: u64 = 2 * 3600;

#[derive(Debug)]
pub struct RunLock {
    path: PathBuf,
}

impl RunLock {
    /// 取锁。`name` 用固定词（如 `state`），同一 dataDir 下同名互斥。
    /// 拿不到时给出**可操作**的错误：谁在跑、锁在哪、确认无进程后怎么清。
    pub fn acquire(dir: &Path, name: &str) -> Result<Self> {
        std::fs::create_dir_all(dir).ok();
        let path = dir.join(format!("bsend-{name}.lock"));
        for attempt in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    let _ = writeln!(f, "pid={}", std::process::id());
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if attempt == 0 && is_stale(&path) {
                        // 陈旧锁：接管一次（删掉重试）
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    bail!(
                        "已有长任务在跑（锁文件 {}）。若确认没有 bsend/GUI 进程，删除该文件后重试",
                        path.display()
                    );
                }
                Err(e) => {
                    return Err(e).with_context(|| format!("创建运行锁失败：{}", path.display()))
                }
            }
        }
        bail!("拿不到运行锁：{}", path.display())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn is_stale(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .map(|d| d > Duration::from_secs(STALE_SECS))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bsend-lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn second_acquire_fails_until_drop() {
        let d = dir("exclusive");
        let first = RunLock::acquire(&d, "state").unwrap();
        let err = RunLock::acquire(&d, "state").unwrap_err();
        assert!(format!("{err:#}").contains("已有长任务在跑"), "{err:#}");
        drop(first);
        // 释放后可再取
        let _again = RunLock::acquire(&d, "state").unwrap();
    }

    #[test]
    fn different_names_do_not_collide() {
        let d = dir("names");
        let _a = RunLock::acquire(&d, "round").unwrap();
        let _b = RunLock::acquire(&d, "autopost").unwrap();
    }
}
