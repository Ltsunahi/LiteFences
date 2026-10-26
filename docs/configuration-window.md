# P1-02b 窗口配置生命周期验收

日期：2026-09-23。范围：普通顶层窗口与配置存储的纵向集成，不是桌面接管或分组编辑。

## 当前行为

- 正常启动在后台创建/打开 `%LOCALAPPDATA%\LiteFences`，加载配置并显示来源、分组/引用数量、可写/只读/失败状态。只创建应用末级目录；环境变量缺失、相对路径、UNC、目录不可访问等均报错，不换成可写空布局。
- `--data-dir` 覆盖为已有绝对路径专用目录，不创建该目录；后端继续拒绝末级重解析目录等不支持位置。此开关便于隔离测试，不代表支持网络盘或恶意同用户进程隔离。
- “保存配置”按钮和 Ctrl+S 提交当前快照。当前只有加载与手动保存，没有分组编辑或布局自动保存；后续 P2 才接入变更及合并保存。主窗口位置仍不保存。
- 加载中、保存中、关闭中及只读状态禁用保存；写入失败停止后续保存并显示错误。UI 不把读取失败当作空白配置，也不自动覆盖未来版本或残留临时文件。
- 关闭请求等待加载/已提交保存，再通知工作线程释放 FileStore，收到 Stopped 后销毁 HWND。保存失败取消当次退出，停留显示错误；用户再次关闭可退出。没有强制取消正在执行的 Windows 文件调用；底层 I/O 长期阻塞时退出可能一直等待。
- 第二实例显示写锁 Busy 并禁止保存；尚未实现激活第一实例。

## 线程与资源

默认一个 UI 线程、一个可复用 I/O 线程，不按保存次数增加线程。请求队列容量 1、结果队列容量 2；UI 使用 try_send/try_recv，不等待磁盘或 join。worker 空闲时阻塞接收请求，UI 用 MsgWaitForMultipleObjectsEx 同时等待消息和自有 auto-reset event，正常模式没有周期计时器。状态改变才触发重绘。

工作线程仅持有 Arc 管理的唤醒事件，不持有 HWND、窗口指针或 RefCell；最后一个所有者关闭事件句柄。窗口拖动等原生模态循环结束后仍能收到保留的事件通知。保存使用 Arc 共享不可变 Config，避免在 UI 线程复制完整路径列表。存储层原始快照和工作内存上限延续 P1-02a，不据此声称 S1 内存达标。

config-store 现为默认 feature，新增所需 Win32 事件及键盘 API feature，没有增加新的第三方 crate。`--no-default-features` 保留原基础窗口。旧 `--smoke-test` 不启动存储或访问用户配置。

## 验证

Windows 11 25H2 x64（26200.9457），Rust 1.98.1：

- Debug/Release 全 feature 各 82 项 Rust 测试通过：原有 65，加保存追踪 6、参数隔离 2、worker 5、窗口会话状态 4。
- 会话测试验证加载前关闭不写空配置、重复保存/过期回执、真实临时文件冲突导致保存失败后取消退出、加载错误禁用保存。
- worker 测试验证后台回读、退出回执前释放写锁、未来版本拒绝写入、显式目录不存在及锁冲突、丢弃通道后释放锁。
- `Test-ConfigWindow.ps1` 的 10 个隐藏 HWND 场景通过：空配置保存并在保存中关闭、重开轮换、非空分组/引用往返、损坏主配置恢复、未来版本/残留临时文件/坏数据只读、目录不存在、写锁冲突、错误预期返回非零。
- 原窗口 `Test-Smoke.ps1 -Iterations 3` 通过；严格 Clippy（全 feature / 无默认 feature）、fmt、Release 构建通过。

隐藏窗口测试主动派发 WM_PAINT，不把不可见区域的 UpdateWindow 当作可视绘制证据。测试只写项目内新建的 `artifacts/storage-tests/` 和 `artifacts/config-window-*`，保留产物及 results.json。源引用字节哈希不变；未重启 Explorer、写用户真实配置或修改启动项。测试看门狗只用于隐藏诊断，失败时最多结束其自身诊断子进程。

本轮没有视觉截图验收、物理鼠标/键盘自动化、混合 DPI、多屏、桌面融合、完整保存时刻强杀/断电或 S1/S2/S3 性能测量。默认用户目录代码已实现，但未对真实用户配置目录进行运行测试。基础日志限额等 P1 剩余项仍待补齐，不把 P1-02 完成等同整个产品或 P0 已通过。

## 复现

```powershell
.\scripts\Cargo.ps1 -CargoArguments @('test','--all-features','--locked','--offline')
.\scripts\Cargo.ps1 -CargoArguments @('test','--release','--all-features','--locked','--offline')
.\scripts\Cargo.ps1 -CargoArguments @('build','--release','--all-features','--locked','--offline')
.\scripts\Test-ConfigWindow.ps1
.\scripts\Test-Smoke.ps1 -Iterations 3
```

按复杂度分工：两个 gpt-6-luna（medium）分别处理保存请求追踪与 worker 集成测试，工作范围不重叠；主 agent 负责 Win32 等待循环、线程/句柄生命周期、退出失败语义、集成审查和最终验证。未启动额外高成本审查任务。
