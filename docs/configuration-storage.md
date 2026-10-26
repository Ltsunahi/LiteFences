# P1-02a 配置存储后端验收

日期：2026-09-23。本文保留 P1-02a 后端实施记录；后续 P1-02b 已接入主窗口、用户数据目录和后台 I/O，当前行为见 [窗口配置验收](configuration-window.md)。P0 桌面兼容验收状态不变。

## 实现范围

可选 `config-store` feature 提供：

- 严格 JSON v1 编解码与完整模型校验，字段/格式见 [共用契约](core-contracts.md)。
- 拒绝未知、缺失、重复字段、嵌套数组冒充对象、非整数数值、未知类型及超限输入。nullable 字段也必须显式存在。先探测 schema，未来版本返回独立错误。
- ID 以十进制字符串保存；路径以 UTF-16 units 保存，包括未配对代理项。不读取或修改配置引用的文件。
- Windows `FileStore` 实现 `ConfigStore`：加载、保存、旧版备份、坏配置回退、写锁与保存 revision 防过期检查。
- 私有路径不会进入 codec/store 的错误消息；没有新增常驻线程、进程、轮询或网页运行时。

依赖锁定 serde 1.0.229、serde_json 1.0.151，间接依赖固定在 Cargo.lock。它们仅随 `config-store` feature 编译；P1-02a 时该 feature 默认关闭，P1-02b 已默认开启，`--no-default-features` 保留基础窗口构建。采用强类型反序列化，不先转成可能吞掉重复键的通用 JSON 对象。[Serde JSON 文档](https://docs.rs/serde_json/1.0.151/serde_json/)

## 文件与提交步骤

调用者须提供已存在的专用配置目录；后端不自行创建用户配置目录。拒绝相对路径、UNC 和末级重解析目录；目录句柄禁止删除共享，配置文件通过不跟随重解析点的句柄读取。不把这些检查当作抵御恶意同用户进程的安全沙箱；不支持网络盘部署。

| 文件 | 用途 |
| --- | --- |
| `.writer.lock` | 独占共享模式句柄；关闭/进程退出释放锁，空文件保留不会造成死锁 |
| `config.json` | 当前已提交配置 |
| `config.bak.json` | 上一版有效配置的 JSON 字节 |
| `config.pending` | 本次新配置临时文件，独占新建并刷新 |
| `backup.pending` | 备份轮换临时文件，独占新建并刷新 |
| `config.corrupt.json` | 从备份恢复后首次保存前保留的坏配置原文；不覆盖已有归档 |

正常保存顺序：校验并编码 → 比较正式/备份文件与载入快照 → 新建并 `sync_all` 新配置临时文件 → 刷新及提交旧版备份 → 再次检查正式文件 → 提交新正式文件 → 确认 revision。先加载才能保存；同一 store 生命周期内 revision 必须严格递增，重复/迟到保存报错。revision 不写进 JSON v1，也不是跨进程持久化版本号。

目标文件存在时使用 `ReplaceFileW`，首次创建使用不允许覆盖的 `MoveFileExW`；所有临时/目标文件同目录。没有使用忽略 ACL 错误的标志或不受支持的 WRITE_THROUGH 标志。替换失败可能留下不同文件状态，所以不会盲目重试/清理：返回错误、禁止本次 store 继续写入、保留临时文件和已经完成的备份。[ReplaceFileW 文档](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew)

这是经本机验证的安全替换流程，不承诺全套文件的多文件事务原子性，也不承诺任意断电、磁盘故障或未测试文件系统下绝不丢失最新修改。没有模拟断电、磁盘写满，未穷举 ReplaceFileW 所有特殊错误。正式/备份内容同步检查与独占写锁能防合作实例及可观测的外部编辑；不构成对恶意或恰在比较后写入的外部程序的文件系统 CAS。

## 加载与恢复行为

| 情况 | 返回配置 | 后续写入 |
| --- | --- | --- |
| 两文件都不存在 | 空配置 | 允许首次保存 |
| 正式有效，备份有效或不存在 | 正式配置 | 允许正常轮换 |
| 正式缺失，备份有效 | 备份 | 允许保存，保留原有效备份 |
| 正式损坏，备份有效 | 备份 | 坏文件原文归档完成后才提交新正式文件 |
| 正式/备份含未来版本 | 有效正式/备份或空配置 | 只读，不覆盖未来数据 |
| 正式有效但备份损坏 | 正式配置 | 只读，不静默丢弃备份 |
| 两文件都无有效配置 | 空配置 | 只读，原文件保留 |
| 有 `.pending`，或恢复所需坏文件归档已存在 | 已提交正式/备份或空配置 | 只读，需后续人工恢复入口；不自动提升临时文件 |
| 文件过大、不可读取、类型/权限错误 | 显式错误 | 禁止保存，不假装空配置 |

`LoadedConfig.source` 与 `write_allowed` 现由 P1-02b UI 显示；不得在只读回退后自动覆盖。当前尚无临时文件选择/归档管理界面，因此有遗留临时文件时安全停写，保留数据供处理。

存储目录写锁不是完整应用“第二次启动激活已有窗口”的单实例功能；后者仍待实现。

## 验证与资源边界

环境：Windows 11 25H2 x64，构建 26200.9457，Rust 1.98.1。Debug 与 Release 全 feature 各 65 项通过：原有 34 项 + codec 13 项 + store 18 项。store 数字包含 1 个独立锁持有进程的测试入口；它由跨进程用例实际调用。

测试源码：`tests/config_codec.rs`、`tests/config_store.rs`、codec 内部有界输出测试。

已验证：

- 保存→重新打开回读、连续备份轮换、缺失/截断/未来版本/非法格式处理。
- 同组数据结构与无损 UTF-16、大整数 ID 的完整往返；每层重复/未知/缺失字段及对象形状拒绝。
- Windows 真实共享模式阻止正式/备份替换时，旧配置仍可用，新临时数据被保留。
- 自有独立测试进程持锁期间其他 store 无法打开；只结束该子进程后可重新取得锁。**这验证锁释放，不是保存各时刻强杀或断电测试**。
- 配置保存/损坏恢复前后，测试引用源文件字节保持一致。
- 超过 8 MiB 拒绝；源读取权限冲突不误恢复为可写空配置；已有坏文件归档不覆盖。

所有故障注入只在项目 `artifacts/storage-tests/` 的新建目录内执行；保留测试产物供检查，没有清理用户数据、重启 Explorer、修改启动项或用户桌面。测试结束已等待自有锁测试子进程退出。

编码借用标题/路径/显示器数组，不复制一整套 UTF-16 数据；旧主文件快照移动为备份快照。输入与输出长度各限制 8 MiB，输出在扩展前检查。为准确检测外部变化，store 持有正式/备份两份原始字节快照，内容合计最多 16 MiB（不包括容器容量、载入模型和临时工作内存）；当前优先保证一致性，之后需结合 S1 实测评估取舍。此处没有宣称已达到完整应用内存预算。

## 复现

```powershell
cd E:\note\LiteFences
.\scripts\Cargo.ps1 test --all-targets --all-features --locked --offline
.\scripts\Cargo.ps1 test --release --all-targets --all-features --locked --offline
.\scripts\Cargo.ps1 -CargoArguments @('clippy', '--all-targets', '--all-features', '--locked', '--offline', '--', '-D', 'warnings')
.\scripts\Cargo.ps1 build --release --all-features --locked --offline
.\scripts\Cargo.ps1 -CargoArguments @('fmt', '--', '--check')
```

首次构建需要获取锁定依赖；下载后可离线执行。Clippy、Release 构建通过；P1-02a 当时未接入默认窗口，后续 P1-02b 已改为正常启动时加载用户配置。`--smoke-test` 始终不访问配置。

分工：一个低成本 `gpt-6-luna`（medium）编写 codec 与初始用例；主 agent 修正模块路径，补充对象形状/逐层字段检查，改为借用编码，并实现 Windows 替换、锁、恢复和集成测试。没有启动额外高成本审查 agent。

后续进展见 [P1-02b](configuration-window.md)。继续保留 P0 未验收项。
