# P0-01 实施与验证记录

日期：2026-09-21。范围：开发计划书 P0-01，环境记录与最小 release 原型。该步骤完成不代表 P0 阶段整体通过。

## 交付

- `target/release/litefences.exe`：Rust + Win32 原生窗口，临时 GDI 绘制。
- `src/main.rs`、`src/platform/mod.rs`：消息循环、窗口回调、资源释放、Per-Monitor V2 初始化、DPI 变化处理、错误提示、隐藏窗口冒烟入口。
- `Cargo.toml`、`Cargo.lock`、`rust-toolchain.toml`：版本锁定和按需 Windows API feature。
- `scripts/Setup-Rust.ps1`、`Cargo.ps1`：项目本地工具链安装/构建，不修改系统 PATH。
- `scripts/Get-Environment.ps1`、`Measure-Process.ps1`：环境和单进程指标采样基础工具。
- `scripts/Test-Smoke.ps1`：可重复执行的隐藏 HWND 生命周期测试。

当前行为：可移动、缩放、关闭的中文状态窗口；正常空闲时阻塞等待 Win32 消息，无轮询计时器或持续动画。程序不持有用户文件、不更改桌面图标、不写配置或启动项、不挂载 Explorer。

默认运行打开窗口；`--smoke-test` 在隐藏状态创建窗口、调整大小、派发绘制消息、关闭窗口并检查回调状态。未知参数返回退出码 2。当前窗口布局尚未做人工视觉验收。

## 构建环境

| 项目 | 实测信息 |
| --- | --- |
| 系统 | Microsoft Windows 11 家庭版 中文版，25H2，26200.9457，x64 |
| CPU | AMD Ryzen 9 9955HX 16-Core Processor |
| 进程可用逻辑 CPU 数 | .NET Environment.ProcessorCount 报告 16；不据此推断物理核心或整机处理器总数 |
| OS 可见内存 | 33,484,439,552 bytes，约 31.18 GiB |
| GPU / 驱动 | AMD Radeon 610M / 32.0.13034.7001；NVIDIA RTX 5080 Laptop / 32.0.16.1664 |
| 额外显示适配器 | GameViewer Virtual Display Adapter / 15.6.5.199；后续显示兼容验证需记录其影响 |
| Rust | rustc 1.98.1，48a229cea，2026-09-01 |
| Cargo | 1.98.1，797e8a9bc，2026-08-05 |
| MSVC 工具 | 14.51.36231 |
| Windows SDK | 10.0.26100.0 |
| 依赖 | windows-sys 0.61.2；windows-link 0.2.1 |

工具链安装在项目 `.tools`，已加入忽略规则。官方下载曾出现连接中断；改用 RsProxy 下载，安装器 SHA-256 与 Rust 官方一致：`6f4bef66261261fcb43131be8720bab817d403a09edec7455c371974b90bdb7e`。最终 release 可在依赖缓存完整时 `--locked --offline` 构建。

来源：[Rust 安装指南](https://rust-lang.org/tools/install/)、[Rustup 环境变量](https://rust-lang.github.io/rustup/environment-variables.html)、[RsProxy 镜像说明](https://rsproxy.cn/)、[微软 windows-rs](https://github.com/microsoft/windows-rs/)。镜像仅在安装进程中设置，不持久修改全局镜像配置。

## 验证结果

| 检查 | 结果 | 证据范围 |
| --- | --- | --- |
| `cargo build --release --locked --offline` | 通过 | 最终源码能够生成 release exe |
| `cargo fmt -- --check` | 通过 | 格式检查 |
| `cargo clippy --all-targets --locked --offline -- -D warnings` | 通过 | 未出现 Clippy 警告 |
| 最终 release 隐藏窗口生命周期 | 5/5 通过，退出码均为 0 | HWND 创建、调整大小、WM_PAINT、关闭与消息循环退出 |
| PowerShell 工具语法 | 通过 | 五个脚本均通过解析；环境脚本最终版本实际运行成功 |
| 采样工具自检 | 通过 | 对 PowerShell 进程采样 2 秒，5 样本；不是 LiteFences 性能数据 |
| 不存在的进程 | 通过 | 返回 `process_not_found` 结构化结果 |
| 缺失指标 | 正确标识 | Private Working Set 为 null，不以总 Working Set 替代 |

最终冒烟测试每次总历时（毫秒）：190.2587、122.1656、138.2218、128.8928、122.8366。这包含启动与退出测试开销，不能解释为“启动至可交互时间”或与 800 ms 预算直接比较。

最终 exe：120,832 bytes（118 KiB）。SHA-256：`DD076898D15A60341A0143325677F8F80BA9C49D7E297D6E05E4DA1F8DEFC433`。文件大小不等于运行内存。

## 尚未验证

- 界面视觉效果、真实键盘/鼠标交互、混合 DPI 和显示器拔插。DPI 处理代码已存在，不代表兼容矩阵已通过。
- Direct2D/DirectWrite、200 个图标与缓存的内存增量，S1/S2/S3 标准负载。
- Private Working Set、GPU、Explorer/DWM 增量，以及 10 分钟空闲/8 小时稳定性数据。
- 桌面宿主挂载、真实图标管理、Win+D、Explorer 重启和全屏覆盖。
- 自动化性能脚本当前按进程可用逻辑 CPU 数归一化；P0-05 必须核对整机逻辑 CPU 数、采样权限和测量范围，再用于计划中的全机 CPU 预算。

因此 P0-02～P0-05 继续保持未完成，不能宣称软件已经比 Fences 更省内存，或已经实现 Fences 式整理。

## Agent 使用与成本控制

本轮只启动一个低成本子 agent：Russell，`gpt-5.6-luna`，medium 推理。任务限定为两个独立 PowerShell 脚本，未复制整个对话。主 agent 处理 Win32/unsafe、工具链、编译和最终集成。

审查后修正了 SDK/MSVC 版本探测、私有工作集的不可靠原生结构、CPU 首末样本时间差，并确保环境检查不触发 Rust 自动安装。没有可用的逐 agent token 实测数据，因此不虚构节省比例。

下一步严格按计划执行 P0-02：空窗口、绘制表面、200 图标三种负载的测量与对比；P0-01 的程序保留为原生窗口基线。
