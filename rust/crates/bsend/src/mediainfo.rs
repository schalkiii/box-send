//! 本地 MediaInfo 提取（Windows）：走 MediaInfo.dll 的 C 接口（GUI 版 exe 无法无头运行）。
//!
//! 用途：源站详情页没有 MediaInfo 块时，从本地文件生成标准文本报告——luckpt 等站发种
//! 要求 MediaInfo 非空，缺了官方预审会直接报错（实测 -9 分）。

#[cfg(windows)]
use anyhow::Context;
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// 视频扩展名（挑代表文件用）
const VIDEO_EXTS: &[&str] = &[
    "mkv", "mp4", "ts", "m2ts", "avi", "rmvb", "wmv", "mov", "iso", "flv", "webm",
];

/// 找 MediaInfo.dll：显式配置 > 环境变量 `BSEND_MEDIAINFO_DLL` > 常见安装路径
pub fn find_dll(explicit: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = explicit.map(str::trim).filter(|p| !p.is_empty()) {
        let pb = PathBuf::from(p);
        return pb.is_file().then_some(pb);
    }
    if let Ok(p) = std::env::var("BSEND_MEDIAINFO_DLL") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    [
        r"C:\Program Files\MediaInfo\MediaInfo.dll",
        r"C:\Program Files (x86)\MediaInfo\MediaInfo.dll",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.is_file())
}

/// 目录里挑一个代表文件（最大的视频，最多下钻 2 层）；单文件直接返回
pub fn pick_media_file(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let mut best: Option<(u64, PathBuf)> = None;
    let mut stack = vec![(path.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 2 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push((p, depth + 1));
                continue;
            }
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !VIDEO_EXTS.contains(&ext.as_str()) {
                continue;
            }
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            if best.as_ref().map(|(s, _)| size > *s).unwrap_or(true) {
                best = Some((size, p));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// 生成 MediaInfo 文本报告（英文标签，与 MediaInfo「文本」视图一致）。
/// `dll` 是显式配置的 DLL 路径；None 走自动发现。
pub fn generate(path: &Path, dll: Option<&str>) -> Result<String> {
    let media = pick_media_file(path)
        .ok_or_else(|| anyhow::anyhow!("目录里没找到视频文件（{}）", path.display()))?;
    generate_file(&media, dll)
}

#[cfg(windows)]
fn generate_file(file: &Path, dll: Option<&str>) -> Result<String> {
    use libloading::Symbol;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    let dll_path = find_dll(dll).context(
        "没找到 MediaInfo.dll（可配 assist.mediaInfoDll，或设环境变量 BSEND_MEDIAINFO_DLL）",
    )?;
    let lib = unsafe { libloading::Library::new(&dll_path) }
        .with_context(|| format!("加载 {} 失败", dll_path.display()))?;

    unsafe {
        let new: Symbol<unsafe extern "C" fn() -> *mut c_void> =
            lib.get(b"MediaInfo_New").context("DLL 缺 MediaInfo_New")?;
        let open: Symbol<unsafe extern "C" fn(*mut c_void, *const u16) -> usize> = lib
            .get(b"MediaInfo_Open")
            .context("DLL 缺 MediaInfo_Open")?;
        let opt: Symbol<unsafe extern "C" fn(*mut c_void, *const u16, *const u16) -> *const u16> =
            lib.get(b"MediaInfo_Option")
                .context("DLL 缺 MediaInfo_Option")?;
        let inform: Symbol<unsafe extern "C" fn(*mut c_void, usize) -> *const u16> = lib
            .get(b"MediaInfo_Inform")
            .context("DLL 缺 MediaInfo_Inform")?;
        let close: Symbol<unsafe extern "C" fn(*mut c_void)> = lib
            .get(b"MediaInfo_Close")
            .context("DLL 缺 MediaInfo_Close")?;
        let delete: Symbol<unsafe extern "C" fn(*mut c_void)> = lib
            .get(b"MediaInfo_Delete")
            .context("DLL 缺 MediaInfo_Delete")?;

        let handle = new();
        if handle.is_null() {
            bail!("MediaInfo_New 返回空句柄");
        }
        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
        let key_lang = wide("Language");
        let val_en = wide("en");
        opt(handle, key_lang.as_ptr(), val_en.as_ptr());
        let wpath: Vec<u16> = file
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let ok = open(handle, wpath.as_ptr());
        if ok == 0 {
            close(handle);
            delete(handle);
            bail!("MediaInfo 打不开文件：{}", file.display());
        }
        let ptr = inform(handle, 0);
        let text = if ptr.is_null() {
            String::new()
        } else {
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
        };
        close(handle);
        delete(handle);
        if text.trim().is_empty() {
            bail!("MediaInfo 输出为空：{}", file.display());
        }
        Ok(format!("{}\n", text.trim_end()))
    }
}

#[cfg(not(windows))]
fn generate_file(_file: &Path, _dll: Option<&str>) -> Result<String> {
    bail!("本地 MediaInfo 生成仅支持 Windows（MediaInfo.dll C 接口）")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_media_file_prefers_largest_video() {
        let dir = std::env::temp_dir().join(format!("bsend-mi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.nfo"), b"x").unwrap();
        std::fs::write(dir.join("small.mp4"), vec![0u8; 10]).unwrap();
        std::fs::write(dir.join("big.MKV"), vec![0u8; 100]).unwrap();
        std::fs::write(dir.join("sub/deep.mp4"), vec![0u8; 50]).unwrap();
        let picked = pick_media_file(&dir).unwrap();
        assert_eq!(picked.file_name().unwrap(), "big.MKV", "{picked:?}");
        // 单文件直接返回自身
        let one = dir.join("small.mp4");
        assert_eq!(pick_media_file(&one).unwrap(), one);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_directory_is_an_error() {
        let dir = std::env::temp_dir().join("bsend-mi-nope-does-not-exist");
        assert!(pick_media_file(&dir).is_none());
        assert!(generate(&dir, None).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn real_dll_reports_text_when_available() {
        // 机器上装了 MediaInfo 才跑：拿本仓库的 .torrent 无关，直接找一个系统媒体文件太难，
        // 这里只验证 DLL 能被加载、符号齐全、对不存在的文件返回错误而不是崩。
        let Some(dll) = find_dll(None) else {
            return;
        };
        let err = generate_file(
            Path::new(r"C:\__bsend_not_a_media_file__.mkv"),
            dll.to_str(),
        );
        assert!(err.is_err(), "不存在的文件应报错");
    }
}
