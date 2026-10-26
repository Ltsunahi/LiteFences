# 普通配置窗口短采样（非 S1）

2026-10-05，Release `litefences-ui-acceptance.exe`，PID 11300，隔离目录 `artifacts/ui-sky-99002-1790600000000000000`。配置 1 个分组、0 引用；有 computer-use/UIA 交互历史，采样期间不发起新的界面输入。不是 5 组/200 项/预热 60 秒/采样 10 分钟的标准 S1，也未控制其它机器负载。

命令：`./scripts/Measure-Process.ps1 -ProcessId 11300 -DurationSeconds 30 -IntervalMilliseconds 1000`。

| 指标 | 本次结果 |
| --- | --- |
| 基础样本 | 31，completed，30.0132617 秒 |
| Private Bytes P95/峰值 | 10,403,840 字节（9.92 MiB） |
| Working Set P95/峰值 | 36,253,696 字节（34.57 MiB） |
| CPU 时间增量/有效墙钟 | 5.375 / 29.9723054 秒 |
| 逻辑 CPU/全机归一化 CPU | 16 / 1.1208263613% |
| 最后句柄数 | 381 |
| Private Working Set、GDI、USER、窗口可见性原生计数 | 不可用，不记作零 |

CPU 尚高，不能宣称达到计划 0.1% 空闲目标；短采样内存数也不代表 S1 达标。应补没有自动化交互的对照、线程资源归属及标准负载长采样，不根据一次结果调整预算或换统计口径。

首次脚本返回 access_failed / 0 样本，但只读 Process 基础计数可用；原因是可选原生调用失败被整段 catch 吞掉。低成本子 agent 把可选计数失败与基础样本隔离，主 agent 补必需计数 null 检查和确定性测试。GDI/USER 返回零保守视为不可用，不误当零资源；无主窗口句柄时 visible/minimized 为 null。

`scripts/tests/Measure-Process.Tests.ps1` 的两项 Pester 回归通过：所有可选原生调用故障注入仍保留有效基础样本；不存在 PID 不生成虚假内存/CPU样本。该修复不扩大计数权限，不改 UI、进程、桌面或源文件。

两份 PowerShell 回归脚本合计 7 项通过。另一低成本子 agent 只读复查消息等待、自动重置事件、图标 recv/drain、绘制配对和外部拖入实现，未找到明确的持续空转路径；30 秒刷新计时器本身不足以解释归一化 CPU。未把 UIA、图标或 COM 判作已确定原因，也没有用增加 sleep 掩盖数据。下一诊断应记录 wait 返回原因、WM_PAINT/WM_TIMER 与线程 CPU 增量，以同条件无自动化对照归因。
