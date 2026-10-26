# 撤销移除/删除：技术验收

## 行为范围

- 仅作用于配置：撤销最近一次成功移除的单个引用，或删除的整个分组；不恢复、复制或修改源文件。
- 每个会话仅保留一条记录，新的成功移除/删除会替换它。记录不跨重启；仅单步撤销，无重做。
- 撤销按原 ID、原位置恢复；原位置超出当前列表长度时夹到末尾。若 ID 或引用目标重复，拒绝恢复且配置无变化。
- 恢复后需保存成功才消费记录。保存失败或 worker 断开时回滚本次恢复、保留记录；当前状态不可撤销，避免把未持久化的状态当成成功。
- Ctrl+Z 在文本输入控件聚焦时交给原生编辑撤销；其它焦点才触发配置撤销。无可撤销记录时无作用。

## 验收结果

- 已通过 Debug、Release 全 target/全 feature 各 234 项测试，strict clippy 全/无默认 feature，以及 fmt。
- 已通过 66 个隐藏 HWND smoke；最终报告：`artifacts/config-window-95caa150069d4b0a8a980fab0b048546/results.json`。
- 新增四个场景分别覆盖第二组单个引用和整个含引用分组的恢复与重启；回读完整字段、ID、顺序、UTF-16 路径及源文件哈希。
- 已验收新增撤销按钮的可见禁用布局，以及重启后零撤销记录时 Ctrl+Z 无作用。
- 已可见验证名称输入框的 Ctrl+Z 只撤回未提交后缀，不改配置；5 项 Session 撤销测试包括提交前断开和待确认保存时断开的确定性故障注入。

## 未验收项

2026-10-10 已在独立测试配置中完成有历史时的单引用及整个分组 Ctrl+Z/按钮可见恢复，详见下方追加记录。历史由程序准备，不代表物理 GUI 删除已验收；真实 Explorer 混批拖入/取消仍待完成。隐藏 HWND smoke 不能替代可见输入证据。

## 独立可见验收入口（2026-10-05 续）

`undo-visible-acceptance.exe` 仅由 `ui-acceptance` feature 构建，无参数，不读取恢复指示文件或用户配置。每次新建 `artifacts/undo-visible-<当前 PID>-<时间戳>`，其中只含自己的 source.txt、单组单引用配置及原始 seed-config.json。平台入口核对 canonical 父目录和当前 PID，拒绝其它目录；两项测试覆盖名称边界与普通项目目录拒绝。

启动后，测试入口直接调用 Session 移除测试引用并后台保存，**这只准备历史，不是物理移除验收**。只有存储确认成功、撤销记录实际可用后才记为 seeded；恢复也需观察到保存成功并消费记录，不能把失败回退后的原配置当作恢复。入口不会自动调用撤销按钮或注入 Ctrl+Z。

```powershell
.\scripts\Cargo.ps1 -CargoArguments @('build','--release','--offline','--features','ui-acceptance','--bin','undo-visible-acceptance')
```

由 computer-use 启动它后，等“配置已保存”且撤销按钮可用：第一轮从非文本焦点测试 Ctrl+Z，第二轮用另一份新 fixture 点击“撤销移除/删除”。每轮看到 source.txt 图标恢复、保存完成后 Esc 正常退出；60 秒 timer 仅请求正常退出，未完成保存不会强制销毁。退出后报告 `undo-visible-report.json` 同时核对历史已准备、已恢复、源字节不变、解码后完整配置等于种子；四项缺一即失败，纯函数测试遍历全部 16 种组合。正常提前关闭不必等 timer，但不能跳过保存确认。

本轮 computer-use launch_app 的应用审批超时，刷新 list_windows 没有目标，也没有产生该次 fixture；未执行任何真实按键/点击。已询问用户是否同意该测试入口并处理应用审批，**仍未验收通过**，不会通过别的输入方式绕过审批。

另从 CLI sandbox 自动运行“不输入”负例，正常 60 秒结束：`artifacts/undo-visible-27376-1791192739897915600/undo-visible-report.json` 中 seeded=true、restored=false、source_unchanged=true、disk_restored_exact=false、status=failed。磁盘一个空分组，源文件仍存在。此结果证明准备历史不会自动恢复或冒报通过，不代表可见交互；CLI 启动与等待不使用 computer-use 注入输入。

最终 Debug/Release 全功能各 473 次执行包含共享模块重复测试；默认功能 170 次、无默认功能 73 次也通过。新增纯状态测试覆盖失败移除回退、待确认恢复、失败恢复、仅添加同目标但记录未消费等不能通过的情况；只有成功保存且消费记录才记录恢复。66 个隐藏场景报告为 `artifacts/config-window-a20fbae33254482c810a8caeae4697a3/results.json`。原验收窗口 PID 11300 的配置及拖入源文件哈希保持不变；不关闭或热更新它。

## 2026-10-10 可见恢复通过及分组入口

上述应用审批与 PID 状态是 10 月 5 日的历史记录。今日开始时旧进程已退出；获用户批准后使用新的独立夹具，完成单引用的 Ctrl+Z 和按钮恢复。随后新增 `undo-group-visible-acceptance.exe`，与单引用入口共享夹具/完整配置比较及四条件判定；仅在 `ui-acceptance` feature 下存在，名称守卫分别校验 reference/group 模式与当前 PID。分组入口调用实际 Session 删除测试配置中的组来准备历史，从不删除源文件、不自动撤销。

用户批准启动分组测试后，两份新夹具均完成真实可见输入：`artifacts/undo-group-visible-12392-1791621169513348300` 使用 Ctrl+Z，`artifacts/undo-group-visible-10060-1791621191641709600` 使用按钮。两轮均从已保存的零组零引用恢复到一组一引用，显示系统文件图标、保存 #2、撤销禁用，Esc 正常退出。对应 `undo-visible-report.json` 均为 passed，四项判定为 true，timeout_fired=false；完整证据见 [今日可见验收](ui-acceptance-2026-10-10.md)。等待期间自动退出的 `undo-group-visible-16380-1791621012820592500` 则为 failed/timeout_fired=true，保留负例，不计入通过。

```powershell
.\scripts\Cargo.ps1 -CargoArguments @('build','--release','--offline','--features','ui-acceptance','--bin','undo-visible-acceptance','--bin','undo-group-visible-acceptance')
```

今日完整回归：Debug/Release 全功能各 549 次执行（含不同入口重复测试），默认 170 次、无默认 73 次、两组严格 Clippy、fmt、66 个隐藏 HWND 场景、5 次生命周期与 7 项 PowerShell 回归通过。沙箱内目录解析失败导致的一轮检查/会话回归失败保留；获准在沙箱外执行相同项目检查后通过，不修改 FileStore 的 canonical 校验或 Windows 权限。新增入口及重构不增加默认应用的依赖、线程或功能。

最终重新用默认 feature 构建正常 `litefences.exe`，保证不含验收计数；再次通过 66 个隐藏配置场景（报告 `artifacts/config-window-27347c32d92c4cfa849474e1f58da73d/results.json`）和 5 次生命周期。

新增只读 `scripts/Test-UndoVisibleAcceptanceReport.ps1`，仅接受项目 artifacts 的直接 undo fixture 子目录，拒绝点段/重解析路径、错误 PID/模式、非布尔标志。检查固定源字节、完整配置与种子的结构相等（对象字段顺序无关，数组顺序有意义）及报告是否自洽；不自行执行恢复，不证明物理键盘/鼠标输入，旧版缺少 undo_kind 的报告不能使用该新核对器。正例与超时负例均按预期核对。低成本子 agent 新增 6 项合成数据回归，主 agent 补旧版 PowerShell 对象兼容、定时退出及进程资源释放后复跑；与原 7 项共同为 13 项通过，合成产物不冒充真实 UI 证据。

```powershell
.\scripts\Test-UndoVisibleAcceptanceReport.ps1 -DataDir 'E:\note\LiteFences\artifacts\undo-group-visible-12392-1791621169513348300'
.\scripts\Test-UndoVisibleAcceptanceReport.ps1 -DataDir 'E:\note\LiteFences\artifacts\undo-group-visible-16380-1791621012820592500' -ExpectedStatus failed
Invoke-Pester -Script '.\scripts\tests\UndoVisibleAcceptanceReport.Tests.ps1'
```

兼容复核：PowerShell 7.6.5 下上述三份 Pester 共 13 项通过；系统 Windows PowerShell 下只读正例/超时负例和新增 6 项 Pester 也通过。后者是相同六项的重复执行，不另计独立用例。最初沙箱内旧宿主报 AuthorizationManager 拒绝访问，获准在沙箱外运行原命令后通过；没有修改执行策略、系统权限或绕过安全提示。
