# 站点适配调试手册（方法学 + 站点档案）

> 适用对象：本仓库 `rust/crates/bsend`（bsend）对 PT 站点做**发种表单适配**、**辅种检索**、
> **转种上传**的全过程。目的：把「每遇到一个站就现场摸索」变成「按固定流程取证 → 归类 →
> 最小改动 → 留档」。所有结论都要求可复现：命令、证据、结论三件套。

## 1. 工具链

| 命令 | 用途 | 是否发请求 |
| --- | --- | --- |
| `bsend scan` | 建/刷新内容索引（qB → 内容 × 站点矩阵） | 否（只读 qB） |
| `bsend formcheck [--sites a,b]` | 逐站拉 upload.php，构造字段核对完备性 | 读站点，绝不 POST |
| `bsend layoutcheck --limit N` | 用下载器实时数据核对推种落盘决策 | 只读 qB |
| `bsend plan --only 名称 [--sites …]` | 预演一轮：检索照跑，推送只出结论、不落盘 | 读站点 |
| `bsend reseed --draft N` | 审阅一条转种草稿（预检 + 官方预审 + 将提交字段） | 读源站/目标站，不发种 |
| `bsend reseed --draft N --yes` | 真发种（预检 → 官方预审 → 上传 → 发后自检 → 回推校验） | 写站点 |
| `bsend gatecheck`（规划 P2） | 门禁矩阵：pass / auto-fix / block + 证据 | 只读 |

## 2. 标准调试流程（七步）

1. **可达性**：DNS 是否解析、TLS 是否握手、首页 HTTP 状态；记录 `curl -I` 或 Python 探测结果。
   - DNS 失败 → 关站/换域名（对照 savept.icu 关站名单，见 §5）。
   - 超时 → 网络/站点侧问题，标「连不通」，不写任何结论（不落负缓存）。
2. **登录态与权限**：带 cookie 拉 `upload.php`，先看是不是登录表单（`type="password"`），
   再看是否含权限提示（简繁都要收：`没有发布种子的权限` / `沒有發布種子的許可權`）。
   结论只有两种：**账号无权限**（只影响发种，辅种照常）或 **cookie 失效**（整站停用）。
3. **表单可解析性**：定位真正的上传表单块（评分：`type="file"` > `takeupload` > `upload`），
   排除导航搜索表单（`search_area` 曾把字段类型污染成「全站」）。
   - 没有任何静态表单 → JS/自定义界面（v1 不支持，记录形态）；
   - 只有 `file + hidden` 且 `<textarea>` 为 0 → JS 驱动界面（hdcity 实测：表单指向独立接收站，
     简介在接收站后补）。
4. **字段完备性**：核对 标题/副标题/简介/MediaInfo/链接/标签 六类字段的落点；
   `formcheck` 的硬缺口只看：分类、媒介、编码、分辨率、音频、标题、简介、MediaInfo。
   - 无标题输入框（标题由种子文件名自动生成，tjupt 实测）→ 记「自动生成」，不算缺口；
   - Gazelle 架构（dicmusic）字段名不同 → 走别名回退（`title`/`subtitle`/`release_desc`）。
5. **选档映射**：逐站导出 `<select>` 的真实选项标签，再决定映射；**先取证后改代码**。
   - 多组下拉（`medium_sel[4]`/`[9]` 等分类分支）→ 各组独立配对，缺口按前缀折叠；
   - 标签差异 → 别名表（`HDTV`→`TV`/`电视`、`1080p`→`1080`），只收实测见过的写法；
   - 同名不同义（`source_sel` 是地区、书站 `audiocodec_sel` 是电子书格式）→ 先做**域相关性**
     判定，域无关直接跳过，不报「没配对」；
   - 同名重复选项（muxuege 两个 `AAC`）→ 不猜，留空 + 点名歧义；
   - 垂直站（曲艺/有声书/音乐）分类里没有影视类 → 软提示「样本无对应项」，不算表单缺口。
6. **发种门禁实测**：`reseed --draft N` 走一遍，看三处：
   ① 本地预检（MediaInfo/链接/标签）；② 官方预审（LuckAudit 站）；③ 将提交的字段清单。
   - 源站没有 MediaInfo → 本地生成（MediaInfo.dll，见 §4）；
   - 完结剧集 → 自动勾「完结」标签（官方预审唯一 ❌ 实测来自这里）；
   - 链接缺失 → 按源页 IMDb/豆瓣/TMDB id 补；源页连 id 都没有 → 阻断（留人工）。
7. **归档**：把站点结论写进 §5 的档案表（状态 / 根因 / 证据 / 处置 / 日期），
   全量原始输出落 `.workbuddy/formcheck-all.txt`，操作留痕落 `doc/REAL-RUN-2026-11-17.md`。

## 3. 根因分类表（实测案例）

| # | 根因 | 识别特征 | 处置 | 案例 |
| --- | --- | --- | --- | --- |
| 1 | 账号无发布权限 | 页面含权限提示（简/繁） | 只停发种，辅种保留 | hdsky、pter 等 29 站 |
| 2 | cookie 失效 / 403 | 跳登录页、HTTP 403 | 整站暂停 | piggo、tu88 |
| 3 | DNS 失效 / 关站 | 解析失败 | 关站档案 + 暂停 | movie52、yinghua |
| 4 | 站点换形态（PT→资源站） | 首页正常但 upload.php 404 | 按关站处理 | ptlao |
| 5 | JS/自定义上传界面 | 无静态表单或仅 file+hidden | v1 不支持，记录形态 | ttg、keepfrds、hdcity（两步上传） |
| 6 | 导航表单污染 | 字段类型变成搜索选项 | 表单块评分定位 | 早期 9 站 |
| 7 | 多组参数化下拉 | `medium_sel[4]` 与 `[9]` 并存 | 各组独立配对 + 前缀折叠 | ptba、iloli、panda、freefarm、crabpt |
| 8 | 选项标签差异 | 站内写 `TV`、裸数字 `1080` | 别名表（实测驱动） | aling、ptcafe、ptsbao |
| 9 | 同名不同义下拉 | `source_sel` 是地区 | 域相关性判定后跳过 | aling、ubits、soulvoice |
| 10 | 同名重复选项 | 两个 `AAC`（无任何属性区分） | 留空不猜 + 歧义点名 | muxuege |
| 11 | 站点不收该媒介 | 站内 0 条 HDTV 标题 | 诚实缺口，不硬塞 | ptfans |
| 12 | 垂直站分类 | 分类只有曲艺/电子书/音乐 | 软提示，不算缺口 | haitang、kelu、vclib、nicept、ziran、soulvoice、musopia、dicmusic |
| 13 | 标题自动生成 | 无 `name`/`title` 输入框 | 不算缺口，发后核对 | tjupt |
| 14 | 非 NexusPHP 框架 | 框架字段不是 NexusPHP | 不参与（v1 检索不支持） | 17 站（Blu/Unit3D/Gazelle 等） |
| 15 | 字符边界 / 标签串格 | 实机 panic 或错误勾选 | `floor_char_boundary` + 遇 `<input` 截断 | xingtan 等 |
| 16 | 元信息假命中 | `<meta name="description">` 被当字段 | 只认 input/textarea/select | hdcity |

## 4. 关键经验（可复用）

- **结构优先于文案**：定位表单看结构（file/takeupload），不要靠页面文字；文案只用于「权限/限流」判定。
- **先取证再动手**：任何映射改动前，先把该站真实的 `<select>` 选项、字段名、权限页拉下来存证。
  没有证据的「猜」一律不做（歧义留空）。
- **一次只改一个判定，并补单测**：本轮 10 条新单测分别钉住别名、多组、歧义、域过滤、垂直站、
  Gazelle 字段、无标题、meta 假命中、UTF-8 边界、完结标签。
- **兜底链而不是特判**：字段名按链回退（`name→title`、`small_descr→subtitle`、`descr→release_desc`），
  站点差异用「候选链 + 首个存在」吸收，不为单站写分支。
- **不猜的三种合法降级**：歧义（多个候选）→ 留空；域无关 → 跳过；不可验证 → 回退旧行为（校验兜底）。
- **发种前的三件套**：本地预检（MediaInfo/链接/标签）→ 官方预审（有则必过）→ 字段清单人工可读。
- **MediaInfo 生成**：MediaInfo GUI 版 `MediaInfo.exe` 无法无头运行（带参数会挂起），必须走
  `MediaInfo.dll` 的 C 接口（`MediaInfo_New/Open/Option/Inform`）；`Language=en` 输出标准
  英文标签文本；内容目录取最大视频作代表（剧集包取单集）。
- **同名重复的急救方式**：通用规则不猜（muxuege 两个 AAC 留空）；确属站点表单重复项时，
  用逐站 `assist.reseedRules.<site>.selectPicks` 指定**实测 value**（ptfans 媒介两个 Encode
  value 7/8，取 8；写入前校验该 value 在表单中真实存在，防旧规则失效）。
- **HDTV 的媒介归类**：站点若有 `HDTV`/`TV` 直接对应；没有但有 `Encode` 且发布名带
  x264/x265 → 归 `Encode`（ptfans 实测）；音乐站只有 CD/SACD/DVD 时不硬塞，留空阻断。
- **演练样本要满足站点预审规范**：LuckAudit 要求 MediaInfo 含 `General` / 空行 /
  `Video` / 空行 / `Audio` 分段且 General 有 `Duration`，简介含 ≥3 张图；样本不合格会假性拦站。
- **选档按官方驳回迭代**：无集数但有年份 → 电影；光盘源 + x264/x265 → `Encode` 而非原盘；
  MediaInfo 的 Audio 段有中文轨 → 「国语」标签。每次改动用官方预审分数验证（79→89→96→100）。
- **发后回包 id 未必是真实种子 id**：luckpt 实测回包 `id=10230`，真实种子是 `61140`
  （用户人工确认可见可下载）。先按发布标题站内检索校正详情页；校正不到才记 Reseeded
  + 人工确认，绝不自动重发。
- **WAF 与内联事件属性**：站点 Cloudflare 对表单体里的 `on*=` 属性按出现次数计分
  （预审载荷实测 2 处放行、4 处 403）；**真实上传同理**（luckpt 403 实测）——简介提交前
  统一 `strip_event_handlers`，正文不动。
- **落盘复用**：推种前按下载器实时 savePath/根名/文件清单判定；仅根名不同 →
  `savepath=既有内容目录 + contentLayout=NoSubfolder`；内层不同 → 待人工（不动下载器）。
- **停用要分层**：`pausedSites`（整站停）vs `reseedPausedSites`（只停发种，辅种照常）；
  关站站点另加 `sourceSites[].statusNote` 标注原因与日期。

## 5. 站点档案（统一总账）

逐站状态不再散落在本文各节——**唯一权威表**是 [`doc/SITE-REGISTER.md`](SITE-REGISTER.md)：
147 站的「域名 / 框架 / 启用 / 全停 / 停转种 / 最近实测结论与原因 / pieces-hash 支持 /
cookie 条目数 / 关站备注」，由命令生成、随时可刷新：

```bash
bsend formcheck             # 刷新实测（结果落 dataDir/formcheck.json）
bsend sites --md > doc/SITE-REGISTER.md   # 重新生成总账表格
```

本文只保留**方法学**与**根因分类**；具体站点的结论请看总账与
`doc/REAL-RUN-2026-11-17.md`（第十五~十七节）、`.workbuddy/formcheck-all.txt`（原始输出）。

### 关站（savept.icu 对照，已暂停）

| 站点 | 关站日 | 实测 |
| --- | --- | --- |
| momentpt | 2026-10-29 | 告别页 |
| ptlao | 2026-10-21 | 变资源站，upload.php 404 |
| movie52 | 2026-10-20 | DNS 失效 |
| yinghua | 2026-09-20 | DNS 失效 |

## 6. 复现模板

```bash
# 1) 单站表单演练（只读）
bsend formcheck --sites <site>

# 2) 单站上传页原始表单（取证用）
python - <<'PY'
import json, ssl, urllib.request
cookies = json.load(open(r'C:/Users/Schal/AppData/Local/BoxSend/cookies.json', encoding='utf-8'))
host = '<site-host>'; url = 'https://<site>/upload.php'
ck = '; '.join(f"{c['name']}={c['value']}" for c in cookies[host])
ctx = ssl.create_default_context(); ctx.check_hostname = False; ctx.verify_mode = ssl.CERT_NONE
html = urllib.request.urlopen(urllib.request.Request(url, headers={'User-Agent':'Mozilla/5.0','Cookie':ck}), timeout=20, context=ctx).read().decode('utf-8','replace')
print(len(html)); print(html[:2000])
PY

# 3) 选档选项取证（打印每个质量下拉的 value/label）
#    见 §2 第 5 步：把 <select name="..._sel..."> 的所有 option 打出来再对照

# 4) 转种审阅（只审不发）
bsend reseed --draft <N>
```

## 7. 变更记录

- 2026-11-18：首版。汇总 104 站演练、5 类首轮根因、20 站二轮选项校准、4 个遗留缺口取证、
  4 个关站站处置、pieces-hash 支持率探测（52/92）。
