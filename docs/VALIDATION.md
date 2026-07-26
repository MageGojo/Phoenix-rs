# 校验（phoenix-validation）

`phoenix-validation` 提供组合式的字段校验：内置规则 + 自定义规则，配合
`Validated<Json<T>>` / `Validated<Form<T>>` 提取器在进入控制器前完成校验，
失败时自动返回 422 JSON。本篇覆盖规则清单、消息本地化（内置 `en` / `zh-CN`）、
消息定制与 422 错误体形状。

## 内置规则清单

| 规则标识（`rule`） | 构造函数 | 说明 | 消息占位符 |
| --- | --- | --- | --- |
| `required` | `required()` | 值必须存在且非空（空串/空数组/空对象/`null` 均视为缺失） | `{field}` |
| `string` | `string()` | 值存在时必须是字符串（`null`/缺失放行，配合 `required` 使用） | `{field}` |
| `min_length` | `min_length(n)` | 字符串字符数（按 `char` 计）不得少于 `n` | `{field}`、`{min}` |
| `max_length` | `max_length(n)` | 字符串字符数不得超过 `n` | `{field}`、`{max}` |

规则标识是稳定契约：前端 `@apizero/react` 按 `rule` 字段做程序化判断，
本地化只改 `message` 文案，**不会**改变 `rule` 标识。

自定义规则用 `custom_rule("name", |ctx| ...)` 或实现 `Rule` trait；
`phoenix-captcha` 的 `captcha_format()` 就是一个第三方规则（`rule: "captcha"`）。

```rust
use phoenix_validation::{Validator, rules, required, string, min_length, max_length};

let data = serde_json::json!({ "name": "Ada" });
Validator::new(&data)
    .field("name", rules![required(), string(), min_length(3), max_length(30)])
    .validate()?;
```

## 消息本地化

所有内置规则的文案都经由进程级消息目录渲染。内置两套完整目录：

- `en`（默认）：与历史硬编码文案逐字节一致；
- `zh-CN`：全部内置规则 + 顶层消息的简体中文翻译。

启用中文（一般在应用启动时调用一次）：

```rust
phoenix_validation::set_locale(phoenix_validation::LOCALE_ZH_CN);
```

之后 422 响应变为：

```json
{
  "message": "提交的数据不合法。",
  "errors": {
    "name": [{ "rule": "required", "message": "name 不能为空。" }]
  }
}
```

zh-CN 内置文案一览：

| 规则 | 文案模板 |
| --- | --- |
| 顶层 message | `提交的数据不合法。` |
| `required` | `{field} 不能为空。` |
| `string` | `{field} 必须是字符串。` |
| `min_length` | `{field} 长度不能小于 {min} 个字符。` |
| `max_length` | `{field} 长度不能超过 {max} 个字符。` |

**向后兼容**：不调用 `set_locale` 时输出与旧版本完全一致（默认 `en`），
既有断言不受影响；当前 locale 缺失某条模板时自动回退到英文。

## 字段名可读化

模板里的 `{field}` 默认取字段原名；注册显示名后全局生效：

```rust
phoenix_validation::register_field_name("email", "邮箱");
phoenix_validation::register_field_names([("password", "密码"), ("title", "标题")]);
// => "邮箱 不能为空。"；未注册的字段仍显示原名
```

`field_display_name("email")` 可在自定义规则里复用同一套映射。

## 消息定制

### 覆盖单条消息（保留其余翻译）

```rust
use phoenix_validation as v;

v::override_message(v::LOCALE_ZH_CN, "required", "请填写 {field}！");
v::override_invalid_message(v::LOCALE_ZH_CN, "数据校验未通过。"); // 顶层 message
```

### 注册整套自定义 locale

```rust
use phoenix_validation::{Messages, register_locale, set_locale};

register_locale(
    "zh-TW",
    Messages::new()
        .invalid("提交的資料不合法。")
        .rule("required", "{field} 不能為空。")
        .rule("min_length", "{field} 長度不能小於 {min} 個字元。"),
);
set_locale("zh-TW"); // 缺失的模板回退英文
```

### 自定义规则接入消息目录

```rust
use phoenix_validation::{custom_rule, rule_message, override_message};

override_message("zh-CN", "even", "{field} 必须是偶数。");
let even = custom_rule("even", |ctx| {
    match ctx.value.and_then(|v| v.as_i64()) {
        Some(n) if n % 2 == 0 => Ok(()),
        _ => Err(rule_message("even", ctx.field, &[])
            .unwrap_or_else(|| format!("The {} field must be even.", ctx.field))),
    }
});
```

`rule_message(rule, field, params)` 按「当前 locale → en → 内置英文」解析模板并
插值；未注册模板时返回 `None`，由调用方兜底。

相关 API 汇总：`set_locale` / `locale` / `register_locale` / `override_message` /
`override_invalid_message` / `register_field_name(s)` / `field_display_name` /
`rule_message` / `invalid_message` / `builtin_locale` / `Messages` /
`BUILT_IN_RULES` / `LOCALE_EN` / `LOCALE_ZH_CN`。全部为进程级设置
（读多写一次），建议只在启动阶段写入。

## 422 错误体形状（稳定契约）

`Validated<T>` 校验失败时返回 `422 Unprocessable Entity`：

```json
{
  "message": "The submitted data is invalid.",
  "errors": {
    "<字段名>": [
      { "rule": "<规则标识>", "message": "<本地化文案>" }
    ]
  }
}
```

- `errors` 的 key 是**字段原名**（与请求 payload 一致，不受显示名影响），
  `@apizero/react` 的表单 hooks 依赖该形状回填错误；
- `rule` 恒为稳定标识（`required` / `min_length` / …），供程序判断；
- `message` 是给人看的文案，是唯一受 locale / 覆盖 / 显示名影响的部分；
- `phoenix-captcha` 的 `CaptchaProtected` 拒绝响应使用相同形状
  （`rule: "captcha"`），与本地化改动完全兼容。

新增内置规则时必须同步补齐 `en` 与 `zh-CN` 模板并加入 `BUILT_IN_RULES`；
测试会枚举断言两套目录与规则清单严格一致，漏译直接失败。
