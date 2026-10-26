# P2 可见交互复验与图标预算加固

2026-10-05，仍在普通窗口的 P2 添加项目/失效引用关卡，不表示桌面融合或 P2 全阶段已完成。

## 本次可见操作

使用默认 Release `target/release/litefences-ui-acceptance.exe`；数据只取项目隔离目录 `artifacts/ui-sky-99002-1790600000000000000`，其中一个分组、一条指向测试 `file.txt` 的引用。恢复标记每次启动后移除，不影响普通用户数据目录。

- computer-use 本次正常选择窗口并点击：文件系统图标出现；点击 tile 后出现选中框，移除按钮从禁用变为可用。
- 第一次点击“移除选中引用（仅配置）”只显示二次确认文案，仍为一条引用；没有执行最终移除点击。
- Esc 取消并保留窗口、条目和选中框。发现原状态栏仍残留“再次点击确认”，已修正为“已取消删除/移除确认；配置和源文件均未改变”，新版可见复验确认正确。
- 折叠后只保留标题，正常退出再启动仍为折叠状态；展开后原文件图标恢复且自动保存，源文件内容未变化。
- 随后正常退出、重建最终含预算加固的 Release，再打开同一隔离目录，原文件系统图标仍正常。该窗口留供下一步最终移除验收。

折叠/展开会合法保存配置，故不声称整轮配置字节从未变化。最终展开配置 SHA-256 为 `9AC60DB1904F13C5FB4F588BCA38C977E93FD683CDA68CC58ACB29E013C30CED`；测试源文件 SHA-256 仍为 `9F5C437A591D5C9FF9F1CCD4B0D8BB5B751A88AC337FC8F42BC797F61EB3F850`。旧 `ui-sky-34276-1790593845612198100` 混合拖入窗口本次开始时已经不存在，不再沿用“仍打开”的记录。

computer-use 的删除确认规则要求最终删除动作前单独确认；虽然这里只删除测试配置引用，仍未执行该最后动作。不把隐藏 HWND 验证视为这一步可见验收。

## 实现与回归

低成本 gpt-6-luna 子 agent 仅修改 `src/platform/preview.rs`，增加共享 `item_content` 几何计算及常见 DPI、非法 DPI、极端坐标和无空间测试。主 agent 将绘制、tile 命中、隐藏移除夹具接入同一函数，避免重复公式漂移。

主 agent 增加明确的 Esc 取消状态；隐藏 `ui-remove` 覆盖首次只确认、Esc 取消并保留引用/可保存状态、重新确认、最终移除和重启回读，仍检查源文件不变。最终 Release 的 62 个隐藏窗口场景通过，唯一非零是预期负例 `wrong-expectation-fails`；报告：`artifacts/config-window-f95969ef7c2248069999af8d62b5c9b6/results.json`。Debug/Release 全目标全 feature 测试、全 feature 和无默认 feature 严格 Clippy、格式检查通过。

图标后台请求队列、结果队列、全部在途请求各有上限（64）；完成但尚未被 UI 接收的结果也算在途。请求队列满时不会留下假 pending，容量恢复后可以重试。单测覆盖未消费结果、容量恢复、重复请求、缓存淘汰与真实系统图标位图检查。

缓存仍最多 256 项，新增 8 MiB 的**记账预算**：mask/color 位图的 `stride × height`、两份 UTF-16 路径及每条 256 字节结构/容器余量。更新不重复计账；超预算按 FIFO 淘汰；无法计量或单图超过预算的 HICON 释放后显示类型占位。它不是精确进程 Private Bytes，也不包含 Windows 不透明 USER/GDI 分配器和 GPU 开销，不能替代计划第 5 节性能实测。[BITMAP 字段说明](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-bitmap)

位图检查仅在后台进行，`GetIconInfo` 创建的 mask/color 副本均调用 `DeleteObject`。Shell 图标线程补齐 STA COM 初始化/同线程清理；初始化失败不调用 Shell，而显示未知/类型占位。字符串形式超过 `MAX_PATH` 的引用保留存在性结果，暂不调用该形式的 Shell 图标 API。[位图副本所有权](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-geticoninfo)、[Shell 图标 API 的后台与 COM 要求](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shgetfileinfow)

## 尚未通过的关卡

实际最终移除按钮点击、同批文件+目录的物理 Explorer 拖入、拖入取消与可见混合结果仍未完整验收。通知式文件变化、Shell 扩展挂起/崩溃隔离、混合 DPI 实机、完整 S1 与长期资源曲线也未通过。当前 30/60 秒低频重查机制仍是临时方案，不是计划要求的通知式增量刷新。本次没有删除任何源文件，也未进入 P3/P4 或勾选 P2 完成。

## 同日续验：用户批准的最终移除已完成

以上“尚未点击最终移除”描述的是批准前的历史状态。收到明确同意后，在隔离窗口重新首次确认，再点击最终移除。截图状态为“配置已保存（本次会话 #1）；仅移除了软件引用，源文件未改动”，选中项消失、移除按钮禁用；正常 Esc 退出和重启后仍为 1 分组、0 引用。

磁盘回读：主配置 0 引用、备份 1 引用、next_item_id 为 2。移除后的主配置 SHA-256 为 `C0E0383C1B625B0FEBC8F59AEFC01460060962C096B97E083388010906A60534`；源文件仍为上述 `9F5C...F850`。移除的是软件引用，源文件未删除；原引用还保留在备份中。后续可见 Ctrl+Z 无历史检查没有恢复旧会话引用。

新增撤销实现与限制见 `undo-removal.md`。新版可见窗口中撤销按钮禁用、不会遮挡预览；重命名输入已不带列表 ID 前缀。名称输入框追加未提交的 `-undo-check` 后，物理 Ctrl+Z 撤回后缀；没有点击重命名或保存按钮。初始一次输入被工具以“user input was detected”拒绝后先重新观察，没有重复执行旧索引。真实有撤销记录时的物理恢复尚未执行，不算通过。

最终 Debug/Release 全目标全 feature 各 234 项测试及严格 Clippy 两种 feature 组合、格式检查通过；66 个隐藏 HWND 场景均符合预期，唯一非零是预期负例，报告 `artifacts/config-window-95caa150069d4b0a8a980fab0b048546/results.json`。5 次隐藏生命周期 smoke 通过。恢复指示文件已移除。真实同批拖入/取消、通知式刷新及 S1 等未通过关卡保持不变。

## 消息诊断与撤销验收准备的追加记录

真实输入桌面的 60 秒空分组复现中，CPU 约 1.90%–2.07%，父窗口只绘制 6 次但有大量跨线程 WM_GETOBJECT 查询。撤回没有证明收益的等待标志改动，不吞消息、不关闭可访问性；不能用 sandbox 的低值宣称问题已解决。完整数值、API 语义和因果限制见 `loop-diagnostic.md`。

新增独立撤销验收入口及“不输入”负例见 `undo-removal.md`。computer-use 启动新程序时应用审批超时，随后未发现目标；未进行按键或按钮恢复。物理有历史撤销、同批 Explorer 拖入/取消仍未通过。现有 PID 11300 和配置保持，主配置 SHA-256 仍为 `C0E0383C1B625B0FEBC8F59AEFC01460060962C096B97E083388010906A60534`；两份混合拖入夹具哈希也未变化，无恢复标记残留。

最新 Debug/Release 全功能各 473 次执行（共享平台模块在多个测试入口重复执行，不是独立用例数量）、默认功能 170 次、无默认功能 73 次、两组严格 Clippy、格式、66 个隐藏 HWND 场景、5 次生命周期及 7 项 PowerShell 测试通过。最新 HWND 报告：`artifacts/config-window-a20fbae33254482c810a8caeae4697a3/results.json`；消息诊断脚本正例/三个负例报告目录：`artifacts/loop-script-check-33024-639267897423238965`。旧版 Pester 曾将负中位数按文本表示比较（-10.0 与 -10），现先要求数值再比较数值相等，复跑 7 项全过；没有修改性能汇总算法或预算。
