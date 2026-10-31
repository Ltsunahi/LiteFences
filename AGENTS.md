# 给 AI 协作者

## 语言

- **回答和内部推理（思考过程）都用简体中文。** 代码、命令、日志消息保持原样。
- 日志、错误提示、注释也优先中文；只有 API 名称、路径、类型名保持英文。

## 这个仓库是什么

LiteFences：Rust + Win32 的 Windows 桌面启动器。常驻托盘、没有主窗口，
唤出后是一块磨砂玻璃面板，把桌面图标按分组铺开，可以从资源管理器把文件拖进来。

- 主线代码在 `src/bin/litefences_desktop/`，入口 `src/bin/litefences_desktop.rs`（Cargo feature `shell`）。
- `src/core`、`src/storage`、`src/platform`、`src/main.rs` 是早期普通窗口原型（v1），只作参考。
- 配置统一在 `%LOCALAPPDATA%\LiteFencesDesktop\config.json`（见 `store.rs`）。

## 约定

- 改动要能验证：跑 `cargo test --release --features shell --offline --bin litefences-desktop`，
  再用 `--self-test` 确认 `failures=0`。
- 不要用分层窗口（`WS_EX_LAYERED`）画文字，GDI 文字会消失；面板是自绘 + 整帧位图叠加。
- 颜色是 `COLORREF`（`0x00BBGGRR`），不是 RGB 顺序。
- 提交信息用中文，正文说明「为什么改」和「怎么验证的」。
