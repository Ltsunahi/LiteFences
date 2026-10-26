# P0-02：绘制与图标资源基线

日期：2026-09-22。范围：开发计划书 P0-02。正式 3×5=15 次采样全部完成，每次 21 个有效样本，共 315 样本。另有一轮宿主进程快照补测，单独存档。

## 本机结果与决策

下表单位为 MiB。内存三列是每轮 P95 的跨轮中位数；GDI 对象是跨轮 P95 中位数。

| 场景 | Private Bytes（私有提交） | Working Set（总驻留） | Private Working Set（私有驻留） | GDI 对象 |
| --- | ---: | ---: | ---: | ---: |
| 空窗口 | 1.71 | 9.95 | 1.20 | 2 |
| GDI 表面 | 4.88 | 13.66 | 4.20 | 7 |
| GDI + 200 图标 | 7.14 | 16.01 | 6.53 | 207 |
| Direct2D/DirectWrite 表面 | 57.68 | 41.47 | 11.54 | 6 |
| Direct2D + 200 图标 | 62.75 | 44.62 | 14.61 | 6 |

同轮差值的中位数：GDI 表面相对空窗口增加 **3.17 MiB** 私有提交；D2D/DWrite 表面增加 **55.97 MiB**；在各自表面上加入 200 图标，GDI 增加 **2.27 MiB**，D2D 增加 **5.14 MiB**。这些包含实现、字体引擎、驱动及分配器的综合影响，不能全部解释为像素本体。

GPU 快照三轮一致：D2D 表面专用 19.13 MiB、共享 1.51 MiB；D2D + 图标专用 21.57 MiB、共享 3.28 MiB。空窗口及 GDI 场景未发现对应 GPU 计数器实例，记为未知；不宣称 GPU 占用为 0，也不把上述数值与进程内存简单求和。

空闲 CPU 的各场景跨轮中位数均为 0%；唯一非零轮次为 D2D 图标场景约 0.00488%（整机逻辑 CPU 归一化）。采样时间短、计时分辨率有限，不能据此承诺永久零 CPU。

技术判断：当前机器上，GDI 更适合作为低内存静态界面的候选基线；继续保留现有 GDI 主程序原型，Direct2D 放在独立 benchmark 中，不默认引入。D2D 图标场景的私有提交量超过原计划 S1 的 50 MiB 初始预算，应在后续功能实现时持续关注，不能通过只报较小的私有工作集掩盖。由于当前不是真实 S1，既不据此判定 S1 失败，也不修改原预算。

本次没有验证透明/半透明面板、动画、实际文件图标或多屏绘制质量。后续若这些功能需要 D2D，将重新评估渲染后端，而不将本机单次技术选型外推到所有环境。未测试 Fences，不作跨软件胜负结论。

## 方法与边界

独立 `render-bench.exe` 通过 `render-bench` feature 构建，避免把测量入口和 Direct2D COM 绑定引入默认主程序。所有场景使用同一个 release 二进制，每轮启动独立进程；按轮轮换场景顺序。

| 场景 | 固定工作负载 |
| --- | --- |
| empty | 普通 Win32 空窗口，白色背景 |
| gdi-surface | 1120×680 的 32-bit DIB 绘制表面、标题及 5 块背景 |
| gdi-icons | GDI 表面加 200 个独立 48×48 DIB 图标 |
| d2d-surface | 默认类型 ID2D1HwndRenderTarget、DirectWrite 标题及 5 块背景 |
| d2d-icons | D2D 表面加 200 个独立 ID2D1Bitmap 图标 |

全部图标采用确定性的 opaque BGRA 图像，200 个图像彼此不同，总原始像素 1,843,200 bytes（1.7578 MiB）。临时像素上传 buffer 用完即释放；没有额外保留完整 CPU 图像副本。这是渲染/图标存储的受控负载，不含 Shell 提取、缩略图、文件路径元数据或完整 LRU 缓存。

每场景 3 轮，每轮首帧成功后预热 5 秒，再按 1 秒间隔采样 20 秒。只在可见、未最小化状态测量；主动隐藏、最小化或退出会使该轮失败。窗口不主动抢焦点。当前机器窗口 DPI=120（125%），绘制坐标/D2D target DPI=96，固定 1120×680 物理像素；本测试不修改系统显示设置。

上述是 P0 渲染微基准。计划中的 S1 是 5 个真实分组、200 个本地项目、1920×1080/100% DPI、60 秒预热、10 分钟采样，尚未执行，不能把短采样当作 S1 通过。

## 统计口径

- 每轮分别取 Private Bytes、总 Working Set、Private Working Set 的 P95 与峰值，保留每个原始样本；表中将显示三轮 P95 的中位数。
- 同轮配对计算 `surface - empty`、`icons - surface` 的 Private Bytes 差值，再取差值的中位数。结果可为负，不能自动归零。
- Private Bytes 为私有提交量，Private Working Set 为驻留私有页，两者不可混称为实际 RAM，更不可相加。
- `PROCESS_MEMORY_COUNTERS_EX2` 使用完整字段（包含 SharedCommitUsage）；x64 结构大小实测为 96 bytes。计数器读取失败时 PWS 为 null，不能以总 Working Set 代替。[微软 EX2 文档](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex2)
- CPU 为首末样本的进程 CPU 时间差 / 实际墙钟差 / `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)`，报告全机逻辑 CPU 归一化值。短时间内 0 仅代表计数器未观察到增量，不证明永久零开销。
- GPU 使用 Windows GPUProcessMemory provider 的稳态快照，独立报告 DedicatedUsage/SharedUsage；无对应实例或查询失败记为未知，不当作 0。GPU 共享内存不和进程内存直接相加。
- 每轮保留启动前、采样前和退出后的 Explorer/DWM 私有提交量与工作集快照。当前为日常桌面环境，宿主的变化可能来自其他窗口，不能全归因于基准应用。

Direct2D 创建方式采用微软文档中的 HWND render target 路径；DEFAULT 类型允许系统选择实际后端，不能只凭 API 名称断言 GPU/硬件选择。本次 GPU provider 的 LUID 已保存在原始数据，未进一步映射到具体显卡型号。[微软 Direct2D 文档](https://learn.microsoft.com/en-us/windows/win32/direct2d/direct2d-quickstart)

## 环境与证据

Windows 11 x64 26200.9457；AMD Ryzen 9 9955HX；OS 可见内存约 31.18 GiB；全机活动逻辑 CPU=16。显卡和驱动：AMD Radeon 610M / 32.0.13034.7001，NVIDIA RTX 5080 Laptop / 32.0.16.1664，另存在 GameViewer Virtual Display Adapter / 15.6.5.199。这不是隔离空白桌面，后台工作及虚拟显示适配器都可能影响系统层测量。

Rust 1.98.1；MSVC 14.51.36231；SDK 10.0.26100.0；windows-sys 0.61.2、可选 windows 0.62.2。被测 render-bench.exe 为 142,336 bytes（139 KiB），SHA-256：`10D44559E4034138F8FDA1D73D0ED15218E63CFB1361746904B6464742DFD9F4`。主程序仍为 120,832 bytes（118 KiB），文件大小不是运行内存。

- [三轮原始数据](measurements/p0-02-20260922/results.json)：包含每轮 ready 元数据、所有进程样本和 GPU/宿主快照。
- [三轮汇总](measurements/p0-02-20260922/summary.json)：脚本可重复生成，所有差值都有 3 个有效同轮配对。
- [宿主补测原始数据](measurements/p0-02-20260922-host-check/results.json)：修正可选启动时间字段失败连带影响内存查询的问题；不混入三轮主表。

首批 Explorer 稳态快照相对启动前的私有提交变化范围为 -4.29～+0.17 MiB，各场景中位数均为 0；可能包含其他桌面活动，不将负数当成优化收益。首批 DWM 启动时间不可读取，旧错误分支还丢失了失败项的名称，因此首批 DWM 条目不可用于定量分析。修复后将启动时间与内存查询分离，并补测宿主快照；旧原始数据完整保留，不以补测值覆盖。

补测 5/5 完成，DWM 私有提交快照的 `采样前 - 启动前` 差值依次为：empty +0.121 MiB，GDI surface 0，GDI icons -0.063 MiB，D2D surface -0.043 MiB，D2D icons -0.035 MiB。Explorer 对应 0、+0.109、+0.125、0、0 MiB。它们是单轮、非隔离桌面的观测值；DWM PID 保持一致，但受权限限制没有启动时间，不能作强身份校验。没有采集 DWM 的 GPU 分配，所以本报告不宣称已得到系统总内存净增量。

## 验证

- 五场景隐藏首帧 smoke 全部通过，包含 200 图标初始化与 EndDraw/BitBlt 返回检查。
- Rust fixture 测试通过：200 图像互异、大小正确、alpha=255，全部落在可见表面以内。
- Release 编译、格式检查、全目标 Clippy（警告视为失败）通过。
- 汇总脚本 Pester 5/5：奇偶中位数、同轮配对、负差值、缺失值、严格数值类型、重复与非法 schema 输入。
- 计数器 ABI/自身进程读取校验通过；CPU 全机逻辑处理器计数为 16。

## 可复现入口

```powershell
.\scripts\Cargo.ps1 build --release --features render-bench --locked --offline
.\scripts\Run-RenderBench.ps1 -SmokeOnly -Rounds 1
.\scripts\Run-RenderBench.ps1 -Rounds 3 -WarmupSeconds 5 -DurationSeconds 20 -OutputDirectory '.\docs\measurements\新的目录'
.\scripts\Summarize-RenderBench.ps1 -ResultsPath '.\docs\measurements\新的目录\results.json'
```

本次正式数据保存于 `docs/measurements/p0-02-20260922/`，预检数据在忽略的 `artifacts/` 下。脚本拒绝覆盖已有目录，采样中断时保留已经完成的轮次。

## Agent 分配

主 agent 实现/审查 Win32 + GDI + Direct2D 资源生命周期、计数器 ABI、采样流程与结果解释。一个 `gpt-5.6-luna`（medium）子 agent 只编写汇总脚本及测试，独占两份文件；主 agent 复查了缺失值、数值类型、同轮配对与 schema 校验。未为简单统计任务启动更高成本模型，未提供未经统计的 token 节省数字。
