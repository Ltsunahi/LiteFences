# P2-08d：Explorer 跨窗口拖入人工验收（单项已执行，混合批次待执行）

这份清单是待执行步骤，不是完整通过记录。单独拖入文件、目录、快捷方式与重复快捷方式的用户操作已在 2026-09-27 验证；文件＋目录**同一次**拖入及取消手势尚待执行。`computer-use` 当前只提供单窗口坐标拖动，不能据隐藏 `IDataObject` 测试宣称已验证这些真实 Explorer 指针手势。所有源对象限定在项目 `artifacts/`，不得从桌面、下载或个人目录拖入；不删除测试产物。

2026-09-27 实测补充：把独立 Explorer 与 LiteFences 窗口并排后，`computer-use` 的拖动调用在目标点离开 Explorer 窗口时直接拒绝，未执行拖放；源文件与隔离配置未变化。故以下步骤仍须由人在可交互桌面完成，不能将该工具拒绝当作产品失败或验收通过。

## 准备

1. 使用普通用户权限，运行 `target/release/litefences-ui-acceptance.exe`（若需重建：`./scripts/Cargo.ps1 build --release --features ui-acceptance`）。它不接收命令行参数，每次在项目 `artifacts/ui-sky-<进程 ID>-<时间戳>` 新建独立配置目录，不打开普通用户配置。
2. 记录本次进程 ID 与对应的 `ui-sky-*` 目录；在界面新建一个分组，把 Explorer 与 LiteFences 并排放置，确认右侧预览可见。
3. 在 Explorer 中仅打开 `E:\note\LiteFences\artifacts\ui-drag-source`。其中 `file.txt` 与 `folder/child.txt` 是专用夹具。验收前分别计算 SHA-256：`file.txt` 应为 `9F5C437A591D5C9FF9F1CCD4B0D8BB5B751A88AC337FC8F42BC797F61EB3F850`，`folder/child.txt` 应为 `20A3B1D48E575FFB57A62A8AB1EBF832B72F813C244CC1916BC3073C86CDF111`。目录下顶层仅应有 `file.txt` 和 `folder`。

## 人工操作与判据

1. 在 Explorer 中同时选中 `file.txt` 与 `folder`，拖向 LiteFences 右侧分组预览。只在指针反馈为链接或复制、且目标分组高亮时释放；若反馈为移动或目标不明确，先取消手势，不释放。期望界面逐项显示 2 条已提交引用，最终状态确认保存完成。
2. 只读检查本次独立目录的 `config.json`：分组恰有 2 条新增引用，类型分别为 `file`、`directory`，UTF-16 路径分别指向上述两个夹具；源目录仍保留原文件与子目录。重新计算两个 SHA-256，应与准备阶段完全一致；子目录内仍只有 `child.txt`。
   在加入快捷方式之前，可运行下面的只读混合批次复核；`pass` 只能证明磁盘结果，是否确属同一次物理手势还须记录操作和可见反馈：

   ```powershell
   .\scripts\Test-MixedExplorerDragAcceptance.ps1 -DataDir 'E:\note\LiteFences\artifacts\ui-sky-<实际进程 ID>-<实际时间戳>'
   ```
3. 再从 `E:\note\LiteFences\artifacts\ui-shortcut-source` 拖入 `source-link.lnk`，同样仅在链接/复制反馈时释放。期望新增 1 条 `shortcut` 引用，路径为 `.lnk` 本身，不是 `source.txt`，并验证两者哈希不变。重复拖入同一 `.lnk` 应显示跳过，引用数不增加。
4. 可选的 MOVE-only 拒绝检查：若能让 Explorer 仅提供移动效果，只有在指针明确显示“不允许”时才释放；若显示“移动”，立即取消。确认 `config.json` 与源内容均未变化。不要以隐藏 smoke 的 MOVE-only 结果代替这一步。
5. 按 Esc 正常退出，确认保存完成并保留配置；如需检验重启恢复，可显式运行正式程序 `target/release/litefences.exe --data-dir "<本次完整 ui-sky 目录>"`，不传参数的验收程序会另建目录，不能用于回读同一配置。记录 Windows 构建号、显示缩放、窗口布局、指针效果、界面反馈、配置目录、源文件前后哈希及任何失败截图。混合 DPI、多屏、Win+D 与 Explorer 重启仍按计划分别验收。

完成第 3 步后，可在项目目录执行只读磁盘复核：

```powershell
.\scripts\Test-ExplorerDragAcceptance.ps1 -DataDir 'E:\note\LiteFences\artifacts\ui-sky-<实际进程 ID>-<实际时间戳>'
```

把占位符替换为本次真实完整目录。脚本只接受 `artifacts/` 下直接子目录，核对恰好一个分组、三种引用的原始绝对路径、两个源目录的项目清单及四个源文件 SHA-256；失败时返回 JSON `fail` 与非零退出码。`pass` 只证明保存的配置与夹具符合预期，**不能证明实际发生了 Explorer 指针拖动**，仍须记录上述可见手势与反馈。脚本不启动程序、不改动源文件或配置。

未完成以上真实手势和磁盘核查前，P2-08d 与“添加项目”均不得标记完成。隐藏自动化最近报告见 `docs/external-drop.md`。
