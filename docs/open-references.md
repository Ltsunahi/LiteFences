# 打开引用

普通窗口中，双击 tile 可打开该引用；选中后按 Enter 也可打开。tile 命中后程序以稳定的 `GroupId` 和 `ItemId` 从当前会话重新查找引用，不以屏幕位置或文件名代替身份。隐藏 smoke/验收窗口禁止执行外部打开。

打开通过 `ShellExecuteExW` 请求 Windows 以系统 `open` 语义处理普通文件、目录和 `.lnk` 快捷方式。它只打开引用，不移动或修改源文件。调用复用现有的一个 STA Shell worker 线程；UI 只投递请求并显示状态，不等待文件检查或 Shell I/O。一次最多允许一个待处理、执行中或尚未消费结果的打开请求；繁忙时提示稍后重试。此功能不增加依赖；虽然启用了 `Win32_System_Registry` 类型绑定，打开流程不写注册表。

调用不传命令行参数，不使用 `runas` 或 zone bypass。执行前检查目标存在及类型是否仍符合引用记录；缺失、类型变化、权限不足、无系统关联程序等情况会显示提示。路径达到 260 个 UTF-16 code unit 时，本版拒绝打开且不截断路径。Shell 调用成功仅表示 Windows 接收了打开请求，不保证外部应用窗口实际出现。COM 初始化或 Shell handler 若长时间阻塞，当前实现无法取消该调用，worker 也可能因此长期无法处理后续请求。

## 验证状态

现有自动化测试覆盖并通过纯策略函数、停止后拒绝请求、单请求队列行为及按稳定 ID 查找引用。

2026-10-10 可见复验：正常关闭已保存的旧测试窗口（#10），新版恢复原 `artifacts/ui-sky-3180-1791621574360600500` 一组、一条 child.txt 引用。实际双击文件 tile 后，状态先为“正在请求系统打开”，再为“打开请求已交给系统处理”；Windows 返回 `child.txt - Notepad` 窗口，画面中可见对应测试夹具文本。随后在 tile 选中、父窗口获得焦点时实际按 Enter，另一次打开请求完成，返回的记事本窗口 ID 变化。未输入或修改外部文档；记事本的独立控制审批超时，不绕过，未自动关闭它。

打开前后主配置 SHA-256 均为 `71558269690D080F7B462218E41FD7403BE8F80EB9BBD52CC16265557BCC82DA`，child.txt 均为 `20A3B1D48E575FFB57A62A8AB1EBF832B72F813C244CC1916BC3073C86CDF111`。临时恢复标记已移除，旧配置保留。文件的双击与 Enter 路径已有可见证据；目录、快捷方式及错误提示的真实可见验收尚不能据此算通过。

## Microsoft 文档

- [ShellExecuteExW function](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecuteexw)
- [SHELLEXECUTEINFOW structure](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-shellexecuteinfow)
