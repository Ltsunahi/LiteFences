# P2-01：创建首个分组

2026-09-24。此步只交付分组创建、标题预览、自动保存和重启恢复；不是完整 P2 分组管理，也不是桌面图标接管。

## 行为与边界

- 普通窗口加载可写配置后，“新建分组”按钮与 Ctrl+N 可用；加载中、只读及保存期间禁用。每次成功创建分配不复用的稳定 ID、默认标题“新分组 N”、固定颜色与 DIP 初始矩形。配置先经纯数据层校验，再提交给既有后台 I/O 工作者自动保存。
- 保存成功后可继续创建；退出会等在途保存完成并释放写锁。保存提交失败、保存回执失败或后台线程中断时，未确认的新分组及 ID 分配回退，不把它当成已持久化数据。
- P2-01 当时窗口显示来源、分组总数和前三个标题；后续 P2-02 已改为可滚动原生列表并加入重命名控件，默认及最小窗口高度相应调整到 640 DIP。仍无独立分组面板、拖动、缩放、删除、折叠、引用拖入或实际桌面挂载。
- 无源文件操作，也不修改 Explorer 或真实桌面图标。测试仅使用 `artifacts/` 下独立目录；普通启动仍按原设计使用用户配置目录。

## 验证

```powershell
.\scripts\Cargo.ps1 -CargoArguments @('test','--release','--all-targets','--all-features','--locked','--offline')
.\scripts\Cargo.ps1 -CargoArguments @('clippy','--all-targets','--all-features','--locked','--offline','--','-D','warnings')
.\scripts\Cargo.ps1 -CargoArguments @('clippy','--all-targets','--no-default-features','--locked','--offline','--','-D','warnings')
.\scripts\Cargo.ps1 -CargoArguments @('fmt','--','--check')
.\scripts\Cargo.ps1 -CargoArguments @('build','--release','--locked','--offline')
.\scripts\Test-ConfigWindow.ps1
.\scripts\Test-Smoke.ps1 -Iterations 3
```

Release 全 feature 99 个 Rust 测试通过。14 个隐藏 HWND 配置场景通过，包括创建、退出、重启后同 ID/标题恢复；其中 `wrong-expectation-fails` 预期退出码为 1，实际符合预期。最终 release 复测产物见 `artifacts/config-window-2f18c50639fb4faba2cf08f94cedfb5a/results.json`。3 次旧窗口生命周期冒烟及全/无默认 feature 严格 Clippy 通过。

可见界面测试进程曾启动，辅助技术能读到“保存配置”和“新建分组”两个原生按钮，但本轮桌面控制接口无法激活该进程且截图为黑屏，因此没有完成可见点击、Ctrl+N 或布局截图验收。该进程已退出；不能把隐藏窗口结果表述为视觉/物理键盘通过。下次在可正常交互的桌面环境补测，尤其检查按钮点击、Ctrl+N、保存后焦点、三个预览行、重启后显示及 Esc。

下一小步按计划继续 P2 的分组命名/可见交互；P0-03 真实桌面融合及 S1 性能仍独立待验收。
