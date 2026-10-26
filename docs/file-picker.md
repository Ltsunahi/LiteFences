# P2-08b：文件选择器入口（单选、多选与快捷方式可见验收通过）

2026-09-26。普通窗口为已选且可写的分组增加“添加文件引用…”按钮，调用 Windows 原生文件选择对话框；支持单选或多选现有文件。保存的是路径引用，不移动、复制或删除源文件。`.lnk` 按扩展名保留为 `Shortcut` 引用，显式设置 `OFN_NODEREFERENCELINKS` 防止选择器改为返回其目标路径；设置 `OFN_DONTADDTORECENT` 避免为选中文件创建最近使用记录。取消选择不调用保存。只接受本地盘符绝对路径，网络共享、设备路径和复杂 Shell 虚拟对象仍在当前模型支持范围外。

原生对话框返回的 Explorer 风格 UTF-16 缓冲区经有界解析：单选为完整路径，多选为目录加多个文件名。解析器保留原始 UTF-16，拒绝空、畸形、非本地路径及超预算输入。选中后沿用 P2-08a 的批量添加、同组精确路径去重、后台自动保存及失败回退。一次选择器缓冲区最多 65,536 个 UTF-16 码元；过大选择提示分批添加。选择器在 UI 线程内以模态方式运行，未来若需要更复杂的 Shell 行为需评估专用 STA 与交互延迟。

隐藏 `ui-add` 在显式测试目录中向新增按钮入口注入同格式的测试选择结果，不弹出真实对话框；它选择第二组，验证只添加到第二组、重启恢复和源文件哈希不变。Debug/Release 全 feature 各 145 项 Rust 测试、48 个隐藏配置窗口场景、严格 Clippy（全/无默认 feature）、格式、Release 构建和 5 次旧窗口生命周期 smoke 通过。报告：`artifacts/config-window-a4b202dee68e4df0872636715f11199e/results.json`。低成本 gpt-6-luna 子 agent 只编写独立 UTF-16 解析器与测试；主 agent 完成 Win32 接入、修正一条畸形测试样本并集成验收。

2026-09-27 用独立项目测试目录和 computer-use 对真实 Windows 文件对话框完成单选、Esc 取消、一次选择两个文件及真实 `.lnk` 的可见验收；`.lnk` 回读为 `shortcut` 且保存快捷方式路径，不解引用为目标路径。重复选择不新增引用，界面现给出明确提示。隐藏 `ui-shortcut` 用同目录真实快捷方式夹具验证第二组添加、重启恢复及双方哈希不变。证据见 `docs/ui-acceptance-2026-09-27.md`。混合 DPI、Explorer 跨窗口拖入、真实桌面面板及性能预算仍未验收，因此 P2 添加项目不标完成。Windows API 格式与快捷方式标志依据 [Microsoft OPENFILENAMEW 文档](https://learn.microsoft.com/en-us/windows/win32/api/commdlg/ns-commdlg-openfilenamew)。
