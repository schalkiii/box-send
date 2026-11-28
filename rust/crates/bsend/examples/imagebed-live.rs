//! 杏坛图床真实上传烟测：只上传一张封面，不发种、不改站点。
//!
//! 用法：cargo run -p bsend --example imagebed-live -- <boxsend.json> <图片URL>
//! 图片 URL 必须是可下载的外链；执行前确认 `cookies.json` 里的 `img.xingtan.one`
//! 登录态有效。输出只打印上传后的官方地址，不打印 Cookie。

use std::env;
use std::path::Path;

use anyhow::{bail, Result};

use bsend::config::Config;
use bsend::imagebed::rehost_xingtan_cover;
use bsend::reseed::PreparedUpload;

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let cfg_path = args.next().unwrap_or_else(|| "boxsend.json".to_string());
    let image_url = args
        .next()
        .unwrap_or_else(|| "https://img.xingtan.one/i/2026/11/28/6b0a2f2e597b3.png".to_string());
    let cfg = Config::load(Path::new(&cfg_path))?;

    let mut prep = PreparedUpload {
        action: "https://xingtan.one/takeupload.php".to_string(),
        form_url: "https://xingtan.one/upload.php".to_string(),
        fields: vec![(
            "descr".to_string(),
            format!("[img]{image_url}[/img]\n\n杏坛图床自动转存烟测。"),
        )],
        picked_tags: Vec::new(),
        tag_warn: None,
        select_notes: Vec::new(),
        required_gaps: Vec::new(),
        form_html: String::new(),
        poster_field: None,
        screenshot_field: None,
    };

    let result = rehost_xingtan_cover(&cfg, "xingtan", &mut prep, &mut |line| {
        eprintln!("{line}");
    })?;
    let Some(done) = result else {
        println!("封面已经在官方图床，没有重复上传");
        return Ok(());
    };
    let _ = prep;
    if done.upload_id.is_none() {
        bail!("上传成功但回包没有 id：{}", done.new_url);
    }
    println!("旧地址：{}", done.old_url);
    println!("新地址：{}", done.new_url);
    println!("上传 id：{}", done.upload_id.unwrap());
    Ok(())
}
