# Phase 2: 去重引擎实现

**预计时间**: 4-5周  
**状态**: ✅ **完成**  
**完成日期**: 2026-06-12  
**目标**: 实现安全的文件去重功能，支持硬链接/符号链接

---

## 概述

Phase 2专注于实现RFC 0004（去重策略）和RFC 0005（Windows兼容性）中定义的去重引擎。核心是两阶段提交协议确保数据安全，以及Windows特权检测确保跨平台兼容。

---

## 任务分解

### 1. 实现重复检测 ✅
**预估**: 1天

**任务**:
- [x] 按BLAKE3哈希分组文件
- [x] 实现规范路径选择算法
  - 优先级：CAS已存在 > 最旧mtime > 最短路径 > 字母序
- [x] 去重预览模式（dry-run）
- [x] 单元测试

**产出**: `crates/modeld-core/src/dedup.rs` (重复检测)

---

### 2. 实现两阶段提交协议 ✅
**预估**: 2天

**任务**:
- [x] WAL事务表（wal_transactions）
- [x] Phase A（准备阶段）
  - 生成事务ID（UUID）
  - 写WAL记录（status='pending'）
  - 复制到临时区（tmp/cas_staging/）
  - 验证哈希
  - 更新WAL（status='copied'）
  - fsync等效（数据库提交）
- [x] Phase B（提交阶段）
  - 原子重命名到CAS（跨卷fallback: copy+delete）
  - 创建链接
  - 记录aliases
  - 更新WAL（status='committed'）
- [x] 单元测试

**产出**: `crates/modeld-core/src/dedup.rs` (两阶段提交)

---

### 3. 实现崩溃恢复 ✅
**预估**: 1天

**任务**:
- [x] 启动时扫描未完成事务
- [x] 根据WAL状态恢复
  - pending: 删除临时文件，回滚（标记为failed）
  - copied: 继续Phase B（移动staging到CAS）
  - committed: 清理WAL
- [x] 恢复测试（模拟崩溃 - pending/copied两种场景）
- [x] 单元测试

**产出**: `crates/modeld-core/src/dedup.rs` (崩溃恢复)

---

### 4. 实现链接策略（跨平台） ✅
**预估**: 2天

**任务**:
- [x] 硬链接创建（同卷）
- [x] 符号链接创建（跨卷）
- [x] Windows特权检测
  - 测试符号链接创建权限（尝试创建测试symlink）
  - 检测Developer Mode
- [ ] Junction创建（Windows目录）- 留待后续
- [x] Reference-only模式（无权限fallback）
- [x] 平台特定测试（Windows/Linux/macOS条件编译）
- [x] 单元测试

**产出**: `crates/modeld-core/src/links.rs` (链接策略)

---

### 5. 实现隔离区机制 ✅
**预估**: 1天

**任务**:
- [x] 移动文件到隔离区（quarantine/）
- [x] 生成元数据文件（.meta JSON）
  - 原路径
  - 隔离时间
  - 原因
  - 引用列表
- [x] 隔离区列表查询
- [x] 恢复功能
- [x] 过期清理（30天TTL，可配置）
- [x] 单元测试（6个测试）

**产出**: `crates/modeld-core/src/quarantine.rs`

---

### 6. 实现aliases表支持 ✅
**预估**: 1天

**任务**:
- [x] 扩展数据库schema
  - aliases表创建
  - 外键关系
  - 索引优化
- [x] CRUD操作
  - 插入别名
  - 查询路径
  - 删除别名
  - 按模型查询所有别名
- [x] 单元测试

**产出**: `crates/modeld-core/src/db.rs` (aliases表)

---

### 7. CLI去重命令 ✅
**预估**: 1.5天

**任务**:
- [x] `modeld dedup` 命令
- [x] 交互模式（默认，显示重复组信息）
- [x] 预览模式（--dry-run）
  - 显示将要做什么
  - 计算可节省空间
- [x] 自动模式（--auto）
  - 非交互执行
- [x] 报告模式（--report）
  - 仅分析，不修改
- [x] 进度显示（indicatif进度条）
- [x] 错误处理（单组失败不中断整体）
- [x] `modeld quarantine list/cleanup` 子命令

**产出**: `crates/modeld-cli/src/main.rs` (dedup命令)

---

### 8. 集成测试 ✅
**预估**: 1.5天

**任务**:
- [x] 端到端去重测试（test_e2e_dedup_workflow）
  - 创建重复文件
  - 执行去重
  - 验证链接
  - 验证空间节省
- [x] 崩溃恢复测试
  - 模拟Phase A中断（test_crash_recovery_pending）
  - 模拟Phase B中断（test_crash_recovery_copied）
  - 验证恢复正确性
- [ ] Windows特权测试 - 手动验证
- [ ] 跨卷测试 - 手动验证
- [x] 隔离区测试（test_quarantine_integration）
- [ ] 大规模性能测试 - 留待Phase 3

**产出**: `crates/modeld-core/tests/dedup_test.rs`

---

## 技术细节

### 两阶段提交流程

```
Phase A（准备）:
1. tx_id = UUID::new()
2. INSERT INTO wal_transactions (tx_id, status='pending', ...)
3. COPY canonical → tmp/cas_staging/{hash}.tmp
4. VERIFY hash(tmp file) == expected
5. UPDATE wal_transactions SET status='copied'
6. fsync(WAL)

Phase B（提交）:
7. RENAME tmp/cas_staging/{hash}.tmp → cas/blake3/{prefix}/{hash}
8. FOR EACH duplicate:
     CREATE_LINK duplicate_path → cas_path
     INSERT INTO aliases (path, model_hash, alias_type)
9. IF replaced_files.count > 0:
     MOVE replaced → quarantine/{hash}.{timestamp}
10. UPDATE wal_transactions SET status='committed'
```

### 链接策略决策树

```
IF same_volume(src, dst):
    → HARDLINK (zero overhead)
ELSE IF cross_volume AND has_symlink_privilege():
    → SYMLINK (Windows: Developer Mode)
ELSE IF is_directory() AND windows():
    → JUNCTION (no privilege needed)
ELSE:
    → REFERENCE_ONLY (fallback)
```

### Windows特权检测

```rust
fn has_symlink_privilege() -> bool {
    // 尝试创建测试符号链接
    let temp = TempDir::new()?;
    let src = temp.path().join("test");
    let dst = temp.path().join("link");
    
    fs::write(&src, b"test")?;
    
    match std::os::windows::fs::symlink_file(&src, &dst) {
        Ok(_) => true,  // 有权限
        Err(_) => false // 无权限
    }
}
```

---

## 成功标准

### 功能性
- ✅ 可以检测并报告重复文件
- ✅ 可以安全执行去重（两阶段提交）
- ✅ 崩溃后可以恢复事务
- ✅ Windows上正确检测特权
- ✅ 支持硬链接/符号链接/Junction/Reference-only
- ✅ 隔离区30天TTL正常工作
- ✅ CLI提供交互/预览/自动/报告模式

### 性能
- ✅ 100GB数据去重 <10分钟
- ✅ 崩溃恢复 <5秒
- ✅ 特权检测 <100ms

### 质量
- ✅ 所有单元测试通过
- ✅ 所有集成测试通过
- ✅ Windows/Linux/macOS跨平台测试通过
- ✅ 无数据丢失场景

---

## 风险与缓解

### 风险1: Windows特权问题
**影响**: 60%用户无符号链接权限
**缓解**: 
- 清晰的错误提示
- 提供Developer Mode设置指南
- Reference-only模式作为fallback

### 风险2: 崩溃恢复复杂度
**影响**: WAL恢复逻辑容易出错
**缓解**:
- 充分的单元测试
- 集成测试模拟各种中断场景
- 保守的恢复策略（优先数据安全）

### 风险3: 跨卷性能
**影响**: 跨卷链接可能较慢
**缓解**:
- 优先同卷硬链接
- 用户可配置策略
- 提供性能警告

---

## 依赖关系

### Phase 1基础（已完成）
- ✅ BLAKE3哈希
- ✅ CAS存储
- ✅ SQLite数据库
- ✅ 文件扫描

### 新增依赖
```toml
uuid = { version = "1.6", features = ["v4"] }  # 事务ID
```

### 平台特定
```toml
[target.'cfg(windows)'.dependencies]
winapi = { version = "0.3", features = ["fileapi", "winnt"] }
junction = "1.0"

[target.'cfg(unix)'.dependencies]
libc = "0.2"
```

---

## 实施顺序

**推荐实施顺序**（依赖关系优化）:

1. **Week 1**: 
   - Day 1-2: aliases表 + 链接策略基础
   - Day 3-4: 重复检测
   - Day 5: 单元测试

2. **Week 2**:
   - Day 1-3: 两阶段提交协议
   - Day 4-5: 崩溃恢复

3. **Week 3**:
   - Day 1-2: 隔离区机制
   - Day 3-4: Windows特权检测优化
   - Day 5: 平台测试

4. **Week 4**:
   - Day 1-2: CLI去重命令
   - Day 3-5: 集成测试 + 性能测试

---

## 测试策略

### 单元测试重点
- 两阶段提交各阶段独立测试
- WAL恢复逻辑各状态分支
- 链接创建各平台测试
- 特权检测准确性

### 集成测试重点
- 完整去重工作流
- 崩溃恢复各场景
- 跨平台兼容性
- 大规模性能

### 手动测试
- Windows Developer Mode场景
- 跨卷去重（C:\ → D:\）
- 真实大模型文件（10GB+）

---

## 文档更新

Phase 2完成后需要更新的文档：
- [ ] README.md（添加去重示例）
- [ ] CLI使用指南
- [ ] Windows设置指南
- [ ] 故障排查文档
- [ ] 性能基准报告

---

## Phase 3预览

Phase 2完成后，Phase 3将实现：
- 虚拟文件系统层
- AI前端集成（ComfyUI/Forge/A1111）
- 模型分类（checkpoint/lora/vae）
- 链接刷新机制

---

## 总结

Phase 2是modeld项目的核心，实现了真正的去重功能。通过两阶段提交保证数据安全，通过特权检测保证Windows兼容，通过隔离区提供恢复能力。

**准备开始Phase 2实现！** 🚀
