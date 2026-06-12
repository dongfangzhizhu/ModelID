# modeld Phase 0: Architecture Design & RFC

## 📋 文档概览

此 spec 包含 modeld 项目 Phase 0（架构设计与 RFC）的完整设计文档。

### 文档结构

1. **spec.md** - 高层次概览
   - 项目背景和目标
   - 关键设计决策总结
   - 架构概览
   - 风险和下一步

2. **design.md** - 完整技术设计
   - 高层设计（系统架构、组件图、数据流）
   - 低层设计（CAS 布局、数据库 Schema、算法）
   - 全部 7 个 RFC 文档的详细内容
   - 技术栈详细说明

3. **requirements.md** - 详细需求
   - 功能需求（FR1-FR6）
   - 非功能需求（NFR1-NFR5）
   - 约束条件
   - 验收标准

4. **tasks.md** - 实施任务清单
   - 15 个具体任务
   - 子任务分解
   - 依赖关系
   - 工作量估算

## 🎯 Phase 0 目标

**核心原则**：在编写任何核心实现代码之前，完成系统设计。

### 主要交付物

- ✅ 7 个 RFC 文档（嵌入在 design.md 中）
  - RFC 0001: Storage Layout
  - RFC 0002: Hash Strategy
  - RFC 0003: Reference Model
  - RFC 0004: Deduplication Strategy
  - RFC 0005: Windows Compatibility (关键)
  - RFC 0006: Virtual FS
  - RFC 0007: HF Interception

- ✅ SQLite Schema v1 完整定义
- ✅ Windows 兼容性策略
- ✅ 系统架构图和数据流图
- ✅ 关键算法伪代码

## 📖 阅读顺序建议

### 快速了解（15 分钟）
1. 阅读 `spec.md` - 获得整体概览
2. 查看 `design.md` 中的架构图
3. 浏览 `tasks.md` - 了解实施计划

### 深入理解（2-3 小时）
1. 完整阅读 `design.md` 中的 7 个 RFC
2. 研究数据库 Schema 设计
3. 理解关键算法（哈希、去重、崩溃恢复）
4. 审查 Windows 兼容性策略

### 实施准备（1 天）
1. 详细研究 `requirements.md` 中的所有需求
2. 理解 `tasks.md` 中的任务依赖关系
3. 评估工作量和优先级
4. 准备开发环境

## 🔑 关键设计亮点

### 1. 内容寻址存储 (CAS)
- BLAKE3 哈希（比 SHA256 快 3-5 倍）
- 前缀分片（`/ab/abcdef...`）
- 不可变对象（只读）
- 可扩展到百万级模型

### 2. Windows 兼容性
- **关键挑战**：符号链接需要特权
- **解决方案**：多层降级策略
  - 同盘 → Hardlink
  - 跨盘 + 特权 → Symlink
  - 跨盘 + 无特权 → Reference-only 模式
- 清晰的用户沟通（开发者模式设置）

### 3. 事务安全
- 两阶段提交协议
- WAL（Write-Ahead Log）崩溃恢复
- 隔离区机制（30 天宽限期）
- 哈希验证在每个阶段

### 4. HuggingFace 生态集成
- 主策略：HF_HOME 环境变量
- 备用策略：Python monkeypatch
- 透明下载去重
- SHA256 ↔ BLAKE3 映射

## 🛠️ 技术栈

| 组件 | 技术选型 | 理由 |
|------|---------|------|
| 核心守护进程 | Rust + Tokio | 性能、内存安全、异步 IO |
| CLI 工具 | Rust + clap | 与守护进程共享代码 |
| 哈希算法 | BLAKE3 | 快 3-5 倍，并行化 |
| 元数据库 | SQLite + WAL | 零部署依赖，并发读 |
| HF 拦截 | Python | 必须与 Python AI 生态对接 |
| 配置文件 | TOML | Rust 生态标准 |

## 📊 性能目标

| 指标 | 目标 | 场景 |
|------|------|------|
| 哈希吞吐量 | ≥2GB/s | NVMe SSD |
| 首次扫描 1TB | ≤15 分钟 | 初始设置 |
| 增量扫描 | ≤30 秒 | 变化检测 |
| 重复检测 | ≤1 秒 | 已扫描数据 |
| 内存占用 | ≤200MB | 扫描期间 |

## ⚠️ 关键风险

### 高风险
1. **Windows 符号链接限制** → 多层降级策略
2. **跨卷 Hardlink 不支持** → Reference-only 模式

### 中风险
3. **HF monkeypatch 脆弱性** → HF_HOME 主策略
4. **性能目标不现实** → Phase 1 验证

## 📝 成功标准

Phase 0 完成的标志：

- [ ] 所有 7 个 RFC 完稿并审查
- [ ] Windows 兼容策略明确无遗漏
- [ ] SQLite schema 完成 v1 版本
- [ ] CAS layout spec 文档化
- [ ] 能够"完整描述系统"而不需查阅文档

## 🚀 下一步行动

### 立即行动
1. **审查设计文档**：阅读所有 RFC，提出问题
2. **Windows 测试**：在 Windows 机器上验证特权检测
3. **性能原型**：BLAKE3 哈希性能测试

### Phase 1 准备
1. 设置 Rust 开发环境
2. 创建 Cargo workspace
3. 设置 CI/CD（GitHub Actions）
4. 开始实现 Core Scanner MVP

## 📚 参考资料

- **项目计划**：`modeld-project-plan.md`（父目录）
- **BLAKE3 规范**：https://github.com/BLAKE3-team/BLAKE3-specs
- **HuggingFace Hub**：https://huggingface.co/docs/huggingface_hub
- **SQLite WAL**：https://www.sqlite.org/wal.html
- **Windows 符号链接**：https://learn.microsoft.com/windows/win32/fileio/symbolic-links

## 🤝 贡献

Phase 0 是设计阶段，欢迎：
- 设计审查和反馈
- 边缘案例识别
- 替代方案建议
- Windows 兼容性测试

## 📄 许可证

待定（建议 MIT 或 Apache 2.0）

---

**文档版本**：1.0  
**状态**：Phase 0 设计完成，准备进入实施  
**日期**：2024  
**预计工期**：2-3 周（约 69 小时）
