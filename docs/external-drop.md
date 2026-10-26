# P2-08d：外部拖入的安全引用链路（单项物理拖入已验收，混合批次与取消待补）

2026-09-26。普通配置窗口的右侧分组布局预览现注册为 OLE `IDropTarget`。仅当目标落在可写的分组预览上、源对象提供 `CF_HDROP` 本地文件列表，且源端允许 LINK 或 COPY 时才接受；优先报告 LINK，其次 COPY，永不报告 MOVE。一次最多读取 256 条路径，只将通过本地规范路径检查、存在性检查和同组去重的文件、目录、`.lnk` 快捷方式存成引用。界面高亮表示可接受的目标，取消或完成后清除高亮。注册失败时选择器仍可使用，预览给出提示。代码不调用 `DoDragDrop`，也不对源路径做写入、移动或删除。效果协商遵循 [Microsoft IDropTarget::DragEnter 文档](https://learn.microsoft.com/en-us/windows/win32/api/oleidl/nf-oleidl-idroptarget-dragenter)；`CF_HDROP` 之外的 Shell 虚拟对象目前拒绝。

普通窗口另有可滚动的只读“拖入结果”框，逐项列出提交保存或跳过原因；路径只显示在当前用户界面，不写诊断日志。后台保存的最终成功/失败仍由状态栏报告，故“已提交保存”不等于“已持久化”。隐藏 `ui-drop`、`ui-drop-folder` 场景在项目专用目录构造真实 Shell `IDataObject`，对已注册目标执行 MOVE-only、COPY-only、LINK+MOVE 三种协商，并在第二组落下测试文件或目录。验证 MOVE-only 在 DragEnter 和 Drop 均被拒、COPY 和 LINK 可接收、结果框有逐项文字、保存后重启引用及类型正确、源文件哈希和目录项不变。纯策略单测覆盖无路径、无支持效果、组合效果以及反馈路径的长度和换行边界。Debug/Release 全 feature 各 147 项 Rust 测试、54 个隐藏配置窗口场景、全/无默认 feature 严格 Clippy、格式、Release 构建和 5 次旧窗口生命周期 smoke 通过。最新隐藏场景报告：`artifacts/config-window-f050c3715a9440c49f39e565374f5d7e/results.json`。

这些证据不是物理 Explorer 拖入验收：可见应用进程曾启动，但 computer-use 工具没有列出其窗口，已停止该自有测试进程。逐项结果框已通过单项隐藏 HWND 检查，混合拖入的逐项内容和滚动呈现仍未验收；真实 Explorer 指针手势、取消、多选快捷方式、混合 DPI、桌面面板和 P95 响应/内存目标也仍未验收。因此 P2 添加项目仍不勾选完成。下一步应补混合批次边界用例，并在可交互桌面验证真实拖入和选择器，不扩大到源文件操作。

2026-09-27 补充：新增 `ui-drop-mixed` 隐藏场景，同一个真实 Shell `IDataObject` 包含测试文件和目录；检查 MOVE-only 进入及落下均返回 NONE、COPY/LINK 协商、结果框出现两条“已提交保存”、第二组两个稳定引用及重启保留，源文件和目录子文件哈希及目录项不变。累计 56 个隐藏窗口场景，最近报告 `artifacts/config-window-e916859f112d4d3788dc1e9a7af6eb20/results.json`。computer-use 现已借隔离验收入口识别普通窗口，验证了文件/目录系统选择器，但尚未做真实 Explorer 跨窗口拖入；混合批次仍只有 Shell 对象注入证据。详见 `docs/ui-acceptance-2026-09-27.md`。

同日追加 `ui-drop-shortcut`：项目隔离子目录内生成真实 `.lnk` 与其目标文件，由真实 Shell `IDataObject` 向已注册目标执行 MOVE-only、COPY-only、LINK+MOVE 协商；MOVE-only 在 DragEnter/Drop 均拒绝，保存的第二组条目为 `shortcut` 且路径仍指向 `.lnk`，重启后不变。快捷方式与目标文件哈希均未变化。低成本 gpt-6-luna 子 agent 只补独立脚本场景，主 agent 完成 smoke 模式和集成。最新全量隐藏报告为 `artifacts/config-window-d190ebb09bdf4425add6198a9f192710/results.json`：60 个场景，除设计为非零的错误预期场景外无失败。真实 Explorer 跨窗口拖入仍未验收。

真实跨窗口拖入所需的安全夹具、操作和判据已整理在 `docs/explorer-drag-manual-acceptance.md`；该清单尚未执行，不能算作验收通过。

2026-09-27 追加边界回归：隐藏 Shell `IDataObject` 测试现在经过 `DragEnter`、分组内 `DragOver`、标题栏（组外）`DragOver`、回到分组的 `DragOver`、`DragLeave`，分别断言 COPY、NONE、COPY 的反馈；文件、目录、快捷方式与混合批次均走此路径。Debug/Release 全功能各 182 项 Rust 测试、60 个隐藏配置窗口场景、全功能/无默认功能严格 Clippy 与格式检查通过；最新报告 `artifacts/config-window-65453c9a168646829d8b8ecdf9e99a8b/results.json`。人工验收脚本还核对测试源目录清单，Windows PowerShell 5.1 上的旧单引用负例正确返回 `fail`。这些仍不是 Explorer 跨窗口真实指针手势证据，P2-08d 保持未完成。

2026-09-27 可见 Release 人工验收补充：用户将项目夹具中的 `file.txt`、目录和 `source-link.lnk` 手动拖入同一个 Release 验收分组。只读脚本 `scripts/Test-ExplorerDragAcceptance.ps1` 验证配置恰含 file、directory、shortcut 三种引用，且四个夹具的 SHA-256/内容均未变化。用户起初没有看到重复项反馈；在界面显示“配置已保存”后再次拖入 `source-link.lnk`，用户报告“显示了跳过”。当时现有 Release UI 尚未渲染条目 tile，因此界面没有显示项目 tile。此段只描述用户操作和脚本核验，不代表桌面集成或整个 P2-08d 已完成。

同日完成后续 Debug 可见预览验收：在另一隔离窗口，分别通过真实文件/目录选择器添加 `source.txt`、`source-link.lnk`、`folder`，截图中同一分组显示“3 项”、F/L/D 类型标记及文件名；窗口正常退出。预览按可见槽位提取最多 24 个短标签，不在 `WM_PAINT` 中读取文件或提取 Shell 图标。这先修复“引用已保存但分组空白”；随后再加入后台系统图标缓存，见下段。旧 Release 验收进程仍在运行，显示旧 UI，不替换运行中的可执行文件。

图标补充验收：单个后台线程以 64 项有界请求队列查询 Shell 图标，UI 只查最多 256 个条目的进程内缓存并按需重绘；失败或未完成时继续显示 F/L/D 标记。第二个隔离 Debug 窗口实际选择 `child.txt`、`source-link.lnk`、`folder` 后，可见文档、带箭头快捷方式、黄色文件夹三个系统图标及名称，保存后正常退出。Debug 和独立构建目录的 Release 全目标测试、全功能与无默认功能检查、格式检查均通过。此缓存暂按条目数限制，尚无实际字节计量、文件变化失效或长期压力曲线，不宣称达到最终 8 MiB/性能目标；桌面面板、混合批次物理拖入和取消手势仍未完成。

用户随后允许关闭旧版 Release 测试窗口并用相同隔离目录重开。关闭前界面显示“配置已保存（本次会话 #4）”，磁盘脚本通过；构建新版 Release 后，测试入口只读取项目 `artifacts/ui-sky-41820-1790505171507617800`，画面直接显示原有三项及文件夹、文档、快捷方式系统图标。重开后只读脚本再次返回 `pass`，21 项检查零失败。新版窗口保留供用户检查；恢复目录的临时指示文件已删除，配置与源文件未删改。另以新版隔离 Release 主程序运行 60 个隐藏 HWND 场景，只有预期负例非零，报告 `artifacts/config-window-bbe89348208542cabdc2b75866793499/results.json`。

2026-09-28 补充：独立只读验收脚本 `scripts/Test-MixedExplorerDragAcceptance.ps1` 已实现，范围限制在项目 `artifacts/ui-sky-*` 子目录；核对一组两条准确 UTF-16 引用、源目录清单与类型，以及两个 SHA-256。合成正例通过，当前空组负例失败；真实单次 Explorer 混合文件与目录拖拽仍待用户执行。另新增后台元数据判定：引用缺失或类型变化，会在可见组内显示红色 `!` 和失效计数；Debug 合成失效配置已完成启动时的可见验收。可见窗口每 30 秒触发重绘，缓存超过 60 秒后只向后台线程重查可见条目；单元测试确认过期请求去重及结果更新。尚未实现 Shell 通知式即时刷新、重新定位或从 UI 移除；不据此宣称 P2 完成。

同日继续以该合成 Debug 配置做运行中双向可见验收：缺失时为红色 `!`；仅在项目测试夹具中临时创建 `gone.txt`，等待缓存重查后正常文件图标出现；再移除这个刚创建的临时文件，等待并重新激活该窗口后红色 `!` 恢复。配置引用没有改变，测试窗口退出，临时恢复指示文件已移除；原有 Release 混合拖入窗口保持打开。为保证从 Explorer 等应用返回时重绘，补充 `WM_ACTIVATEAPP` 前台激活触发。此实现属于有界轮询而非 Shell 即时通知，重新定位仍未实现；新激活分支尚无单独的可见二次验收。

同日稍后补“仅移除软件引用”：GroupId+ItemId 精确定位与事务回退，普通窗口可点击 tile 选中，需再次点击“移除选中引用（仅配置）”确认，源文件从未作为删除目标。隔离 Release 隐藏 HWND 回归 62 场景通过，新增案例检查第一次点击不删除、第二次才保存移除、重启保留、源文件哈希和内容不变，报告 `artifacts/config-window-f1a76421813a4f7986b2c643175aee7f/results.json`。另一个可见 Debug 测试窗口只确认初始 tile 与禁用按钮布局；computer-use 输入连续报 `failed to activate captured window`，故没有完成可见点选/双确认，不将隐藏结果替代其验收。当前 Release 混合拖入窗口仍是较早构建，未热更新本功能。
