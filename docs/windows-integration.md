# Windows 集成与 i18n

状态：当前用户安装器、经典 COM 菜单、维护工具、桌面任务与共享双语代码已实现。安装包默认未签名，证书签名为可选功能，不改变菜单行为；证书签名构建及真实 Windows 菜单与生命周期验收仍待完成。本文记录实现契约，源码检查结果与验证缺口见 [验收现状](acceptance.md)。

## 1. 交付范围与固定决定

- 默认交付当前用户安装包 `Unlinka-Setup-<version>-x64.exe`，使用 Inno Setup，不请求管理员权限。
- 首版覆盖原生 x64 Explorer。安装包在 Windows 10 注册经典菜单，在 Windows 11 注册“显示更多选项”中的经典菜单。
- 菜单入口默认启用，CLI 默认加入用户 PATH；高级安装选项允许关闭菜单或 PATH 集成。
- 保持用户已有的 Explorer 菜单设置。菜单适配共用任务协议、桌面进程和删除引擎。
- 首批语言为 `zh-CN` 与 `en-US`，默认跟随 Windows 显示语言。
- 沿用现有永久删除、预检查、身份复核、不跟随重解析点、取消和失败汇总语义。
- 首版不包含全用户安装、ARM64/x86 扩展、自动更新服务或目录背景菜单。

## 2. 安装布局与状态归属

`<root>` 默认是当前用户的 `%LocalAppData%\Programs\Unlinka`。安装位置固定，避免目录迁移引入额外的 PATH 和文件归属变更。

~~~text
%LocalAppData%\Programs\Unlinka\
  bin\
    unlinka.exe                       # 稳定 CLI 启动器
  maintenance\
    unlinka-maintenance.exe            # 安装、修复及卸载辅助工具
  versions\
    <package-version>\
      unlinka.exe                     # 现有 CLI 的实际程序
      unlinka-desktop.exe              # 无控制台的桌面任务进程
      unlinka-shell.dll                # 经典菜单适配器
      payload.json                    # schema 1、signed、版本、协议和 SHA-256
  staging\
    <transaction-id>\                  # 尚未提交的新版本
  state\
    activity.lock                     # 任务与维护的共享/独占锁
    transaction.json                  # 未完成维护事务及恢复信息
    ownership.json                    # 本安装实际拥有的文件与注册项
  unins000.exe                         # Inno Setup 卸载入口
  unins000.dat
  unins000.msg                         # 卸载器的本地化消息

%LocalAppData%\Unlinka\
  requests\                           # 临时 Shell 请求
  logs\                               # 有界维护诊断
  maintenance\                        # 维护工具运行时副本
~~~

文件布局中的 `<package-version>` 使用生成的四段 Windows 包版本，不能包含路径分隔符或任意相对路径。每个发行构建都有唯一且递增的包版本；同一版本的 payload 不做原地升级。Cargo 版本、包版本、文件资源版本和安装器版本由构建元数据统一生成。

安装注册表使用当前用户的 64 位视图：

| 位置                                 | 数据与责任                                                                           |
| ------------------------------------ | ------------------------------------------------------------------------------------ |
| `HKCU\Software\Unlinka\Installation` | `InstallRoot`、`ActiveVersion`、`State`、安装架构和 PATH 所有权；由维护工具写入      |
| `HKCU\Software\Unlinka\Settings`     | `Language=auto\|zh-CN\|en-US`、`MenusEnabled`、`PathEnabled`；由设置入口及安装器维护 |
| `<root>\state`                       | 操作清单、事务前状态和未完成清理；用于恢复，不作为第二份活动版本来源                 |

`ActiveVersion` 是唯一的活动版本来源。启动器和桌面进程先获得共享维护锁，再读取 `State` 与 `ActiveVersion`；维护工具持独占锁修改状态。菜单构建时的状态检查只用于显隐，执行入口必须重新检查。

维护状态为 `Installing`、`Ready`、`Updating`、`RepairRequired`、`Removing`、`RemovalPending`。只有 `Ready` 接受新的删除任务；缺失安装记录也按不可启动处理。纯版本和帮助查询可由稳定启动器在维护期间直接给出必要诊断。

共享/独占维护锁使用 `LockFileEx` 的固定字节范围，各进程独立打开同一个 lock 文件；进程退出后由系统释放锁，不使用需要手工纠正的活动任务计数。维护等待可取消，菜单线程不能同步等待独占锁释放。[锁接口文档](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex)

### 2.1 稳定 CLI 入口与 PATH

- `bin\unlinka.exe` 由构建出的 `unlinka-launcher.exe` 在打包时改名；现有 CLI 仍输出 `unlinka.exe`，避免 Cargo 产物重名。
- 启动器使用明确的完整程序路径，转发原始 Windows 参数、调用者工作目录、标准输入输出及实际退出码；不能有损转换参数或丢失引号、末尾反斜线和 `--`。
- 启动器等待子进程，并让子进程完成 Ctrl+C / Ctrl+Break 取消与汇总，不能因启动器提前退出而截断结果。
- 安装器只向 `HKCU\Environment\Path` 添加 `<root>\bin`，保留其他项、顺序及原始注册表类型，按展开后的路径判断已有等价项。
- 记录本安装是否真正添加了该项。已存在的等价项不取得所有权；卸载仅移除本安装添加的项，不恢复整个旧 PATH 覆盖用户后续修改。
- 安装、修复、卸载后广播环境变更；界面说明已有终端可能需要重开。

Inno Setup 的 `ChangesEnvironment=yes` 用于通知运行中的应用重新加载环境变量。[官方说明](https://jrsoftware.org/ishelp/topic_setup_changesenvironment.htm)

## 3. 经典菜单的注册契约

### 3.1 固定产品标识

以下标识在产品生命周期中保持不变，不随版本重新生成：

| 标识                    | 值                                       |
| ----------------------- | ---------------------------------------- |
| Inno Setup AppId        | `{6C273E32-2E9B-450A-A683-4AED2D9D6A04}` |
| 经典菜单 CLSID          | `{3830A560-FF20-4D90-8B90-BEEF4A7AE496}` |
| 经典菜单 canonical verb | `Unlinka.PermanentDelete`                |

`--publisher` 指定的证书 Subject 记为 `signer_subject`。固定标识的唯一来源为 [identity.json](../packaging/identity.json)，签名与未签名构建共用这些标识、安装目录及 `Unlinka` 注册表/数据命名空间。构建不添加证书或修改信任。未签名安装不要求 Authenticode 签名，仍执行文件哈希与归属核验；签名安装额外验证签名与证书 Subject。

### 3.2 经典菜单：当前用户 COM 注册

`unlinka-shell.dll` 提供经典类，实现 `IShellExtInit` 和 `IContextMenu`。使用一个 `AllFileSystemObjects` 关联覆盖文件和文件夹，避免同时注册多个相同经典关联。

~~~text
HKCU\Software\Classes\CLSID\{3830A560-FF20-4D90-8B90-BEEF4A7AE496}
  (Default) = Unlinka Classic Context Menu
  InprocServer32
    (Default) = <root>\versions\<package-version>\unlinka-shell.dll
    ThreadingModel = Apartment

HKCU\Software\Classes\AllFileSystemObjects\shellex\ContextMenuHandlers\Unlinka
  (Default) = {3830A560-FF20-4D90-8B90-BEEF4A7AE496}
~~~

安装器直接管理上述专属注册项，不使用 `regsvr32`。不修改文件默认关联、系统删除 verb、Shell Extensions 的全局策略或其他产品的 CLSID。已有同名但归属不匹配的注册项视为冲突。

微软列出 `AllFileSystemObjects` 为文件和文件夹的菜单关联点，并要求 Shell 扩展使用 Apartment 线程模型；注册变更后使用 `SHChangeNotify(SHCNE_ASSOCCHANGED)` 通知 Shell。[注册文档](https://learn.microsoft.com/en-us/windows/win32/shell/reg-shell-exts)

经典类必须正确处理 `QueryContextMenu` 的命令编号范围和 `CMF_DEFAULTONLY`，支持按编号及 Unicode canonical verb 调用，并正确实现 COM 对象、类工厂和 DLL 卸载计数。

### 3.3 菜单行为与进程交接

- 菜单文案为“用 Unlinka 永久删除…” / “Permanently delete with Unlinka…”；只提供一个主要操作，不增加子菜单。
- 支持单选与文件/目录混合多选，整批交给一次任务；不将每个对象启动为独立进程。
- 经典入口由 `IDataObject` 获取选中项，再复制为原始 UTF-16 文件系统路径。
- 空选择、虚拟对象、目录背景和卷根不提供操作；所有选中项必须可表达为文件系统对象，不能默默丢弃其中一部分。路径保护的最终判断仍由引擎负责。
- 普通 `.lnk` 文件及重解析点按选中的入口处理，不主动解析快捷方式目的地。
- 菜单构建方法只进行轻量状态和选择检查，不扫描目录。调用动作后才准备请求与启动进程，耗时准备不得阻塞 Explorer 的菜单线程。
- 所有 COM 边界返回明确 HRESULT，Rust panic 不得跨 FFI；`DllMain` 不做翻译解析、进程启动或注册表维护。
- 异步工作只接收已复制的路径，或正确 marshaling 后的 COM 接口；不能跨线程传递未封送的接口。后台工作期间保持 DLL 引用，结束时释放。

请求协议 v1 使用 JSON，包含 `schema_version`、固定的 `action=permanent_delete`、`paths_utf16` 和界面语言；每个路径是 `u16` 数组，保留非配对 UTF-16。协议不包含 `yes`、提权或任意可执行命令。

请求最多 65,536 个选中项、32 MiB；超过任一限制时整批报告错误，不截断。请求目录 ACL 限定当前用户与 SYSTEM，使用随机 GUID 和 create-new 创建文件；桌面进程仅接受 `--shell-request <guid>`，自行解析请求目录。拒绝目录/文件的重解析入口、未知 schema、非法字段及不完整数据，读取期间禁止其他普通写入。

DLL 从所属安装根和活动版本定位桌面程序，使用显式绝对程序路径启动；不依赖 PATH。桌面进程取得共享维护锁并重新核对安装状态后，读取完整请求并清理文件，再预检查、确认和执行。所有路径必须完整读取成功才显示确认。

请求不会被后台自动执行。启动失败时清理本次请求；下次任务或维护只清理可确认归属且超过 24 小时的遗留请求。升级期间已加载的旧 DLL 可通过 v1 启动新的活动桌面程序；首版各版本保持 v1 兼容，不要求通过重启 Explorer 才能使用新任务进程。

## 4. 安装、升级、修复和卸载

### 4.1 统一事务规则

Inno Setup 负责安装向导、文件部署入口和稳定 AppId；`unlinka-maintenance.exe` 负责可重试的状态、注册、PATH 和文件部署操作。辅助工具和稳定启动器只在没有运行任务的维护阶段更新。

Inno 脚本固定 `PrivilegesRequired=lowest`、`DisableDirPage=yes`、`AllowCancelDuringInstall=no`、`CloseApplications=no`、`RestartApplications=no`。向导初始化和安装准备阶段强制固定目录，`/DIR` 与 `/LOADINF` 不能让 Inno 的卸载记录与维护工具的安装根分离。准备阶段只解包并保存参数；`ssPostInstall` 在 Inno 保存卸载器与产品条目之后调用维护工具。维护失败返回非零安装退出码并显示失败文案；升级回滚后恢复原活动版本的快捷方式与显示图标。没有 Installation 记录而只剩 Inno 元数据时，卸载器仍可清理自己的记录。

所有维护动作由核验过的安装根外运行副本执行，以便替换安装根内的辅助工具。设置界面先检查稳定维护工具的完整路径、文件归属和事务认可的哈希，再复制并直接运行副本，避免稳定维护 EXE 被等待中的父进程占用。版本 payload、产品注册和 PATH 的卸载由维护清单负责，不能让 Inno 再执行一套未经归属校验的重复清理。

CLI、桌面确认和删除任务持有共享 activity lock；维护必须取得独占锁。有任务运行时，提示等待完成或取消本次维护，不结束删除进程。菜单请求与维护竞争时，桌面任务应显示维护中诊断，不开始删除。

每次维护先记录事务前状态，并在每一步成功后持久化进展，覆盖文件、注册项和 PATH 所有权。事务记录只能操作准确的产品路径与身份，所有删除都核对真实安装根、专属文件清单与路径包含关系；不递归清理未知用户文件或跟随重解析目录。

撤销注册项之前比较当前值与本安装写入的值；后来被其他程序或用户改写的项保留并报告冲突。设置变化也遵守维护锁，不会与注册或清理事务交叉提交。

只有文件、所选集成项、适用的包注册和安装状态均核对成功才能提交 `Ready`。失败按相反顺序补偿；恢复失败进入 `RepairRequired`，保留旧/新 payload、日志和维护入口，不显示安装成功。

启动下一次安装、修复或卸载时先处理未完成事务。PATH 回滚只逆转本安装的修改，不能覆盖事务期间其他程序或用户的环境修改。写入前发现并发冲突时撤销尚未执行的 PATH intent，防止回滚误删外部刚加入的同名项。活动锁只有实际锁竞争返回“忙碌”；其他 Windows 错误保留安装位置和 HRESULT。

`signed=false` 不跳过 SHA-256、请求 ACL、维护锁、ownership、路径包含关系或事务日志保护。`signed=true` 的运行副本及 EXE/DLL 须通过签名核验；清单和签名状态不一致时拒绝安装。

### 4.2 首次安装

1. 检查 Windows、原生 x64、当前用户、安装位置、磁盘空间、现存注册冲突及所选组件；验证构建元数据、文件清单和 SHA-256；新生成的 schema 1 `payload.json` 显式声明 `signed`，签名构建另核验签名和证书 Subject。旧版清单缺少该字段时按签名构建处理，保留原有签名要求。
2. 取得维护锁，记录原状态，进入 `Installing`，将全部 payload 复制到 staging 并验证清单。
3. 发布完整版本目录，部署稳定启动器和维护工具。
4. 菜单启用时注册经典入口。
5. 按选项加入用户 PATH，保存初始语言和选项；核对所有注册与程序路径。
6. 写入 `ActiveVersion` 并提交 `Ready`，通知 Shell 与环境变更，完成向导。首次安装要求全部所选集成操作成功。

安装向导中的语言、菜单和 PATH 选项保持直接可见；卸载信息只有一个产品条目。首次安装失败时撤销新增集成，按清单清理已部署文件；仍未清理的内容保留恢复记录。

### 4.3 升级

1. 取得独占维护锁并保存旧活动版本、注册项、包及选项；进入 `Updating`。保留已有语言、菜单和 PATH 设置。
2. 部署并核验新的完整版本目录；不覆盖旧版 DLL。
3. 菜单启用时切换经典 COM 路径。
4. 核对已部署文件、注册和协议兼容，再切换 `ActiveVersion`，提交 `Ready` 并通知 Shell。
5. 保留上一完整版本用于恢复；清理更旧且未被占用的专属文件。被占用的旧 DLL 留在清理清单中，在后续维护时重试，不启动常驻清理服务。

升级失败时按持久化事务记录恢复旧文件、经典注册和旧活动版本。维护忙时提示稍后重试，不强制关闭运行任务。

禁止通过普通安装器隐式降级；同版本走修复，要求文件哈希与签名策略一致。改变签名状态或证书必须使用新的、较高的构建号，不能以同版本修复替换 payload。较旧发行包提示使用当前或更新版本。事务恢复使用保留的旧版本，不等于向用户提供任意版本降级。

### 4.4 修复与菜单设置

- 复验活动版本文件、经典注册和 PATH 所有权；保留用户设置。
- 文件缺失或损坏时从同版本、签名状态一致且通过核验的安装包恢复；被加载文件无法替换时保留状态并提示重试，不宣告修复完成。
- 设置界面的维护副本不会映射稳定维护 EXE；直接从安装目录调用维护命令仍会等待其副本。在中断升级需要回滚维护 EXE 本身的场景，等待中的原进程可能阻止替换，应重新运行当前或更新 Setup 恢复，不能将稳定命令重试视为已验证的恢复路径。
- 关闭“Explorer 菜单”先禁止新的调用，再移除经典入口；CLI 仍可使用。
- 开启菜单注册经典入口，成功后恢复启用状态。
- 更新语言不重装程序；下次打开的菜单和任务使用新语言，正在执行的任务保持原语言。
- 提供 `unlinka --integration-status`，显示程序、经典菜单、签名证书主体、PATH 和待清理状态；它与删除参数互斥，在维护/待清理状态也可使用。采用长选项避免把现有名为 integration 的文件路径改成子命令。

### 4.5 卸载

1. 取得独占维护锁；有确认或删除任务时提示完成任务后重试，不主动取消任务。
2. 记录可重试的卸载事务，进入 `Removing`，阻止已加载 DLL 和启动器发起新任务。
3. 移除经典菜单关联和本安装的经典 CLSID。
4. 移除本安装拥有的 PATH 项，通知 Shell 与环境变更。
5. 维护工具从安装根外的用户专属运行副本执行，按 ownership 清单删除 payload；不能删除正在执行的辅助工具来假定清理成功。
6. 文件清理完成后才让 Inno Setup 完成其卸载入口与产品条目的移除，并清除 Installation 状态。

经典 DLL 仍被 Explorer 加载而无法删除时，进入 `RemovalPending`：菜单入口和 PATH 已移除，但保留必要的维护文件、事务和“应用与功能”卸载入口，界面明确显示等待文件释放。用户关闭相关窗口、重新启动 Explorer 或重新登录后，再运行卸载即可完成清理。不强制结束 Explorer，也不依赖需要提权的重启替换。

Inno 的 `InitializeUninstall` 只校验并复制维护工具至临时目录。Inno 内建卸载确认之后，`CurUninstallStepChanged(usAppMutexCheck)` 执行临时副本；辅助工具再提供包含保留/清除偏好的原生确认。成功后才允许 Inno 删除其产品条目、日志与快捷方式；取消、`RemovalPending` 或失败以 `Abort` 中止最终清理。不能等待仍映射在安装根内的维护 EXE，否则它无法删除自己。直接运行已安装维护工具的卸载命令会转交 Inno 卸载入口。再次卸载从剩余步骤继续；[Inno 事件文档](https://jrsoftware.org/ishelp/topic_scriptevents.htm)、[7.1 卸载确认顺序](https://github.com/jrsoftware/issrc/blob/is-7_1_0/Projects/Src/Setup.Uninstall.pas#L670)

默认保留 `Settings` 中的语言、菜单和 PATH 偏好及有界维护日志，提供“同时删除设置和诊断日志”选项。已经关闭菜单时，卸载临时禁用菜单不产生变更记录，收尾也不删除原有禁用偏好。偏好、安装注册表和内部文件收尾失败均走同一恢复路径，恢复维护入口并保留 `RemovalPending`。请求和操作记录不保留为可再次执行的队列。不删除证书信任、其他安装的注册项、用户改动的 PATH 或未知文件。

清除偏好的选择在删除前保存到卸载事务，失败后重试默认继续该选择。清除已经开始后不能通过下一次取消勾选恢复已删除的数据，收尾按持久化选择继续，避免部分偏好恢复而其他偏好丢失。

若卸载在删除维护工具后中断，仍有卸载事务但根内工具缺失时，重新运行通过核验的 Setup 只恢复核验过的维护工具、保留 `RemovalPending` 并提示重跑卸载，不发布新 payload 或继续普通安装。未知文件保留并报告，不能仅因目录非空而永久进入待清理状态。

## 5. 桌面任务与 i18n

桌面任务使用 Windows 原生对话框，完成预检查 → 永久删除确认 → 执行 → 结果。删除确认默认取消；执行显示活动指示、已完成数量、耗时和取消，不显示未经完整扫描的百分比。关闭执行窗口请求取消，并等待已经开始的调用返回。

确认只写“永久删除”，单项目显示名称与位置，多项目显示数量。“查看目标”打开固定大小的原生只读滚动窗口，保留完整路径并支持选取复制；关闭详情回到确认，不触发删除，也不撑大主弹窗。结果区分完成、未完成与已取消，失败总数及首条路径/原因直接可见，“查看失败详情”使用同一滚动窗口。取消期间保留进度计数。CLI 默认显示 5 条失败，`-v` 展示最多 100 条保留详情及操作、错误码；JSON 保持原有完整诊断边界。常见系统错误使用简短的本地化原因，未知错误保留系统说明。

引擎发布合并后的进度快照，刷新最多 10 次/秒，不在内部锁中调用界面逻辑。桌面进程只在 UI 线程更新控件；未启用观察的 CLI 沿用原有结果汇总。

桌面进程工作目录固定为安装根；CLI 保留调用者目录。core 的现有路径保护不因 Shell 入口放宽。预检查及执行前复核仍覆盖整批目标，确认期间目标替换应整批拒绝。

新增共享 `unlinka-i18n`，采用 Fluent，资源编译进程序；英文单复数使用 Fluent selector。[Fluent 说明](https://projectfluent.org/fluent/guide/selectors.html)

- 覆盖 CLI 帮助、参数错误、确认、执行状态、结果、菜单、安装器专属文案和维护诊断。
- CLI 优先级：显式 `--lang` → `UNLINKA_LANG` → 保存语言 → Windows 首选显示语言 → `en-US`。显式 `--lang auto` 直接按 Windows 显示语言协商。
- Shell 和安装器使用保存语言，再按 Windows 显示语言协商；不依赖 Explorer 的环境变量副本。简体中文区域/脚本与英语变体映射到已有资源；未支持的繁体中文继续检查下一首选语言，最终回退英语。
- `--lang` 必须在 clap 渲染帮助或参数错误前生效，预解析不得将 `--` 后面的文件名当成语言选项。clap 保留参数解析，使用其结构化错误上下文进行本地化。
- 删除引擎提供稳定诊断类型、操作标识、路径和系统错误码，不解析或翻译已有英文字符串；界面翻译已知原因，未知系统错误保留错误码及可用系统说明。
- 确认窗口的删除、取消按钮均使用应用资源，不依赖系统按钮语言与当前应用语言一致。
- 安装器标准向导使用维护的 `.isl` 翻译；Unlinka 专属文案从共享翻译资源导出，限制为安装器可表达的模板。
- 新增 `--json` 最终结果输出：执行、预检查失败及取消等非帮助结果在 stdout 仅写一个有 schema 版本的 JSON，stderr 保留人类诊断；帮助/版本查询保留文本并优先处理。字段、退出码、诊断标识、原始路径表示和数值不受语言影响。JSON 模式仍需按现有规则确认，不能隐式跳过确认。现有 benchmark 改读 JSON。

## 6. 代码与构建边界

| 位置                  | 实现职责                                               |
| --------------------- | ------------------------------------------------------ |
| `modules/core`        | 保持独立删除引擎；增加结构化诊断和可选、节流的进度观察 |
| `modules/app`         | 原有 CLI；语言引导、安装任务锁和 JSON 结果             |
| `modules/i18n`        | 语言协商、FTL 资源和格式化                             |
| `modules/integration` | 注册与状态、任务协议、PATH/安装生命周期；维护工具 bin  |
| `modules/launcher`    | 稳定 CLI 启动器                                        |
| `modules/desktop`     | 桌面任务文案、删除 worker/取消/结果及语言/菜单设置入口 |
| `modules/ui`          | WinSafe 任务对话框、详情窗口、进度事件及诊断提示       |
| `modules/shell`       | cdylib、经典 COM 类及选中项适配                        |
| `assets`              | 可编辑 SVG 源图及生成的 Windows PNG / 多尺寸 ICO 资源  |
| `packaging`           | Inno 脚本、固定产品标识及签名脚本                      |
| `modules/xtask`       | 图标生成、编译器获取、打包、payload 核验和 benchmark   |

Shell 不链接删除引擎；core 不依赖 i18n、注册表设置或 Shell。`unlinka-ui` 通过 WinSafe 0.0.29 集中原生 UI 操作，不依赖 core、integration 或 i18n；调用方提供文案并管理业务。详情使用 WinSafe 控件构造有 owner 的模态窗口，按 owner DPI 初始化，并在 `WM_DPICHANGED` 时调整尺寸和字体。UI 的 `dialogs` feature 仅供桌面和维护工具使用，Shell 只调用基本诊断提示；integration 的 `maintenance-ui` 为维护 bin 的默认可选 feature，workspace 中的库调用方关闭它。COM 与 Windows 集成采用 `windows` 0.62.2 及匹配的 `windows-core`，已有 core 的 `windows-sys` 调用保持现状，UI 用私有封装补足 WinSafe 未覆盖的任务消息和 DPI API。Fluent 使用 `fluent-bundle` 0.16.0 / `unic-langid` 0.9.6；版本与 features 在 [Cargo.toml](../Cargo.toml) 声明并通过 Cargo.lock 固定。

所有构建先生成版本、构建程序并嵌入资源。四个 EXE 共用 Windows manifest 与构建辅助模块，均声明 Common Controls v6。打包产物静态链接 MSVC CRT，不要求另装 VC++ 运行库；RC 显式使用 UTF-8 读取资源脚本和路径。可选签名构建先签名 EXE/DLL，再构建并签名 Setup 及内嵌卸载器；签名后不能改写二进制。

### 6.1 构建统一安装包

构建需要 Rust 1.99 MSVC 和 Windows SDK x64 的 `rc`，签名构建另外需要 `signtool`。`--sdk-bin` 可显式指定 SDK x64 bin，否则选取已安装最高版本。Inno 编译器由 `xtask` 自动下载、校验和缓存，无需单独安装 Inno、NuGet 或 .NET，也无需传编译器路径。

~~~powershell
# 默认未签名包
cargo xtask pack --build-number 2

# 可选签名：三个签名参数必须同时提供，Subject 与 --publisher 完全一致
cargo xtask pack --publisher "CN=Your Publisher" --certificate-thumbprint <40位公开指纹> --timestamp-url <RFC3161服务URL> --build-number 3
~~~

编译器使用 [Tools.InnoSetup](https://www.nuget.org/packages/Tools.InnoSetup/7.1.0) 7.1.0。它是第三方对官方编译器的重新分发，包含编译器依赖和官方中英文翻译。当前保持已验证的 7.1.0 以配合现有脚本；[官方 7.1.1](https://jrsoftware.org/files/is7-whatsnew.htm) 已发布，但该重新分发包尚未同步。版本和归档 SHA-256 由源码固定，普通打包不会自动追随上游版本变化。

首次打包使用 HTTPS 下载归档；复用前核对固定 SHA-256，从可信归档读取预期文件列表与内容，再核对缓存。缺失或损坏的缓存重新构建，下载或校验失败时停止，不执行未核验的编译器。缓存保存在仓库 `target/tools` 下，工具获取受文件锁保护；已有有效缓存时无需联网。下载、缓存复用和修复结果写到 stderr。

签名证书从当前用户 `My` 存储选取，私钥或密码不作为命令参数。EXE/DLL 使用 `signtool verify /pa /all` 核验；Inno 使用仓库的 `sign-artifact.ps1` 签名并核验 Setup、内嵌卸载器及临时副本。构建不读取编译器 IDE 的额外签名配置。省略全部签名参数即生成未签名包；`--publisher`、`--certificate-thumbprint` 和 `--timestamp-url` 必须全部提供或全部省略，指纹为 40 位十六进制。[Inno 签名卸载器说明](https://jrsoftware.org/ishelp/topic_setup_signeduninstaller.htm)

payload schema 1 使用 `signed` 和 `signer_subject` 描述签名策略。现有未签名清单的 `publisher` 仍可读取；新记录只写 `signer_subject`。已有未签名 ownership 和未完成事务记录中的空旧字段可读取，提交后的新记录省略这些字段。旧安装元数据中的不再使用的注册表值在维护时清理；文件、设置和 PATH 的归属规则保持一致。

安装包版本为 Cargo 的 `major.minor.patch` 加 `--build-number` 的第四段，每段受 `u16` 限制；现有输出不可覆盖。两种签名状态的包路径均为 `target/packages/<version>`，Setup 均名为 `Unlinka-Setup-<version>-x64.exe`。切换签名状态也必须使用新的构建号。输出包含 Setup、工具 EXE、完整 payload、生成资源和 Inno 脚本。

### 6.2 GitHub Actions 发行

[Release 工作流](../.github/workflows/release.yml) 接收 `v<major.minor.patch>` 和 `v<major.minor.patch>.<build-number>` 标签。前三段必须与仓库 Cargo 版本完全一致，各段为不含多余前导零的十进制数，范围为 `0`–`65535`；省略第四段时使用 `0`。例如 Cargo 版本为 `0.1.1` 时，`v0.1.1` 生成 `0.1.1.0`，`v0.1.1.5` 生成 `0.1.1.5`。不匹配的标签在检查和打包前失败。

标签通过验证后，并行调用[共用检查](../.github/workflows/checks.yml)和[安装包构建](../.github/workflows/package.yml)。两者全部成功后，通过 GitHub CLI 为已有标签创建公开 Release，自动生成发行说明，并附上 `Unlinka-Setup-<version>-x64.exe` 和同名 `.exe.sha256` 文件。所有自动构建均省略签名参数。只有发布任务拥有 `contents: write` 权限，无需额外配置访问令牌或签名证书。

手动运行 Release 工作流时选择分支并填写 `build-number`，默认 `0`；该入口仅构建安装包并上传 Actions 产物，不运行共用检查、不创建标签或 Release。安装包和校验文件的 Actions 产物保留 14 天。已发布的 Release 不自动覆盖；发布成功后重新运行整个工作流会在创建同名 Release 时失败。

检查与打包共用 Rust 安装和缓存配置，分别保存 Cargo 依赖构建结果；打包另外缓存专用静态 CRT 构建目录和经过校验的 Inno 工具。默认分支成功运行后保存 Rust 缓存，标签和拉取请求可复用。版本输出目录不缓存。常规 CI 只响应分支推送，避免标签推送重复执行同一套检查。

## 7. 待完成的发行与验收

| 范围         | 验收内容                                                                                           |
| ------------ | -------------------------------------------------------------------------------------------------- |
| 可选签名     | 稳定证书 Subject、代码签名证书和时间戳服务，签名构建与可信环境安装                                 |
| 经典菜单     | Win10 及 Win11“显示更多选项”各一个入口，混合多选、中文/长路径、快捷方式和 junction 完整传递        |
| 桌面任务     | 中英文确认、默认取消、执行取消、失败详情、DPI 和目标替换                                           |
| 安装生命周期 | 普通用户安装、同版本修复、跨版本及签名状态切换升级、进程中断恢复、PATH 外部修改和占用 DLL 卸载重试 |

需要 Win10 x64 与 Win11 x64 的实际验收环境。源码与打包检查不能替代真实安装、Explorer/UI、删除或性能验证。x64 Setup 在 ARM64 上运行不能证明原生 Explorer 扩展受支持。当前检查结果和未验证范围见 [验收现状](acceptance.md)。
