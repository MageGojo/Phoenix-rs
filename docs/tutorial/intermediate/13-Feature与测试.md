# 13 · Feature 与测试

← [12 安全与中间件](./12-安全与中间件.md) · 下一章 → [14 中级验收](./14-中级验收.md)

## 目标

学会用 Cargo feature 按需开启能力，并写一个最小 feature 测试。

## 必做

### 1. 阅读能力表

打开 `docs/FEATURES.md`，浏览两类能力：

- **基础设施**：`sqlite` / `pgsql` / `mysql` / `auth` / `jwt` / `password` / `tls` / `websocket` / `sse` / `metrics` / `storage` / `queue` / `mail` / `redis` / `schedule`
- **业务型 Feature**（`FeatureSet::plugin` 装配）：`captcha` / `pay` / `notify`

规则：**需要什么开什么**。数据库连接串在 `.env` 的 `DATABASE_URL`，必须与已启用的驱动 feature 一致（开了 `sqlite` 就别指向 postgres）。

### 2. 做一个「只开文档、暂不接业务」的选择

在笔记中写下：你的练习应用下一步**不**开哪些（例如暂缓 redis/mail），避免 feature 膨胀。

### 3. 最小测试

在 `tests/feature/`（无则创建）增加测试：例如 GET `/notes` 返回 200，或 POST 校验 422。  
参照 `docs/TESTING_AND_STORAGE.md` 与示例测试风格；优先用框架测试工具，避免必须抢固定端口。

```bash
cargo test
```

### 4. （可选）跑官方烟测示例

若本机已克隆框架仓库：

```bash
cd examples/render-modes-smoke
# 按该示例 README / docs/工具与约定.md 启动并 verify
```

中级不要求全部绿灯，只要求你会找到这条验证路径。

## 讲解

第三方扩展用 `FeatureSet::plugin`（高级第 17 章），不要在业务里隐式全局注册黑魔法。

框架自带的业务型 Feature（验证码 / 支付 / 通知）也走同一条装配路径，还会带来自己的路由与迁移——所以「开一个 feature」不是零成本，先想清楚要不要。想先看看它们各自解决什么、**不**解决什么，去 [番外 · 业务能力速览](../advanced/番外-业务能力速览.md)。

## 验收

- [ ] 能根据 FEATURES.md 解释「为何默认不要全开」  
- [ ] `cargo test` 至少有一条与你应用相关的测试通过  

## 延伸阅读

- `docs/FEATURES.md`
- `docs/TESTING_AND_STORAGE.md`

## 下一章预告

中级总验收。
