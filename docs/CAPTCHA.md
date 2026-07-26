# 图形验证码（phoenix-captcha）

## 目标

提供 Laravel 风格的图形验证码 Feature：纯 Rust 生成 SVG（无图像库、无 C 依赖），答案以哈希保存在服务端，一次一用、大小写不敏感、常数时间比较。以 `Plugin` 形式装配（见 [FEATURES.md](FEATURES.md)）。

> **安全边界声明**：图形验证码只是**低成本摩擦**，不是安全边界。对脚本化攻击者（OCR / 打码平台）它只能提高成本，必须与 `phoenix_security::RateLimit` 组合使用，并配合登录失败锁定等策略。

## 两条存储路径

生成器、哈希方式、一次一用语义**完全共用**，区别只在哈希存哪里：

| | Session 流 | Store 流 |
| --- | --- | --- |
| 入口 | `Captcha::issue` / `verify` | `Captcha::issue_stored` / `verify_stored` |
| 路由 | `GET /captcha` → SVG（`captcha.image`） | `GET /captcha/challenge` → JSON（`captcha.challenge`） |
| 前置条件 | 必须挂 `SessionMiddleware` | 无需 session，也无需 Cookie |
| 客户端携带 | 只带答案（挑战跟着 session 走） | 带 `id` + 答案 |
| 寿命 | = session 寿命 | `CaptchaConfig::ttl`（默认 5 分钟） |
| 一次一用的权威 | 每个 session 各自的存储 | 单一存储；`DbCaptchaStore` 下跨实例成立 |
| 适用 | 浏览器表单 | 无 session 的 API 客户端 / 移动端 / 多实例部署 |

选哪条：**页面里有 session 就用 session 流**（更少活动部件、无需建表）；**客户端拿不到 Cookie，或想让一次一用跨实例成立，就用 store 流**。

## 公开 API

| 类型 | 职责 |
| --- | --- |
| `CaptchaConfig` | 字符集 / 长度 / 画布尺寸 / 干扰曲线与噪点数量 / session key / `ttl`，全部可配置 |
| `CaptchaError` | 稳定错误：`EmptyCharset`、`NonAlphanumericCharset`、`InvalidLength`、`InvalidDimensions`、`EmptySessionKey`、`InvalidTtl` |
| `Captcha` | `generate()` / `store()` / `issue()` / `verify()` / `issue_stored()` / `verify_stored()`；廉价 Clone |
| `Challenge` | `answer`（明文，仅存在于进程内）+ `svg`（`image/svg+xml` 文档） |
| `IssuedChallenge` | store 流签发结果：`id` / `svg` / `expires_in`，`to_json()` 即路由响应体 |
| `CaptchaStore` | 存储 trait：`insert` / `take`（原子claim）/ `purge_expired` |
| `MemoryCaptchaStore` / `DbCaptchaStore` | 进程内实现 / Toasty 实现（`CaptchaRow` + `captchas` 迁移） |
| `CaptchaStoreError` | `Backend(String)`、`DuplicateId(String)`；**任何存储错误都按验证失败处理** |
| `CaptchaFeature` | `Plugin` 实现：`GET /captcha`（`captcha.image`）；`with_store(...)` 再加 `captcha.challenge` 与 `captchas` 迁移 |
| `CaptchaInput` / `CaptchaProtected<E>` | 校验接入：包装提取器，在 handler 之前完成一次性验证（**仅 session 流**） |
| `captcha_format(len)` | `phoenix_validation` 规则：仅做格式检查（必填 / 定长 / 字母数字） |
| `verify_with_key(session, key, input)` | 自定义 session key 时的显式验证入口 |
| `captcha_error_response(field)` | 与 `CaptchaProtected` 逐字节一致的 422 体，供手写 handler 复用 |

默认字符集去掉了易混淆的 `0 O 1 l I`；默认 5 个字符、160×60 画布、3 条干扰曲线、28 个噪点、session key `_captcha`、ttl 5 分钟（合法区间 1 秒–1 天）。

## 装配 Feature

```rust
use phoenix::plugin::FeatureSet;
use phoenix_captcha::CaptchaFeature;

let feature = CaptchaFeature::new();          // GET /captcha，路由名 captcha.image
let captcha = feature.captcha();              // 留一个句柄给登录 handler 用

let features = FeatureSet::new().plugin(feature)?;
let routes = features.merge_into(app_routes())
    .with_middleware(SessionMiddleware::new(store, SessionConfig::default()));
```

要点：

- **必须**挂载 `SessionMiddleware`（本地 `SessionStore` 或 Redis 分布式后端均可），否则验证码路由返回 500。
- 默认命名空间下路由名为 **`captcha.image`**；`router.url("captcha.image", &[])` 生成 URL。
- 自定义：`CaptchaFeature::with_config(config)?.path("/kaptcha")`。

## 装配 store 流

```rust
use std::sync::Arc;
use phoenix::plugin::FeatureSet;
use phoenix_captcha::{CaptchaFeature, CaptchaRow, CaptchaStore, DbCaptchaStore};
use phoenix_database::{Database, models};

// CaptchaRow 必须进应用的 models!(...)，否则 Toasty 不认识这张表。
let db = Database::builder(models!(crate::*, CaptchaRow)).connect(&url).await?;
let store: Arc<dyn CaptchaStore> = Arc::new(DbCaptchaStore::new(db));

let feature = CaptchaFeature::new().with_store(Arc::clone(&store));
let captcha = feature.captcha();          // 留给 handler
let features = FeatureSet::new().plugin(feature)?;   // 同时带来 captchas 迁移
```

要点：

- `with_store` 之外**什么都不用改**：没有 store 时不注册 `captcha.challenge`、不产出迁移，session 流保持原样。
- 迁移 id `202607260003`（排在 `phoenix-pay` 的 `…0001`、`phoenix-notify` 的 `…0002` 之后）；建表 SQL 面向 SQLite/PostgreSQL，MySQL 的 `DROP INDEX` 需另写（与 payments / notifications 同一已知项）。
- 单机/测试用 `MemoryCaptchaStore`：同一进程内成立，重启即失，不跨实例。

`GET /captcha/challenge` 响应（`Cache-Control: no-store, no-cache, must-revalidate`）：

```json
{ "id": "9f2c…（32 位十六进制）", "svg": "<svg …>", "expires_in": 300 }
```

存储不可用时返回 500 与一句通用文案，**不泄露后端细节**。

### 一次一用是怎么保证的

`CaptchaStore::take` 是**原子 claim**：同一个 id 在并发下最多只有一个调用方拿到 `Some(_)`，其余一律 `None`。

- `MemoryCaptchaStore`：在同一把锁里读并删除。
- `DbCaptchaStore`：先读行，再用 `DELETE … WHERE id = ?` 认领并检查**影响行数**——影响 0 行说明别人先拿走了，本次按「无待验证挑战」处理。读完就无条件相信读到的行会让重复提交花掉同一个挑战两次，所以这里不能省。

无论验证成败、是否已过期，`take` 都会把行claim掉：失败的尝试同样消耗挑战。过期行也可以由 `purge_expired(now)` 批量回收（只清「签发后从没提交过」的那些），挂到 `phoenix-schedule` 的定时任务里即可（见 [SCHEDULE.md](SCHEDULE.md)）。

> `Database::table_prefix` 与 Feature 自带迁移目前不兼容：迁移写死裸表名，而 ORM 会加前缀。`DbCaptchaStore` 的裸 SQL 走 `Database::table_name()`，与 ORM 保持同一侧。

### handler 里怎么验（store 流）

`CaptchaProtected` 提取器是**同步**的，无法等待存储，所以 store 流在 handler 里显式验证：

```rust
use phoenix_captcha::captcha_error_response;

pub async fn login(Validated(Json(input)): Validated<Json<LoginInput>>) -> Response {
    match captcha.verify_stored(store.as_ref(), &input.captcha_id, &input.captcha).await {
        Ok(true) => { /* 通过，挑战已失效，继续校验凭据 */ }
        // 失败与存储不可用都不放行；422 体与 CaptchaProtected 逐字节一致
        Ok(false) => return captcha_error_response("captcha"),
        Err(_) => return captcha_error_response("captcha"),
    }
    // …
}
```

## 响应与 Session 交互

`GET /captcha` 每次请求都会：

1. 生成新挑战（每字符独立 `<text>`，随机旋转 / 平移 / 字号抖动 + 干扰曲线与噪点，答案不会以整串出现在 SVG 里）；
2. 把 **小写化后的 SHA-256 哈希**（十六进制，从不存明文）写入 session key `_captcha`，覆盖旧挑战；
3. 返回 `image/svg+xml; charset=utf-8`，带 `Cache-Control: no-store, no-cache, must-revalidate` 与 `Pragma: no-cache`。

验证语义（`Captcha::verify` / `verify_with_key`）：

- **一次一用**：比较前先从 session 删除存储值——无论成败，同一挑战不可二次尝试；
- **大小写不敏感**：输入 trim + 小写后哈希；
- **常数时间比较**存储哈希与输入哈希；
- 无待验证挑战、输入为空或超长（>128 字节）一律返回 `false`。

## 在登录表单里怎么用

推荐方式：`CaptchaProtected` 提取器（要求默认 session key）。

```rust
use phoenix_captcha::{CaptchaInput, CaptchaProtected};

#[phoenix::contract(input)]
#[derive(Deserialize)]
pub struct LoginInput {
    pub email: String,
    pub password: String,
    pub captcha: String,
}

impl CaptchaInput for LoginInput {
    fn captcha_input(&self) -> &str {
        &self.captcha
    }
}

pub async fn login(
    CaptchaProtected(Validated(Json(input))): CaptchaProtected<Validated<Json<LoginInput>>>,
) -> impl IntoResponse {
    // 走到这里说明验证码已通过并已失效；继续做凭据校验。
}
```

验证失败返回 422，错误体与 `phoenix_validation` 同构，前端表单可直接消费：

```json
{
  "message": "The submitted data is invalid.",
  "errors": { "captcha": [{ "rule": "captcha", "message": "The captcha is invalid or has expired." }] }
}
```

显式方式（自定义 session key、或想自己控制顺序时）：

```rust
pub async fn login(request: Request, /* … */) -> Response {
    let session = request.extensions().get::<Session>().cloned().expect("SessionMiddleware");
    if !captcha.verify(&session, &input.captcha) {
        // 返回 422 / 表单错误；此时挑战已失效，前端应刷新验证码图片
    }
    // …
}
```

可选：在 `Validate` 实现里加格式规则（只查形状，不查答案）：

```rust
.field("captcha", rules![required(), captcha_format(5)])
```

## 前端用法（React）

`@apizero/react` 内置 `CaptchaImage` / `useCaptcha`（基于 named route **`captcha.image`** 解析地址，自动加时间戳防缓存，点击图片换一张）：

```tsx
import { CaptchaImage, Form, FieldError, useCaptcha } from "@apizero/react";
import { login } from "../generated/routes.js";

function LoginForm() {
  const captcha = useCaptcha(); // { src, refresh }
  return (
    <Form
      action={login.store}
      initialValues={{ email: "", password: "", captcha: "" }}
      onError={() => captcha.refresh()}   // 422 后旧挑战已被消耗，必须换图
    >
      {(form) => (
        <>
          {/* email / password 略 */}
          <img src={captcha.src} alt="验证码" onClick={captcha.refresh} />
          <input {...form.field("captcha")} autoComplete="off" />
          <FieldError errors={form.errors} name="captcha" />
          <button disabled={form.processing}>登录</button>
        </>
      )}
    </Form>
  );
}
```

不需要联动刷新时直接用 `<CaptchaImage />`（默认点击刷新，`route` 可改路由名）。约定：

- 字段名 **`captcha`**（`CaptchaInput::captcha_field()` 可改错误字段名）；
- 提交失败（422 `errors.captcha`）后**必须** `refresh()`，因为旧挑战已被一次性消耗；
- 响应是 `no-store`，任何重新请求都会生成新挑战并覆盖旧的。

### store 流（无 session）

`StoredCaptchaImage` / `useStoredCaptcha` 走 `captcha.challenge`，把挑战 id 交回表单：

```tsx
import { Form, FieldError, StoredCaptchaImage } from "@apizero/react";
import { login } from "../generated/routes.js";

function LoginForm() {
  return (
    <Form action={login.store} initialValues={{ email: "", password: "", captcha: "", captcha_id: "" }}>
      {(form) => (
        <>
          {/* email / password 略 */}
          <StoredCaptchaImage
            alt="验证码"
            onChallenge={(id) => form.setField("captcha_id", id)}
          />
          <input {...form.field("captcha")} autoComplete="off" />
          <FieldError errors={form.errors} name="captcha" />
          <button disabled={form.processing}>登录</button>
        </>
      )}
    </Form>
  );
}
```

要点：

- `onChallenge` 在每次拿到新挑战时触发（首次加载 + 点击刷新），一行把 id 同步进表单；
- SVG 以 **`data:` URL** 内联，服务端返回的字符串**从不**作为 HTML 注入 DOM；
- 需要自己控制渲染时用 `useStoredCaptcha()` → `{ id, src, expiresIn, loading, error, refresh }`；请求失败会丢弃旧挑战（`src` 为空、不上报 id），避免提交一个服务端必然拒绝的 id。

## 安全注意

- 与 `RateLimit` 组合：两条路由**都**要限流。session 流的生成会写 session；store 流会**写库**，不限流等于给了一个免费的插行接口；
- 只存哈希：即使存储泄露也不直接暴露答案明文（注意：字符空间小，哈希只是纵深防御，不是保密保证）；
- session 流的挑战寿命 = session 寿命，重新请求图片即作废旧挑战（同 key 覆盖）；store 流按 `ttl` 独立过期；
- 挑战 id 是 128 位 CSPRNG 随机数的十六进制，不可枚举；超长 / 空 id 在进存储前就被拒绝；
- 自定义字符集必须为 ASCII 字母数字（构造时校验），杜绝 SVG/XML 注入。

## 非目标（首版）

- 音频验证码 / 无障碍替代通道（请为受影响用户提供其它路径）；
- 行为式 / 滑块验证码；
- store 流的自动过期回收（`purge_expired` 已提供，但**不**自带定时任务，需要你挂到 `phoenix-schedule`）。

## 验收

```bash
cargo test -p phoenix-captcha --locked
cargo clippy -p phoenix-captcha --all-targets --locked -- -D warnings
```
