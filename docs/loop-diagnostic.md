# P2 消息循环短诊断（非性能达标证据）

仅在 `ui-acceptance` 测试 feature 中编译。默认普通程序不编译计数；不引入常驻采样线程、消息日志或外部服务。事件使用饱和 `u64`；父窗口消息直方图固定 1,024 个计数槽，较大 ID 仅累计总数，退出后最多输出 32 类。另记录跨线程消息及其中 WM_GETOBJECT 的数量，不保存发送者身份、消息参数、源路径或输入文本。

```powershell
.\scripts\Cargo.ps1 -CargoArguments @('build','--release','--offline','--features','ui-acceptance','--bin','loop-diagnostic')
.\target\release\loop-diagnostic.exe --seconds 30
.\target\release\loop-diagnostic.exe --seconds 30 --visible
.\scripts\Test-LoopDiagnostic.ps1
```

每次在项目 `artifacts/loop-diagnostic-<PID>-<时间戳>` 新建一份 1 分组、0 引用的隔离配置，拒绝外部 `--data-dir`；默认隐藏，时长限 1–60 秒。timer 3 仅请求正常关闭，存储工作线程停止后才输出 stdout JSON 并保存 `loop-report.json`。若用户提前关闭，报告为 `closed_early`，不是完整定时采样；后台 I/O 未确认时不强制销毁，实际完成时间可能长于请求时长。

`wait_worker/input/other` 是主等待调用的返回类别；`session_changes/icon_changes` 是 drain 得到更新的次数；`window_messages` 仅计自有父窗口回调，**不是所有子控件消息数量**；`paints` 是父窗口 WM_PAINT 回调次数；`refresh_timers` 是临时 timer 2 的触发次数。等待仍按 Windows 消息/事件机制阻塞，没有通过 sleep 限速。[等待返回值](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-msgwaitformultipleobjectsex)、[计时器消息](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-settimer)

进程 CPU 是本进程 kernel/user 时间增量，包含启动、正常关闭及所有自有线程；墙钟为同段 Instant。归一化除以所有处理器组的逻辑 CPU 数，查询失败则 CPU 百分比不可用。不把它等同于预热后的 S1 稳态。[GetProcessTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes)、[处理器组计数](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getactiveprocessorcount)

`initial_window/closing_window` 查询自有 HWND 的 IsWindowVisible/IsIconic；前者证明 WS_VISIBLE 状态，不证明未被遮挡、正在用户输入桌面或 computer-use 可捕获。[可见性语义](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-iswindowvisible)

## 2026-10-05 的短对照

| 启动方式与请求状态 | 秒数 | 全机归一化 CPU | 等待返回 | 父窗口绘制 / 刷新 timer |
| --- | ---: | ---: | ---: | ---: |
| CLI，隐藏 | 30 | 0.003253% | 4 | 0 / 0 |
| CLI，WS_VISIBLE=true | 30 | 0.022733% | 6 | 5 / 1 |
| CLI，WS_VISIBLE=true | 60 | 0.006503% | 7 | 6 / 2 |
| CLI，WS_VISIBLE=true，再次尝试列窗口 | 60 | 0.011382% | 9 | 6 / 2 |

依次对应 `artifacts/loop-diagnostic-7136-1791189034162070300`、`loop-diagnostic-15392-1791189216206177700`、`loop-diagnostic-7396-1791189420149905400`、`loop-diagnostic-14564-1791189551206373400` 下的 `loop-report.json`。所有样本 icon_changes 为 0，session_changes 为 2；没有文件引用或 Shell 图标负载，不能外推到 200/1,000 项。

computer-use 本轮仅调用 list_windows：首次枚举返回时定时窗口已经退出；随后重跑仍未列出新诊断窗口，但列出了原 `litefences-ui-acceptance.exe`。因此没有成功执行目标的截图/UIA 读取，不把后二轮当作“捕获后的 CPU 对照”，也不证明前轮与旧窗口处在相同显示上下文。只读 SessionId 查询表明调用 PowerShell 和旧进程都在 Session 1，单靠这个值仍不能确认窗口站/桌面相同。

旧验收窗口 30 秒的 1.120826% 样本继续保留，不作废、不用较低 CLI 样本替换。这些 CLI 测试中没有明显自有父窗口绘制/等待风暴；此处记录的是交互桌面追加复现之前的结论。没有修改资源预算或把 P2/S1 勾成完成。

追加自身上下文查询后，2 秒 CLI 报告 `loop-diagnostic-7348-1791190091212689600/loop-report.json` 明确为 WinSta0 上的 `CodexSandboxDesktop-*`，`receives_input=false`；WS_VISIBLE=true 也不能把这份数据认作用户输入桌面的表现。早先短对照未记录此字段，不能逐份倒填上下文，更不能凭 SessionId 相同消除这个限制。自身查询只借用 GetProcessWindowStation/GetThreadDesktop 句柄并查询 UOI_NAME/UOI_IO；固定 256 UTF-16 单元缓冲，失败/无权限返回不可用，不打开、切换或更改任何桌面。[对象名称/输入状态](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getuserobjectinformationw)、[句柄所有权](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getprocesswindowstation)

为同环境复验增加无参数 `loop-diagnostic-visible.exe`，由 computer-use 的 launch_app 启动，固定 60 秒、独立配置、拒绝所有参数，退出后只在新目录保存报告。它与 CLI 共享 runner、fixture、CPU 计数及关闭逻辑，不通过权限修改或桌面切换强行把 sandbox 窗口显示出来。需要另建该测试入口，是因为 launch_app 不支持命令行参数；正式程序的 CLI 和用户配置行为没有改变。

脚本隐藏正例及三个非法参数负例通过；正例回读配置、报告、无 pending 文件与窗口隐藏状态，负例退出码 2 且没有新增 fixture。测试 stdout/stderr 保留在 `artifacts/loop-script-check-*`，不删除产物。首次脚本的 Split-Path 参数组合和空 stderr 读取错误已修正，没有把失败次数计作通过。

## 真实输入桌面的追加复现

computer-use 仅 launch_app/list_windows，未截图、读取目标 UIA 树或注入输入；三份报告自身查询均为 WinSta0 / Default / receives_input=true。不能因此假设没有其他可访问性客户端在运行。

| 60 秒空分组测试 | 全机归一化 CPU | 输入唤醒 | 父窗口消息 | 绘制 |
| --- | ---: | ---: | ---: | ---: |
| 原等待方式 | 1.945768% | 351,533 | 88,947 | 6 |
| 临时取消 INPUTAVAILABLE，先探测队列 | 2.068796% | 339,188 | 85,878 | 6 |
| 撤回等待改动，增加来源与线程计数 | 1.898129% | 355,818 | 90,006 | 6 |

依次为 `artifacts/loop-diagnostic-24944-1791190504684888400`、`loop-diagnostic-12400-1791191065999829200`、`loop-diagnostic-38152-1791191348645135400` 的报告。第二轮的 wait_input 含队列探测直接就绪，并不全是等待 API 返回；其 wait_strategy 字段明确记录实验方式。取消标志没有证明收益，**已撤回**；正式循环保持原 MWMO_INPUTAVAILABLE，不添加 sleep，不吞掉消息。

第二轮 73,513 条消息为 WM_GETOBJECT。第三轮共有 89,663 条来自其它线程，77,067 条是跨线程 WM_GETOBJECT；仅查询本进程当前 UI 线程 CPU，增量为 10.21875 秒，全进程为 18.265625 秒。线程读取失败时为 null。父窗口中没有该消息的自定义业务分支，仍交给 DefWindowProc，不抑制可访问性支持。WM_GETOBJECT 属于 MSAA/UIA 对象查询；InSendMessageEx 能证明来自其它线程，**不能证明来自其它进程或锁定具体客户端**。目前证据指向可访问性查询负载与高 CPU 同时存在，不足以把全部 CPU 因果归属于它，也不证明正常用户环境达标。[WM_GETOBJECT](https://learn.microsoft.com/en-us/windows/win32/winauto/wm-getobject)、[跨线程发送语义](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-insendmessageex)、[线程 CPU](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getthreadtimes)

脚本新增直方图顺序/总量、跨线程计数包含关系及环境可空字段检查。首次集成因误用 environment.name 以及 PowerShell 方法调用中 -replace 的逗号优先级失败，修正为 station_name/desktop_name 与括号后复跑通过：`artifacts/loop-script-check-32052-639267882551155328`。保留失败证据，不计为通过。后续仍需无额外可访问性查询的同桌面对照与标准 S1，不终止外部客户端、不更改系统权限来完成测试。

## 2026-10-10 复测（仍未解决）

新版 computer-use 技能下，仅 launch_app/list_windows 启动并列出既有 Release 诊断入口，没有读取该窗口 UIA 树、截图或注入输入。报告 `artifacts/loop-diagnostic-15384-1791620167858851000/loop-report.json` 自身环境仍为 WinSta0 / Default / receives_input=true，正常定时退出。

60.1221369 秒内进程 CPU 增量 23.890625 秒、UI 线程 14.203125 秒，16 逻辑 CPU 归一化为 2.483551%。输入唤醒 422,373 次，父窗口消息 106,843 条，跨线程 106,491 条，其中跨线程 WM_GETOBJECT 91,528 条；绘制 6 次，图标结果变化 0，刷新 timer 2 次。仍是一个空分组、包括启动/退出的短样本，不是 S1。没有减少可访问性支持、更改等待方式或调整预算。只能确认此测试环境查询负载持续存在，仍不能识别调用方、排除其它客户端或把所有 CPU 归因于这些查询。
