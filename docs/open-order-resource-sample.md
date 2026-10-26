# 打开与排序版短时资源观察

2026-10-10，对通过 computer-use 打开的 Release `litefences-ui-acceptance.exe` PID 12124，执行 `Measure-Process.ps1 -ProcessId 12124 -DurationSeconds 30`。一组、两条本地引用，未最小化；之前实际完成排序、重启、文件打开，采样期间 agent 没有窗口输入。该入口包含 ui-acceptance 诊断，不是默认主程序的标准 S1 负载。

31 个有效样本，30.015649 秒：Private Bytes P95/峰值 7,827,456 字节（约 7.46 MiB）；Working Set 43,053,056 字节（约 41.06 MiB）；Private Working Set 4,808,704 字节（约 4.59 MiB，PROCESS_MEMORY_COUNTERS_EX2）。GDI 54、USER 70，最后句柄数 542；本轮可选计数可用，不替换此前不可用记录。

CPU 时间增加 8.796875 秒，16 个逻辑 CPU，整机归一化 1.834454%。仍明显高于 0.1% 预算；未定位或解决高 CPU 根因，不称为达标或优化收益。不关闭可访问性、不主动 trim 工作集、不更换统计口径，也不与之前不同窗口/负载结果直接作性能优劣比较。外部记事本与 Explorer/DWM 未纳入此短采样。

本次仅保留汇总（脚本未启用 IncludeSamples）：[原始命令汇总](../artifacts/open-order-sample-12124.json)。不是 5 组/200 项、预热 60 秒并采样 10 分钟的 S1，也不是 Fences 同机对照或长时稳定性验收。
