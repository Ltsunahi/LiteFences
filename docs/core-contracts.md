# P1-01 共用数据与接口契约 v1

后续状态（2026-09-23）：P1-02a 已按下述 v1 格式实现编解码与存储后端，见 [配置存储验收](configuration-storage.md)。本文“本步/本轮未实现”描述保留为 P1-01 阶段记录；主窗口目前仍未接入存储。

按计划第 6 节允许的分支推进共用模块。P0-03 桌面兼容、P0-04 图标策略、P0-05 完整负载均未完成；此契约不选择最终桌面接管路线。

## 数据与边界

- GroupId、ItemId：独立命名空间的非零 u64，配置内稳定唯一；删除不复用。持久化 next_group_id / next_item_id，须大于所有对应现有 ID。溢出报错，不回绕。
- RectDip { x:i32, y:i32, width:u32, height:u32 }：DIP。x/y 范围 ±1,000,000；宽 240..=16,384，高 120..=16,384。允许负坐标；多屏可见区修正留给 P3。转物理像素使用 48..=768 DPI，最近整数，半值远离零。
- 显示器：可选不透明 UTF-16 标识，1..=1024 units，无 NUL；不依赖枚举顺序。匹配与显示器变化处理尚未实现。
- 分组：ID、标题、RGB 颜色、DIP 矩形、显示器标识、折叠状态、有序项目数组。标题去空白后不能空，最多 128 个 Unicode scalar，不含控制字符；不自动裁剪用户输入。
- 项目：ID、FileReference（本地绝对路径 + file/directory/shortcut 类型）。不保存 COM、PIDL、HWND 或图标缓存。存在性/快捷方式类型验证是平台层责任，纯模型不触盘。
- 路径用 UTF-16 units，可保留未配对代理项；不使用有损 UTF-8。仅接受盘符绝对路径和反斜线分隔；拒绝 UNC、设备命名空间、相对路径、点段、ADS、NUL、通配符、尾空格/句点及保留设备名。最多 32767 units，不含尾 NUL。这不是防重解析点的文件系统沙箱。
- 同组按完全相同的 UTF-16 路径去重，不看 kind；跨组可重复目标但 ItemId 不同。不做大小写折叠：大小写敏感目录、硬链接、短路径等别名可能并存，不宣称物理文件完全去重。
- 限额：128 组、总计 10000 项、全部路径合计 1,000,000 UTF-16 units；未来 JSON 输入上限 8 MiB。超限报错，不截断；限额不等于性能预算通过。
- 设置 v1 仅 launch_at_login，默认 false；模型本身不注册启动项。

## JSON v1 格式冻结

```json
{
  "schema_version": 1,
  "next_group_id": "2",
  "next_item_id": "2",
  "settings": { "launch_at_login": false },
  "groups": [{
    "id": "1", "title": "工作", "color": [60, 110, 180],
    "rect": { "x": 40, "y": 40, "width": 360, "height": 280 },
    "monitor_key_utf16": null, "collapsed": false,
    "items": [{ "id": "1", "target": {
      "kind": "file",
      "path_utf16": [67, 58, 92, 100, 101, 109, 111, 46, 116, 120, 116]
    }}]
  }]
}
```

例子为虚构 C:\demo.txt，不创建/读取该文件。ID 与 next ID 用十进制字符串，禁止前导零、符号和空白，避免浮点精度损失；坐标、schema、UTF-16 units 为整数。数组顺序就是显示顺序。kind 仅 file/directory/shortcut。UTF-16 数组必须原样回读代理项。

P1-02 编解码须拒绝缺失/未知/重复字段、非整数、未知枚举、非法/重复 ID、超限和不支持的 schema；损坏/未来版本不得自动覆盖。v1 不实现迁移。**本步固定格式与模型校验，不实现 JSON 编解码或磁盘保存**。

## 命令、事件与模块接口

AppCommand 描述创建/命名/布局/颜色/折叠/移除分组、添加/移除/移动/排序引用、撤销、打开引用、保存意图。本步不实现 P2 执行器或 UI。AddReferences 后续逐项处理、允许部分成功；取消拖动不提交命令，外部拖放只报告 LINK/COPY，绝不 MOVE。Remove/Delete 只改变配置，不删除源文件。

RequestToken { request_id, generation, item_id } 区分请求，均非零；后台结果必须与 UI 当前待处理 token 完全匹配、且项目仍存在才接收。传递拥有所有权的 Rust 值，不跨线程传 COM 或 HWND。有界队列、取消调度尚未实现。

AppEvent：模型 revision 变化、打开完成、图标完成、保存完成、失败。保存确认带实际已落盘 revision；旧确认不能清除更新后的 dirty。undo 历史有界，尚未实现。

- ShellService：路径解析/类型识别、打开引用和所在位置。阻塞调用只在专属 STA 执行；接口不创建线程。统一 AppError 不默认包含私人路径。
- IconProvider：加载拥有内存的 BGRA 图标，尺寸与字节数校验。8 MiB 总缓存由未来 services 负责；接口本身无缓存/GPU 对象。
- ConfigStore：读回配置、来源（空/正式/备份）、可否写回；保存带 revision 快照。坏文件回退后的写入须先保留坏文件；未来版本/权限问题默认禁止写回。P1-02 实现独占写入、同目录临时文件刷新、备份和 Windows 原子替换，在项目专用目录测试。

这些 trait 是待实现边界，不证明功能可用。本步主窗口仍是原有原型，不读写用户配置，不开放分组编辑或文件打开。

路径规则采用首版本地 Shell 引用子集，并非支持所有 Windows 命名空间。设备保留名（包括 COM/LPT 的上标数字）、尾空格/句点等规则参考 [微软文件命名文档](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file)。模型不查询卷的单组件长度上限，也不保证长路径能被 Shell 打开；平台解析必须反馈不支持的情况，不得截断路径。

## 实现与验证记录

2026-09-22：契约对应 `src/core/model.rs`、`layout.rs`、`contracts.rs` 已实现；公共库禁止 unsafe，不依赖 OS API，不创建线程，不做文件 I/O。未新增第三方依赖。ID 分配失败不修改计数器；最大 u64 保留为耗尽状态，不发出无法持久化后继计数器的 ID。调用方必须先验证载入配置，再进行分配/编辑。

命令执行细节固定为：MoveReference 的 index 是源引用移除后的目标数组索引（允许尾部插入），目标同路径冲突时整体拒绝，不先移除源引用。ReorderGroups 必须包含当前每组恰好一次。未来 AddReferences 返回逐项结果。这些规则将在 P2 执行器中实现和验证，目前仅定义值类型。

IconPixels 用 Arc 共享已经校验的像素内存，clone 不再复制图标内容。只检查尺寸与 byte 数，不扫描或更改 alpha；预乘 BGRA 的正确性由平台提供者负责。RequestToken 匹配辅助函数校验 token 与项目存活，但本轮没有启动真实异步任务。

验证命令：

```powershell
.\scripts\Cargo.ps1 test --all-targets --all-features --locked --offline
.\scripts\Cargo.ps1 -CargoArguments @('clippy', '--all-targets', '--all-features', '--locked', '--offline', '--', '-D', 'warnings')
.\scripts\Cargo.ps1 build --release --all-features --locked --offline
.\scripts\Cargo.ps1 -CargoArguments @('fmt', '--', '--check')
```

新增 25 项通过：6 个几何测试 + 19 个集成测试；原有 9 项通过，总计 34 项。覆盖未配对 UTF-16 的 Windows OsString 原样往返、设备路径拒绝、稳定 ID 超大整数/耗尽、重复目标、1000 项与 10000 项边界、路径总量、过期请求、图标共享所有权。测试使用虚构路径，不创建或读取对应文件。

Release 和 Clippy 通过。主程序文件仍为 120832 字节；共用库尚未接入 UI，因此不能把文件大小不变称为功能集成后内存达标，未新增运行时内存结论。JSON 编解码、原子保存、备份回退、独占写入及 P2 可见分组功能仍未实现。

分工：低成本 gpt-5.6-luna（medium）仅实现 layout.rs 和 6 项测试，主 agent 审查并负责其余契约、模型和 19 项集成测试。没有额外高成本子 agent。
