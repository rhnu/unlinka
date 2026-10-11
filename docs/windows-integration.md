# Windows 集成与 i18n

状态：安装器、双 COM 菜单、维护工具、桌面任务与共享双语代码已实现。统一安装包默认未签名且可安装，证书签名为可选功能；证书签名构建及真实 Windows 菜单与生命周期验收尚未运行。本文记录实现契约，源码检查结果与验证缺口见 [验收现状](acceptance.md)。

## 1. 交付范围与固定决定

- 默认交付当前用户安装包 `Unlinka-Setup-<version>-x64.exe`，使用 Inno Setup，不请求管理员权限。
- 首版覆盖原生 x64 Explorer。默认未签名安装包在 Windows 10 注册经典菜单，在 Windows 11 注册“显示更多选项”中的经典菜单；签名构建在 Windows 11 还注册新菜单。
- 菜单入口默认启用，CLI 默认加入用户 PATH；高级安装选项允许关闭菜单或 PATH 集成。
- 保持用户已有的 Explorer 菜单设置。菜单适配共用任务协议、桌面进程和删除引擎。
- 首批语言为 `zh-CN` 与 `en-US`，默认跟随 Windows 显示语言。
- 沿用现有永久删除、预检查、身份复核、不跟随重解析点、取消和失败汇总语义。
- 首版不包含全用户安装、ARM64/x86 扩展、自动更新服务或目录背景菜单。这些范围不影响本次 x64 安装器和双菜单交付。

Windows 11 新菜单采用 `IExplorerCommand` 和应用包身份；签名构建通过 sparse MSIX 提供身份，由安装器部署外部程序文件。未签名构建不生成 MSIX，也不内嵌桌面包身份。[微软集成说明](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/integrate-packaged-app-with-file-explorer)

## 2. 安装布局与状态归属

`<root>` 默认是当前用户的 `%LocalAppData%\Programs\Unlinka`。首版固定该位置，避免迁移目录、PATH 和包的 external location 形成额外生命周期。

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
      unlinka-shell.dll                # 两种菜单适配器
      Unlinka.ShellIntegration.msix    # 仅签名构建包含的身份包
      payload.json                    # schema 1、signed、版本、协议和 SHA-256
      Assets\
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

| 位置                                 | 数据与责任                                                                              |
| ------------------------------------ | --------------------------------------------------------------------------------------- |
| `HKCU\Software\Unlinka\Installation` | `InstallRoot`、`ActiveVersion`、`State`、包身份、安装架构和 PATH 所有权；由维护工具写入 |
| `HKCU\Software\Unlinka\Settings`     | `Language=auto\|zh-CN\|en-US`、`MenusEnabled`、`PathEnabled`；由设置入口及安装器维护    |
| `<root>\state`                       | 操作清单、事务前状态和未完成清理；用于恢复，不作为第二份活动版本来源                    |

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

## 3. 两种菜单的注册契约

### 3.1 固定产品标识

以下标识在产品生命周期中保持不变，不随版本重新生成：

| 标识                    | 值                                       |
| ----------------------- | ---------------------------------------- |
| Inno Setup AppId        | `{6C273E32-2E9B-450A-A683-4AED2D9D6A04}` |
| 经典菜单 CLSID          | `{3830A560-FF20-4D90-8B90-BEEF4A7AE496}` |
| 新菜单 CLSID            | `{6842E106-7F2C-48CF-8F0A-DD54D36D9BA1}` |
| 删除动作 canonical GUID | `{E792E7B0-7A60-46A4-92C6-327B24915404}` |
| 经典菜单 canonical verb | `Unlinka.PermanentDelete`                |
| 身份包 Name             | `Unlinka.ShellIntegration`               |
| 身份包 Application Id   | `UnlinkaDesktop`                         |

签名构建的 `Publisher` 取自签名证书，并在首次部署身份包后保持身份连续。固定标识的唯一来源为 [identity.json](../packaging/identity.json)，签名与未签名构建共用这些标识、安装目录及 `Unlinka` 注册表/数据命名空间。构建不添加证书或修改信任。未签名安装不要求 Authenticode 签名，仍执行文件哈希与归属核验；签名安装额外验证签名与 Publisher。

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

### 3.3 Windows 11 新菜单：签名身份包

同一 DLL 的新菜单类实现 `IExplorerCommand`，使用独立 CLSID。身份包通过 `windows.comServer` 的 surrogate server 注册该类，通过 `windows.fileExplorerContextMenus` 分别关联 `*` 与 `Directory`：

~~~xml
<com:Extension Category="windows.comServer">
  <com:ComServer>
    <com:SurrogateServer DisplayName="Unlinka">
      <com:Class
        Id="6842E106-7F2C-48CF-8F0A-DD54D36D9BA1"
        Path="unlinka-shell.dll"
        ThreadingModel="STA" />
    </com:SurrogateServer>
  </com:ComServer>
</com:Extension>
<desktop4:Extension Category="windows.fileExplorerContextMenus">
  <desktop4:FileExplorerContextMenus>
    <desktop5:ItemType Type="*">
      <desktop5:Verb Id="UnlinkaDeleteFile"
        Clsid="6842E106-7F2C-48CF-8F0A-DD54D36D9BA1" />
    </desktop5:ItemType>
    <desktop5:ItemType Type="Directory">
      <desktop5:Verb Id="UnlinkaDeleteDirectory"
        Clsid="6842E106-7F2C-48CF-8F0A-DD54D36D9BA1" />
    </desktop5:ItemType>
  </desktop4:FileExplorerContextMenus>
</desktop4:Extension>
~~~

此片段只表示扩展注册部分，完整 manifest 由模板生成。外部位置固定为该版本的 `<root>\versions\<package-version>`，DLL、桌面程序和 Assets 均相对于该位置。签名构建的桌面程序内嵌 msix 元数据，匹配包 Name、Publisher 和 Application Id；未签名桌面程序不包含该身份。

签名构建在 Windows 11 启用菜单时注册身份包；未签名构建及 Windows 10 仅使用经典注册，不部署新菜单身份。包 manifest 的最低版本为 Windows 11 build 22000，架构为 x64，声明外部内容和所需 Win32 能力。包不额外创建开始菜单中的重复应用。

现代包只注册新菜单类；经典类统一由上面的 HKCU 项注册，不再通过 `desktop9` 重复注册。两种菜单各只出现一次，尤其需要核对文件与目录混合多选。

维护工具使用 Windows `PackageManager` API 完成包注册、查询和移除，等待异步操作完成并保留 HRESULT；不通过拼接 PowerShell 命令传递路径。添加使用 `AddPackageByUriAsync` 与 `AddPackageOptions.ExternalLocationUri`，关闭 `AllowUnsigned`、`ForceAppShutdown`、`ForceTargetAppShutdown` 和 `DeferRegistrationWhenPackagesAreInUse`。所有包操作匹配准确的 Name、Publisher、架构和当前用户，只操作本产品身份。[部署选项文档](https://learn.microsoft.com/en-us/uwp/api/windows.management.deployment.addpackageoptions?view=winrt-26100)

受信任签名、Publisher 一致性、可重装版本和 external location 均是部署前提。同版本包若已正确注册，修复时直接复用；必须重注册时先移除准确的现有包，再注册保留的签名包。[微软身份包说明](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/grant-identity-to-nonpackaged-apps)

### 3.4 菜单行为与进程交接

- 菜单文案为“用 Unlinka 永久删除…” / “Permanently delete with Unlinka…”；只提供一个主要操作，不增加子菜单。
- 支持单选与文件/目录混合多选，整批交给一次任务；不将每个对象启动为独立进程。
- 经典入口由 `IDataObject` 获取选中项，新入口由 `IShellItemArray` 获取选中项，最终统一为原始 UTF-16 文件系统路径。
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

Inno Setup 负责安装向导、文件部署入口和稳定 AppId；`unlinka-maintenance.exe` 负责可重试的状态、注册、PATH 和包部署操作。辅助工具和稳定启动器只在没有运行任务的维护阶段更新。

Inno 脚本固定 `PrivilegesRequired=lowest`、`DisableDirPage=yes`、`AllowCancelDuringInstall=no`、`CloseApplications=no`、`RestartApplications=no`。向导初始化和安装准备阶段强制固定目录，`/DIR` 与 `/LOADINF` 不能让 Inno 的卸载记录与维护工具的安装根分离。准备阶段只解包并保存参数；`ssPostInstall` 在 Inno 保存卸载器与产品条目之后调用维护工具。维护失败返回非零安装退出码并显示失败文案；升级回滚后恢复原活动版本的快捷方式与显示图标。没有 Installation 记录而只剩 Inno 元数据时，卸载器仍可清理自己的记录。

所有维护动作由核验过的安装根外运行副本执行，以便替换安装根内的辅助工具。设置界面先检查稳定维护工具的完整路径、文件归属和事务认可的哈希，再复制并直接运行副本，避免稳定维护 EXE 被等待中的父进程占用。版本 payload、产品注册和 PATH 的卸载由维护清单负责，不能让 Inno 再执行一套未经归属校验的重复清理。

CLI、桌面确认和删除任务持有共享 activity lock；维护必须取得独占锁。有任务运行时，提示等待完成或取消本次维护，不结束删除进程。菜单请求与维护竞争时，桌面任务应显示维护中诊断，不开始删除。

每次维护先记录事务前状态，并在每一步成功后持久化进展，覆盖文件、注册项、包身份、external location 和 PATH 所有权。事务记录只能操作准确的产品路径与身份，所有删除都核对真实安装根、专属文件清单与路径包含关系；不递归清理未知用户文件或跟随重解析目录。

撤销注册项之前比较当前值与本安装写入的值；后来被其他程序或用户改写的项保留并报告冲突。设置变化也遵守维护锁，不会与注册或清理事务交叉提交。

只有文件、所选集成项、适用的包注册和安装状态均核对成功才能提交 `Ready`。失败按相反顺序补偿；恢复失败进入 `RepairRequired`，保留旧/新 payload、日志和维护入口，不显示安装成功。

启动下一次安装、修复或卸载时先处理未完成事务。PATH 回滚只逆转本安装的修改，不能覆盖事务期间其他程序或用户的环境修改。写入前发现并发冲突时撤销尚未执行的 PATH intent，防止回滚误删外部刚加入的同名项。活动锁只有实际锁竞争返回“忙碌”；其他 Windows 错误保留安装位置和 HRESULT。

`signed=false` 不跳过 SHA-256、请求 ACL、维护锁、ownership、路径包含关系或事务日志保护。`signed=true` 的运行副本、EXE/DLL 和适用身份包须通过签名核验；清单和签名状态不一致时拒绝安装。

### 4.2 首次安装

1. 检查 Windows、原生 x64、当前用户、安装位置、磁盘空间、现存注册冲突及所选组件；验证构建元数据、文件清单和 SHA-256；新生成的 schema 1 `payload.json` 显式声明 `signed`，签名构建另核验签名和 Publisher。旧版清单缺少该字段时按签名构建处理，保留原有签名要求。
2. 取得维护锁，记录原状态，进入 `Installing`，将全部 payload 复制到 staging 并验证清单。
3. 发布完整版本目录，部署稳定启动器和维护工具。
4. 菜单启用时注册经典入口；签名构建在 Windows 11 同时注册该版本的身份包，并查询实际 external location。
5. 按选项加入用户 PATH，保存初始语言和选项；核对所有注册与程序路径。
6. 写入 `ActiveVersion` 并提交 `Ready`，通知 Shell 与环境变更，完成向导。首次安装要求当前构建及系统适用的菜单全部成功；签名构建的新菜单注册失败不能悄悄降为经典菜单完成安装。

安装向导中的语言、菜单和 PATH 选项保持直接可见；卸载信息只有一个产品条目。首次安装失败时撤销新增集成，按清单清理已部署文件；仍未清理的内容保留恢复记录。

### 4.3 升级

1. 取得独占维护锁并保存旧活动版本、注册项、包及选项；进入 `Updating`。保留已有语言、菜单和 PATH 设置。
2. 部署并核验新的完整版本目录；不覆盖旧版 DLL。
3. 菜单启用时切换经典 COM 路径。未签名升级为签名构建时，在 Windows 11 部署该版本的身份包；签名版本升级时更新同一 Name / Publisher 的身份包，external location 指向新版本。签名升级为未签名构建时移除本安装拥有的身份包，保留经典菜单。
4. 确认包部署已实际完成，核对注册和协议兼容，再切换 `ActiveVersion`，提交 `Ready` 并通知 Shell。
5. 保留上一完整版本及适用的签名包用于恢复；清理更旧且未被占用的专属文件。被占用的旧 DLL 留在清理清单中，在后续维护时重试，不启动常驻清理服务。

升级失败时恢复旧经典注册和旧活动版本。包需要回退时移除准确的新包，再按旧版本签名状态恢复包：旧版本已签名则注册保留的旧签名包，旧版本未签名则保持无身份包。不假定较低版本可以直接覆盖较高版本。部署忙时提示稍后重试，不使用强制关闭或静默延迟注册后宣告成功。

禁止通过普通安装器隐式降级；同版本走修复，要求文件哈希与签名策略一致。改变签名状态或证书必须使用新的、较高的构建号，不能以同版本修复替换 payload。较旧发行包提示使用当前或更新版本。事务恢复使用保留的旧版本，不等于向用户提供任意版本降级。

### 4.4 修复与菜单设置

- 复验活动版本文件、经典注册、适用的 Windows 11 身份包及 external location、PATH 所有权；保留用户设置。
- 文件缺失或损坏时从同版本、签名状态一致且通过核验的安装包恢复；被加载文件无法替换时保留状态并提示重试，不宣告修复完成。
- 设置界面的维护副本不会映射稳定维护 EXE；直接从安装目录调用维护命令仍会等待其副本。在中断升级需要回滚维护 EXE 本身的场景，等待中的原进程可能阻止替换，应重新运行当前或更新 Setup 恢复，不能将稳定命令重试视为已验证的恢复路径。
- 关闭“Explorer 菜单”先禁止新的调用，再移除两种入口；CLI 仍可使用。
- 开启菜单按当前构建签名状态和系统注册全部适用入口，成功后恢复启用状态。
- 更新语言不重装程序；两种菜单下次构建和新任务使用新语言，正在执行的任务保持原语言。
- 提供 `unlinka --integration-status`，显示程序、两种菜单、身份包、PATH 和待清理状态；它与删除参数互斥，在维护/待清理状态也可使用。采用长选项避免把现有名为 integration 的文件路径改成子命令。

### 4.5 卸载

1. 取得独占维护锁；有确认或删除任务时提示完成任务后重试，不主动取消任务。
2. 记录可重试的卸载事务，进入 `Removing`，阻止已加载 DLL 和启动器发起新任务。
3. 移除 Windows 11 的准确产品身份包及经典关联，然后移除本安装的经典 CLSID。
4. 移除本安装拥有的 PATH 项，通知 Shell 与环境变更。
5. 维护工具从安装根外的用户专属运行副本执行，按 ownership 清单删除 payload；不能删除正在执行的辅助工具来假定清理成功。
6. 文件清理完成后才让 Inno Setup 完成其卸载入口与产品条目的移除，并清除 Installation 状态。

经典 DLL 仍被 Explorer 加载而无法删除时，进入 `RemovalPending`：菜单入口和 PATH 已移除，但保留必要的维护文件、事务和“应用与功能”卸载入口，界面明确显示等待文件释放。用户关闭相关窗口、重新启动 Explorer 或重新登录后，再运行卸载即可完成清理。不强制结束 Explorer，也不依赖需要提权的重启替换。

Inno 的 `InitializeUninstall` 只校验并复制维护工具至临时目录。Inno 内建卸载确认之后，`CurUninstallStepChanged(usAppMutexCheck)` 执行临时副本；辅助工具再提供包含保留/清除偏好的原生确认。成功后才允许 Inno 删除其产品条目、日志与快捷方式；取消、`RemovalPending` 或失败以 `Abort` 中止最终清理。不能等待仍映射在安装根内的维护 EXE，否则它无法删除自己。直接运行已安装维护工具的卸载命令会转交 Inno 卸载入口。再次卸载从剩余步骤继续；若包尚未移除，保留其所需 payload。[Inno 事件文档](https://jrsoftware.org/ishelp/topic_scriptevents.htm)、[7.1 卸载确认顺序](https://github.com/jrsoftware/issrc/blob/is-7_1_0/Projects/Src/Setup.Uninstall.pas#L670)

默认保留 `Settings` 中的语言、菜单和 PATH 偏好及有界维护日志，提供“同时删除设置和诊断日志”选项。已经关闭菜单时，卸载临时禁用菜单不产生变更记录，收尾也不删除原有禁用偏好。偏好、安装注册表和内部文件收尾失败均走同一恢复路径，恢复维护入口并保留 `RemovalPending`。请求和操作记录不保留为可再次执行的队列。不删除证书信任、其他安装的注册项、用户改动的 PATH 或未知文件。

清除偏好的选择在删除前保存到卸载事务，失败后重试默认继续该选择。清除已经开始后不能通过下一次取消勾选恢复已删除的数据，收尾按持久化选择继续，避免部分偏好恢复而其他偏好丢失。

若卸载在删除维护工具后中断，仍有卸载事务但根内工具缺失时，重新运行通过核验的 Setup 只恢复核验过的维护工具、保留 `RemovalPending` 并提示重跑卸载，不发布新 payload 或继续普通安装。未知文件保留并报告，不能仅因目录非空而永久进入待清理状态。

## 5. 桌面任务与 i18n

桌面任务使用 Windows 原生对话框，完成预检查 → 永久删除确认 → 执行 → 结果。删除确认默认取消；执行显示活动指示、已完成数量、耗时和取消，不显示未经完整扫描的百分比。关闭执行窗口请求取消，并等待已经开始的调用返回。

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
| `modules/integration` | 注册与状态、任务协议、PATH/包生命周期；维护工具 bin    |
| `modules/launcher`    | 稳定 CLI 启动器                                        |
| `modules/desktop`     | 原生桌面任务进程及简短语言/菜单设置入口                |
| `modules/shell`       | cdylib、两个 COM 类及公共选中项适配                    |
| `assets`              | 可编辑 SVG 源图及生成的 Windows PNG / 多尺寸 ICO 资源  |
| `packaging`           | Inno 脚本、身份 manifest 模板、固定身份及签名脚本      |
| `modules/xtask`       | 图标生成、打包、版本生成、payload 核验和 benchmark     |

Shell 不链接删除引擎；core 不依赖 i18n、注册表设置或 Shell。COM 与包部署采用 `windows` 0.62.2 及匹配的 `windows-core`，已有 core 的 `windows-sys` 调用保持现状。Fluent 使用 `fluent-bundle` 0.16.0 / `unic-langid` 0.9.6；版本与 features 在 [Cargo.toml](../Cargo.toml) 声明并通过 Cargo.lock 固定。

所有构建先生成版本、构建程序并嵌入资源。四个 EXE 共用 Windows manifest 与构建辅助模块，均声明 Common Controls v6；只有签名桌面构建额外嵌入 MSIX 身份。打包产物静态链接 MSVC CRT，不要求另装 VC++ 运行库；RC 显式使用 UTF-8 读取资源脚本和路径。默认未签名构建直接生成 payload 和 Setup，不生成身份包或桌面包身份。可选签名构建先签名 EXE/DLL、构建并签名身份包，再构建并签名 Setup；签名后不能改写二进制。签名构建必须核对包 Publisher、程序身份和证书 Subject 一致。

### 6.1 构建统一安装包

构建需要 Windows MSVC、Windows SDK x64 的 `rc` 及 Inno Setup 7.1；签名构建另外需要 `makeappx` 和 `signtool`。`--iscc` 或环境变量 `UNLINKA_ISCC` 指定编译器；`--sdk-bin` 可显式指定 SDK x64 bin，否则选取已安装最高版本。`.github/workflows/ci.yml` 构建相同的默认未签名安装包。

图标统一维护在根目录 `assets/`。编辑 `assets/Unlinka.svg` 后运行 `cargo xtask assets`，更新 `assets/windows` 中的多尺寸 ICO、manifest 基础 PNG、100–400% 缩放及应用列表目标尺寸资源。`cargo xtask assets --check` 检查生成资源与 SVG 是否一致；CI 执行此检查。图标采用黑白几何断链，同一设计用于明暗主题，外缘保持透明。[图标资源说明](architecture.md#icon-assets) 列出尺寸和生成依赖。

打包时从 SVG 重新生成独立的 `target/packages/<version>/assets`，再编译 EXE/DLL 与 Setup 图标，并复制到 payload 的 `Assets`；不依赖可能过期的仓库 PNG/ICO。安装后的 `Assets` 路径与 manifest、Explorer 图标引用一致。

~~~powershell
# 默认可安装的未签名包，提供经典菜单
cargo xtask pack --build-number 1 --iscc "C:\Tools\Inno\ISCC.exe"

# 可选签名：三个签名参数必须同时提供，Subject 与 --publisher 完全一致
cargo xtask pack --publisher "CN=Your Publisher" --certificate-thumbprint <40位公开指纹> --timestamp-url <RFC3161服务URL> --build-number 2 --iscc "C:\Tools\Inno\ISCC.exe"
~~~

签名证书从当前用户 `My` 存储选取，私钥或密码不作为命令参数。EXE/DLL 与 MSIX 分别签名并用 `signtool verify /pa /all` 核验；Inno 使用仓库的 `sign-artifact.ps1` 签名并核验 Setup、内嵌卸载器及临时副本。构建不读取编译器 IDE 的额外签名配置，签名后不改写二进制。省略全部签名参数即生成未签名包；`--publisher`、`--certificate-thumbprint` 和 `--timestamp-url` 必须全部提供或全部省略，指纹为 40 位十六进制。不再需要开发版或未签名开关。[Inno 签名卸载器说明](https://jrsoftware.org/ishelp/topic_setup_signeduninstaller.htm)

包版本为 Cargo 的 `major.minor.patch` 加 `--build-number` 的第四段，每段受 `u16` 限制；现有输出不可覆盖。两种签名状态的包路径均为 `target/packages/<version>`，Setup 均名为 `Unlinka-Setup-<version>-x64.exe`。切换签名状态也必须使用新的构建号，不能覆盖旧产物。输出包含 Setup、工具 EXE、完整 payload、生成资源和 Inno 脚本；签名构建另包含 MSIX manifest 和身份包。

## 7. 待完成的发行与验收

当前实现的检查与构建结果见 [验收现状](acceptance.md)；以下仍需执行，不将静态检查或注册成功当作 Explorer 实际行为验收。

| 待完成项           | 完成条件                                                                                                                   |
| ------------------ | -------------------------------------------------------------------------------------------------------------------------- |
| 发行身份与可信签名 | 可选配置稳定 Publisher、代码签名证书和时间戳服务，签名构建与可信干净环境安装通过                                           |
| 两种菜单实际行为   | 未签名包的 Win10/Win11 经典入口，以及签名包的 Win11 新/经典各一个入口；混合多选、中文/长路径、快捷方式和 junction 完整传递 |
| 桌面任务           | 默认取消、目标替换、执行取消、部分失败与两种语言通过人工 QA                                                                |
| 安装生命周期       | 普通用户安装、同版本修复、跨版本及签名状态切换升级、包失败补偿、进程中断恢复、PATH 外部修改、占用 DLL 卸载重试通过人工 QA  |

开发检查沿用 `cargo check` → 格式 → lint → 本次相关纯逻辑单元测试。安装、包部署、Explorer、实际删除和取消场景作为明确的 Windows 人工 QA 范围；实现期间未经用户要求不自动添加或运行额外集成、UI 或性能测试。

默认未签名安装不依赖证书；新菜单需完成可选签名配置。需要 Win10 x64 与 Win11 x64 的实际验收环境；本轮未自动执行安装、包注册、Explorer/UI 操作、删除或性能测试。首版 x64 Setup 在 ARM64 上不能借助 EXE 模拟运行来宣称原生 Explorer 扩展受支持。原删除引擎的历史验收记录见 [acceptance](acceptance.md)，不代表新增生命周期已经验收。
