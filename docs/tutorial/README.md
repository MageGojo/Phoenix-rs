# Phoenix-rs 系统教程

面向要用 **Phoenix-rs**（`px` CLI + Rust + React）从零做出可上线应用的开发者。  
本目录是**按顺序学完**的教材；深度参考仍以仓库 `docs/` 专章与 Skill 为准。

出品：[极数本源 ApiZero](https://apizero.cn/) · 框架仓库 [Phoenix-rs](https://github.com/MageGojo/Phoenix-rs)

---

## 怎么学（强制约定）

1. **只按本章路径前进**：未完成上一章的验收，不要跳到下一章。
2. **动手优先**：每章有「必做」；只看不练等于没学。
3. **契约与脚手架铁律**（全教程有效）：
   - 用 `px new` / `px make:*`，不要手搓目录树
   - 契约写在 Rust（`#[phoenix::contract]`）；**禁止手改** `views/generated/`
   - React 用生成的 named route / action；破坏性写操作走 typed action
4. 卡住时查：同章「延伸阅读」→ `docs/BUSINESS_GUIDE.md` / `docs/DX.md` → Skill

建议练习应用名：`learn-phoenix`（或沿用你自己的 `px_sh`）。  
每级结束有**验收清单**，过关再进下一级。

---

## 学习路径总览

```text
初级（能跑、能改一页、能交一张表）
  01 环境与安装
  02 第一个应用
  03 项目地图
  04 页面与 Props
  05 路由与控制器
  06 表单与校验
  07 初级验收

中级（SQLite 业务切片 + 契约 + 安全默认）
  08 启用 SQLite
  09 模型、迁移与 CRUD
  10 契约与生成物
  11 渲染模式（SPA / Islands / SSR）
  12 安全与中间件
  13 Feature 与测试
  14 中级验收

高级（发版、部署、扩展、生产心智）
  15 发版流水线
  16 部署安装与回滚
  17 Plugin 扩展
  18 生产与多库
  19 契约深水区与源码边界
  20 高级验收
  番外 业务能力速览（选修）
```

| 级别 | 目标 | 预计投入 | 入口 |
| --- | --- | --- | --- |
| 初级 | `px dev` 跑通；改 Home；做一个带校验的表单 action | 1–2 天 | [beginner/01](./beginner/01-环境与安装.md) |
| 中级 | SQLite Note（或同类）全链路 CRUD；懂契约与三种渲染 | 3–5 天 | [intermediate/08](./intermediate/08-启用SQLite.md) |
| 高级 | `px release` → `release:install` → 更新/回滚；会开 Feature/Plugin | 2–4 天 | [advanced/15](./advanced/15-发版流水线.md) |

---

## 章节目录（按序打开）

### 准备

- [00 · 怎么用这套教程](./00-怎么用这套教程.md)

### 初级

1. [环境与安装](./beginner/01-环境与安装.md)
2. [第一个应用](./beginner/02-第一个应用.md)
3. [项目地图](./beginner/03-项目地图.md)
4. [页面与 Props](./beginner/04-页面与Props.md)
5. [路由与控制器](./beginner/05-路由与控制器.md)
6. [表单与校验](./beginner/06-表单与校验.md)
7. [初级验收](./beginner/07-初级验收.md)

### 中级

8. [启用 SQLite](./intermediate/08-启用SQLite.md)
9. [模型、迁移与 CRUD](./intermediate/09-模型迁移与CRUD.md)
10. [契约与生成物](./intermediate/10-契约与生成物.md)
11. [渲染模式](./intermediate/11-渲染模式.md)
12. [安全与中间件](./intermediate/12-安全与中间件.md)
13. [Feature 与测试](./intermediate/13-Feature与测试.md)
14. [中级验收](./intermediate/14-中级验收.md)

- 选修：[番外 · 文件上传](./intermediate/番外-文件上传.md)

### 高级

15. [发版流水线](./advanced/15-发版流水线.md)
16. [部署安装与回滚](./advanced/16-部署安装与回滚.md)
17. [Plugin 扩展](./advanced/17-Plugin扩展.md)
18. [生产与多库](./advanced/18-生产与多库.md)
19. [契约深水区与源码边界](./advanced/19-契约深水区与源码边界.md)
20. [高级验收](./advanced/20-高级验收.md)

- 选修：[番外 · 业务能力速览](./advanced/番外-业务能力速览.md)（支付 / 通知 / 验证码 / 实时 / 加密传输）

---

## 与仓库其它文档的关系

| 文档 | 角色 |
| --- | --- |
| **本教程 `docs/tutorial/`** | 学习顺序与必做练习 |
| `docs/BUSINESS_GUIDE.md`、`docs/DX.md` | 业务写法与命令手册 |
| `docs/REACT.md` | React 日常 API 一页速查（大道至简） |
| `docs/CONTRACTS.md`、`docs/RENDERING.md`、`docs/DATABASE.md` | 专题深挖 |
| `docs/RELEASE_PIPELINE.md`、`docs/FEATURES.md` | 发版与能力开关 |
| `.cursor/skills/phoenix/SKILL.md` | AI / 人类的铁律清单 |

学完教程后，日常开发以 Skill + 专章为准；本教程不必反复重读。

---

## 进度自检（复制到笔记）

```text
- [ ] 初级 01–07 验收通过
- [ ] 中级 08–14 验收通过
- [ ] 高级 15–20 验收通过
练习应用路径: _______________
最后学到章节: _______________
```
