# Unlinka

[English](README.md) | [简体中文](README.zh-CN.md)

Windows 快速删除工具，提供双语 CLI、Windows 原生任务窗口和 Explorer 右键菜单。Unlinka 支持永久删除、终端确认、扫描预览、有界并发、取消和失败汇总。

删除不会进入回收站。取消只停止尚未开始的工作，不能恢复已经删除的项目。

## 安装

x64 当前用户安装器面向 Windows 10/11，无需管理员权限。默认未签名构建注册经典菜单：Windows 10 直接显示，Windows 11 位于“显示更多选项”。使用受信任证书签名后，Windows 11 还可使用新菜单。菜单和用户 PATH 默认启用。安装位置固定为 `%LocalAppData%\Programs\Unlinka`，开始菜单快捷方式打开语言、菜单和 PATH 设置。右键操作始终先确认，再显示执行进度、取消和结果。

安装、升级、修复与卸载代码已实现。未签名构建可以安装，不包含 MSIX 或桌面包身份；签名与未签名构建使用同一产品标识和安装位置。证书签名构建及真实 Windows 生命周期/菜单验收仍待完成，签名选项与待验收范围见 [Windows 集成说明](docs/windows-integration.md)。

准备 Rust 1.99 MSVC、Windows SDK x64 构建工具和 Inno Setup 7.1，构建默认安装包：

```powershell
cargo xtask pack --build-number 1 --iscc "C:\Tools\Inno\ISCC.exe"
```

产物位于 `target/packages/<四段版本>`，已有版本不可覆盖，每次打包使用新的构建号。两种签名状态均使用 `Unlinka-Setup-<四段版本>-x64.exe`。需要签名时，同时提供 `--publisher`、`--certificate-thumbprint` 和 `--timestamp-url`；Publisher 必须与证书 Subject 完全一致。payload 中的 `unlinka.exe` 也可作为便携 CLI 使用。升级保留设置及上一完整版本用于回退。卸载默认保留偏好和日志；DLL 被占用时报告待清理状态并保留卸载入口，释放后重试。

## 使用

以下示例均假设 `unlinka` 已在 `PATH` 中，不需要额外 GUI 运行时。

```powershell
unlinka --help

# 本地化帮助；auto 跟随 Windows 显示语言
unlinka --lang zh-CN --help

# 查询安装状态，不开始删除
unlinka --integration-status --json

# 在终端中交互确认；空回答取消删除
unlinka -r .\node_modules

# 在脚本中明确跳过确认
unlinka -r --yes .\node_modules

# 仅扫描并统计，不修改目标，也不请求确认
unlinka -r --dry-run .\node_modules

# 供脚本读取的版本化 JSON；确认规则仍然适用
unlinka -r --yes --json .\node_modules

# 删除多个目标；线程预算包含扫描线程
unlinka -r --threads 8 .\one .\two

# 处理以短横线开头的文件名
unlinka -- -example
```

普通目录必须显式使用 `-r`。符号链接、junction 和其他重解析点作为入口处理，不递归扫描其目标。`--threads` 范围为 1–64，默认取 CPU 可用线程数与 4 的较小值。预算为 1 时同步执行；更大的预算分配一个扫描线程，其余线程执行删除。扫描时就可以删除已发现的文件，不等待整棵目录树统计完毕。

CLI 删除始终使用终端确认，要求交互式 stdin 和 stderr。只有输入 `y` 或 `yes` 才确认删除（不区分大小写），空回答取消。脚本可用 `--yes` / `-y` 跳过确认，或用 `--dry-run` 仅扫描。原生确认、进度和结果窗口用于 Explorer Shell 操作。“查看目标”和“查看失败详情”打开固定大小的滚动文本窗口，保留完整路径并支持选取复制，主弹窗不随详情数量改变大小。

结果与诊断输出至 stderr。普通执行计数为成功删除的文件、目录和链接入口；dry-run 计数为扫描到的候选项。汇总区分完成、未完成和已取消，并给出耗时和失败总数。终端默认最多列出 5 条失败的路径与原因，`-v` 显示最多 100 条保留的失败及其操作、错误码。Shell 详情和 JSON 同样保留最多 100 条失败，并明确给出未显示的数量。执行时可以按 Ctrl+C 取消，Unlinka 不能恢复已经删除的项目。

语言优先级为 `--lang` → `UNLINKA_LANG` → 保存的偏好 → Windows 显示语言 → 英语，支持 `auto`、`zh-CN` 和 `en-US`。JSON 使用 schema v1、稳定诊断标识、数值计数和原始 UTF-16 路径数组；非帮助查询的最终结果在 stdout 只写一个文档。`--integration-status` 与删除参数互斥。

| 退出码 | 含义                                   |
| ------ | -------------------------------------- |
| 0      | 完成且无失败，或 dry-run 扫描完成      |
| 1      | 执行失败或部分失败                     |
| 2      | 参数错误或目标预检查失败，尚未开始删除 |
| 3      | 用户取消；执行阶段可能已经删除部分内容 |
| 4      | 确认窗口或确认输入失败，尚未开始删除   |

## 边界

Unlinka 不修改只读属性、不提权，也不结束占用文件的进程。执行前会重新验证所有顶层目标，任一目标无法打开或身份变化时整批停止。执行中的权限或共享冲突作为失败返回，其他独立项目继续处理。保护路径包括卷根、Windows 目录及其内容、用户目录、当前工作目录及相关祖先。这些保护不能替代其他重要数据的备份。

预检查记录目标身份，执行前重新打开并复核。扫描时通过目录句柄限制常规写入与重命名，并复验目录类型，报告共享冲突而不忽略。新增文件可能导致目录非空失败；文件若在打开前被替换，同名的新文件仍可能被删除。叶子项通过已打开句柄删除，即使另一进程随后将它移出所选目录，仍处理这个已选对象。仅修改属性的重解析点操作仍有检查与打开之间的竞争窗口。Unlinka 不提供隔离恶意并发修改的保证。

目前不包含回收站删除、精确百分比进度、自动删除重试、占用进程处理，以及保留目录本身的清空模式。Shell 集成面向原生 x64 Explorer，首版不包含 ARM64/x86 和目录背景菜单。主要优化目标是本地 NTFS 的大量小文件；其他文件系统及网络路径尚未验收。原生窗口使用系统任务对话框，布局可能与不同 Windows 版本的 Explorer 有所区别。

## 文档

- [架构与开发](docs/architecture.md)：模块职责、删除约束、原生确认、依赖和源码检查。
- [验收现状](docs/acceptance.md)：当前结果、验证缺口、性能结论和验证命令。
- [Windows 集成与 i18n](docs/windows-integration.md)：已实现的安装布局、两种菜单注册、升级与卸载恢复、打包及待完成验收。
