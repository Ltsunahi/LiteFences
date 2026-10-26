# P0-03 桌面集成调查与验证

日期：2026-09-22。状态：**部分完成，未通过 P0-03 出口**。本调查完成时未启动 P0-04/P1，不把模拟测试视为桌面版可行性结论。后续用户确认无专用环境并要求继续，已按计划第 6 节开展 P1 共用模块，详见计划第 14 节；此处桌面验收状态不变。

## 本轮实现

- 独立 `desktop-probe` 可选诊断程序，默认产品构建不包含它。
- `--snapshot`：枚举 Progman、WorkerW、Shell_TrayWnd 及其指定视图子窗口，只记录类名、HWND、PID、可见性、矩形和 DPI；不读取其他应用标题或桌面文件名。
- 通过 `IShellWindows → IServiceProvider → IShellBrowser → IFolderView` 读取桌面视图项目总数、自动排列状态。不选择、定位、移动或隐藏原生图标。
- 纯 Rust 宿主恢复状态机：清除旧身份、每次请求分配代次、拒绝迟到结果、限制重试次数、Shell 提示只触发探测。
- `--self-test`：本进程创建隐藏父/子 HWND，销毁父窗口，确认子窗口也失效；模拟宿主缺失、重试耗尽、重建、拒绝旧结果。共 20 次。不发送全局键盘输入或对 Explorer 进行故障注入。
- `Test-DesktopProbe.ps1` 给每个诊断进程默认 15 秒超时；只结束脚本自己启动的超时子进程，保留不完整日志。Shell 不可用输出 HRESULT，不把空结果计为兼容通过。

诊断使用 STA，所有 COM 引用在 `CoUninitialize` 前释放。它是独立短时进程，不是产品 Shell 后台工作者的最终实现。COM 超时没有被当作调用已经安全取消；脚本只隔离和结束自有诊断进程。

## 已观测事实

环境：Windows 11 家庭中文版 25H2，构建 26200.9457，x64；Shell 窗口 DPI 120（125%），桌面视图矩形 2560×1440。硬件/驱动细节沿用 [P0-02 性能基线](performance-baseline.md)。本轮不是内存基准，没有重新测量 S1。

原始记录：

- [只读快照](measurements/p0-03-20260922/snapshot.json)
- [模拟恢复结果](measurements/p0-03-20260922/self-test.json)

2026-09-22 22:46（Asia/Shanghai）的一次快照中：

- `GetShellWindow()` 返回 Progman，视图层级为 Progman → SHELLDLL_DefView → SysListView32；三者 PID 均为 12724。
- COM 桌面视图读取成功，项目数 80，自动排列关闭。没有记录项目路径、名称或位置。
- 找到多个 WorkerW，其中部分属于其他 PID，且本次均不可见。因此**类名匹配不是宿主身份验证，更不是可挂载协议**。快照的 `windows` 只是候选清单，不自动挑选挂载目标。
- 快照不具备原子性，窗口可能在枚举期间消失或被重用；不能把落盘的 HWND 用于后续操作。

本轮没有调用跨进程 `SetParent`，没有发送未公开的桌面消息，没有隐藏原生图标、修改注册表、注入 Explorer 或操作桌面文件。

## 测试结果与边界

| 测试 | 结果 | 不能据此推断 |
| --- | --- | --- |
| 恢复状态机单元测试 | 8/8 通过 | 真实 Explorer 已恢复 |
| 同进程隐藏父/子 HWND 销毁重建 | 20/20 通过 | 跨进程桌面挂载可靠 |
| 主程序隐藏窗口生命周期回归 | 5/5，退出码 0 | Win+D、全屏或混合 DPI 通过 |
| 普通窗口显示与最大化 | 中文可见，最大化后重绘正常 | 游戏/独占全屏覆盖正常 |
| 普通窗口最小化、显式激活恢复 | 最小化被工具确认，恢复后可访问焦点在原型窗口 | 后台恢复不抢焦点 |
| Esc 退出 | 已退出，无自有诊断/原型进程遗留 | 长时间稳定性通过 |
| 全 feature Rust 测试 | 9/9（上述 8 项 + 原有图标样本 1 项） | 完整业务流程通过 |
| Release / Clippy `-D warnings` / fmt | 通过 | 桌面兼容矩阵通过 |

可见交互使用 computer-use 检查；没有把最大化当作全屏。截图在任务中查看，不保存含用户其他窗口内容的图像到仓库。主程序仍是原有 P0-01 面板，诊断能力未接入其运行时。

`RecoveryAction::Attached` 仅表示**接受宿主身份**，不表示窗口完成挂载。`Detached` 在迟到结果/无效计时器场景表示忽略本次输入，不会清除已接受的新身份。状态机不负责 OS 事件订阅、实际计时器、退避间隔、COM 重取或跨进程窗口重建；这些均待适配器实现。未来适配器还需取消旧计时器，避免旧计时器影响新一轮等待。

HWND+PID 也不是永久稳定 ID；句柄可复用。将来每次使用前须重新验证宿主、类、进程与层级，销毁/切换代次时废弃 COM 引用，不能只用 `IsWindow` 判断身份。

## API 边界与后续实验

公开 Shell 查询接口可以取得视图信息，不自动提供围栏容器。本轮仅验证读取；图标选择、定位、恢复策略仍待 P0-04，不能提前称为“接管”。[IFolderView 文档](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-ifolderview)、[FindWindowSW 文档](https://learn.microsoft.com/en-us/windows/win32/api/exdisp/nf-exdisp-ishellwindows-findwindowsw)

`SetParent` 不会自动调整 WS_CHILD/WS_POPUP，跨进程的 DPI 上下文也有限制。Explorer 窗口结构不是稳定的扩展协议，后续实验挂载必须隔离、默认关闭，失败回退到自有普通窗口。[SetParent 文档](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setparent)

`TaskbarCreated` 只能触发重新探测，不能直接宣布桌面 Shell 视图就绪。本轮只测试了对应状态机事件，没有订阅或伪造系统广播。[任务栏通知文档](https://learn.microsoft.com/en-us/windows/win32/shell/taskbar)

本机未发现 Windows Sandbox 可执行文件；VMware 清单仅有 Ubuntu 虚拟机，运行中虚拟机数为 0。因此按计划第 8 节，**未在日常桌面执行 Explorer 重启或挂载故障注入**。需要用户指定可重置的 Windows 专用测试环境，才继续实现并验收实验适配器。

Win+D 另需人工操作：本会话 computer-use 规则禁止 Windows 键及组合键；不采用其他注入方式绕开限制。手工执行 50 次往返，分别记录普通面板和后续实验桌面模式，不能互相替代。

待执行矩阵见 [兼容验收清单](compatibility-matrix.md)。P0-03 仍未勾选；此文不是 P0-04 产品路线决策。

## 复现

```powershell
cd E:\note\LiteFences
.\scripts\Cargo.ps1 build --release --features desktop-probe --locked --offline
.\scripts\Test-DesktopProbe.ps1
.\scripts\Cargo.ps1 test --all-targets --all-features --locked --offline
.\scripts\Cargo.ps1 -CargoArguments @('clippy', '--all-targets', '--all-features', '--locked', '--offline', '--', '-D', 'warnings')
```

输出默认保存到新的 `artifacts/desktop-*` 目录；指定 `-OutputDirectory` 时必须是不存在的路径，避免覆盖历史记录。`--snapshot` 成功运行不保证 Shell 查询成功，应检查 `folder_view.status`。直接运行可执行文件没有外部超时保障，建议用脚本。

本次构建：`desktop-probe.exe` 130560 字节，SHA-256 `3C88DD63C9C3ABA7A5C762E1CB485A886F53EBA4A598C6B49E3962FEB5C7E783`；主程序 120832 字节，SHA-256 `1296CBDA73089FD6DDAD1FFCE614F1009A63A74CBA1B50C4AA35D36640B623E0`。不以文件大小代表运行内存。

## Agent 成本控制

本轮只启用 1 个 `gpt-5.6-luna`（medium），负责单一纯 Rust 恢复状态机和测试。主 agent 负责 Win32/COM、进程隔离、集成、实机验证与文档；审查时补上“同一恢复轮次中上一请求迟到”与零 PID 用例。不额外启动高成本审查 agent。
