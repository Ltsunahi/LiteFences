# 历史记录：v1 普通窗口原型（2026-09 ~ 2026-10）

> 这份文件是早期 README，记录 **v1 普通窗口原型**（`src/main.rs` + `src/platform`）的开发过程。
> 现在的产品是桌面启动器，说明见仓库根目录 README 与 [desktop-launcher.md](desktop-launcher.md)。


Rust + Win32 桌面整理软件的技术原型。主程序当前提供一个可移动、可缩放、可关闭的原生窗口；P0-02 另提供独立绘制基准，用于比较 GDI 与 Direct2D/DirectWrite 的空表面和 200 图标开销。

主界面当前是普通窗口中的分组预览，支持创建、重命名、移动/缩放、颜色、折叠、文件/目录选择器与 Shell 拖入、双击/Enter 打开、打开所在位置、组内排序、按钮式及鼠标组间移动引用、仅配置的引用移除/分组删除及会话内单步撤销；操作自动保存，有只读/失败提示。后台查询引用存在性和系统图标，打开交给 Windows 系统关联程序处理；软件不移动或删除源文件，不修改注册表或挂载到 Explorer。默认仅使用 `%LOCALAPPDATA%\LiteFences` 存放自身配置；尚无托盘、自启动或桌面图标接管。真实混合 Explorer 拖入/取消、图标组间拖动的物理验收、完整桌面兼容及 S1 性能仍待完成，不能把普通窗口原型称为完整 Fences 替代品。

最新操作只需看[快速试用](快速试用.md)。本轮新增[定位引用本身](reveal-reference.md)和[组间移动](group-transfer.md)：文件/目录/快捷方式实际定位、按钮移动后图标显示及重启保留已经可见复验；原生图标拖动已接入并通过 4 种隐藏 HWND 场景＋4 次回读，真人鼠标验收因旧窗口最小化及输入守卫暂停。最终 Debug/Release 全功能复跑各 708 次执行、默认 204 次、无默认 82 次通过（含共享入口重复），全/无默认严格 Clippy、fmt 通过；默认程序为 454656 字节（444 KiB），不是常驻内存达标证明。最终 66 个既有配置场景、8 次图标拖动/回读、5 次生命周期通过，报告分别为 `artifacts/config-window-e0c71cdcac544c4c967b937734a78fa7/results.json` 和 `artifacts/item-drag-603fdd8833aa4926ba2ddcde368f0a05/results.json`。Release 初轮验收测试进程曾以访问异常退出，事件模块为 `wetype_tip.dll_unloaded`；复跑通过但根因未彻底确认，兼容性风险保留。下方旧阶段计数为历史记录。

2026-10-10 本轮新增两项可用功能：[双击/Enter 打开](open-references.md)与[组内手动排序](manual-order.md)。文件打开及排序前移/后移、重启顺序恢复经过可见窗口复验；测试源文件不变。Debug/Release 全功能各 619 次执行、默认 185 次、无默认 77 次通过（含入口间重复模块，不是独立用例数）；全/无默认严格 Clippy、fmt、最终默认 Release 的 66 个配置 HWND 场景及 5 次生命周期通过。最终隐藏报告为 `artifacts/config-window-8720b8d8fd804cd4911aa2da25195d6a/results.json`。Release 主程序 442,368 字节（432 KiB）；文件大小不是常驻内存达标证据。

2026-10-10 此前撤销检查：Debug/Release 全功能各 549 次测试执行、默认功能 170 次、无默认功能 73 次、此前默认 Release 的 66 个隐藏配置窗口场景、5 次生命周期 smoke 与 13 项 PowerShell 回归通过；全/无默认功能严格 Clippy、格式与 Release 构建通过。549 包含多个入口重复执行的共享模块，不是独立用例数量。程序化准备测试历史后，单引用和整个分组的可见 Ctrl+Z/按钮恢复均通过；不是物理 GUI 删除验收。沙箱内的目录访问失败单独保留，获准在沙箱外复跑项目检查后通过，没有放宽软件安全检查。真实输入桌面短诊断 CPU 为 2.483551%，高 CPU 尚未解决，不是 S1 达标证据。[此前交互及回归证据](ui-acceptance-2026-10-10.md)、[撤销验收入口](undo-removal.md)、[消息诊断及限制](loop-diagnostic.md)。真实 Explorer 混批拖入/取消、桌面融合与完整性能关卡仍未通过；下方各阶段计数为历史记录。

## 构建

新版可见测试窗口的 30 秒短采样：Private Bytes P95 约 7.46 MiB，整机归一化 CPU 仍为 1.834454%，高于预算；不是标准 S1，未宣称优化达标。详见[资源观察](open-order-resource-sample.md)。

需要 Windows x64 和 Visual Studio 的 C++ Build Tools / Windows SDK。项目工具链存放在 `.tools`，不修改系统 PATH。首次安装和下载依赖需要联网。

```powershell
cd E:\note\LiteFences
.\scripts\Setup-Rust.ps1
.\scripts\Cargo.ps1 build --release --locked
.\scripts\Cargo.ps1 -CargoArguments @('fmt', '--', '--check')
.\scripts\Cargo.ps1 -CargoArguments @('clippy', '--all-targets', '--locked', '--', '-D', 'warnings')
```

本机官方下载中断后使用了 `Setup-Rust.ps1 -UseMirror`，组件通过 [RsProxy 公共镜像](https://rsproxy.cn/) 下载，安装器以 Rust 官方 SHA-256 核对。该开关仅影响当前安装进程，不修改全局镜像设置。

仓库保留首次构建生成的 `Cargo.lock`；工具链以 `rust-toolchain.toml` 锁定。含 `--` 分隔符的命令使用上面的数组形式，避免 PowerShell 吞掉分隔符。不要将 `.tools` 和 `target` 加入版本控制。

## 使用

双击分组中的文件图标，或点选后按 Enter 打开。排序时先点选图标，再点击预览上方的“引用前移”/“引用后移”；顺序自动保存，首尾对应方向禁用。打开与排序均不会由 LiteFences 移动源文件，但外部关联程序自身的行为由 Windows/该程序决定。

打开 `target\release\litefences.exe`，自动在后台加载用户配置。Ctrl+N 新建分组，选中列表项目后修改名称、位置、尺寸或颜色并点对应按钮；预览标题可拖动，右下角手柄可缩放。添加文件/目录按钮和拖入预览只保存引用。点击条目后可双确认“移除选中引用（仅配置）”，删除分组也需双确认；Ctrl+Z 或撤销按钮恢复本会话最近一次成功移除/删除，不跨重启，输入框内 Ctrl+Z 仍是文本撤销。Ctrl+S 保存快照。加载中、保存中和只读状态禁用提交；Esc 优先取消拖动或删除确认，否则正常退出并等待已提交保存。源文件从不因这些移除/撤销操作被改动。主窗口位置保存、完整 Tab/Enter 流程和桌面面板仍待实现或验收。

默认构建启用 `config-store`。可用 `--data-dir "绝对路径"` 指定一个已存在的专用目录；不会自动创建该显式目录。`--no-default-features` 可编译不访问配置的基础窗口。

## 验证

```powershell
.\scripts\Test-Smoke.ps1
.\scripts\Test-ConfigWindow.ps1
.\scripts\Get-Environment.ps1
# 对已启动的进程做短测；将 1234 换成实际进程 ID。
.\scripts\Measure-Process.ps1 -ProcessId 1234 -DurationSeconds 30
```

冒烟测试创建隐藏的真实 Win32 窗口并自动退出，不模拟全局按键、不操作 Explorer。通过只说明窗口生命周期能运行，不代表视觉、多屏或桌面集成验收通过。

进程采样脚本现在读取 Private Bytes、Working Set、Private Working Set、GDI/USER 数与按整机逻辑 CPU 数归一化的 CPU 时间。无法取得的指标保留 null；只有 `Run-RenderBench.ps1` 才会另外尝试记录 GPU 和 Explorer/DWM 快照。短采样不能判定完整性能预算达标。

## P0-02 绘制对比

基准依赖以 `render-bench` feature 隔离，主程序默认构建不启用 `windows` COM 绘制依赖。

```powershell
.\scripts\Cargo.ps1 build --release --features render-bench --locked
.\scripts\Run-RenderBench.ps1 -SmokeOnly -Rounds 1
.\scripts\Run-RenderBench.ps1 -Rounds 3 -WarmupSeconds 5 -DurationSeconds 20
```

正式采样会依次显示不抢焦点的测试窗口并自动关闭。期间不要最小化/关闭测试窗口；脚本会拒绝隐藏、最小化或提前退出的采样。默认输出至 `artifacts/render-日期-随机编号/`，用 `-OutputDirectory` 指定一个不存在的目录以保留证据。

五个场景为 `empty`、`gdi-surface`、`gdi-icons`、`d2d-surface`、`d2d-icons`；均使用 1120×680 物理像素窗口。200 个 48×48 图标是固定种子的非透明 BGRA 测试图像，不读取桌面或安装应用的图标。GDI 保留一张绘制表面与独立图标 DIB，D2D 使用 HWND render target、DirectWrite 与独立 bitmap。两个图标路径均释放临时上传 buffer，没有额外保存一份完整 CPU 图标缓存。

原始样本存于 `results.json`，跨轮统计存于 `summary.json`，每轮的首帧与图标数记录在 `*-ready.json`。可独立重算汇总：

```powershell
.\scripts\Summarize-RenderBench.ps1 -ResultsPath '实际目录\results.json'
Invoke-Pester -Script '.\scripts\tests\Summarize-RenderBench.Tests.ps1'
.\scripts\Cargo.ps1 test --features render-bench --locked --offline
```

该测试是绘制资源微基准，未实现真实 Shell 图标提取、用户交互、分组管理或文件变化监听，不能当作 S1 完整应用验收。测量结果和限制见 [P0-02 性能基线](performance-baseline.md)。

## P0-03 桌面调查（部分完成）

```powershell
.\scripts\Cargo.ps1 build --release --features desktop-probe --locked --offline
.\scripts\Test-DesktopProbe.ps1
.\scripts\Cargo.ps1 test --all-targets --all-features --locked --offline
```

`desktop-probe` 与默认主程序隔离。脚本先读取桌面宿主候选与 Shell 视图的数量/自动排列状态，再运行 20 次隐藏的**本进程模拟宿主**恢复测试；每个进程默认 15 秒超时，只结束自身诊断子进程。输出到新的 `artifacts/desktop-*` 目录，可用 `-OutputDirectory` 指定尚不存在的目录。

已通过只读探测、8 项恢复逻辑测试及 20 次模拟恢复，尚未实现或验证跨进程桌面挂载。真实 Explorer 重启次数为 0；Win+D、全屏及混合 DPI 待专用 Windows 环境/人工验收。不会把模拟宿主测试算作 Explorer 重启验收。

详情见 [桌面集成调查](desktop-integration.md) 和 [兼容矩阵](compatibility-matrix.md)。完整范围与后续步骤见 [开发计划书](开发计划书.md)。第一步记录保留在 [P0-01 验证记录](p0-01-validation.md)。

## P1-01 共用数据契约

按计划第 6 节，在 P0 桌面验收尚未完成时继续共用模块，不更改最终桌面产品目标。P1-01 新增纯 Rust 公共库，包含分组/引用数据、稳定 ID、UTF-16 路径、DIP 布局、配置校验、命令/事件与平台接口；当时未接入 UI，现已由 P1-02 接入配置生命周期。

新增 25 项测试全部通过，全 feature 合计 34 项。无新增依赖、线程或常驻服务；这不是完整软件的性能验收。接口、JSON v1 格式、安全边界和复现命令见 [共用契约](core-contracts.md)。下一步是 P1-02 配置编解码与保存恢复，然后再开发可见分组交互。

## P1-02a 配置存储后端

`config-store` feature 提供严格 JSON v1、Windows 安全替换、备份/损坏回退、独占写锁和过期保存拦截。未来版本及未完成的临时文件会停写保留，不自动覆盖。P1-02a 阶段仅实现后端；P1-02b 已默认启用并接入窗口。

```powershell
.\scripts\Cargo.ps1 test --features config-store --locked --offline
.\scripts\Cargo.ps1 test --release --all-targets --all-features --locked --offline
```

P1-02a 时共 65 个测试项通过。故障测试仅在新的项目测试子目录运行，保留产物；跨进程测试只结束自身测试子进程。详见 [配置存储验收](configuration-storage.md)。

## P1-02b 配置生命周期

一个可复用 I/O 线程独占配置文件，UI 使用有界队列和内核事件接收结果；空闲时等待，不安装轮询计时器。保存共享 `Arc<Config>`，不在 UI 线程复制整份配置或访问磁盘。正常退出收到文件锁释放回执后销毁窗口。

P1-02b 时全 feature Debug/Release 各 82 个 Rust 测试通过，另有 10 个隐藏窗口配置场景及 3 次原窗口冒烟。配置测试涵盖保存/重开、分组和引用往返、损坏恢复、未来版本/临时文件保护、写锁冲突及退出等待；不代表视觉、物理键盘、多屏、桌面融合或 S1 性能验收。详见 [窗口配置验收](configuration-window.md)。

## P1-03 基础诊断与键盘焦点修复

新增配置目录内 `diagnostics.log`，最多 64 KiB，满后停止追加；不自动删除、清空或轮换。不记录私人路径和文件名。日志不可用时窗口提示，配置保存独立运行；没有新增线程或依赖。

当前 Debug/Release 全 feature 各 94 项 Rust 测试通过，隐藏配置窗口场景扩充至 12 项。可见窗口已验证鼠标保存、Ctrl+S、Tab、Enter、Esc 的连续操作，以及只读和失败提示。P1-03 完成，进入 P2 分组交互；桌面融合及性能仍按原计划验收。限制与复现记录见 [基础诊断记录](diagnostics.md)。

## P2-01 首个分组（界面验收待补）

已实现创建默认分组、分组列表、自动保存、保存失败回退及重启恢复；最初 99 项 Release 全 feature 测试和 14 个隐藏配置窗口场景通过，后续集成测试数量见下节。可见测试窗口的按钮可被辅助技术识别，但桌面控制接口尚未能操作它，点击、Ctrl+N 与布局截图验收待补，P2-01 在计划书中保留未勾选。详情见 [分组创建记录](group-creation.md)。

## P2-02 分组重命名（可见验收待补）

纯数据层和后台保存链路可重命名已有分组，失败恢复旧标题；普通窗口现有原生列表、名称输入框和提交按钮。Debug/Release 全 feature 各 103 项测试、20 个隐藏窗口配置场景通过，包括真实控件命令改名第二组及重启回读。可见视觉与物理按键仍待验收，故计划书未将 P2-02 勾选完成。边界与复现见 [分组重命名记录](group-rename.md)。

## P2-03 位置编辑与布局预览（可见验收待补）

分组位置可在纯数据层修改、自动保存及失败回退；普通窗口已有 X/Y（DIP）输入框和“应用位置”按钮，还提供可拖动的分组布局预览。拖动时仅预览，松开后保存，Esc 可取消。隐藏 HWND 回归通过 26 个配置场景，包括拖动第二组及重启回读；但桌面上的真实面板尚未接入，可见鼠标、键盘与布局验收也未通过。范围及剩余验收见 [分组移动记录](group-move.md)。

## P2-04 分组缩放（可见验收待补）

已有纯数据宽高规则、自动保存及失败回退；普通窗口支持宽/高输入与右侧预览的右下角拖动手柄。Debug/Release 全 feature 各 115 项测试和 32 个隐藏配置窗口场景通过，但可见操作及真实桌面面板尚未验收，故 P2-04 未标完成。详见 [分组缩放记录](group-resize.md)。

## P2-05 删除分组配置（可见验收待补）

删除只移除软件配置中的分组和引用，不触碰源文件；普通窗口要求对同一选中组连续点击两次确认。保存失败会恢复原组，ID 不复用。当前 Debug/Release 全 feature 各 120 项测试、36 个隐藏配置窗口场景通过；可见点击与焦点流程仍待验收。详见 [分组删除记录](group-delete.md)。

## P2-06 分组颜色（可见验收待补）

普通窗口支持 `#RRGGBB` 输入并在布局预览中显示颜色；自动保存、失败回退和重启恢复已接通。Debug/Release 全 feature 各 124 项测试、40 个隐藏配置窗口场景通过。可见操作与真实桌面面板仍待验收，详见 [分组颜色记录](group-color.md)。

## P2-07 折叠与展开（可见验收待补）

分组折叠只改变配置中的显示状态，不改变尺寸、位置或引用；普通窗口可用按钮折叠/展开，布局预览折叠后仅显示标题条。隐藏窗口和重启回读已通过，可见交互仍待验收。详见 [分组折叠记录](group-fold.md)。

## P2-08a 添加项目引用的数据链路

已实现对单个分组批量添加本地路径引用、同组去重、稳定 ID、自动保存及失败回退。隐藏窗口在独立测试目录验证了添加、重启恢复和源文件字节不变；Debug/Release 各 140 项 Rust 测试、46 个隐藏配置窗口场景通过。文件选择器、Explorer 拖入及可见操作尚未实现，P2 添加项目未标完成。详见 [添加引用记录](add-references.md)。

## P2-08b 文件选择器入口（可见验收待补）

普通窗口现可为所选分组单选或多选文件、`.lnk` 快捷方式，只保存路径引用。隐藏 HWND 已验证新增按钮入口的目标组选择、保存及重启恢复；Debug/Release 各 145 项 Rust 测试和 48 个隐藏配置窗口场景通过。真实系统对话框的可见操作、目录选择与 Explorer 拖入未验收或实现，详见 [文件选择器记录](file-picker.md)。

## P2-08c 目录选择入口（可见验收待补）

普通窗口新增目录选择按钮，只保存现有本地目录路径引用；隐藏 HWND 验证第二组添加、重启恢复和来源目录内容不变。Debug/Release 各 145 项 Rust 测试、50 个隐藏配置窗口场景通过。真实文件夹对话框与 Explorer 拖入仍待验收或实现，详见 [目录选择记录](folder-picker.md)。

## P2-08d 外部拖入安全链路（物理验收待补）

普通窗口布局预览可接收 Explorer 风格的本地文件列表，只添加路径引用；仅向源端报告其允许的 LINK/COPY，绝不报告 MOVE。新增可滚动的逐项结果框，显示提交或跳过原因。隐藏 Shell 数据对象在第二组验证文件和目录拖入、重启恢复及源内容不变；Debug/Release 各 147 项 Rust 测试、54 个隐藏窗口场景通过。真实 Explorer 混合拖入的物理验收仍待补，详见 [外部拖入记录](external-drop.md)。

2026-09-27 可见复验：独立验收程序使 computer-use 能识别窗口。已用真实系统选择器验证创建分组、文件单选与双文件多选、目录和真实 `.lnk` 引用、Esc 取消、重复引用提示和源文件不变；另有文件＋目录混合及 `.lnk` Shell 拖入的隐藏回归。Explorer 跨窗口拖入仍未验收。最新全 feature Debug/Release 各 182 项测试、60 个隐藏窗口场景通过，细节见[可见界面复验记录](ui-acceptance-2026-09-27.md)。

真实 Explorer 拖入仍需按[项目内人工验收清单](explorer-drag-manual-acceptance.md)完成，清单本身不算通过证据。

清单附有[只读磁盘复核脚本](scripts/Test-ExplorerDragAcceptance.ps1)，可在真实手势后核对独立配置和源文件哈希；脚本通过也不能单独证明物理拖入。
