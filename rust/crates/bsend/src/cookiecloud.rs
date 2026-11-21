//! CookieCloud 拉取（easychen/CookieCloud，PT 站 cookie 的端对端加密云备份）。
//!
//! 服务器只存密文，解密在本地。协议与 Swift 版 `Sync/CookieCloudSync.swift` 逐字节一致
//! （各家客户端互通的前提就是这个格式）：
//!
//! ```text
//! GET {host}/get/{key}  →  { "encrypted": "<base64>", "crypto_type": "legacy" | "aes-128-cbc-fixed" }
//! key16 = MD5(key + "-" + password) 的前 16 个 hex 字符
//! aes-128-cbc-fixed：AES-128-CBC，key = key16 的 UTF-8 字节，IV = 16 个零字节，PKCS7
//! legacy：CryptoJS AES（EVP_BytesToKey(MD5)、AES-256-CBC、"Salted__" 包裹、口令 = key16）
//! 明文 JSON：{ "cookie_data": { "host": [ { name, value, domain, expirationDate, … } ] }, … }
//! ```
//!
//! 只做「拉取 + 解密 + 归一化」，导入策略交给调用方（`bsend` 会按 host 覆盖 cookies.json）。

use std::collections::HashMap;

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use anyhow::{anyhow, Context, Result};
use md5::{Digest, Md5};
use reqwest::blocking::Client;
use serde_json::Value;

use crate::config::CookieEntry;

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

/// boxsend.json 的 `cookieCloud` 段
#[derive(Debug, Clone, Default)]
pub struct CookieCloudSettings {
    pub host: String,
    pub key: String,
    pub password: String,
}

impl CookieCloudSettings {
    pub fn is_configured(&self) -> bool {
        !self.host.trim().is_empty() && !self.key.trim().is_empty()
    }
}

pub struct CookieCloud {
    pub settings: CookieCloudSettings,
    client: Client,
}

impl CookieCloud {
    pub fn new(settings: CookieCloudSettings) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            // 与下载器/站点一致：一律直连。自建 CookieCloud 多半在本机或内网，
            // 系统代理残留会让请求卡满超时（实机踩过）
            .no_proxy()
            .build()
            .context("构建 CookieCloud HTTP 客户端失败")?;
        Ok(Self { settings, client })
    }

    /// GET `{host}/get/{key}`，取出密文与加密类型
    pub fn fetch_envelope(&self) -> Result<(String, String)> {
        let mut base = self.settings.host.trim().to_string();
        while base.ends_with('/') {
            base.pop();
        }
        let key = self.settings.key.trim();
        if base.is_empty() || key.is_empty() {
            return Err(anyhow!("请先在设置里填写 CookieCloud 服务器地址与 KEY"));
        }
        let url = format!("{base}/get/{key}");
        let resp = self
            .client
            .get(&url)
            .send()
            // 不能把 URL 带进错误：`/get/{key}` 里的 KEY 是访问凭据（终端/日志会留存）
            .map_err(|e| {
                let why = if e.is_timeout() {
                    "超时"
                } else if e.is_connect() {
                    "连接失败"
                } else {
                    "请求失败"
                };
                anyhow!("请求 CookieCloud 没通（host={base}，{why}；KEY 已隐去）")
            })?;
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        if !(200..300).contains(&status) {
            let head: String = body.chars().take(200).collect();
            return Err(anyhow!("CookieCloud 回 HTTP {status}：{head}"));
        }
        let obj: Value = serde_json::from_str(&body)
            .map_err(|e| anyhow!("CookieCloud 回包不是 JSON（host={base}）：{e}"))?;
        let encrypted = obj
            .get("encrypted")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("CookieCloud 响应缺少 encrypted 字段（KEY 可能不正确）"))?;
        // 老服务端没有 crypto_type 字段，按 legacy 处理（与 Swift 版一致）
        let crypto_type = obj
            .get("crypto_type")
            .and_then(|v| v.as_str())
            .unwrap_or("legacy")
            .to_string();
        Ok((encrypted.to_string(), crypto_type))
    }

    /// 拉取并解密成明文 JSON
    pub fn fetch_plain(&self) -> Result<Value> {
        let (encrypted, crypto_type) = self.fetch_envelope()?;
        let plain = decrypt(
            &encrypted,
            &crypto_type,
            &self.settings.key,
            &self.settings.password,
        )?;
        serde_json::from_slice::<Value>(&plain)
            .map_err(|_| anyhow!("CookieCloud 解密内容不是 JSON（加密密码或 KEY 可能不正确）"))
    }

    /// 拉取全部 cookie：host → 条目（host 归一化：小写、去前导点）
    pub fn pull(&self) -> Result<HashMap<String, Vec<CookieEntry>>> {
        let plain = self.fetch_plain()?;
        parse_cookie_data(&plain)
    }
}

/// 解密后的 JSON → host → cookie 条目
pub fn parse_cookie_data(plain: &Value) -> Result<HashMap<String, Vec<CookieEntry>>> {
    let map = plain
        .get("cookie_data")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("CookieCloud 解密内容缺少 cookie_data（版本或配置可能不匹配）"))?;
    let mut out: HashMap<String, Vec<CookieEntry>> = HashMap::new();
    for (host, arr) in map {
        let Some(list) = arr.as_array() else { continue };
        let entries: Vec<CookieEntry> = list
            .iter()
            .filter_map(|c| serde_json::from_value::<CookieEntry>(c.clone()).ok())
            .filter(|c| !c.name.is_empty())
            .collect();
        if entries.is_empty() {
            continue;
        }
        let mut h = host.trim().to_lowercase();
        while let Some(rest) = h.strip_prefix('.') {
            h = rest.to_string();
        }
        if h.is_empty() {
            continue;
        }
        // 同一主域的多个条目（.example.com 与 example.com 各有一份）合并去重
        let slot = out.entry(h).or_default();
        for e in entries {
            if let Some(i) = slot.iter().position(|x| x.name == e.name) {
                slot[i] = e;
            } else {
                slot.push(e);
            }
        }
    }
    if out.is_empty() {
        return Err(anyhow!(
            "CookieCloud 返回 0 条 cookie（请在扩展里勾选 PT 站域名后重新同步）"
        ));
    }
    Ok(out)
}

/// key16 = MD5(key + "-" + password) 的前 16 个 hex 字符
pub fn derive_key16(key: &str, password: &str) -> String {
    let digest = Md5::digest(format!("{key}-{password}").as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..16].to_string()
}

/// 按 crypto_type 解密
pub fn decrypt(encrypted: &str, crypto_type: &str, key: &str, password: &str) -> Result<Vec<u8>> {
    let key16 = derive_key16(key, password);
    match crypto_type {
        "aes-128-cbc-fixed" => decrypt_aes128_fixed(encrypted, &key16),
        "legacy" => decrypt_openssl(encrypted, &key16),
        other => Err(anyhow!("未知 CookieCloud 加密类型：{other}")),
    }
}

/// AES-128-CBC（key = key16 的 UTF-8 字节、IV 全零、PKCS7）
fn decrypt_aes128_fixed(encrypted: &str, key16: &str) -> Result<Vec<u8>> {
    let b64: String = encrypted.chars().filter(|c| !c.is_whitespace()).collect();
    let raw = base64_decode(&b64)?;
    if raw.len() % 16 != 0 {
        return Err(anyhow!(
            "CookieCloud 密文长度 {} 不是 16 的倍数（加密密码或 KEY 可能不正确）",
            raw.len()
        ));
    }
    let mut buf = raw;
    Aes128CbcDec::new(key16.as_bytes().into(), [0u8; 16].as_ref().into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map(|plain| plain.to_vec())
        .map_err(|_| anyhow!("AES-128-CBC 解密失败（加密密码或 KEY 可能不正确）"))
}

/// CryptoJS / `openssl enc -aes-256-cbc -md md5` 的 "Salted__" 格式
fn decrypt_openssl(base64_text: &str, password: &str) -> Result<Vec<u8>> {
    let b64: String = base64_text.chars().filter(|c| !c.is_whitespace()).collect();
    let raw = base64_decode(&b64)?;
    if raw.len() <= 16 || &raw[..8] != b"Salted__" {
        // 允许明文 JSON（未加密备份）
        if raw.first().is_some_and(|b| *b == b'{' || *b == b'[') {
            return Ok(raw);
        }
        return Err(anyhow!("CookieCloud 密文不是 OpenSSL Salted 格式"));
    }
    let salt = &raw[8..16];
    let cipher = &raw[16..];
    let (key, iv) = evp_bytes_to_key(password.as_bytes(), salt, 32, 16);
    let mut buf = cipher.to_vec();
    Aes256CbcDec::new(key.as_slice().into(), iv.as_slice().into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map(|plain| plain.to_vec())
        .map_err(|_| anyhow!("AES-256-CBC 解密失败（加密密码或 KEY 可能不正确）"))
}

/// OpenSSL EVP_BytesToKey（MD5）：D_i = MD5(D_{i-1} || password || salt)
fn evp_bytes_to_key(
    password: &[u8],
    salt: &[u8],
    key_len: usize,
    iv_len: usize,
) -> (Vec<u8>, Vec<u8>) {
    let mut out: Vec<u8> = Vec::with_capacity(key_len + iv_len);
    let mut prev: Vec<u8> = Vec::new();
    while out.len() < key_len + iv_len {
        let mut h = Md5::new();
        h.update(&prev);
        h.update(password);
        h.update(salt);
        prev = h.finalize().to_vec();
        out.extend_from_slice(&prev);
    }
    (
        out[..key_len].to_vec(),
        out[key_len..key_len + iv_len].to_vec(),
    )
}

/// 标准 base64 解码（容忍换行与 URL-safe 变体）
fn base64_decode(text: &str) -> Result<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut rev = [255u8; 256];
    for (i, c) in TABLE.iter().enumerate() {
        rev[*c as usize] = i as u8;
    }
    // URL-safe 变体
    rev[b'-' as usize] = 62;
    rev[b'_' as usize] = 63;
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in text.bytes() {
        if c == b'=' {
            break;
        }
        let v = rev[c as usize];
        if v == 255 {
            return Err(anyhow!("CookieCloud 密文不是 base64"));
        }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 由 Node 的 crypto 按 CryptoJS 兼容规则生成的固定向量（key/password 见下）
    const PLAIN_MARK: &str = "abc123";
    const FIXED_B64: &str = "fDtmdhXOvFd1Vu0Vg0tjUzqdDMxXMWxBrCHxy9bSvZUwBJMO6Ux2XAp27SU09foARUL+rU1z2viy7MV/4pfTUCXRFuK18UIMV67yqvf22Att2f7gMsOVjhnXLugKMn7nFxjPOpZqnPDDRNrXNAjwseGonGmKSYCMZTDJNFDBgsYjXC9rnrahkUpuGn6WcjkwEO6pa2wIiwl6QrmxDRDabx8l4c9bef/BpW2kIG25CeKCAv9fnNMy421b0hTV6wt7YjAvnj5UP99VxeC/QCC053vKG8iMP7PvsisgvyIxRmegNZHNkVODPDs7YpoA/pn+XTOJ5CwNnjlDUweESBcgk+3cd1q9NUknj86iUGA1RT4/YH8W2UmaTHDABEpU8Uoi";
    const LEGACY_B64: &str = "U2FsdGVkX18BAgMEBQYHCH7f8JQpiXjeKdWIAhwxf96stJDChcCMQmJlyXAWc1YvCh9+DfXG8zkJVFyRYrMtpMm7//YA/PY+ZWiq9PQnNj3jLQ2e+TJWG7dM53Mu6iy98DRNHDfKYjZwJtI3S/sJE7m2zkCEUNyOx25JEXAkIRCLTRF3jkyn2a6Ujyzy9GCk5dnmp/BEOBkJ1n1u0bJEFJsV+bQAXp2vGQFh5lMi4pF0CLO4795M+BMnYsHNprw98iHGXUD1ljUpp4Cce5RCyN4Hjoj6X+PUblr01LEFyrgde2XWJG+jSSKh0CpQT+bb4iqsVqkmJwEGDHikVoODf0icBeYsfuq3G4iAJJjwKE/l8vlxEkOJylNB2d9wL3CCqFP1HkdbJKuQmDWF5v+SFA==";
    const KEY: &str = "test-key-0001";
    const PASSWORD: &str = "test-password";

    #[test]
    fn key_derivation_matches_cryptojs() {
        assert_eq!(derive_key16(KEY, PASSWORD), "560380b5c3f05b0e");
    }

    #[test]
    fn aes128_fixed_roundtrip() {
        let plain = decrypt(FIXED_B64, "aes-128-cbc-fixed", KEY, PASSWORD).unwrap();
        let v: Value = serde_json::from_slice(&plain).unwrap();
        assert_eq!(
            v["cookie_data"]["example.com"][0]["value"].as_str(),
            Some(PLAIN_MARK)
        );
    }

    #[test]
    fn legacy_openssl_roundtrip() {
        let plain = decrypt(LEGACY_B64, "legacy", KEY, PASSWORD).unwrap();
        let v: Value = serde_json::from_slice(&plain).unwrap();
        assert_eq!(
            v["cookie_data"]["pt.example.net"][1]["value"].as_str(),
            Some("deadbeef")
        );
    }

    #[test]
    fn wrong_password_is_an_error() {
        assert!(decrypt(FIXED_B64, "aes-128-cbc-fixed", KEY, "wrong-password").is_err());
        assert!(decrypt(LEGACY_B64, "legacy", KEY, "wrong-password").is_err());
    }

    #[test]
    fn cookie_data_normalizes_hosts_and_dedupes() {
        let plain: Value = serde_json::from_str(
            r#"{"cookie_data": {
                 ".Example.COM": [{"name":"sid","value":"1","domain":".Example.COM"},
                                  {"name":"keep","value":"2"}],
                 "example.com": [{"name":"sid","value":"3"}],
                 "empty.example": [],
                 "no-name.example": [{"value":"x"}]
               }}"#,
        )
        .unwrap();
        let map = parse_cookie_data(&plain).unwrap();
        assert_eq!(
            map.len(),
            1,
            "去前导点后同一主域要合并；空列表/无名字条目丢掉"
        );
        let list = &map["example.com"];
        assert_eq!(list.len(), 2, "同名 cookie 以后出现的为准，不重复堆叠");
        assert_eq!(list.iter().find(|c| c.name == "sid").unwrap().value, "3");
    }

    #[test]
    fn missing_cookie_data_is_an_error() {
        let plain: Value = serde_json::from_str(r#"{"local_storage_data":{}}"#).unwrap();
        assert!(parse_cookie_data(&plain).is_err());
    }

    #[test]
    fn fetch_error_does_not_leak_key() {
        // host 不可达：错误信息里不能出现 /get/{key} 的 KEY
        let settings = CookieCloudSettings {
            host: "http://127.0.0.1:1".into(),
            key: "SECRET-KEY-XYZ".into(),
            password: "p".into(),
        };
        let cc = CookieCloud::new(settings).unwrap();
        let err = cc.fetch_envelope().unwrap_err();
        let text = format!("{err:#}");
        assert!(!text.contains("SECRET-KEY-XYZ"), "KEY 泄漏：{text}");
    }
}
