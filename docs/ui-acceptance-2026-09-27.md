# P2-08 可见界面复验：2026-09-27

本次只使用 `computer-use` 控制 LiteFences 本身及其系统选择对话框。正式 `litefences.exe --data-dir` 从终端启动后仍不在 computer-use 的窗口清单中；为避免无参数启动正式程序时打开用户配置根目录，新增仅供验收的 `litefences-ui-acceptance.exe`。它不接收参数，每次运行只在项目 `artifacts/ui-sky-<进程 ID>-<时间戳>` 下新建配置目录（最初一次验收构建使用 `artifacts/ui-sky-<进程 ID>`）。由 computer-use 启动后，工具返回唯一 LiteFences 窗口，可获得截图与控件树。本次测试目录为 `artifacts/ui-sky-9836`，进程已通过普通 Esc 退出；目录和证据保留。

可见操作及磁盘回读：

- 点击“新建分组”，列表和右侧预览立即出现第 1 组，`config.json` 自动保存。
- 点击“添加文件引用…”打开真实 Windows 文件对话框，Esc 取消后仍是 0 条引用。随后在文件名栏输入 `source.txt` 并确认，回读为 `file` 引用，完整路径指向项目测试文件。
- 点击“添加目录引用…”打开真实 Windows 目录对话框，进入 `source-folder` 并选择，回读为 `directory` 引用。再次打开并 Esc 取消后，条目数不变。
- 测试前后 `source.txt` 的 SHA-256 为 `C1BF1FF09BEB49CA50EC47E0C60D2C445E138E4DC6BDBEDB9C9011C49B36958C`；`source-folder/child.txt` 为 `5413D21DC857CC7FF86171FD6BB592B2822F3419F280E86A5EDD7C9A171B5E75`，目录项保持 1 个。

一次截图坐标选择误选了同一隔离目录中的 `config.json`。回读确认这条额外引用确实存在；没有删除它，也没有触碰桌面文件。最终测试配置有 1 个分组、3 条引用（`config.json`、`source.txt`、`source-folder`）。因此文件选择的成功判据以回读 `source.txt` 路径为准，不以首次误选为证据。该经历说明截图坐标在缩放和系统模态窗口中不能代替持久化路径核查。

第二次独立可见验收使用 `artifacts/ui-sky-35516-1790481342844143600`：新建分组后，在真实 Windows 文件对话框的文件名栏一次输入 `"alpha.txt" "beta.txt"` 并确认。窗口立即报告“引用已添加”，磁盘 `config.json` 回读为 1 个分组、2 条 `file` 引用，路径分别指向该目录的 `alpha.txt` 与 `beta.txt`。两份源文件仍分别包含原有的 `LiteFences multi-select alpha fixture.`、`LiteFences multi-select beta fixture.` 内容。该窗口已通过 Esc 退出，测试目录保留。

第三次在项目 `artifacts/ui-shortcut-source` 中用 Explorer 的“新建快捷方式”向导生成真实 `source-link.lnk`，目标为同目录 `source.txt`。隔离进程 `artifacts/ui-sky-16428-1790482569221766700` 通过真实文件选择器选择 `.lnk`；配置回读为 1 个 `shortcut` 引用，保存的是 `.lnk` 路径而非目标文本文件。再次选择同一路径后仍只有 1 条引用；当时界面给出笼统的 `invalid_data` 提示。随后针对重复路径补了明确提示，在 `artifacts/ui-sky-31844-1790482940437029200` 复验显示“所选路径已在该分组中，未重复添加”，回读仍为 1 条。两次选择前后，`source.txt` 的 SHA-256 始终是 `6854D82492AC4D47E80D40D77A8D4E5BAEADEC1105FAA98CBF7B28586C8A70B4`，`source-link.lnk` 始终是 `E8B9BC9658DEBD8B714605412C24284F14A4ACC106095799759515EFF9CDEAA1`；两测试进程均已通过 Esc 退出。此项只验收文件选择器，不代表外部拖放。

本次没有物理操作 Explorer 到预览面板的跨窗口拖入，也没有可见混合文件/目录拖入批次、混合 DPI 或 P95/内存测量。隐藏 Shell 数据对象的 LINK/COPY 与绝不 MOVE 验证仍见 `docs/external-drop.md`，不等同物理 Explorer 拖入。P2 添加项目仍不勾选完成。

增加验收入口及混合拖入回归后，Debug/Release 全 feature 各 181 项测试、全/无默认 feature 严格 Clippy、格式、Release 构建、56 个隐藏配置窗口场景及 5 次生命周期 smoke 通过。最近隐藏报告：`artifacts/config-window-e916859f112d4d3788dc1e9a7af6eb20/results.json`。

追加快捷方式场景和重复路径提示后，最新 Debug/Release 全 feature 各 182 项 Rust 测试、58 个隐藏窗口场景通过；新增两项分别验证真实 `.lnk` 测试夹具的第二组添加和重启恢复，目标文件与快捷方式本身的哈希均不变。最新隐藏报告为 `artifacts/config-window-68651c55b07e48438025262f521e998b/results.json`。一次集成初跑因新 smoke 模式未列入保存完成白名单而失败，补齐后全量复跑通过；失败目录保留，不算通过证据。

随后另补 `.lnk` 的 Shell 拖入隐藏场景及重启回读，最新累计 60 个隐藏场景，报告为 `artifacts/config-window-d190ebb09bdf4425add6198a9f192710/results.json`。它验证 LINK/COPY 协商与绝不 MOVE，但仍不是可见 Explorer 跨窗口拖入，详见 `docs/external-drop.md`。
