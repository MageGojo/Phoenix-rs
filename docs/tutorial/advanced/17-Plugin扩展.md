# 17 · Plugin 扩展

← [16 部署安装与回滚](./16-部署安装与回滚.md) · 下一章 → [18 生产与多库](./18-生产与多库.md)

## 目标

理解第三方能力应通过 `FeatureSet::plugin`（或文档所述 Plugin API）**显式**接入，而不是改框架全局单例。

## 必做

### 1. 读文档

精读 `docs/FEATURES.md` 中 **Plugin / FeatureSet** 章节（名称以文档为准）。  
在 `examples/render-modes-smoke`（或文档示例）中找到一处 `plugin(...)` 注册，记下：

- Plugin 提供了什么（路由？命令？服务？）  
- 在哪里 merge 进应用  

### 2. 最小实验（二选一）

**A. 阅读型：** 画出「应用 routes + plugin routes」合并示意图，写入笔记。  

**B. 动手型：** 按文档实现一个 hello 级 Plugin（例如注册一条 `/plugin-hello` 或 console 命令），在练习应用启用，`px dev` 验证。

### 3. 反模式自检

确认你的应用里没有：

- 在库代码里隐式 `lazy_static` 全局注册路由  
- 为图方便 fork 框架改核心以「偷偷」挂中间件  

## 讲解

Compile-time 插件与 Cargo feature 互补：feature 决定链什么；Plugin 决定如何组装进应用。  
保持可测试、可裁剪。

## 验收

- [ ] 能用自己的话解释 Plugin 与「改框架核心」的区别  
- [ ] 完成阅读型或动手型任一实验  

## 延伸阅读

- `docs/FEATURES.md`
- 示例中的 Plugin 命令（如 smoke 的 `greet`）

## 下一章预告

生产配置、多数据库与运维纪律。
