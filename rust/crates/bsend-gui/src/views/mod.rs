//! 各页视图：一页一文件，全部只依赖 `theme` / `widgets`，彼此不互相 import。
//! 新增页面 = 加一个模块 + 在 `main.rs` 的 `match self.tab` 里挂一行。

pub(crate) mod autopost;
pub(crate) mod config;
pub(crate) mod diag;
pub(crate) mod drafts;
pub(crate) mod help;
pub(crate) mod log;
pub(crate) mod overview;
