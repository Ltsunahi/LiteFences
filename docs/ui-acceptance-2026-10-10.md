# UI 可见验收记录（2026-10-10）

## 范围与运行对象

此前两轮单引用验收使用的是 Release 目录中 10 月 5 日已构建的 `undo-visible-acceptance.exe`，不是本次新构建的分组验收 binary。每轮都使用新的 fixture，并由程序建立待撤销历史（Session remove）。这一步只是准备应用内历史，不是物理删除文件。

本记录另含本次单组可见撤销验收：测试配置中的一个组及一个文件引用，经真实撤销输入后恢复。它不代表物理 GUI 删除或 Explorer 拖放验收。

## 第一轮：窗口非文本焦点下的 Ctrl+Z

初始界面显示已保存会话 `#1`，一组且 `0 refs`，撤销操作可用。窗口不在文本输入焦点时发送 `Control_L+z`。截图中随后显示已保存会话 `#2`，状态为“已撤销最近一次移除/删除”；组内出现 `1 ref` 的 `source.txt` tile，撤销按钮变灰禁用。按 `Esc` 后正常退出。

报告：[`undo-visible-report.json`](../artifacts/undo-visible-31720-1791620068592791000/undo-visible-report.json)

## 第二轮：真实点击撤销控件

使用全新 fixture。通过可见界面观察到 element 3 并真实点击撤销控件。截图同样显示已撤销状态、`1 ref` 的 `source.txt` tile，以及实际系统文件图标。按 `Esc` 后正常退出。

报告：[`undo-visible-report.json`](../artifacts/undo-visible-9932-1791620100146346400/undo-visible-report.json)

## 报告与 fixture 复核

两份 JSON 报告均记录：`status=passed`、`timeout_fired=false`、`undo_fixture_seeded=true`、`undo_fixture_restored=true`、`source_unchanged=true`、`disk_restored_exact=true`。对应的 `seed-config.json` 均显示一个名为 `Undo acceptance fixture` 的组，其中有一个指向该轮 `source.txt` 的文件项；每轮目录另有运行后 `config.json` 和 `config.bak.json`。报告和 seed/config 文件可用于只读复核。

两轮报告的 scope 字段均说明：程序化建立历史后，仍需单独观察真实 Ctrl+Z/按钮输入；不属于 Explorer drag acceptance。上面的界面结果记录来自实际截图观察，报告字段用于佐证 fixture 与磁盘状态。

## 单组可见撤销：新构建 binary

使用本次新构建的 `target/release/undo-group-visible-acceptance.exe`。程序通过 `session.delete_group` 在测试配置中准备待撤销历史；没有在物理 GUI 中删除组或文件。两轮分别使用新 fixture，随后在可见窗口中进行真实输入验收。

第一轮在 fresh 窗口中发送 `Ctrl+Z`。截图从已保存会话 `#1`、`0 组 0 引用` 变为已保存会话 `#2`、`1 组 1 引用`；恢复的 `source.txt` 显示系统文件图标，撤销按钮变灰，按 `Esc` 正常退出。

报告：[`undo-visible-report.json`](../artifacts/undo-group-visible-12392-1791621169513348300/undo-visible-report.json)

第二轮在 fresh 窗口中真实点击撤销按钮。截图同样从已保存会话 `#1`、`0 组 0 引用` 变为已保存会话 `#2`、`1 组 1 引用`；`source.txt` 显示系统文件图标，撤销按钮变灰，按 `Esc` 正常退出。

报告：[`undo-visible-report.json`](../artifacts/undo-group-visible-10060-1791621191641709600/undo-visible-report.json)

两份报告都记录 `status=passed`、`timeout_fired=false`、`undo_fixture_seeded=true`、`undo_fixture_restored=true`、`source_unchanged=true`、`disk_restored_exact=true`、`undo_kind=group`。只读复核的 seed/config 数据显示各自测试配置仅含一个组及一个指向该轮 `source.txt` 的文件项；恢复后 `config.json` 含该组，`config.bak.json` 为空。两轮 source 文件 SHA-256 均为 `8B5FD6AD98523D46741A7F8580B5929408590C568CB078E8A466F7F930B07C6D`。这些是测试 fixture 与磁盘状态佐证；历史由程序化配置准备，撤销输入才是在可见窗口中真实操作。

前一窗口的等待轮次报告为 [`undo-visible-report.json`](../artifacts/undo-group-visible-16380-1791621012820592500/undo-visible-report.json)：`status=failed`、`timeout_fired=true`、`undo_fixture_restored=false`、`disk_restored_exact=false`。该轮是超时失败，不能记作通过；后续两轮 fresh 验收各自独立通过。

因此，当前“分组未通过”的结论更新为：以上测试配置中的单组恢复已通过 Ctrl+Z 与真实撤销按钮两种可见输入。结论不外推到物理 GUI 删除、Explorer 混批/cancel、P2 完成功能或性能。

## 未覆盖项与历史记录边界

- Explorer 混批/cancel、通知刷新、桌面兼容、S1：仍未通过；P2 不勾选。
- 不操作桌面文件，不删除 GUI 数据。
- 今天的完整回归已完成，见下方复跑记录；`473` 是 10 月 5 日的历史全功能执行次数。
- 10 月 5 日的应用审批超时属于历史情况；今日分组入口也曾审批超时，获用户明确同意后成功启动；实际输入未绕过审批。
- 原 PID `11300` 今日开始时已不运行。

## 文件哈希基线

以下既有哈希保持不变：

- 原目录主文件：`C0E0383C1B625B0FEBC8F59AEFC01460060962C096B97E083388010906A60534`
- drag 源文件：`9F5C437A591D5C9FF9F1CCD4B0D8BB5B751A88AC337FC8F42BC797F61EB3F850`
- folder child：`20A3B1D48E575FFB57A62A8AB1EBF832B72F813C244CC1916BC3073C86CDF111`

本记录不将这些单引用撤销结果外推为分组删除的物理恢复保证。

## 今日回归与检查环境

Debug/Release 全 target、全 feature 各 549 次测试执行均通过；各入口 passed 数为 45、8、70、70、72、72、1、71、71、9、12、18、6、19、5。共享平台代码在多个入口重复测试，因此总数不是独立用例数。初次口头汇总误记为 649，已按逐入口求和更正。默认 feature 为 170 次，无默认 feature 为 73 次，均通过。全功能与无默认功能的严格 Clippy、rustfmt --check 和 Release 构建通过；Clippy 发现的共享 runner 测试模块顺序问题已修正。

最初沙箱内 Clippy/rustfmt 无法 canonicalize 项目目录并报拒绝访问；完整 Rust 回归也出现 27 项配置会话失败。设置独立空 Clippy 配置仍不能消除路径限制。同一配置用例在获准的沙箱外环境通过，随后在该环境完整 Debug/Release 及 feature 矩阵通过。上述初始失败不抹去、不记为通过，未修改软件目录守卫、Windows 权限或 lint 标准。

新版 Release 的 66 个隐藏配置 HWND 场景通过，只有设计负例 `wrong-expectation-fails` 返回 1；报告：[results.json](../artifacts/config-window-b1ab34e2232d47d880a7b37506225ea5/results.json)。5 次独立隐藏窗口生命周期全部退出码 0；两份 PowerShell/Pester 测试文件共 7 项通过。消息诊断脚本隐藏正例及 3 个非法参数负例通过，证据目录为 `artifacts/loop-script-check-12556-639272183591932222`。隐藏结果不算物理 Explorer 拖入、桌面兼容或 S1 性能验收。

另在全新 `artifacts/ui-sky-3180-1791621574360600500` 打开新版可见普通窗口，通过真实点击创建并保存一个空组（#1），留给真人进行混批拖入及先取消手势。用户最初误选“取消后出现引用”，随后明确更正为“还没有完成取消拖入手势”；现场只读复核仍为一组零引用、save revision=1。不能判为取消缺陷，也不能将准备窗口计作拖入/取消通过；不使用恢复标记，也未动原目录。等待真人操作期间停止窗口输入。

最终正常主程序用默认 feature 重建，不保留 `ui-acceptance` 计数。66 个隐藏配置场景再次通过：[最终默认 Release results.json](../artifacts/config-window-27347c32d92c4cfa849474e1f58da73d/results.json)，5 次生命周期正常退出。新增只读撤销报告核对脚本后，与原两份 Pester 文件共同复跑 13 项全过；其中 6 项仅为合成报告、类型与结构比较检查，不算物理 UI 交互。源字节/报告核对未改变真实夹具。

后续只读检查该可见目录已保存到 revision=9：一组、一条指向 `ui-drag-source/folder/child.txt` 的文件引用，分组位置变化、next_group_id=3。这不是指定 file.txt＋folder 混批结果；期间主 agent 没有输入窗口。已询问用户具体手势，尚不能把这些自主操作认定为取消失败或混批通过，也不回退/删除其测试配置。

核对器另在系统 Windows PowerShell 复验两份真实报告及 6 项合成 Pester，全部符合预期；与 PowerShell 7.6.5 的 13 项结果分别记录，不把重复执行累加为独立用例。沙箱内旧宿主 AuthorizationManager 拒绝的尝试保留；获准在沙箱外运行相同命令，不改执行策略后通过。

## 同日继续开发：打开与手动排序

此次新增功能及实际可见操作分别记于 [打开引用](open-references.md) 和 [手动排序](manual-order.md)。文件双击/Enter 已实际请求系统并出现对应文本窗口；手动后移、保存、退出重启、前移和排序后打开正确文件已复验。新合成排序目录不是 Explorer 物理拖入，旧引用目录保留且源哈希不变。

最终全功能 Debug/Release 各 619 次执行、默认 185 次、无默认 77 次，严格 Clippy/fmt 通过。Release 首次因测试 exe 被正在运行的窗口占用而失败，正常退出后复跑通过；最终默认主程序的 66 个隐藏 HWND 场景和 5 次生命周期通过，[新版报告](../artifacts/config-window-8720b8d8fd804cd4911aa2da25195d6a/results.json)。只有 wrong-expectation-fails 为设计非零负例。432 KiB 主程序已重新构建，测试入口仍独立。

新版可见测试窗口 30 秒采样的私有提交约 7.46 MiB，但整机归一化 CPU 仍 1.834454%，不是 S1 或达标证明，见 [短时资源观察](open-order-resource-sample.md)。尚未解决桌面融合、真实混批/取消、通知/DPI/完整性能验收，不将普通窗口原型描述为成品。

## 同日继续：定位与组间转移

实际定位 file.txt、folder、source-link.lnk，Explorer 均选中父目录内的引用本身；根目录与不存在目标提示可见且引用保留。新夹具 `ui-sky-99004-1791628000000000000` 按钮移动后 file.txt 立即出现在绿色目标组，原 ID 1 追加于 ID 6 后；正常退出/重开保持顺序，源组 ID 为 2、3、4、5，next_item_id 7。详情见 [定位](reveal-reference.md) 和 [组间移动](group-transfer.md)。旧夹具配置及三个现存源文件哈希未变，临时恢复指示已移除。

进一步接入鼠标组间拖动，隐藏真实 HWND 的移动/Esc/空白落点/捕获丢失与四次回读通过，[最终拖动报告](../artifacts/item-drag-603fdd8833aa4926ba2ddcde368f0a05/results.json)。取消不写配置、不生成备份，完整字段和源字节一致。可见窗口最小化及真人输入守卫导致恢复失败，停止输入并请求还原；物理组间拖动尚未通过，不用隐藏结果替代。

最终全功能 Debug/Release 复跑各 708 次执行、默认 204 次、无默认 82 次，全/无默认严格 Clippy、fmt 通过。默认主程序 444 KiB，[66 个既有隐藏场景](../artifacts/config-window-e0c71cdcac544c4c967b937734a78fa7/results.json) 和 5 次生命周期通过。Release 首轮验收测试进程曾以 0xC0000005 退出；Application Error 指向 `wetype_tip.dll_unloaded`，单入口和完整复跑均通过，但根因未完全确认，记录兼容性风险，不修改输入法/系统设置。未重新测 S1 或修复前述 CPU 观察。此轮没有完成整个 P2，也未进入 P3/P4。
