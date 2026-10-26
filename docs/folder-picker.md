# P2-08c：目录选择入口（可见验收待补）

2026-09-26。普通窗口新增“添加目录引用…”按钮，仅在已选分组、配置可写且没有在途保存时启用。使用 Windows `IFileOpenDialog` 与 `FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST` 选择现有文件系统目录，关闭最近使用记录写入；取得 `SIGDN_FILESYSPATH` 后按当前模型只接受规范本地盘符绝对路径。取消不保存，成功添加 `Directory` 引用，不遍历、移动、复制或删除目录内容。COM 在 UI 线程以 STA 临时初始化，返回后释放接口、字符串与公寓计数；现有单后台 I/O 工作者继续负责配置保存。Windows 推荐的选择目录 API 见 [Microsoft SHBrowseForFolder 文档](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shbrowseforfolderw) 对 `IFileDialog/FOS_PICKFOLDERS` 的说明。

隐藏 `ui-folder` 使用项目独立目录中的 `source-folder`，向新增目录按钮入口注入其本地路径，选择第二组并走 P2-08a 的自动保存。跨进程回读确认第一组未变、第二组有一条 `directory` 引用、稳定 ID 与分配器正确；添加前后及重启后检查子目录唯一文件哈希和目录项数量不变。Debug/Release 全 feature 各 145 项 Rust 测试、50 个隐藏配置窗口场景、全/无默认 feature 严格 Clippy、格式、Release 构建和 5 次旧窗口生命周期 smoke 通过。报告：`artifacts/config-window-6784cfff573f4ba88ec787af69c9779a/results.json`。

一次可见文件按钮测试尝试中，自有进程 PID 11376 有响应且拥有主窗口，但 computer-use 工具无法列出它；已停止该进程。没有获得文件或目录选择器的真实可见点击、取消、键盘或混合 DPI 证据，也没有操作真实桌面文件、Explorer 或注册表。因此 P2 添加项目仍未完成；下一步按计划实现 Explorer 拖入的安全引用协商，仍须补可见验收和性能测量。
