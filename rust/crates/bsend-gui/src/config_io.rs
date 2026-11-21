//! 配置写回（GUI 配置页）：读原始 JSON → 只改目标键 → 备份 → 原子替换。
//! 纯文件逻辑，抽出来便于单测（GUI 其它部分依赖 GPUI 上下文，不易测）。

use std::path::Path;

use anyhow::{Context as _, Result};

/// 读 `path` → `mutate` 修改（保留未知键）→ 备份 `.bak-gui` → 原子写回。
/// 解析失败时直接报错且**不碰**原文件（不会把配置写坏）。
pub(crate) fn save_config_with_backup(
    path: &Path,
    mutate: impl FnOnce(&mut serde_json::Value),
) -> Result<()> {
    let text = std::fs::read_to_string(path).context("读配置失败")?;
    let mut root: serde_json::Value = serde_json::from_str(&text).context("配置不是合法 JSON")?;
    mutate(&mut root);
    let file_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let backup = path.with_file_name(format!("{file_name}.bak-gui"));
    std::fs::write(&backup, &text).context("写备份失败")?;
    let body = serde_json::to_string_pretty(&root)?;
    bsend::atomic_write(path, body.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("bsend-gui-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn mutate_preserves_unknown_keys_and_backs_up() {
        let dir = temp("preserve");
        let path = dir.join("boxsend.json");
        let original = r#"{"unknownKey": {"keep": 1}, "assist": {"publishEnabled": false}}"#;
        std::fs::write(&path, original).unwrap();

        save_config_with_backup(&path, |root| {
            root["assist"]["publishEnabled"] = serde_json::json!(true);
            root["assist"]["dailyLimit"] = serde_json::json!(3);
        })
        .unwrap();

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["unknownKey"]["keep"], 1, "未知键必须保留");
        assert_eq!(saved["assist"]["publishEnabled"], true);
        assert_eq!(saved["assist"]["dailyLimit"], 3);

        let backup = dir.join("boxsend.json.bak-gui");
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            original,
            "备份必须是修改前的原文"
        );
    }

    #[test]
    fn invalid_json_is_rejected_without_touching_file() {
        let dir = temp("invalid");
        let path = dir.join("boxsend.json");
        let broken = "{not json";
        std::fs::write(&path, broken).unwrap();
        let err = save_config_with_backup(&path, |_| {}).unwrap_err();
        assert!(format!("{err:#}").contains("不是合法 JSON"), "{err:#}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            broken,
            "解析失败不能改动原文件"
        );
    }
}
