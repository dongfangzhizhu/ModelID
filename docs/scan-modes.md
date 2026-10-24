# 扫描模式（Index / Adopt / Copy）

> 对应任务：[ROADMAP A5](ROADMAP.md#a5-扫描摄取模式消除磁盘翻倍)

## 为什么需要三种模式

早期版本的 `modeld scan` 会把每个扫到的模型**无条件复制**进 CAS。对一个
50 GB 的模型目录来说，这意味着磁盘上出现两份完全相同的 50 GB —— 一份在原位，
一份在 store 里。而用户跑 `scan` 的初衷往往只是"让 modeld 认识这些文件"。

A5 之后，扫描与"是否复制字节"解耦，由模式决定：

| 模式 | CLI | WebUI `mode` | 磁盘成本 | 原文件 |
| --- | --- | --- | --- | --- |
| Index（默认） | 无参数 / `--index-only` | `"index"` | 0 | 不动 |
| Adopt | `--adopt` | `"adopt"` | 0（硬链接） | 变成指向 CAS 对象的只读链接 |
| Copy | `--copy` | `"copy"` | 等于文件大小 | 保留一份独立副本 |

## Index（默认）

只写数据库（`models` / `aliases` / `path_index`），文件系统完全不动。

- 适合：只想建立索引、打标签、查重、统计。
- 代价：**CAS 里没有对象**。在 `--adopt` / `--copy` / 下载之前，
  `modeld proxy` 无法对外提供这些 blob（会返回 404），`modeld gc` 也不会
  处理它们。

## Adopt

把文件**硬链接**进 CAS，再把原路径原子替换为该链接（暂存 + `rename`，与 A1 的
原子替换协议一致）。全程不读写文件内容，所以 10 GB 的模型只花一个目录项。

- 前提：store 与原文件**在同一个卷**上（硬链接不能跨卷）。
- 副作用：CAS 对象是只读的，因此**原文件也会变成只读**。这是"文件已由 store 托管"的
  信号；需要改文件时先 `modeld unlink <path>` 还原成独立副本。
- 若 CAS 中已存在同内容的另一份对象，则不做新链接，直接把原路径原子替换为指向既有
  对象的链接 —— 顺带完成去重。
- 扫描后文件被改动（大小变化）时拒绝 adopt 并报错，避免把错误内容挂到正确的哈希下。

跨卷时会明确失败，不会静默降级为复制：

```
错误：1
  D:\models\model.safetensors：adopt failed: cross-volume: D:\models\model.safetensors
  and the store (E:\store\cas\blake3\fa) are on different volumes, so it cannot be
  hard-linked; use --copy instead
```

## Copy

即 A5 之前的行为，显式选择才会发生。store 与模型目录可以跨卷，store 自包含。

## 扩展名

默认扩展名（`scanner::MODEL_EXTENSIONS`）：

```
safetensors gguf ckpt pth pt bin onnx tflite h5 pb engine npz
```

可在 store 的 `modeld.toml` 中覆盖（非空即以配置为准，会去掉前导 `.` 并转小写）：

```toml
[scan]
extensions = ["safetensors", "gguf", "onnx"]
```

CLI 的 `modeld scan` 与 WebUI 的 `POST /api/v1/scan` 都读取同一份配置。

## 验收数据（2026-10-24）

128 MiB 单文件、三种模式各用独立 store：

| 模式 | 卷剩余空间变化 | CAS 对象数 | 原文件与 CAS 的 fileid |
| --- | --- | --- | --- |
| Index（默认） | 0.20 MB（噪声） | 0 | — |
| Adopt | 0.30 MB（噪声） | 1 | 相同 |
| Copy | 128.20 MB | 1 | 不同 |

## 已知限制

- `scan` 的退出码目前不反映 per-file 错误（跨卷 `--adopt` 会打印错误但退出码仍为 0），
  见 ROADMAP §12.3。
- `modeld status` 尚未区分"仅索引"与"已入库"的模型，见 ROADMAP §12.3。
