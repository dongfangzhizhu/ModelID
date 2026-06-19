# modeld-hook: Build and Publish to PyPI
# 功能: 一键打包、测试和发布 Python 包到 PyPI
# 用法: .\build_and_publish.ps1 [--publish] [--dry-run] [--version VERSION]

param(
    [switch]$publish,
    [switch]$dry_run,
    [string]$version = ""
)

$ErrorActionPreference = "Stop"

# 颜色定义
$colors = @{
    Green = "`e[32m"
    Red = "`e[31m"
    Yellow = "`e[33m"
    Cyan = "`e[36m"
    Reset = "`e[0m"
}

function Write-Status($message, $type = "info") {
    $prefix = switch($type) {
        "success" { "$($colors.Green)[OK]$($colors.Reset)" }
        "error"   { "$($colors.Red)[FAIL]$($colors.Reset)" }
        "warn"    { "$($colors.Yellow)[WARN]$($colors.Reset)" }
        "info"    { "$($colors.Cyan)[INFO]$($colors.Reset)" }
    }
    Write-Host "$prefix $message"
}

function Confirm-Action($message) {
    Write-Host "$($colors.Yellow)$message (y/n): $($colors.Reset)" -NoNewline
    $response = Read-Host
    return $response -eq "y" -or $response -eq "Y"
}

# 1. 获取项目路径
$script_dir = Split-Path -Parent $MyInvocation.MyCommand.Path
$project_root = Split-Path -Parent $script_dir
$python_dir = Join-Path $project_root "python"

Write-Status "项目根目录: $project_root" "info"
Write-Status "Python 包目录: $python_dir" "info"
Write-Host ""

# 2. 检查必要工具
Write-Status "检查必要工具..." "info"

try {
    $python_version = python --version 2>&1
    Write-Status "Python: $python_version" "success"
} catch {
    Write-Status "Python 未找到，请先安装 Python 3.9+" "error"
    exit 1
}

try {
    pip show build | Out-Null
} catch {
    Write-Status "安装 build 工具..." "warn"
    pip install build -q
}

try {
    pip show twine | Out-Null
} catch {
    Write-Status "安装 twine 工具..." "warn"
    pip install twine -q
}

Write-Host ""

# 3. 更新版本（如果提供）
if ($version) {
    Write-Status "更新版本号: $version" "info"
    $pyproject_file = Join-Path $python_dir "pyproject.toml"
    $content = Get-Content $pyproject_file -Raw
    $content = $content -replace 'version = "[^"]*"', "version = `"$version`""
    Set-Content $pyproject_file $content -NoNewline
    Write-Status "版本号已更新" "success"
    Write-Host ""
}

# 4. 运行测试
Write-Status "运行 Python 测试..." "info"
Push-Location $python_dir

try {
    pip install -e ".[dev]" -q 2>&1 | Out-Null
    pytest tests/ -v
    
    if ($LASTEXITCODE -ne 0) {
        Write-Status "测试失败" "error"
        exit 1
    }
    Write-Status "测试通过" "success"
} catch {
    Write-Status "测试失败: $_" "error"
    exit 1
}

Write-Host ""

# 5. 清理旧的构建文件
Write-Status "清理旧的构建文件..." "info"
$dist_dir = Join-Path $python_dir "dist"
$build_dir = Join-Path $python_dir "build"
$egg_dir = Join-Path $python_dir "*.egg-info"

if (Test-Path $dist_dir) { Remove-Item -Recurse -Force $dist_dir -ErrorAction SilentlyContinue }
if (Test-Path $build_dir) { Remove-Item -Recurse -Force $build_dir -ErrorAction SilentlyContinue }
Get-Item $egg_dir -ErrorAction SilentlyContinue | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue

Write-Status "旧文件已清理" "success"
Write-Host ""

# 6. 构建 wheel 包
Write-Status "构建 wheel 包..." "info"

try {
    python -m build --wheel
    
    if ($LASTEXITCODE -ne 0) {
        Write-Status "构建失败" "error"
        exit 1
    }
    
    $wheels = Get-ChildItem $dist_dir -Filter "*.whl" | Measure-Object | Select-Object -ExpandProperty Count
    Write-Status "成功生成 $wheels 个 wheel 包" "success"
    
    # 显示生成的文件
    Write-Host ""
    Write-Host "$($colors.Cyan)生成的文件:$($colors.Reset)"
    Get-ChildItem $dist_dir | ForEach-Object {
        Write-Host "  - $($_.Name) ($([math]::Round($_.Length / 1MB, 2)) MB)"
    }
} catch {
    Write-Status "构建失败: $_" "error"
    exit 1
}

Write-Host ""

# 7. 验证包
Write-Status "验证包完整性..." "info"

try {
    $wheel_files = Get-ChildItem $dist_dir -Filter "*.whl"
    
    foreach ($wheel in $wheel_files) {
        $wheel_path = $wheel.FullName
        
        # 使用 twine 检查
        twine check $wheel_path
        
        if ($LASTEXITCODE -ne 0) {
            Write-Status "包验证失败" "error"
            exit 1
        }
        
        Write-Status "包 $($wheel.Name) 验证成功" "success"
    }
} catch {
    Write-Status "验证失败: $_" "error"
    exit 1
}

Write-Host ""

# 8. 发布到 PyPI
if ($publish) {
    Write-Host "$($colors.Yellow)===========================================$($colors.Reset)"
    Write-Host "$($colors.Yellow)准备发布到 PyPI$($colors.Reset)"
    Write-Host "$($colors.Yellow)===========================================$($colors.Reset)"
    Write-Host ""
    
    # 检查 .pypirc 配置
    $pypirc_path = Join-Path $env:USERPROFILE ".pypirc"
    if (-not (Test-Path $pypirc_path)) {
        Write-Status "未找到 .pypirc 配置文件" "error"
        Write-Host "请按照以下步骤配置 PyPI 凭证:"
        Write-Host ""
        Write-Host "1. 访问 https://pypi.org/account/register/"
        Write-Host "2. 创建账户或登录"
        Write-Host "3. 创建 API token: https://pypi.org/manage/account/tokens/"
        Write-Host "4. 在 $pypirc_path 中配置:"
        Write-Host ""
        Write-Host @"
[testpypi]
repository = https://test.pypi.org/legacy/
username = __token__
password = pypi-AgEIcHlwaS5vcmc...

[pypi]
repository = https://upload.pypi.org/legacy/
username = __token__
password = pypi-AgEIcHlwaS5vcmc...
"@
        Write-Host ""
        exit 1
    }
    
    Write-Status ".pypirc 配置已找到" "success"
    Write-Host ""
    
    # 显示待发布的包
    Write-Host "$($colors.Cyan)待发布的包:$($colors.Reset)"
    Get-ChildItem $dist_dir -Filter "*.whl" | ForEach-Object {
        Write-Host "  - $($_.Name)"
    }
    Write-Host ""
    
    if ($dry_run) {
        Write-Status "执行试运行（不实际上传）..." "warn"
        Write-Host ""
        twine upload --dry-run --repository pypi dist/*
        Write-Status "试运行成功！实际发布时移除 --dry-run 参数" "success"
    } else {
        if (-not (Confirm-Action "确认发布到 PyPI？")) {
            Write-Status "取消发布" "warn"
            exit 0
        }
        
        Write-Status "上传到 PyPI..." "info"
        Write-Host ""
        
        twine upload --repository pypi dist/*
        
        if ($LASTEXITCODE -eq 0) {
            Write-Status "成功发布到 PyPI！" "success"
            Write-Host ""
            Write-Host "$($colors.Green)安装命令:$($colors.Reset)"
            Write-Host "  pip install modeld-hook"
            Write-Host ""
        } else {
            Write-Status "发布失败" "error"
            exit 1
        }
    }
} else {
    Write-Host "$($colors.Cyan)===========================================$($colors.Reset)"
    Write-Host "$($colors.Cyan)构建完成！$($colors.Reset)"
    Write-Host "$($colors.Cyan)===========================================$($colors.Reset)"
    Write-Host ""
    Write-Host "下一步:"
    Write-Host "  1. 本地测试: pip install dist/modeld_hook-*.whl"
    Write-Host "  2. 发布: .\build_and_publish.ps1 -publish"
    Write-Host "  3. 试运行: .\build_and_publish.ps1 -publish -dry_run"
    Write-Host ""
}

Pop-Location
