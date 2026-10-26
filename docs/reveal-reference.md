# 打开引用所在位置

普通窗口中，选择引用后点击“打开所在位置”，可让 Windows 资源管理器定位该引用。程序按稳定的 `GroupId` 和 `ItemId` 重新查找当前引用，不依赖 tile 的屏幕位置或文件名。

文件和目录都会在其父目录中选中引用本体；不会进入目录。`.lnk` 快捷方式选中的是快捷方式文件本身，不会解析或运行它指向的目标。像 `C:\` 这样没有普通父目录的根路径，本版提示不支持。操作失败时引用仍保留在分组中。

此功能只请求 Explorer 显示并选中项目，不移动、删除或修改源文件。Windows Shell 接收请求不等于已确认 Explorer 窗口实际可见。

2026-10-10 已在 `ui-sky-99003-1791624000000000000` 的真实可见窗口点选 file.txt 并点击定位按钮；Explorer 打开 `artifacts/ui-drag-source`，高亮 file.txt，预览内容与 48 字节选中状态吻合。源文件 SHA-256 仍为 `9F5C437A591D5C9FF9F1CCD4B0D8BB5B751A88AC337FC8F42BC797F61EB3F850`，配置仍为 `EB719790BC78B0A68AC82C69A5E46050D9BDC6EDED0E1CBD805F68E4937A4152`。

在新版 `ui-sky-99004-1791628000000000000` 合成夹具中，实际定位 folder 后，Explorer 仍位于 `ui-drag-source` 父目录、选中 folder 而非进入它；定位 source-link.lnk 后，Explorer 位于 `ui-shortcut-source`，高亮类型为“快捷方式”的 source-link，而非相邻 source.txt。两个操作不保存配置，配置哈希保持 `88E1DCFC5007660EC8BD42E1841C27AE1944E2E67C5D6B387F91260F173A3228`。快捷方式未被执行，源快捷方式哈希仍为 `E8B9BC9658DEBD8B714605412C24284F14A4ACC106095799759515EFF9CDEAA1`。错误提示需分别验证，不能由成功用例推断全部通过。

实现复用现有唯一 Shell STA 工作者。普通打开与“打开所在位置”共用一个待处理请求上限；窗口退出前尚未执行的请求会取消。隐藏验收窗口不会打开外部应用。元数据检查和 PIDL 创建、使用均在 STA 后台完成，PIDL 只释放一次。路径达到 260 个 UTF-16 code unit 时拒绝请求，不截断。

同一可见夹具分别点选 C:\ 根目录及合成缺失目标再定位，状态栏显示“不支持定位”和“目标已不存在；引用仍保留”，没有打开错误目标；配置哈希仍为上述值。权限拒绝、外部 Shell 长阻塞和更多路径错误未做真人验收，保留限制。

本功能不删除或移动源文件，不新增自有常驻进程、线程或 crate，不使用 Explorer 注入或注册表写入。Windows Explorer 是系统外部进程，其内存尚未纳入 app 内存预算，需待 S1 测量时单独处理。

实现使用 Windows Shell 的 [SHOpenFolderAndSelectItems](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shopenfolderandselectitems) 选择项目，并使用 [ILCreateFromPathW](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-ilcreatefrompathw) 从路径创建 PIDL。
