# 用途：校验官方 FidelityFX SDK 1.1.4，在当前工作树 .local 中应用可审计补丁并自动构建 Vulkan DLL，不复制其他工作树运行包。
# 用法：在 SDK 根运行 pwsh -File crates/feathrise_fsr3_frame_generation/scripts/构建FSR修订运行库.ps1 -SdkRoot <FidelityFX-SDK-v1.1.4绝对目录> -PatchRoot <feathrise_patches/fidelityfx-sdk-patches绝对目录>；可用 -CmakePath 显式指定 CMake。
# 依赖：PowerShell 7、Git、CMake >=3.23、Visual Studio 2022 C++ x64 工具链、Vulkan SDK >=1.3.250；原 SDK 自带 glslangValidator/FFX_SC。
# 产物：.local/fsr3-sdk-patched/runtime/{amd_fidelityfx_vk.dll,manifest.json,LICENSE.txt}；独立于Cargo缓存，修订产物没有AMD签名，必须按源码/补丁/产物哈希校验。
[CmdletBinding()] # 允许显式参数和一致的错误处理。
param( # 所有构建操作均在离线复制的源码树中执行。
    [string]$SdkRoot = $env:FIDELITYFX_SDK, # 只读官方原件由显式参数或标准构建环境提供。
    [string]$PatchRoot = $env:FEATHRISE_FSR3_PATCH_ROOT, # 独立补丁仓材料由调用者显式选择，不读取私有父仓。
    [string]$CmakePath = $env:BL5_FSR3_CMAKE, # 显式参数或环境变量优先于自动发现。
    [string]$VulkanSdk = $env:VULKAN_SDK, # 使用机器已安装的 Vulkan SDK。
    [ValidateRange(1,8)][int]$Jobs = 2 # 限制同时编译的工程数量，避免无界占用。
) # 无额外下载、签名或发行副作用。
$ErrorActionPreference = 'Stop' # 输入不符时立即停止，不产出看似成功的身份记录。
if ([string]::IsNullOrWhiteSpace($SdkRoot)) { throw '请传入 -SdkRoot 或设置 FIDELITYFX_SDK；普通预编译消费无需运行此脚本。' }
if ([string]::IsNullOrWhiteSpace($PatchRoot)) { throw '请传入 -PatchRoot 或设置 FEATHRISE_FSR3_PATCH_ROOT，指向 feathrise_patches/fidelityfx-sdk-patches。' }
$SdkRoot = [IO.Path]::GetFullPath($SdkRoot) # 固定外部 SDK 位置，不搜索维护者机器上的历史目录。
$buildSearchPath = $env:Path # 某些宿主注入 Path/PATH 两个大小写条目，MSBuild 的大小写无关字典会拒绝它们。
[Environment]::SetEnvironmentVariable('PATH', $null, 'Process') # 先清理大写变体，仅影响本构建进程。
[Environment]::SetEnvironmentVariable('Path', $null, 'Process') # 再清理标准变体，避免重复键。
[Environment]::SetEnvironmentVariable('Path', $buildSearchPath, 'Process') # 恢复唯一条目，保留原有工具搜索路径。
function Invoke-SdkCmake([string[]]$Arguments) { # 为 MSBuild 创建去重后的环境块，避免宿主 PATH/Path 重复条目。
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new() # 使用结构化参数，不拼接 shell 命令。
    $startInfo.FileName = $CmakePath # 自动发现已把 CMake 固定为明确可执行文件路径。
    $startInfo.UseShellExecute = $false # 直接启动编译器前端，无额外 shell 解析。
    $startInfo.Environment.Clear() # 不继承含重复大小写键的原始环境块。
    foreach ($entry in [Environment]::GetEnvironmentVariables().GetEnumerator()) { $startInfo.Environment[$entry.Key.ToUpperInvariant()] = $entry.Value } # 统一键大小写，保留每个实际值。
    foreach ($argument in $Arguments) { $startInfo.ArgumentList.Add($argument) } # 包含空格的路径按单参数传递。
    $process = [System.Diagnostics.Process]::Start($startInfo) # 标准输出仍由调用终端/日志捕获。
    $process.WaitForExit() # 此离线构建脚本可等待编译完成，不进入游戏帧路径。
    if ($process.ExitCode -ne 0) { throw "CMake 失败，退出码 $($process.ExitCode)。" } # 任一构建错误阻止发布。
    $process.Dispose() # 回收已退出的构建进程句柄。
} # 仅规范进程环境，不修改用户或系统环境变量。
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../..')) # 脚本归所属 crate/scripts；写入仍限定 SDK 自有目录。
$patchRoot = [IO.Path]::GetFullPath($PatchRoot) # 外部补丁及来源材料只读；原文字节参与运行库身份链。
foreach ($name in @('LICENSE.txt','README.md','v1.1.4-vulkan-lifetime.patch','source-manifest.json')) {
    if (-not (Test-Path -LiteralPath (Join-Path $patchRoot $name) -PathType Leaf)) { throw "补丁目录缺少来源材料：$name（$patchRoot）" }
}
$buildRoot = Join-Path $repoRoot '.local/fsr3-sdk-patched' # 源码、构建缓存与运行库持久保留，不受cargo clean影响且由Git忽略。
$sourceRoot = Join-Path $buildRoot 'source' # 外部 SDK 的独立副本。
$runtimeRoot = Join-Path $buildRoot 'runtime' # 宿主构建与打包约定的固定产物位置。
$patchPath = Join-Path $patchRoot 'v1.1.4-vulkan-lifetime.patch' # 固定且可审计的源码差异。
$sourceManifestPath = Join-Path $patchRoot 'source-manifest.json' # 锁定全部 SDK/API 输入，不仅三个被修改的文件。
function Resolve-CmakeExecutable([string]$RequestedPath) { # 只接受显式参数/环境或标准 PATH，不读取其他工作树的维护缓存。
    if (-not [string]::IsNullOrWhiteSpace($RequestedPath)) {
        return (Get-Command -Name $RequestedPath -CommandType Application -ErrorAction Stop).Source
    }
    $pathCommand = Get-Command -Name 'cmake' -CommandType Application -ErrorAction SilentlyContinue
    if ($pathCommand) { return $pathCommand.Source }
    throw '未找到 CMake >=3.23；请设置 BL5_FSR3_CMAKE、传入 -CmakePath，或把已安装的 CMake 加入 PATH。'
}

function Assert-OwnedRuntimeDirectory([string]$Directory) { # 原子发布只允许替换脚本自己定义的三个运行文件。
    if (-not (Test-Path -LiteralPath $Directory)) { return } # 全新工作树没有旧目录时无需审计。
    if (-not (Test-Path -LiteralPath $Directory -PathType Container)) { throw "FSR 运行路径不是目录，拒绝替换：$Directory" } # 不覆盖普通文件或其他路径类型。
    $allowedNames = @('amd_fidelityfx_vk.dll','manifest.json','LICENSE.txt') # 运行目录所有权严格限制为脚本产物。
    foreach ($entry in Get-ChildItem -Force -LiteralPath $Directory) { # 发布前逐项拒绝未知文件和子目录。
        if ($entry.PSIsContainer -or $entry.Name -notin $allowedNames) { throw "FSR 运行目录含有未知内容，拒绝替换：$($entry.FullName)" } # 保留人工现场，不扩大清理范围。
    } # 旧运行目录审计结束。
} # Assert-OwnedRuntimeDirectory 结束。

function Remove-OwnedRuntimeDirectory([string]$Directory) { # 只删除已通过固定白名单审计的脚本私有目录。
    if (-not (Test-Path -LiteralPath $Directory)) { return } # 已不存在时视为完成。
    Assert-OwnedRuntimeDirectory $Directory # 删除前再次确认没有并发加入未知内容。
    foreach ($name in @('amd_fidelityfx_vk.dll','manifest.json','LICENSE.txt')) { # 逐个删除固定普通文件，避免宽泛递归操作。
        $path = Join-Path $Directory $name # 目标始终位于已审计的精确目录。
        if (Test-Path -LiteralPath $path -PathType Leaf) { Remove-Item -LiteralPath $path -Force } # 只删除脚本拥有的具体文件。
    } # 固定运行文件清理结束。
    Remove-Item -LiteralPath $Directory -Force # 目录此时必须为空，不使用递归删除。
} # Remove-OwnedRuntimeDirectory 结束。

function Publish-RuntimeDirectory([string]$StagingDirectory,[string]$DestinationDirectory) { # 目录改名保证 DLL、manifest 与许可一次对外出现。
    Assert-OwnedRuntimeDirectory $StagingDirectory # 暂存目录只能包含本脚本固定三项。
    foreach ($requiredName in @('amd_fidelityfx_vk.dll','manifest.json','LICENSE.txt')) { # 发布前要求完整文件集合。
        if (-not (Test-Path -LiteralPath (Join-Path $StagingDirectory $requiredName) -PathType Leaf)) { throw "FSR 运行暂存目录缺少 $requiredName：$StagingDirectory" } # 不发布半包。
    } # 暂存完整性检查结束。
    $backupDirectory = "$DestinationDirectory.backup-$PID-$([guid]::NewGuid().ToString('N'))" # 重建已有包时保留同父目录可回滚副本。
    $hasBackup = $false # 仅在旧目录成功让位后允许执行回滚或清理。
    if (Test-Path -LiteralPath $DestinationDirectory) { # 手动重建允许替换已有或已知半包。
        Assert-OwnedRuntimeDirectory $DestinationDirectory # 未知内容永不被自动删除。
        Move-Item -LiteralPath $DestinationDirectory -Destination $backupDirectory # 同卷原子改名保留旧包。
        $hasBackup = $true # 标记后续失败需要恢复旧目录。
    } # 旧运行目录让位结束。
    try { # 提交失败时尽力恢复完整旧目录。
        Move-Item -LiteralPath $StagingDirectory -Destination $DestinationDirectory # 同卷原子发布完整新包。
    } catch { # Windows 占用或并发错误不能留下无运行目录状态。
        if ($hasBackup -and -not (Test-Path -LiteralPath $DestinationDirectory)) { Move-Item -LiteralPath $backupDirectory -Destination $DestinationDirectory } # 仅在目标仍缺失时恢复本次备份。
        throw # 保留原始发布失败诊断。
    } # 新包提交结束。
    if ($hasBackup) { Remove-OwnedRuntimeDirectory $backupDirectory } # 成功发布后只清理已审计旧包。
} # Publish-RuntimeDirectory 结束。

$CmakePath = Resolve-CmakeExecutable $CmakePath # 后续版本记录和两个 CMake 进程使用同一明确工具身份。
$officialDllHash = 'A1624CC4238FEF046F30C4D80CE3F47BE63FC5F5373F49E3EE9EDB9960F54C78' # AMD 官方已签名 Vulkan DLL 的来源锚点。
$officialDll = Join-Path $SdkRoot 'PrebuiltSignedDLL/amd_fidelityfx_vk.dll' # 官方 DLL 仅用于来源核验，不用于修改。
if ((Get-FileHash -Algorithm SHA256 -LiteralPath $officialDll).Hash -ne $officialDllHash) { throw '官方 DLL SHA256 与 SDK 1.1.4 锁定原件不符。' } # 拒绝混用其他版本。
$officialSignature = Get-AuthenticodeSignature -LiteralPath $officialDll # 保留 AMD 发布者身份链。
if ($officialSignature.Status -ne 'Valid' -or $officialSignature.SignerCertificate.Subject -notmatch 'Advanced Micro Devices') { throw '官方 DLL 必须具有有效 AMD 签名。' } # 修订不能削弱原件来源检查。
$sourceManifest = Get-Content -Encoding UTF8 -Raw -LiteralPath $sourceManifestPath | ConvertFrom-Json # 显式 UTF-8，避免中文路径被系统编码破坏。
$actualInputCount = @(Get-ChildItem -LiteralPath (Join-Path $SdkRoot 'sdk'),(Join-Path $SdkRoot 'ffx-api') -Recurse -File).Count # 拒绝清单以外的额外编译源混入副本。
if ($actualInputCount -ne @($sourceManifest.files.PSObject.Properties).Count) { throw 'SDK 输入文件数量与固定清单不符。' } # 缺失或额外文件均需重新审核来源。
if ((Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $patchRoot 'LICENSE.txt')).Hash -ne $sourceManifest.files.'sdk/LICENSE.txt') { throw '仓库保留许可与官方许可不符。' } # 衍生发行必须保留完整官方许可。
foreach ($entry in $sourceManifest.files.PSObject.Properties) { # 验证每个输入文件，包括着色器编译器二进制。
    $inputPath = Join-Path $SdkRoot $entry.Name # 固定清单只包含 sdk/ffx-api 下的相对路径。
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $inputPath).Hash -ne $entry.Value) { throw "SDK 源码身份不符：$($entry.Name)" } # 不在未知源码上猜测应用补丁。
} # 完整身份验证结束。
New-Item -ItemType Directory -Force -Path $sourceRoot | Out-Null # 不删除已有源码与 CMake 构建缓存，运行目录只在完整发布时出现。
foreach ($name in @('sdk','ffx-api')) { Copy-Item -LiteralPath (Join-Path $SdkRoot $name) -Destination $sourceRoot -Recurse -Force } # 每次还原原始输入，保证补丁只应用一次。
& git apply --check --unsafe-paths --directory=$sourceRoot $patchPath # 在已还原的独立树验证补丁上下文。
if ($LASTEXITCODE -ne 0) { throw 'SDK 补丁上下文检查失败。' } # 不继续使用半修订源码。
& git apply --unsafe-paths --directory=$sourceRoot $patchPath # 仅写 .local 中的源码副本，外部SDK保持只读。
if ($LASTEXITCODE -ne 0) { throw 'SDK 补丁应用失败。' } # 外部 SDK 始终保持原样。
$shaderRoot = Join-Path $sourceRoot 'sdk/src/backends/vk/FrameInterpolationSwapchain' # UI shader 不属于 SDK 的自动 permutation 构建，需要显式再生成。
$glslang = Join-Path $sourceRoot 'sdk/tools/binary_store/glslangValidator.exe' # 该二进制已经过 source-manifest 哈希核验。
foreach ($variant in @(@{Suffix='';Premul=0;Symbol='g_mainPS'},@{Suffix='Premul';Premul=1;Symbol='g_mainPremulPS'})) { # 保留普通及预乘两条官方透明度路径。
    $headerPath = Join-Path $shaderRoot "FrameInterpolationSwapchainUiComposition$($variant.Suffix)PS.h" # 输出在源码副本，避免提交生成二进制数组。
    & $glslang -o $headerPath "-DFFX_UI_PREMUL=$($variant.Premul)" -Os --vn $variant.Symbol -S frag -e main --target-env vulkan1.2 (Join-Path $shaderRoot 'FrameInterpolationSwapchainUiCompositionPS.glsl') # 从已修订 GLSL 生成 SPIR-V 头文件。
    if ($LASTEXITCODE -ne 0) { throw 'UI 合成 shader 编译失败。' } # 绝不使用 SDK 原有旧 shader 头假装修复完成。
} # 两个 fragment variant 均已重建。
if ($VulkanSdk) { $env:VULKAN_SDK = $VulkanSdk } # 仅影响本构建进程及其子进程。
$cmakeVersion = (& $CmakePath --version | Select-Object -First 1) # 记录构建器版本，便于审计复现环境。
if ($LASTEXITCODE -ne 0) { throw '无法执行 CMake。' } # 可通过 -CmakePath 指定工具位置。
Invoke-SdkCmake @('-S',(Join-Path $sourceRoot 'ffx-api'),'-B',(Join-Path $buildRoot 'build'),'-G','Visual Studio 17 2022','-A','x64','-DFFX_API_BACKEND=VK_X64','-DCMAKE_CXX_FLAGS=/utf-8') # 使用官方 API DLL 目标，并明确源码编码以保留中文关键注释。
Invoke-SdkCmake @('--build',(Join-Path $buildRoot 'build'),'--config','Release','--target','amd_fidelityfx_vk','--parallel',"$Jobs") # 只构建所需 Vulkan DLL；与项目 cargo release 无关。
$builtDll = Join-Path $sourceRoot 'ffx-api/bin/amd_fidelityfx_vk.dll' # 官方 CMake 固定的输出位置。
$runtimeStagingRoot = Join-Path $buildRoot ".runtime-staging-$PID-$([guid]::NewGuid().ToString('N'))" # 每次构建在同卷独占目录准备完整运行包。
New-Item -ItemType Directory -Path $runtimeStagingRoot | Out-Null # 随机目录名通过原子创建取得当前进程所有权。
try { # 任何生成失败都不触碰现有可用运行目录。
    $stagedRuntimeDll = Join-Path $runtimeStagingRoot 'amd_fidelityfx_vk.dll' # manifest 必须从将要发布的准确 DLL 字节计算。
    Copy-Item -LiteralPath $builtDll -Destination $stagedRuntimeDll # 编译成功后先写私有暂存目录。
    Copy-Item -LiteralPath (Join-Path $patchRoot 'LICENSE.txt') -Destination (Join-Path $runtimeStagingRoot 'LICENSE.txt') # 衍生发行保留 AMD MIT 许可。
    $manifest = [ordered]@{ # 所有哈希都从实际输入/产物重新计算，不声称保留 AMD 签名。
        schema_version = 1 # 固定结构版本。
        sdk_version = '1.1.4' # 官方基础版本。
        revision = 'bl5-vulkan-lifetime-ui-1' # 区分源码修订产物。
        official_dll_sha256 = $officialDllHash # 官方来源锚点。
        license_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $patchRoot 'LICENSE.txt')).Hash # 许可身份。
        patch_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $patchPath).Hash # 固定修订内容。
        source_manifest_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $sourceManifestPath).Hash # 全部原始源码/工具输入身份。
        dll_sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $stagedRuntimeDll).Hash # 宿主和打包需将此身份锁入构建产物。
        dll_size = (Get-Item -LiteralPath $stagedRuntimeDll).Length # 额外尺寸核对。
        authenticode = (Get-AuthenticodeSignature -LiteralPath $stagedRuntimeDll).Status.ToString() # 正常为 NotSigned；从不伪造官方签名。
        cmake_version = $cmakeVersion # 实际配置工具版本。
        generator = 'Visual Studio 17 2022 x64' # 固定编译架构。
        build_configuration = 'Release' # SDK 的优化 DLL 配置。
    } # manifest 由宿主构建同时锁定 patch/source 身份后才可信。
    $manifest | ConvertTo-Json | Set-Content -Encoding UTF8 -LiteralPath (Join-Path $runtimeStagingRoot 'manifest.json') # UTF-8 输出在暂存目录与 DLL 成对完成。
    Publish-RuntimeDirectory $runtimeStagingRoot $runtimeRoot # 一次目录提交替换旧包，Cargo 不会观察到半写 manifest。
} finally { # 发布函数成功后暂存目录已被改名，失败时只清理本次固定白名单文件。
    if (Test-Path -LiteralPath $runtimeStagingRoot -PathType Container) { Remove-OwnedRuntimeDirectory $runtimeStagingRoot } # 不递归删除未知或非本次文件。
} # 运行包暂存生命周期结束。
$runtimeDll = Join-Path $runtimeRoot 'amd_fidelityfx_vk.dll' # 日志引用最终稳定宿主消费路径。
Write-Output "修订 FSR Vulkan DLL 已构建：$runtimeDll" # 明确给出可验证产物。
Write-Output "SHA256：$($manifest.dll_sha256)；签名：$($manifest.authenticode)" # 明确区分源码修订与 AMD 官方签名原件。
